//! The driver-neutral half of the realtime callback.
//!
//! Everything the audio thread does between "a block is due" and "here are
//! its samples" lives here: the ordered command stream, the reclaim ring,
//! MIDI decoding, rendering, metering, xrun reporting and the load meter. A
//! driver adapter owns one [`Executor`] and, once per block, hands it that
//! block's MIDI input and output buffers. What the adapter keeps is only what
//! its host API makes different: where the buffers come from, how the thread
//! is created, and how dropouts are reported.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use mooloop_core::{EngineEvent, MidiMessage, MidiPortId};
use mooloop_dsp::MAX_BLOCK_SIZE;
use rtrb::Consumer;

use crate::load::LoadMeters;
use crate::render::{RenderState, RetiredPreviews};
use crate::{RealtimeCommand, StructuralReclaim};

/// Per-block MIDI input ceiling. Bounded so the callback never allocates.
const MAX_MIDI_PER_BLOCK: usize = 256;

pub(crate) struct ExecutorIo {
    pub cmd_rx: Consumer<RealtimeCommand>,
    pub evt_tx: rtrb::Producer<EngineEvent>,
    pub reclaim_tx: rtrb::Producer<StructuralReclaim>,
}

pub(crate) struct Executor {
    render: Box<RenderState>,
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
    /// Bounded: see [`RetiredPreviews`].
    retired_previews: RetiredPreviews,
    /// Incremented by the driver, wherever its host reports a dropout.
    xrun_count: Arc<AtomicU64>,
    last_seen_xruns: u64,
    /// Frames per second, for turning a block length into the wall-clock
    /// budget it has to finish inside.
    sample_rate: u32,
    load: Arc<LoadMeters>,
    /// When the previous callback was entered, so the gap between wake-ups
    /// can be measured. `None` before the first block of a run.
    last_entered: Option<Instant>,
    /// Whether the scheduling policy of this thread has been asked for yet.
    /// The answer cannot change without the driver making a new thread, and
    /// a driver that does calls [`Executor::begin_run`], so it is asked once
    /// per thread.
    checked_scheduling: bool,
}

impl Executor {
    pub(crate) fn new(
        io: ExecutorIo,
        render: Box<RenderState>,
        xrun_count: Arc<AtomicU64>,
        sample_rate: u32,
        load: Arc<LoadMeters>,
    ) -> Self {
        Self {
            render,
            midi_scratch: [MidiMessage {
                offset: 0,
                port: MidiPortId::FIRST,
                channel: 0,
                kind: mooloop_core::MidiKind::NoteOff { note: 0 },
            }; MAX_MIDI_PER_BLOCK],
            cmd_rx: io.cmd_rx,
            evt_tx: io.evt_tx,
            reclaim_tx: io.reclaim_tx,
            pending_command: None,
            retired_previews: RetiredPreviews::new(),
            xrun_count,
            last_seen_xruns: 0,
            sample_rate,
            load,
            last_entered: None,
            checked_scheduling: false,
        }
    }

