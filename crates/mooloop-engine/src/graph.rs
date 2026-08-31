//! JACK adapter around the allocation-free shared render state.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use jack::ProcessHandler;
use jack::{AudioOut, Client, Control, MidiIn, Port, PortId, ProcessScope};
use mooloop_core::{EngineEvent, MidiMessage};
use mooloop_dsp::{SampleData, MAX_BLOCK_SIZE};
use rtrb::Consumer;

use crate::render::RenderState;
use crate::{RealtimeCommand, StructuralReclaim};

/// Per-block MIDI input ceiling. Bounded so the callback never allocates.
const MAX_MIDI_PER_BLOCK: usize = 256;

pub(crate) struct GraphIo {
    pub out_l: Port<AudioOut>,
    pub out_r: Port<AudioOut>,
    pub midi_in: Port<MidiIn>,
    pub cmd_rx: Consumer<RealtimeCommand>,
    pub evt_tx: rtrb::Producer<EngineEvent>,
    pub reclaim_tx: rtrb::Producer<StructuralReclaim>,
}

pub(crate) struct Graph {
    render: Box<RenderState>,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    midi_in: Port<MidiIn>,
    /// Decoded once per block into a fixed buffer. Sized for far more input
    /// than a human or a sequencer produces in one period; the overflow is
    /// dropped rather than allocated for.
    midi_scratch: [MidiMessage; MAX_MIDI_PER_BLOCK],
    cmd_rx: Consumer<RealtimeCommand>,
    evt_tx: rtrb::Producer<EngineEvent>,
    reclaim_tx: rtrb::Producer<StructuralReclaim>,
    /// A command popped from the ordered stream while reclamation is
    /// backpressured. No later command may pass it.
    pending_command: Option<RealtimeCommand>,
    /// Preview samples whose voice has finished, awaiting reclaim-ring slots.
    retired_previews: Vec<Arc<SampleData>>,
    xrun_count: Arc<AtomicU64>,
    last_seen_xruns: u64,
}

impl Graph {
    pub(crate) fn new(io: GraphIo, render: Box<RenderState>, xrun_count: Arc<AtomicU64>) -> Self {
        Self {
            render,
            out_l: io.out_l,
            out_r: io.out_r,
            midi_in: io.midi_in,
            midi_scratch: [MidiMessage {
                offset: 0,
                channel: 0,
                kind: mooloop_core::MidiKind::NoteOff { note: 0 },
            }; MAX_MIDI_PER_BLOCK],
            cmd_rx: io.cmd_rx,
            evt_tx: io.evt_tx,
            reclaim_tx: io.reclaim_tx,
            pending_command: None,
            retired_previews: Vec::new(),
            xrun_count,
            last_seen_xruns: 0,
        }
    }
}

