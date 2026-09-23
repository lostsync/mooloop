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
use crate::{PreparedProject, RealtimeCommand, StructuralCommand, StructuralReclaim};

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
    /// The renderer this executor is running, for tests outside this module
    /// that ask what a command did to it.
    #[cfg(test)]
    pub(crate) fn render(&self) -> &RenderState {
        &self.render
    }

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
    // Tests only, as of `audio-recording/01`: Core Audio was the last caller
    // without an input, and now that it has one, both drivers go through
    // `process_with_input`. Kept because it is the contract a driver with no
    // input renders against, and because the executor's own tests are written
    // against a block with nothing coming in.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn process<'m>(
        &mut self,
        midi: impl IntoIterator<Item = (MidiPortId, u32, &'m [u8])>,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        self.process_with_input(midi, &[], &[], out_l, out_r);
    }

    /// [`Self::process_with_input`], with a panic anywhere inside it costing
    /// one block of silence rather than the audio for the rest of the
    /// session. What every driver calls.
    ///
    /// Without it, a device that panics in `process` unwinds into the
    /// driver: jack-rs catches it, marks the client invalid and returns
    /// `Quit`, so JACK stops calling it for good, and the interface goes on
    /// taking edits against an engine that will never render again (R1 in
    /// `reports/teams-2026-09-22.md`). Here the block is silenced, the fault
    /// is counted in the load meter for the interface to report, and the
    /// next block renders -- with the same device, which may panic again.
    /// That is a silent channel the user can find and remove, not a dead
    /// program.
    ///
    /// The panic payload is the one allocation this has to answer for. The
    /// unwind has already allocated on this thread, but freeing is still
    /// kept off it: the payload leaves through the reclaim ring when there is
    /// room, and is leaked otherwise, as jack-rs does with its own.
    pub(crate) fn process_contained<'m>(
        &mut self,
        midi: impl IntoIterator<Item = (MidiPortId, u32, &'m [u8])>,
        in_l: &[f32],
        in_r: &[f32],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.process_with_input(midi, in_l, in_r, out_l, out_r);
        }));
        if let Err(payload) = rendered {
            out_l.fill(0.0);
            out_r.fill(0.0);
            self.load.record_fault();
            // A panic mid-block can leave this block's timing half-written;
            // the next block starts a fresh wake-up measurement.
            self.last_entered = None;
            if let Err(rtrb::PushError::Full(StructuralReclaim::PanicPayload(payload))) =
                self.reclaim_tx.push(StructuralReclaim::PanicPayload(payload))
            {
                std::mem::forget(payload);
            }
        }
    }

    /// [`Self::process`] with the driver's audio input for this block
    /// (`audio-recording/01`). The input is copied into the renderer's input
    /// bus before anything renders, so a take reading it sees this block's
    /// input. Empty slices mean no input, and the bus stays silent.
    pub(crate) fn process_with_input<'m>(
        &mut self,
        midi: impl IntoIterator<Item = (MidiPortId, u32, &'m [u8])>,
        in_l: &[f32],
        in_r: &[f32],
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
            // Once per thread, and on the first block only when the driver
            // gave no earlier chance: see `prepare_audio_thread`.
            prepare_audio_thread();
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
                RealtimeCommand::Deferred { when, command } => {
                    // Parked in renderer state rather than held here: it has
                    // to survive a project install, and the renderer is what
                    // carries performance state across one.
                    self.render.defer_command(when, command);
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
                    // A removal waits for its device to fade out of the path
                    // (MOO-108), and holds what is behind it in order.
                    if let StructuralCommand::RemoveEffect { target, slot } = &command {
                        if !self.render.effect_removal_ready(*target, *slot, frames) {
                            self.pending_command = Some(RealtimeCommand::Structural(command));
                            break;
                        }
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
            // **Read at the swap, not at preparation.** The song has gone on
            // playing while the incoming renderer was built on the control
            // thread, so the outgoing renderer is the only place the current
            // position, the held keys and the open take notes exist. Copying
            // a position captured earlier would step the song backwards by
            // however long the install took, and a key pressed during the
            // install would not be in a set captured before it.
            // Destructured here and not before the capacity check, which
            // needs the struct whole to put it back on `pending_command`.
            // Taking the fields apart is what keeps the arm realtime-safe:
            // `carry` owns four `Vec`s, and anything still owned by
            // `prepared` when this block ends is freed by drop glue, on this
            // thread. Every field leaves by hand, and the plan leaves through
            // the reclaim ring with the generation it was made against.
            let PreparedProject {
                generation,
                mut render,
                keep_transport,
                carry,
            } = prepared;
            if keep_transport {
                render.adopt_performance_state(&self.render);
            }
            // Before the swap, because both generations have to be reachable:
            // the live strips are moved into the incoming state and the ones
            // built for it go back, to leave with the retired generation and
            // be freed off this thread.
            render.carry_strips_from(&mut self.render, &carry);
            let retired = std::mem::replace(&mut self.render, render);
            match self
                .reclaim_tx
                .push(StructuralReclaim::RenderState { retired, carry })
            {
                Ok(()) => {}
                Err(_) => unreachable!("reclaim capacity checked before project swap"),
            }
            let _ = self
                .evt_tx
                .push(EngineEvent::ProjectInstalled { generation });
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

        self.render.load_input(in_l, in_r, frames);
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
/// with no single attributable cause. Both the x86_64 (MXCSR) and aarch64
/// (FPCR) control registers below are per-thread, so this must run on the
/// realtime callback's own thread rather than at engine construction.
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

/// aarch64's equivalent of the x86_64 arm above: bit 24 (FZ) of FPCR, the
/// 64-bit floating-point control register the ARM ARM documents. Apple
/// silicon (the Core Audio driver's own architecture, see
/// `reports/fable-2026-09-22.md` finding 6) handles subnormals in hardware
/// at little cost, so this closes a gap in the stated contract more than a
/// measured one -- but a plugin host or a Linux aarch64 build is not
/// guaranteed the same, and nothing here should assume it.
///
/// There is no separate DAZ bit to set at this width: FZ alone flushes both
/// subnormal inputs and subnormal outputs for the single- and
/// double-precision instructions this engine uses. `FZ16` (bit 19), which
/// would do the same for half-precision arithmetic, does not apply -- the
/// DSP graph never runs in `f16`.
#[cfg(target_arch = "aarch64")]
#[inline]
fn enable_flush_to_zero() {
    use std::arch::asm;
    const FLUSH_TO_ZERO: u64 = 1 << 24;
    unsafe {
        let mut fpcr: u64;
        asm!("mrs {0}, fpcr", out(reg) fpcr, options(nostack, preserves_flags));
        fpcr |= FLUSH_TO_ZERO;
        asm!("msr fpcr, {0}", in(reg) fpcr, options(nostack, preserves_flags));
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
#[inline]
fn enable_flush_to_zero() {}

/// Do, on the audio thread, the one-time work its first block would
/// otherwise do by allocating.
///
/// `arc-swap` keeps a per-thread "debt" node, allocated (128 bytes) the first
/// time a thread loads *any* `ArcSwap` and reused for the life of the
/// thread. The sampler loads its channel's audio that way on every trigger,
/// so on a fresh callback thread the first note a sampler plays mallocs --
/// found by the engine soak (MOO-113), which no per-path test had reached
/// because each warmed its thread first.
///
/// A driver with a thread-start hook (JACK's `thread_init`) should call this
/// there, before the first callback. The executor also calls it at the top of
/// the first block it runs on a thread, so a driver without one allocates
/// once, in a block with nothing yet playing, rather than mid-song on a note.
pub(crate) fn prepare_audio_thread() {
    let warm: arc_swap::ArcSwapOption<()> = arc_swap::ArcSwapOption::empty();
    drop(warm.load());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::RenderState;
    use mooloop_core::{EngineCommand, Project};

    const SAMPLE_RATE: u32 = 48_000;
    const BLOCK: usize = 256;

    /// An executor with nothing in flight, plus the ends of its rings.
    fn executor() -> (
        Executor,
        rtrb::Producer<RealtimeCommand>,
        Consumer<StructuralReclaim>,
    ) {
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(8);
        let (evt_tx, _evt_rx) = rtrb::RingBuffer::new(8);
        let (reclaim_tx, reclaim_rx) = rtrb::RingBuffer::new(8);
        let render = Box::new(RenderState::from_project(
            SAMPLE_RATE,
            &Project::default(),
            &[],
        ));
        let executor = Executor::new(
            ExecutorIo {
                cmd_rx,
                evt_tx,
                reclaim_tx,
            },
            render,
            Arc::new(AtomicU64::new(0)),
            SAMPLE_RATE,
            LoadMeters::new(),
        );
        (executor, cmd_tx, reclaim_rx)
    }

    fn prepared(generation: u64, keep_transport: bool) -> PreparedProject {
        PreparedProject {
            generation,
            render: Box::new(RenderState::from_project(
                SAMPLE_RATE,
                &Project::default(),
                &[],
            )),
            keep_transport,
            // The transport tests are about the clock, not the strips.
            carry: crate::CarryPlan::default(),
        }
    }

    /// **A structural edit must not stop the song.** `LOOSE_ENDS.md`, "Every
    /// structural edit stops the song": moving one channel halted the
    /// transport and rewound the arrangement, including for the channels the
    /// edit never touched.
    ///
    /// The position is asserted to be *past* where it was when the install was
    /// queued, not merely non-zero, because that is the half the flag exists
    /// for. The song goes on playing while the incoming renderer is built on
    /// the control thread, so a position captured at preparation time would
    /// step the song backwards by the length of its own install. The executor
    /// reads the outgoing transport at the moment it swaps.
    #[test]
    fn a_kept_transport_carries_the_position_as_of_the_swap() {
        let (mut executor, mut cmd_tx, _reclaim) = executor();
        let mut out_l = [0.0f32; BLOCK];
        let mut out_r = [0.0f32; BLOCK];

        executor.render.play();
        for _ in 0..4 {
            executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        }
        let queued_at = executor.render.transport().position_ticks;
        assert!(queued_at > 0.0, "the song has to be somewhere to be carried");

        // One more block runs *after* the install is queued and before it is
        // consumed, which is what a real install does.
        cmd_tx
            .push(RealtimeCommand::InstallProject(prepared(1, true)))
            .expect("room in the ring");
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);

        assert!(executor.render.transport().playing, "the install stopped the song");
        assert!(
            executor.render.transport().position_ticks > queued_at,
            "the song jumped back to where it was when the install was prepared: \
             {} against {queued_at}",
            executor.render.transport().position_ticks
        );
    }

    /// The swap carries the held keys, and it carries them **under the same
    /// flag as the transport**.
    ///
    /// The render-level twin of this
    /// (`a_swap_carries_the_keys_that_are_down_and_the_notes_being_taken`)
    /// proves the copy; this proves the executor asks for it, and that an
    /// *open* -- a document load, a new song -- still starts from nothing.
    /// Getting that half backwards would leave a key held from the previous
    /// song sounding into the one just opened.
    #[test]
    fn a_kept_install_carries_the_held_keys_and_a_cleared_one_does_not() {
        use crate::render::MidiRouting;
        use mooloop_core::{
            MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage, MidiPortId, MidiRouteSource,
        };

        let held_after = |keep_transport: bool| {
            let (mut executor, mut cmd_tx, _reclaim) = executor();
            let mut out_l = [0.0f32; BLOCK];
            let mut out_r = [0.0f32; BLOCK];
            drop(executor.render.set_midi_routing(Box::new(MidiRouting {
                routes: vec![MidiInputRoute {
                    source: MidiRouteSource::AllPorts,
                    channel: MidiChannelFilter::Omni,
                }],
            })));
            executor.render.play();
            executor.render.apply_midi(&[MidiMessage {
                offset: 0,
                port: MidiPortId::FIRST,
                channel: 0,
                kind: MidiKind::NoteOn {
                    note: 60,
                    velocity: 90,
                },
            }]);
            executor.process(std::iter::empty(), &mut out_l, &mut out_r);
            assert!(executor.render.key_is_held(60, 0), "the premise: a key is down");

            cmd_tx
                .push(RealtimeCommand::InstallProject(prepared(1, keep_transport)))
                .expect("room in the ring");
            executor.process(std::iter::empty(), &mut out_l, &mut out_r);
            executor.render.any_key_is_held()
        };

        assert!(held_after(true), "an edit dropped a key that was still down");
        assert!(
            !held_after(false),
            "an open started holding a key from the song it replaced"
        );
    }

    /// **An install no longer silences the song.** The measurement from
    /// 2026-09-17, inverted by this step exactly as its doc comment said it
    /// should be.
    ///
    /// Adam listened to step 04 and reported that a channel move kept time
    /// but dropped audio. Measured at the executor, a *null* install -- the
    /// same project, nothing moved -- silenced the master as completely as a
    /// reorder did, because the incoming renderer was a fresh graph and every
    /// voice, tail and delay line left with the outgoing one. That was on
    /// every channel, not the moved one; a drum channel merely hid it,
    /// because its next hit arrives within a step.
    ///
    /// Now a channel whose id and setup are unchanged has its live strip
    /// moved across, so both cases go on sounding. The `without_carry` arm is
    /// kept beside them as the control: it is the old behaviour, and it shows
    /// the difference is the carry rather than anything else about the block.
    #[test]
    fn an_install_keeps_the_voices_of_every_channel_it_did_not_change() {
        let project = two_held_notes();
        let baseline = held_note_rms(&project, None);
        assert!(baseline > 1e-3, "nothing was sounding to measure: {baseline}");

        let after_null = held_note_rms(&project, Some(project.clone()));
        let mut reordered = project.clone();
        reordered.move_channel(0, 1).expect("a real move");
        let after_move = held_note_rms(&project, Some(reordered.clone()));

        for (what, level) in [("a null install", after_null), ("a reorder", after_move)] {
            assert!(
                level > baseline * 0.5,
                "{what} cut the song: {level} against a baseline of {baseline}"
            );
        }

        // The control: the same swap, refusing to carry, is the behaviour
        // this step replaced.
        let without_carry = held_note_rms_with(&project, Some(reordered), false);
        assert_eq!(
            without_carry, 0.0,
            "the comparison is not measuring what it claims to"
        );
    }

    /// **A carried strip must read the incoming generation's audio slot.**
    ///
    /// Every install builds a fresh `ChannelAudioBank`, and a strip binds to
    /// its slot when it is constructed. A strip that survives an install is
    /// therefore still holding the *retired* generation's slot, at the index
    /// it used to occupy -- and the handle publishes into the new bank. Left
    /// alone, a sample loaded onto that channel after a reorder would land
    /// somewhere the strip never looks: the channel would go on playing the
    /// old file, indefinitely, with nothing anywhere to say why.
    ///
    /// It sounds right at the moment of the swap, which is what makes it the
    /// dangerous kind of bug, so this asserts against the slot rather than
    /// against the audio.
    ///
    /// Samplers, because only a sampler holds a slot: since MOO-56 a strip
    /// running another kind has no sampler resident to bind one.
    #[test]
    fn a_carried_strip_reads_the_new_generations_audio_slot() {
        let mut project = two_held_notes();
        for (index, channel) in project.channels.iter_mut().enumerate() {
            channel.setup = mooloop_core::ChannelSetup::sampler(format!("held {index}"));
        }
        let mut reordered = project.clone();
        reordered.move_channel(0, 1).expect("a real move");

        let mut live = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        let mut incoming = RenderState::from_project(SAMPLE_RATE, &reordered, &[]);
        // The banks are per generation, so no slot is shared between them.
        let wanted = incoming.audio_slot_ptr(1);
        assert_ne!(
            wanted,
            live.audio_slot_ptr(0),
            "the two generations were handed the same bank, so this proves nothing"
        );

        incoming.carry_strips_from(&mut live, &crate::carry_plan(&project, &reordered));

        assert_eq!(
            incoming.strip_audio_slot_ptr(1),
            wanted,
            "the carried strip kept the retired generation's slot, so anything \
             published for it after this install would be inaudible"
        );
    }

    /// **The swap allocates and frees nothing on the audio thread.**
    ///
    /// The whole point of deciding the carry on the control thread is that
    /// this one does no work beyond moving boxes: no comparison, no
    /// reasoning, and above all no allocation. A carried strip is a pointer
    /// exchanged with the one built for it, and the strip it displaces leaves
    /// through the reclaim ring to be dropped elsewhere. So does the plan
    /// itself, which rides out with the retired generation and is freed on
    /// the control thread.
    ///
    /// **This measures the swap and only the swap.** The install around it --
    /// the `PreparedProject` that owns the plan, and what the callback does
    /// with the fields it does not move -- is
    /// [`installing_a_project_allocates_and_frees_nothing`].
    ///
    /// Measured rather than reasoned, in the manner of
    /// `no_buffer_operation_allocates_on_the_callback`. A floor rather than a
    /// ceiling: it proves this path does not allocate, not that no path does.
    #[test]
    fn carrying_strips_allocates_nothing() {
        let mut project = two_routed_notes();
        // A send, so the send bank's edge matching is on the measured path.
        project.buses[1].sends.push(mooloop_core::AuxSend::new(2));
        let mut reordered = project.clone();
        reordered.move_channel(0, 1).expect("a real move");
        reordered.move_track(1, 2).expect("a real move");
        let plan = crate::carry_plan(&project, &reordered);
        assert!(
            !plan.channels.is_empty() && plan.tracks.len() > 1,
            "nothing would be carried, so nothing is measured: {plan:?}"
        );

        // Everything that allocates happens before the counter is read.
        let mut live = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        let mut incoming = RenderState::from_project(SAMPLE_RATE, &reordered, &[]);

        let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        incoming.carry_strips_from(&mut live, &plan);
        let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());

        assert_eq!(
            after, before,
            "the swap allocated or freed on the thread that would be the callback"
        );
    }

    /// **The driver's input reaches the input bus**, whole, before anything
    /// renders, and a block with no input leaves it silent -- all without
    /// allocating (`audio-recording/01`).
    #[test]
    fn a_blocks_input_reaches_the_input_bus_without_allocating() {
        let (mut executor, _cmd_tx, _reclaim) = executor();
        let mut out_l = [0.0f32; BLOCK];
        let mut out_r = [0.0f32; BLOCK];
        let in_l: Vec<f32> = (0..BLOCK).map(|i| i as f32 / BLOCK as f32).collect();
        let in_r: Vec<f32> = in_l.iter().map(|x| -x).collect();
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);

        let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        executor.process_with_input(std::iter::empty(), &in_l, &in_r, &mut out_l, &mut out_r);
        let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        assert_eq!(after, before, "the input allocated on the callback");
        assert_eq!(&executor.render.input_bus().l[..BLOCK], &in_l[..]);
        assert_eq!(&executor.render.input_bus().r[..BLOCK], &in_r[..]);

        executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        assert!(executor.render.input_bus().l[..BLOCK].iter().all(|x| *x == 0.0));
    }

    /// **The whole install allocates and frees nothing on the audio thread.**
    ///
    /// [`carrying_strips_allocates_nothing`] measures a bare
    /// `carry_strips_from` on two loose renderers. That is the swap, and the
    /// swap was never the problem: the `PreparedProject` the callback takes
    /// off the command ring owns the [`crate::CarryPlan`] as well as the
    /// renderer, so whatever the arm leaves un-moved is freed by drop glue at
    /// the end of the block -- four `Vec`s, on every structural edit. A test
    /// that never builds a `PreparedProject` inside its measured window
    /// cannot see that, which is AGENTS.md's one-sided guard.
    ///
    /// So this one measures `Executor::process` itself, with a real install
    /// queued and a plan whose vectors are genuinely non-empty, and asserts
    /// both counters. The plan must leave with the retired generation through
    /// the reclaim ring and be freed on the control thread.
    #[test]
    fn installing_a_project_allocates_and_frees_nothing() {
        let mut project = two_routed_notes();
        // A send, so the send bank's edge matching is on the measured path.
        project.buses[1].sends.push(mooloop_core::AuxSend::new(2));
        let mut reordered = project.clone();
        reordered.move_channel(0, 1).expect("a real move");
        reordered.move_track(1, 2).expect("a real move");
        let plan = crate::carry_plan(&project, &reordered);
        let populated = [
            &plan.channels,
            &plan.tracks,
            &plan.channel_seats,
            &plan.track_seats,
        ]
        .into_iter()
        .filter(|vectors| !vectors.is_empty())
        .count();
        assert!(
            populated > 0,
            "every vector in the plan is empty, so nothing would be freed and \
             this test would pass whatever the executor does: {plan:?}"
        );

        // Not the `executor()` helper: its renderer is built from
        // `Project::default()`, and the plan's indices are `project`'s.
        let (mut cmd_tx, cmd_rx) = rtrb::RingBuffer::new(8);
        let (evt_tx, _evt_rx) = rtrb::RingBuffer::new(8);
        let (reclaim_tx, _reclaim_rx) = rtrb::RingBuffer::new(8);
        let mut executor = Executor::new(
            ExecutorIo {
                cmd_rx,
                evt_tx,
                reclaim_tx,
            },
            Box::new(RenderState::from_project(SAMPLE_RATE, &project, &[])),
            Arc::new(AtomicU64::new(0)),
            SAMPLE_RATE,
            LoadMeters::new(),
        );
        let mut out_l = [0.0f32; BLOCK];
        let mut out_r = [0.0f32; BLOCK];
        // Warm up, so block state initialised on first use is not counted.
        executor.render.play();
        for _ in 0..40 {
            executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        }

        // Not the `prepared()` helper: it hardcodes `CarryPlan::default()`,
        // whose four empty vectors free nothing. Everything that allocates
        // happens before the counter is read, the ring included.
        cmd_tx
            .push(RealtimeCommand::InstallProject(PreparedProject {
                generation: 1,
                render: Box::new(RenderState::from_project(SAMPLE_RATE, &reordered, &[])),
                keep_transport: true,
                carry: plan,
            }))
            .expect("room in the ring");

        let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());

        assert_eq!(
            after, before,
            "the install allocated or freed on the audio thread: \
             {} allocations and {} frees, against a plan with {populated} \
             non-empty vectors",
            after.0 - before.0,
            after.1 - before.1
        );
    }

    /// **A routing change frees nothing on the callback.** The table it
    /// replaces leaves through the reclaim ring -- the reason the routings are
    /// structural commands rather than `ArcSwap` cells, one of which could be
    /// last-owned by a guard on this thread (`reports/fable-2026-09-19.md`,
    /// finding 2).
    #[test]
    fn a_routing_change_frees_nothing_on_the_callback() {
        let (mut executor, mut cmd_tx, _reclaim) = executor();
        let mut out_l = [0.0f32; BLOCK];
        let mut out_r = [0.0f32; BLOCK];
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        cmd_tx
            .push(RealtimeCommand::Structural(
                crate::StructuralCommand::SetMidiRouting(Box::new(crate::render::MidiRouting {
                    routes: vec![mooloop_core::MidiInputRoute::default(); 4],
                })),
            ))
            .expect("room in the ring");
        cmd_tx
            .push(RealtimeCommand::Structural(
                crate::StructuralCommand::SetAudioInputRouting(Box::new(
                    crate::render::AudioInputRouting {
                        taps: vec![Some(mooloop_core::AudioTap::Master); 4],
                    },
                )),
            ))
            .expect("room in the ring");
        let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        assert_eq!(after, before, "a routing change freed on the callback");
    }

    /// A channel-0 strip destination, and the same for a device.
    fn strip_lane(param: u32) -> mooloop_core::ParamAddr {
        mooloop_core::ParamAddr::strip(mooloop_core::EffectTarget::Channel(0), param)
    }

    /// Render one block with `commands` in the ring and answer what the
    /// callback allocated and freed doing it.
    ///
    /// The warm-up matters: a first block initialises block state, and the
    /// ring itself is written before the counter is read.
    fn allocation_of(
        executor: &mut Executor,
        cmd_tx: &mut rtrb::Producer<RealtimeCommand>,
        commands: Vec<RealtimeCommand>,
    ) -> (usize, usize) {
        let mut out_l = [0.0f32; BLOCK];
        let mut out_r = [0.0f32; BLOCK];
        for _ in 0..4 {
            executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        }
        for command in commands {
            cmd_tx.push(command).expect("room in the ring");
        }
        let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        (after.0 - before.0, after.1 - before.1)
    }

    /// **Opening an automation lane allocates nothing on the callback.**
    ///
    /// It used to allocate twice over: `AutomationLane::new` is a
    /// `Vec::with_capacity(MAX_AUTOMATION_POINTS_PER_LANE)` -- 12 KB -- and
    /// `UpsertAutomationPoint` opens the lane the same way, so the first
    /// breakpoint drawn on a destination that had none paid for it again.
    /// The lane bank now takes its point storage from a pool filled at
    /// install (`reports/fable-2026-09-21.md`, finding 1).
    #[test]
    fn opening_a_lane_allocates_nothing_on_the_callback() {
        let (mut executor, mut cmd_tx, _reclaim) = executor();
        let (allocations, frees) = allocation_of(
            &mut executor,
            &mut cmd_tx,
            vec![
                RealtimeCommand::Engine(EngineCommand::OpenAutomationLane {
                    pattern: 0,
                    channel: 0,
                    target: strip_lane(0),
                }),
                RealtimeCommand::Engine(EngineCommand::UpsertAutomationPoint {
                    pattern: 0,
                    channel: 0,
                    target: strip_lane(1),
                    point: mooloop_core::AutomationPoint::new(1, 0, 0.5),
                }),
            ],
        );
        assert_eq!(
            (allocations, frees),
            (0, 0),
            "opening a lane allocated or freed on the callback"
        );
    }

    /// **Closing an automation lane frees nothing on the callback.** The slot
    /// keeps its point storage for the next lane that lands in it, rather
    /// than the 12 KB vector being dropped inside the audio callback.
    #[test]
    fn removing_a_lane_frees_nothing_on_the_callback() {
        let (mut executor, mut cmd_tx, _reclaim) = executor();
        let _ = allocation_of(
            &mut executor,
            &mut cmd_tx,
            vec![RealtimeCommand::Engine(EngineCommand::OpenAutomationLane {
                pattern: 0,
                channel: 0,
                target: strip_lane(0),
            })],
        );
        let (allocations, frees) = allocation_of(
            &mut executor,
            &mut cmd_tx,
            vec![RealtimeCommand::Engine(
                EngineCommand::RemoveAutomationLane {
                    pattern: 0,
                    channel: 0,
                    target: strip_lane(0),
                },
            )],
        );
        assert_eq!(
            (allocations, frees),
            (0, 0),
            "closing a lane allocated or freed on the callback"
        );
    }

    /// **Removing an effect frees no lane on the callback.** Its lanes go
    /// with it -- `ChannelPattern::forget_device`, which used to be a
    /// `Vec::retain` under a comment claiming it did not allocate, and which
    /// freed every point vector it dropped.
    #[test]
    fn removing_an_effect_with_lanes_frees_nothing_on_the_callback() {
        let mut project = Project::default();
        project.channels[0]
            .setup
            .push_effect(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Filter,
            ))
            .expect("room in the chain");
        let device = project.channels[0].setup.effects[0].id;
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(8);
        let (evt_tx, _evt_rx) = rtrb::RingBuffer::new(8);
        let (reclaim_tx, _reclaim_rx) = rtrb::RingBuffer::new(8);
        let mut cmd_tx = cmd_tx;
        let mut executor = Executor::new(
            ExecutorIo {
                cmd_rx,
                evt_tx,
                reclaim_tx,
            },
            Box::new(RenderState::from_project(SAMPLE_RATE, &project, &[])),
            Arc::new(AtomicU64::new(0)),
            SAMPLE_RATE,
            LoadMeters::new(),
        );
        let target = mooloop_core::EffectTarget::Channel(0);
        let _ = allocation_of(
            &mut executor,
            &mut cmd_tx,
            vec![RealtimeCommand::Engine(EngineCommand::OpenAutomationLane {
                pattern: 0,
                channel: 0,
                target: mooloop_core::ParamAddr::effect(target, device, 0),
            })],
        );
        let (_, frees) = allocation_of(
            &mut executor,
            &mut cmd_tx,
            vec![RealtimeCommand::Structural(
                crate::StructuralCommand::RemoveEffect { target, slot: 0 },
            )],
        );
        assert_eq!(frees, 0, "removing an effect freed a lane on the callback");
    }

    /// **Adding a channel frees no lane on the callback.** The seat it takes
    /// is cleared across the pattern bank first, and every lane in it used to
    /// be dropped there -- up to 256 patterns by eight lanes, in one block
    /// (`reports/fable-2026-09-21.md`, finding 1).
    ///
    /// Measured as a difference rather than against zero, because the command
    /// frees one allocation of its own -- the modulation rack the seat had --
    /// which is a separate matter recorded in `docs/LOOSE_ENDS.md`. What this
    /// test holds is that the automation in the seat costs nothing to clear.
    #[test]
    fn adding_a_channel_frees_no_lane_on_the_callback() {
        fn add_channel_frees(lanes: u32) -> usize {
            let (mut executor, mut cmd_tx, _reclaim) = executor();
            for param in 0..lanes {
                let _ = allocation_of(
                    &mut executor,
                    &mut cmd_tx,
                    vec![RealtimeCommand::Engine(EngineCommand::OpenAutomationLane {
                        pattern: 0,
                        channel: 1,
                        target: mooloop_core::ParamAddr::strip(
                            mooloop_core::EffectTarget::Channel(1),
                            param,
                        ),
                    })],
                );
            }
            let slot = crate::render::empty_channel_audio_bank()[1].clone();
            let storage = crate::render::RenderState::build_channel(
                slot,
                mooloop_core::DeviceKind::Sampler,
                SAMPLE_RATE,
            );
            allocation_of(
                &mut executor,
                &mut cmd_tx,
                vec![RealtimeCommand::Structural(
                    crate::StructuralCommand::AddChannel { storage },
                )],
            )
            .1
        }

        let bare = add_channel_frees(0);
        let with_lanes = add_channel_frees(mooloop_core::MAX_AUTOMATION_LANES_PER_CHANNEL as u32);
        assert_eq!(
            with_lanes, bare,
            "adding a channel freed the seat's automation lanes on the callback"
        );
    }

    /// The channel the edit was *about* is still rebuilt, and still cuts.
    ///
    /// That is the deliberate limit: a chain that changed needs new nodes, and
    /// there is nothing of its old sound to carry into them. It is also the
    /// channel the user just edited, which is the one place a discontinuity is
    /// least surprising.
    #[test]
    fn a_channel_whose_chain_changed_is_rebuilt() {
        let project = two_held_notes();
        let mut edited = project.clone();
        edited.channels[0]
            .setup
            .push_effect(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Filter,
            ))
            .expect("room in the chain");

        let plan = crate::carry_plan(&project, &edited);
        assert_eq!(
            plan.channels,
            vec![(1, 1)],
            "only the untouched channel should be carried"
        );
    }

    /// A deleted channel is not carried, and the survivors are -- at their new
    /// seats, which is the case a positional scheme could not express at all.
    #[test]
    fn a_deleted_channel_is_dropped_and_the_rest_move_down() {
        let mut project = two_held_notes();
        project
            .insert_channel(2, mooloop_core::ProjectChannel::mono_synth(2, 1))
            .expect("room");
        let gone = project.channels[0].id;

        let mut after = project.clone();
        after.remove_channel(0).expect("channel 0 exists");

        let plan = crate::carry_plan(&project, &after);
        assert_eq!(
            plan.channels,
            vec![(1, 0), (2, 1)],
            "the survivors did not follow their channels down a seat"
        );
        assert!(
            after.channel_index(gone).is_none(),
            "the deleted channel is gone, so nothing can carry its strip"
        );
    }

    /// **A track move carries the channels that feed the tracks it moved**,
    /// and each carried strip sums into its track's new seat.
    ///
    /// A track is a seat, so moving one renumbers `channel.bus` on every
    /// channel routed to it. The carry used to compare that field, so those
    /// channels were rebuilt and cut -- half of the glitch Adam heard on a
    /// track move on 2026-09-18. The seat assertion is the other half of the
    /// fix: a strip carried without its destination would go on feeding the
    /// seat its track had vacated, which is some *other* track now.
    #[test]
    fn a_track_move_carries_its_feeders_to_the_new_seat() {
        let project = two_routed_notes();
        let mut moved = project.clone();
        moved.move_track(1, 2).expect("a real move");
        assert_eq!(
            (moved.channels[0].setup.channel.bus, moved.channels[1].setup.channel.bus),
            (2, 1),
            "the premise: the move renumbered both channels' destinations"
        );

        let plan = crate::carry_plan(&project, &moved);
        assert_eq!(plan.channels, vec![(0, 0), (1, 1)], "a track move rebuilt a channel strip");

        let mut live = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        let mut incoming = RenderState::from_project(SAMPLE_RATE, &moved, &[]);
        incoming.carry_strips_from(&mut live, &plan);
        assert_eq!(
            (incoming.strip_destination(0), incoming.strip_destination(1)),
            (2, 1),
            "a carried strip kept feeding the seat its track moved out of"
        );
    }

    /// The same move, measured at the master rather than in the plan.
    #[test]
    fn a_track_move_keeps_the_voices_routed_through_it() {
        let project = two_routed_notes();
        let baseline = held_note_rms(&project, None);
        assert!(baseline > 1e-3, "nothing was sounding to measure: {baseline}");

        let mut moved = project.clone();
        moved.move_track(1, 2).expect("a real move");
        let after = held_note_rms(&project, Some(moved.clone()));
        assert!(
            after > baseline * 0.5,
            "a track move cut the song: {after} against a baseline of {baseline}"
        );

        let without_carry = held_note_rms_with(&project, Some(moved), false);
        assert_eq!(
            without_carry, 0.0,
            "the comparison is not measuring what it claims to"
        );
    }

    /// **A track move carries the tracks too**, matched by `TrackId`: every
    /// one of them, at its new seat. `incremental-structure/02`.
    #[test]
    fn a_track_move_carries_every_track_to_its_new_seat() {
        let project = two_routed_notes();
        let mut moved = project.clone();
        moved.move_track(1, 2).expect("a real move");

        let plan = crate::carry_plan(&project, &moved);
        assert_eq!(plan.tracks, vec![(0, 0), (2, 1), (1, 2)]);
    }

    /// A track whose own setup changed is rebuilt, and only that one -- the
    /// channel rule, one list over.
    #[test]
    fn a_track_whose_chain_changed_is_rebuilt() {
        let project = two_routed_notes();
        let mut edited = project.clone();
        edited.buses[2]
            .push_effect(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Filter,
            ))
            .expect("room in the chain");

        let plan = crate::carry_plan(&project, &edited);
        assert_eq!(plan.tracks, vec![(0, 0), (1, 1)]);
    }

    /// **A track's tail survives a track move.** The other half of the glitch
    /// Adam heard on 2026-09-18: with the channels carried, what still cut
    /// was each track's own strip -- here a reverb ringing after its note has
    /// ended, which a rebuilt track would start from silence.
    ///
    /// Measured three ways against one swap: carrying everything, carrying
    /// only the channels (what the engine did before this step), and not
    /// swapping at all. The first must sound like the third.
    #[test]
    fn a_track_move_keeps_the_tail_on_the_track() {
        let mut project = two_routed_notes();
        for channel in &mut project.channels {
            for note in &mut channel.notes[0] {
                note.duration_ticks = 24;
            }
        }
        let mut reverb = mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Reverb);
        reverb.wet_dry = 1.0;
        project.buses[1].push_effect(reverb).expect("room in the chain");
        let mut moved = project.clone();
        moved.move_track(1, 2).expect("a real move");

        let untouched = rms_after_install(&project, None);
        let full = crate::carry_plan(&project, &moved);
        let channels_only = crate::CarryPlan {
            tracks: Vec::new(),
            ..full.clone()
        };
        let carried = rms_after_install(&project, Some((moved.clone(), full)));
        let rebuilt = rms_after_install(&project, Some((moved, channels_only)));

        assert!(untouched > 1e-4, "no tail to measure: {untouched}");
        assert!(
            (carried - untouched).abs() <= untouched * 0.01,
            "the carried track does not sound like the untouched one: \
             {carried} against {untouched}"
        );
        assert!(
            rebuilt < untouched * 0.5,
            "rebuilding the track did not cut its tail, so this measures nothing: \
             {rebuilt} against {untouched}"
        );
    }

    /// What a carried track keeps is its own state; what the **graph**
    /// decides about it comes from the incoming project. Removing the soloed
    /// track leaves its sibling unchanged -- so carried -- but no longer
    /// silenced by anybody's solo, and a carry that kept the live verdict
    /// would leave it muted by a track that is not there.
    #[test]
    fn a_carried_track_takes_the_new_graphs_solo_verdict() {
        let mut project = two_routed_notes();
        project.buses[1].bus.solo = true;
        let mut after = project.clone();
        after.remove_track(1).expect("not the master");

        let plan = crate::carry_plan(&project, &after);
        assert!(plan.tracks.contains(&(2, 1)), "the sibling was not carried: {plan:?}");

        let mut live = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        assert!(live.track_solo_silenced(2), "the premise: the sibling starts silenced");
        let mut incoming = RenderState::from_project(SAMPLE_RATE, &after, &[]);
        incoming.carry_strips_from(&mut live, &plan);
        assert!(
            !incoming.track_solo_silenced(1),
            "a carried track kept the solo verdict of a graph that is gone"
        );
    }

    /// [`two_held_notes`], with each channel feeding a track of its own.
    fn two_routed_notes() -> Project {
        let mut project = two_held_notes();
        project.ensure_tracks(3);
        project.channels[0].setup.channel.bus = 1;
        project.channels[1].setup.channel.bus = 2;
        project
    }

    /// Two channels, each holding a long note, so a cut voice is audible
    /// rather than being hidden by the next hit.
    fn two_held_notes() -> Project {
        let mut project = Project::default();
        project.channels.push(mooloop_core::ProjectChannel::mono_synth(1, 1));
        project.pattern_lengths[0] = 16;
        for (index, channel) in project.channels.iter_mut().enumerate() {
            channel.setup = mooloop_core::ChannelSetup::mono_synth(format!("held {index}"));
            channel.notes[0].push(mooloop_core::NoteEvent::new(
                1,
                0,
                96 * 8,
                60 + index as u8 * 7,
                100,
            ));
        }
        project.assign_channel_ids();
        project
    }

    /// Render `project` until its notes are sounding, optionally swap
    /// `install` in, and report the level of the block right after.
    fn held_note_rms(project: &Project, install: Option<Project>) -> f32 {
        held_note_rms_with(project, install, true)
    }

    /// `carry` says whether the install is allowed to take the live strips
    /// over, so a test can measure both sides of the same swap.
    fn held_note_rms_with(project: &Project, install: Option<Project>, carry: bool) -> f32 {
        let install = install.map(|install| {
            let plan = if carry {
                crate::carry_plan(project, &install)
            } else {
                crate::CarryPlan::default()
            };
            (install, plan)
        });
        rms_after_install(project, install)
    }

    /// Render `project` for forty blocks, optionally swap `install` in under
    /// the given plan, and report the level of the block right after.
    fn rms_after_install(project: &Project, install: Option<(Project, crate::CarryPlan)>) -> f32 {
        let (mut cmd_tx, cmd_rx) = rtrb::RingBuffer::new(8);
        let (evt_tx, _evt_rx) = rtrb::RingBuffer::new(8);
        let (reclaim_tx, _reclaim_rx) = rtrb::RingBuffer::new(8);
        let mut executor = Executor::new(
            ExecutorIo { cmd_rx, evt_tx, reclaim_tx },
            Box::new(RenderState::from_project(SAMPLE_RATE, project, &[])),
            Arc::new(AtomicU64::new(0)),
            SAMPLE_RATE,
            LoadMeters::new(),
        );
        let mut out_l = [0.0f32; BLOCK];
        let mut out_r = [0.0f32; BLOCK];
        executor.render.play();
        for _ in 0..40 {
            executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        }
        if let Some((install, plan)) = install {
            cmd_tx
                .push(RealtimeCommand::InstallProject(PreparedProject {
                    generation: 1,
                    render: Box::new(RenderState::from_project(SAMPLE_RATE, &install, &[])),
                    keep_transport: true,
                    carry: plan,
                }))
                .expect("room in the ring");
        }
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        (out_l.iter().map(|s| s * s).sum::<f32>() / out_l.len() as f32).sqrt()
    }

    /// The other half: opening a document stops and rewinds, which is what
    /// opening a document means.
    #[test]
    fn an_install_that_does_not_keep_the_transport_stops_and_rewinds() {
        let (mut executor, mut cmd_tx, _reclaim) = executor();
        let mut out_l = [0.0f32; BLOCK];
        let mut out_r = [0.0f32; BLOCK];

        executor.render.play();
        for _ in 0..4 {
            executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        }
        assert!(executor.render.transport().position_ticks > 0.0);

        cmd_tx
            .push(RealtimeCommand::InstallProject(prepared(1, false)))
            .expect("room in the ring");
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);

        assert!(!executor.render.transport().playing);
        assert_eq!(executor.render.transport().position_ticks, 0.0);
    }

    /// A master-bus device that writes a steady level, and panics in
    /// `process` while `armed` is set.
    struct PanicsWhenArmed {
        armed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl mooloop_dsp::AudioNode for PanicsWhenArmed {
        fn process(
            &mut self,
            ctx: &mooloop_dsp::ProcessContext,
            bus: &mut mooloop_dsp::StereoBus,
            _events_in: &mooloop_dsp::EventList,
            _events_out: Option<&mut mooloop_dsp::EventList>,
        ) {
            if self.armed.load(Ordering::Relaxed) {
                panic!("a device that panics in process");
            }
            bus.l[..ctx.frames].fill(0.25);
            bus.r[..ctx.frames].fill(0.25);
        }
    }

    /// R1 in `reports/teams-2026-09-22.md`: a device that panics costs one
    /// silent block and a fault the interface can report -- and the next
    /// block renders. Before `process_contained` the panic unwound into the
    /// driver, and under JACK that ended the client for the session.
    #[test]
    fn a_device_panic_costs_one_silent_block_and_is_reported() {
        let (mut executor, mut cmd_tx, mut reclaim) = executor();
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let node: Box<dyn mooloop_dsp::AudioNode + Send> =
            Box::new(PanicsWhenArmed { armed: armed.clone() });
        cmd_tx
            .push(RealtimeCommand::Structural(crate::StructuralCommand::InstallEffect {
                target: mooloop_core::EffectTarget::Bus(0),
                slot: 0,
                kind: mooloop_core::EffectKind::Filter,
                resource_key: None,
                node,
                align: None,
                analyzer: Box::new(mooloop_dsp::SpectrumAnalyzer::new()),
                state: Box::new(crate::EffectSlot::for_device(mooloop_core::DeviceId(7))),
            }))
            .expect("room in the ring");
        let mut out_l = [0.0f32; BLOCK];
        let mut out_r = [0.0f32; BLOCK];
        let none = || std::iter::empty::<(MidiPortId, u32, &[u8])>();

        executor.process_contained(none(), &[], &[], &mut out_l, &mut out_r);
        assert!(
            out_l.iter().any(|sample| *sample != 0.0),
            "the device is not reaching the output, so this test could not see a silenced block"
        );
        assert_eq!(executor.load.take().faults, 0);

        armed.store(true, Ordering::Relaxed);
        out_l.fill(1.0);
        out_r.fill(1.0);
        executor.process_contained(none(), &[], &[], &mut out_l, &mut out_r);
        assert!(out_l.iter().chain(&out_r).all(|sample| *sample == 0.0), "the panicked block was not silenced");
        assert_eq!(executor.load.take().faults, 1, "the fault was not reported");
        assert!(
            std::iter::from_fn(|| reclaim.pop().ok())
                .any(|item| matches!(item, StructuralReclaim::PanicPayload(_))),
            "the panic payload was freed on the audio thread instead of reclaimed"
        );

        armed.store(false, Ordering::Relaxed);
        executor.process_contained(none(), &[], &[], &mut out_l, &mut out_r);
        assert!(out_l.iter().any(|sample| *sample != 0.0), "the block after the panic did not render");
        assert_eq!(executor.load.take().faults, 0);
    }
}