    /// Forget everything that belonged to the previous callback thread.
    ///
    /// For a driver that reopens its stream under a living executor -- a new
    /// device, a new buffer size -- the next block arrives on a thread nobody
    /// has asked about, after a gap that is not a late wake-up. Without this
    /// the scheduling readout would describe a thread that no longer exists,
    /// and the load meter would report the reopen as the worst deadline miss
    /// of the session.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn begin_run(&mut self) {
        self.last_entered = None;
        self.checked_scheduling = false;
    }

    /// Send as many effects a cleared chain displaced down the reclaim ring
    /// as it has room for. The rest wait in the renderer for a later block,
    /// and structural edits wait behind them.
    fn forward_displaced_effects(&mut self) {
        for _ in 0..self.reclaim_tx.slots() {
            let Some(effect) = self.render.pop_displaced_effect() else {
                break;
            };
            if self.reclaim_tx.push(StructuralReclaim::Effect(effect)).is_err() {
                unreachable!("reclaim capacity checked before forwarding");
            }
        }
    }

    /// Render one block into `out_l` and `out_r`.
    ///
    /// `midi` yields `(port, frame offset, raw message)` triples for this
    /// block, in time order and one whole message each. The port is the
    /// driver's own numbering for this run, and is what a channel's input
    /// setting and a control binding's port filter are resolved against; a
    /// driver with one input port passes [`MidiPortId::FIRST`] for every
    /// message. At most [`MAX_BLOCK_SIZE`] frames are rendered; anything past
    /// that in the buffers is silenced.
    pub(crate) fn process<'m>(
        &mut self,
        midi: impl IntoIterator<Item = (MidiPortId, u32, &'m [u8])>,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        enable_flush_to_zero();
        // On this thread rather than at engine construction, and for the same
        // reason as the flush-to-zero write above: the property being read
        // belongs to the callback's own thread, which the driver created.
        // Whether the realtime request made on its behalf was actually granted
        // is not observable anywhere else, and a denied one is silent -- the
        // audio simply glitches under load and nothing in the program says
        // why.
        if !self.checked_scheduling {
            self.checked_scheduling = true;
            self.load.set_realtime(crate::load::thread_realtime_status());
        }
        // `Instant::now` is a vDSO read of the monotonic clock on Linux and
        // `mach_absolute_time` on macOS: no syscall, no lock, tens of
        // nanoseconds against a budget of millions.
        let entered = Instant::now();
        let period = self
            .last_entered
            .map(|last| entered.duration_since(last).as_nanos() as u64);
        self.last_entered = Some(entered);
        let frames = out_l.len().min(out_r.len()).min(MAX_BLOCK_SIZE);
        // Value edits, structural ownership transfers, and prepared projects
        // share one ordered stream. Only apply an ownership-changing command
        // when its displaced object can immediately leave through the reclaim
        // ring; otherwise retain it and let no later command cross the
        // generation boundary.
        self.forward_displaced_effects();
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
                    // Also held back while effects a previous edit displaced
                    // are still waiting: the renderer reserved room for one
                    // edit's worth, not for an edit on top of a backlog.
                    if self.reclaim_tx.slots() == 0 || self.render.has_displaced_effects() {
                        self.pending_command = Some(RealtimeCommand::Structural(command));
                        break;
                    }
                    if let Some(displaced) = self.render.apply_structural(command) {
                        match self.reclaim_tx.push(displaced) {
                            Ok(()) => {}
                            Err(_) => {
                                unreachable!("reclaim capacity checked before structural edit")
                            }
                        }
                    }
                    self.forward_displaced_effects();
                    continue;
                }
                RealtimeCommand::Preview(command) => {
                    // Checked *before* applying, the way the reclaim-ring
                    // check below defers an install rather than dropping it.
                    // Applying first and then finding nowhere to put the
                    // replaced sample would leave the callback holding an
                    // `Arc` it must not free.
                    if self.retired_previews.is_full() {
                        self.pending_command = Some(RealtimeCommand::Preview(command));
                        break;
                    }
                    if let Some(sample) = self.render.apply_preview(command) {
                        // The replaced sample leaves through the reclaim ring
                        // with the rest, below. The push runs unconditionally
                        // and is judged afterwards -- inside a `debug_assert!`
                        // it would not happen at all in a release build, and
                        // the `Arc` would be freed here, on the audio thread.
                        let refused = self.retired_previews.push(sample);
                        debug_assert!(
                            refused.is_none(),
                            "retirement refused a sample after a capacity check"
                        );
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
        // block's audio. Drivers hand over whole messages already ordered by
        // time, so no sort is needed.
        let mut midi_len = 0;
        for (port, offset, bytes) in midi {
            if midi_len == MAX_MIDI_PER_BLOCK {
                break;
            }
            if let Some(message) = MidiMessage::decode(port, offset, bytes) {
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
        while !self.retired_previews.is_full() {
            let Some(sample) = self.render.pop_retired_preview() else {
                break;
            };
            // Unconditionally, then judged. Same reason as above.
            let refused = self.retired_previews.push(sample);
            debug_assert!(
                refused.is_none(),
                "retirement refused a sample the loop had checked room for"
            );
        }
        // Partial rather than all-or-nothing. Waiting for room for every held
        // sample is what let the backlog build in the first place; draining
        // as many as the ring has slots for makes the bounded case much
        // harder to reach and costs nothing when, as usual, there is one.
        for _ in 0..self.reclaim_tx.slots().min(self.retired_previews.len()) {
            let Some(sample) = self.retired_previews.pop() else {
                break;
            };
            let _ = self
                .reclaim_tx
                .push(StructuralReclaim::PreviewSample { sample });
        }
        // Samples a note-on took off a voice, straight to the ring. No holding
        // area here, unlike the preview: each sampler's own fixed ring already
        // is one, and a sampler whose ring stays full refuses notes rather
        // than dropping anything. Popped only once a slot is known to be
        // free, so the push cannot hand the handle back to be dropped.
        while self.reclaim_tx.slots() > 0 {
            let Some(audio) = self.render.pop_retired_sampler_audio() else {
                break;
            };
            let _ = self.reclaim_tx.push(StructuralReclaim::SamplerAudio(audio));
        }
        let master = self.render.master();
        out_l[..frames].copy_from_slice(&master.l[..frames]);
        out_r[..frames].copy_from_slice(&master.r[..frames]);
        out_l[frames..].fill(0.0);
        out_r[frames..].fill(0.0);

        // What the control layer has to see: control changes to map, and
        // notes that were recorded. After the render, so a note reported here
        // has already sounded. Two slots are left for the position and
        // metering events below, which are this block's own truth and must
        // not be crowded out by a desk sending a fader stream; what does not
        // fit waits in the renderer for the next block.
        while self.evt_tx.slots() > 2 {
            let Some(event) = self.render.pop_outgoing() else {
                break;
            };
            let _ = self.evt_tx.push(event);
        }
        let _ = self.evt_tx.push(EngineEvent::Position {
            tick: report.position_tick,
            beat_in_bar: report.beat_in_bar,
            playing: report.playing,
        });
        let _ = self.evt_tx.push(EngineEvent::Metering {
            peak_l: report.peak_l,
            peak_r: report.peak_r,
        });
        // The difference, not the fact of one. Several xruns can land between
        // two callbacks -- a burst is the normal shape of the fault -- and
        // reporting the change as a single event is what made a run of
        // dropouts read as one line in the log.
        let xruns = self.xrun_count.load(Ordering::Relaxed);
        if xruns != self.last_seen_xruns {
            let count = xruns.saturating_sub(self.last_seen_xruns);
            self.last_seen_xruns = xruns;
            let _ = self.evt_tx.push(EngineEvent::Xrun {
                count: u32::try_from(count).unwrap_or(u32::MAX),
            });
        }
        // Last, so the figure covers everything the callback does and not
        // just the render. The budget is this block's own: a driver may
        // change the buffer size under a running stream.
        let budget = (frames as u64)
            .saturating_mul(1_000_000_000)
            .checked_div(u64::from(self.sample_rate))
            .unwrap_or(0);
        self.load
            .record(entered.elapsed().as_nanos() as u64, budget, period);
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