/// Flush subnormal floats to zero on this thread. Recursive DSP state
/// (filter feedback, envelope followers, parameter smoothers) decays
/// asymptotically toward zero and spends time in subnormal range on the way;
/// without this, the CPU can take an order of magnitude longer per
/// arithmetic op on those values, which reads as constant background load
/// with no single attributable cause. MXCSR is per-thread, so this must run
/// on the realtime callback's own thread rather than at engine construction.
#[cfg(target_arch = "x86_64")]
#[inline]
fn enable_flush_to_zero() {
    // `_mm_getcsr`/`_mm_setcsr` are deprecated for soundness reasons (their
    // signature doesn't tell the optimizer they observe/change global FP
    // state), so this reads and writes MXCSR directly instead.
    use std::arch::asm;
    const FLUSH_TO_ZERO: u32 = 1 << 15;
    const DENORMALS_ARE_ZERO: u32 = 1 << 6;
    unsafe {
        let mut csr: u32 = 0;
        asm!("stmxcsr [{0}]", in(reg) &mut csr, options(nostack, preserves_flags));
        csr |= FLUSH_TO_ZERO | DENORMALS_ARE_ZERO;
        asm!("ldmxcsr [{0}]", in(reg) &csr, options(nostack, preserves_flags));
    }
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
fn enable_flush_to_zero() {}

impl ProcessHandler for Graph {
    fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
        enable_flush_to_zero();
        let frames = (scope.n_frames() as usize).min(MAX_BLOCK_SIZE);
        // Value edits, structural ownership transfers, and prepared projects
        // share one ordered stream. Only apply an ownership-changing command
        // when its displaced object can immediately leave through the reclaim
        // ring; otherwise retain it and let no later command cross the
        // generation boundary.
        loop {
            let command = match self.pending_command.take() {
                Some(command) => command,
                None => match self.cmd_rx.pop() {
                    Ok(command) => command,
                    Err(_) => break,
                },
            };
            let prepared = match command {
                RealtimeCommand::Engine(command) => {
                    self.render.apply_command(command);
                    continue;
                }
                RealtimeCommand::Structural(command) => {
                    if self.reclaim_tx.slots() == 0 {
                        self.pending_command = Some(RealtimeCommand::Structural(command));
                        break;
                    }
                    if let Some(displaced) = self.render.apply_structural(command) {
                        match self.reclaim_tx.push(StructuralReclaim::Effect(displaced)) {
                            Ok(()) => {}
                            Err(_) => {
                                unreachable!("reclaim capacity checked before structural edit")
                            }
                        }
                    }
                    continue;
                }
                RealtimeCommand::Preview(command) => {
                    if let Some(sample) = self.render.apply_preview(command) {
                        // The replaced sample leaves through the reclaim ring
                        // with the rest, below.
                        self.retired_previews.push(sample);
                    }
                    continue;
                }
                RealtimeCommand::InstallProject(prepared) => prepared,
            };
            if self.reclaim_tx.slots() == 0 {
                self.pending_command = Some(RealtimeCommand::InstallProject(prepared));
                break;
            }
            let retired = std::mem::replace(&mut self.render, prepared.render);
            match self
                .reclaim_tx
                .push(StructuralReclaim::RenderState(retired))
            {
                Ok(()) => {}
                Err(_) => unreachable!("reclaim capacity checked before project swap"),
            }
            let _ = self.evt_tx.push(EngineEvent::ProjectInstalled {
                generation: prepared.generation,
            });
        }

        // Decode before rendering so this block's input can act on this
        // block's audio. JACK hands over whole messages already ordered by
        // time, so no sort is needed.
        let mut midi_len = 0;
        for raw in self.midi_in.iter(scope) {
            if midi_len == MAX_MIDI_PER_BLOCK {
                break;
            }
            if let Some(message) = MidiMessage::decode(raw.time, raw.bytes) {
                self.midi_scratch[midi_len] = message;
                midi_len += 1;
            }
        }
        self.render.apply_midi(&self.midi_scratch[..midi_len]);

        let report = self.render.process_block(frames);
        // Finished preview samples return to the UI thread for disposal,
        // the same ownership round trip displaced effect nodes take. The
        // reclaim ring is never the sample's last reference, so a full ring
        // just delays disposal to a later block.
        while let Some(sample) = self.render.pop_retired_preview() {
            self.retired_previews.push(sample);
        }
        if self.reclaim_tx.slots() >= self.retired_previews.len() {
            for sample in self.retired_previews.drain(..) {
                let _ = self
                    .reclaim_tx
                    .push(StructuralReclaim::PreviewSample { sample });
            }
        }
        let master = self.render.master();
        let buffer_l = self.out_l.as_mut_slice(scope);
        let buffer_r = self.out_r.as_mut_slice(scope);
        buffer_l[..frames].copy_from_slice(&master.l[..frames]);
        buffer_r[..frames].copy_from_slice(&master.r[..frames]);
        buffer_l[frames..].fill(0.0);
        buffer_r[frames..].fill(0.0);

        let _ = self.evt_tx.push(EngineEvent::Position {
            tick: report.position_tick,
            beat_in_bar: report.beat_in_bar,
            playing: report.playing,
        });
        let _ = self.evt_tx.push(EngineEvent::Metering {
            peak_l: report.peak_l,
            peak_r: report.peak_r,
        });
        let xruns = self.xrun_count.load(Ordering::Relaxed);
        if xruns != self.last_seen_xruns {
            self.last_seen_xruns = xruns;
            let _ = self.evt_tx.push(EngineEvent::Xrun);
        }
        Control::Continue
    }
}

pub(crate) struct Notifications {
    pub xrun_count: Arc<AtomicU64>,
    /// Whether to retry connecting `target` when the port graph changes and
    /// it is currently unconnected. Shared with `EngineHandle::set_auto_reconnect`.
    pub auto_reconnect: Arc<AtomicBool>,
    /// The configured output target, shared with `EngineHandle`.
    pub target: Arc<ArcSwap<(String, String)>>,
}

impl jack::NotificationHandler for Notifications {
    fn xrun(&mut self, _: &Client) -> Control {
        self.xrun_count.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }

    // Runs on JACK's notification thread, not the realtime audio thread, so
    // ordinary allocation and the `ArcSwap` load below are fine here. A
    // hot-plugged device (e.g. headphones) surfaces to a JACK client as
    // ports registering, not as a "default device changed" event, so port
    // registration is what auto-reconnect actually watches.
    fn port_registration(&mut self, client: &Client, _port_id: PortId, is_registered: bool) {
        if !is_registered || !self.auto_reconnect.load(Ordering::Relaxed) {
            return;
        }
        let target = self.target.load_full();
        if client.port_by_name(&target.0).is_none() || client.port_by_name(&target.1).is_none() {
            return;
        }
        for (src, dst) in [
            (crate::OUT_L_NAME, target.0.as_str()),
            (crate::OUT_R_NAME, target.1.as_str()),
        ] {
            match client.connect_ports_by_name(src, dst) {
                Ok(()) | Err(jack::Error::PortAlreadyConnected(_, _)) => {}
                // JACK's graph-change notification, not the process callback:
                // formatting and locking are both fine here.
                Err(e) => mooloop_core::log_warn!(
                    "audio",
                    "auto-reconnect could not connect {src} -> {dst} ({e})"
                ),
            }
        }
    }
}

pub(crate) type AsyncClient = jack::AsyncClient<Notifications, Graph>;
