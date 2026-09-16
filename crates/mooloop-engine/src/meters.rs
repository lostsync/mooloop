//! Per-bus peak publication from the realtime thread to the GUI.
//!
//! This deliberately does not use the event ring. Peaks are produced once per
//! bus per block — seventeen values every few milliseconds — which would
//! swamp a queue the UI drains on a timer, and a queue is the wrong shape
//! anyway: the GUI only ever wants the most recent reading, never the
//! backlog. A preallocated array of atomics is wait-free on both sides, costs no
//! allocation, and cannot overflow.
//!
//! The value published is a *held* peak: the audio thread raises it and only
//! the GUI's read clears it. That way a transient landing between two UI
//! frames is still seen, rather than being missed because the block that
//! contained it was already overwritten.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use mooloop_core::MAX_BUSES;
use mooloop_core::{
    modulation::CONTROL_SOURCE_SLOTS, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL, MAX_SAMPLER_VOICES,
};
use mooloop_dsp::dynamics::db_to_lin;
use mooloop_dsp::{BufferDisplay, DynamicsFrame, SPECTRUM_BINS, WAVEFORM_BINS};

/// Peak-hold cells for every bus, left and right interleaved, and beside
/// them the deepest gain reduction each track's channel strip applied.
pub struct BusMeters {
    cells: Vec<AtomicU32>,
    /// How much reduction the strip's compressor took, in dB **as a
    /// positive amount**, held until read.
    ///
    /// Positive so that `fetch_max` finds the deepest one for the same
    /// reason it finds the loudest peak: a transient landing between two UI
    /// frames should still light the lamp rather than being overwritten by
    /// the block after it. Reduction is negative everywhere else in this
    /// codebase, so the sign is flipped here and nowhere else -- at the
    /// publish, which is the one line that knows why.
    ///
    /// A strip whose compressor is out publishes nothing, so the cell falls
    /// to zero on the next read and the lamp goes dark.
    reduction: Vec<AtomicU32>,
}

/// Held input/output peaks for every visible device, plus the held detector
/// level and gain reduction of the ones that reduce gain. Stage zero is a
/// source (its input is deliberately never published); later stages are the
/// effect slots. This gives a device face a truthful pair of meters instead
/// of borrowing the master meter for every rectangle in the rack.
///
/// Targets are addressed across channels *and* buses: a channel is its own
/// index, a bus is `MAX_CHANNELS + bus index`, so a chain publishes the same
/// way no matter what owns it.
pub struct DeviceMeters {
    cells: Vec<AtomicU32>,
}

/// How many device stages may have a live spectrum at once.
///
/// A pool, not a ceiling on anything a user can make: every addressable stage
/// can still subscribe, and what is bounded is how many are drawn at the same
/// moment.
///
/// The array this replaced was `(MAX_CHANNELS + MAX_BUSES) *
/// (MAX_EFFECTS_PER_CHANNEL + 1) * SPECTRUM_BINS` -- **12.85 MB allocated
/// whether or not a single analyzer was ever switched on**, growing with two
/// ceilings multiplied together. `docs/CAPACITY_POLICY.md` is about that exact
/// shape. Sixty-four slots is twelve kilobytes, and it is what makes a *better*
/// analyzer affordable: at 256 bins the old array would have been 68 MB and
/// this one is 64 KB.
pub const SPECTRUM_SLOTS: usize = 64;

/// How many Buffer faces may have a live waveform at once.
///
/// The same shape as [`SPECTRUM_SLOTS`] and for the same reason -- an array
/// per addressable stage would be `(MAX_CHANNELS + MAX_BUSES) *
/// (MAX_EFFECTS_PER_CHANNEL + 1) * WAVEFORM_BINS` whether or not a Buffer
/// existed, which is the shape `docs/CAPACITY_POLICY.md` is about. Eight is
/// far fewer than sixty-four because the number of Buffer faces a person can
/// be *looking at* is smaller again: a spectrum is offered on every device
/// stage, and this is offered on one device kind.
pub const WAVEFORM_SLOTS: usize = 8;

/// Latest display-oriented data for every device stage.
///
/// This is intentionally a distinct transport from `EngineEvent`: analyzer
/// vectors are continuous observations, so a GUI wants the latest snapshot
/// rather than every historical frame. More display features can be added to
/// this stable device-stage address space without exposing PCM to the UI.
pub struct DeviceTelemetry {
    /// Which pool slot each device stage's spectrum lives in, plus one, so
    /// that zero means "not subscribed". One `u32` per addressable stage.
    ///
    /// **This doubles as the subscription flag**, which is what makes the
    /// pool below cost nothing to look up: the audio thread already loads
    /// this cell to decide whether to analyse at all, and the same load tells
    /// it where to publish.
    spectrum_enabled: Vec<AtomicU32>,
    /// The pool. [`SPECTRUM_SLOTS`] spectra, handed out to whichever stages
    /// are subscribed.
    spectrum: Vec<AtomicU32>,
    /// Which stage owns each pool slot, plus one; zero is free. Only the
    /// control thread allocates, so a scan and a store is enough and no
    /// compare-and-swap is needed.
    spectrum_slot_owner: Vec<AtomicU32>,
    /// Running count of retained-audio writer/read-head collisions per device
    /// stage. A monotonic counter rather than a flag: the UI compares it
    /// against the value it last saw, so a forced return landing between two
    /// GUI frames is still noticed.
    buffer_collisions: Vec<AtomicU32>,
    /// Which waveform pool slot each Buffer stage holds, plus one. Zero is
    /// unsubscribed, exactly as `spectrum_enabled` works.
    waveform_enabled: Vec<AtomicU32>,
    waveform: Vec<AtomicU32>,
    waveform_slot_owner: Vec<AtomicU32>,
    /// Head, writer and window as fractions of the ring, and a flag word.
    ///
    /// Scalars, so they are unpooled and published unconditionally: five
    /// atomic stores a block against a subscription check that would cost
    /// about as much. Only the peaks are worth a pool.
    buffer_marks: Vec<AtomicU32>,
}

/// How many scalar cells [`DeviceTelemetry::publish_buffer_display`] writes
/// per stage: head, writer, window start, window end, flags.
const BUFFER_MARKS: usize = 5;

/// Bits in the flag word.
const BUFFER_FLAG_FROZEN: u32 = 1;
const BUFFER_FLAG_ARMED: u32 = 2;
const BUFFER_FLAG_ARMED_FREEZE: u32 = 4;

/// What a Buffer face draws, read back out of the bank.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BufferMarks {
    /// Where the read head is, as a fraction of the ring, or `None` while the
    /// device is following and there is no detached head to draw.
    pub head: Option<f32>,
    pub write: f32,
    pub window: Option<(f32, f32)>,
    pub frozen: bool,
    pub armed_freeze: Option<bool>,
}

impl DeviceTelemetry {
    const TARGETS: usize = MAX_CHANNELS + MAX_BUSES;
    const STAGES: usize = MAX_EFFECTS_PER_CHANNEL + 1;

    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            spectrum_enabled: (0..Self::TARGETS * Self::STAGES)
                .map(|_| AtomicU32::new(0))
                .collect(),
            spectrum: (0..SPECTRUM_SLOTS * SPECTRUM_BINS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            spectrum_slot_owner: (0..SPECTRUM_SLOTS).map(|_| AtomicU32::new(0)).collect(),
            buffer_collisions: (0..Self::TARGETS * Self::STAGES)
                .map(|_| AtomicU32::new(0))
                .collect(),
            waveform_enabled: (0..Self::TARGETS * Self::STAGES)
                .map(|_| AtomicU32::new(0))
                .collect(),
            waveform: (0..WAVEFORM_SLOTS * WAVEFORM_BINS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            waveform_slot_owner: (0..WAVEFORM_SLOTS).map(|_| AtomicU32::new(0)).collect(),
            buffer_marks: (0..Self::TARGETS * Self::STAGES * BUFFER_MARKS)
                .map(|_| AtomicU32::new(0))
                .collect(),
        })
    }

    fn enabled_index(target: usize, stage: usize) -> Option<usize> {
        (target < Self::TARGETS && stage < Self::STAGES).then_some(target * Self::STAGES + stage)
    }

    /// Where this stage's spectrum lives in the pool, if it is subscribed.
    fn slot_of(&self, target: usize, stage: usize) -> Option<usize> {
        let index = Self::enabled_index(target, stage)?;
        match self.spectrum_enabled[index].load(Ordering::Relaxed) {
            0 => None,
            slot => Some(slot as usize - 1),
        }
    }

    fn clear_slot(&self, slot: usize) {
        let base = slot * SPECTRUM_BINS;
        for cell in &self.spectrum[base..base + SPECTRUM_BINS] {
            cell.store(0, Ordering::Relaxed);
        }
    }

    /// Subscribe or unsubscribe a device stage, returning whether it is
    /// subscribed afterwards.
    ///
    /// Subscribing takes a pool slot, and `false` means the pool was full.
    /// That is a real answer rather than a failure to hide: the display reads
    /// zeros, which is what an analyzer that is not running looks like. With
    /// [`SPECTRUM_SLOTS`] slots against the handful of analyzers a person can
    /// look at, it should not arise.
    ///
    /// Control thread only, which is what lets the scan below be a scan.
    pub fn set_spectrum_enabled(&self, target: usize, stage: usize, enabled: bool) -> bool {
        let Some(index) = Self::enabled_index(target, stage) else {
            return false;
        };
        let held = self.spectrum_enabled[index].load(Ordering::Relaxed);
        if !enabled {
            // Clear the flag before freeing the slot, so the audio thread has
            // stopped publishing into it before it can be handed to someone
            // else. A relaxed store is enough for a display: the worst a race
            // can produce is one stale frame in a meter.
            self.spectrum_enabled[index].store(0, Ordering::Relaxed);
            if held != 0 {
                let slot = held as usize - 1;
                self.spectrum_slot_owner[slot].store(0, Ordering::Relaxed);
                self.clear_slot(slot);
            }
            return false;
        }
        if held != 0 {
            return true;
        }
        let Some(slot) = self
            .spectrum_slot_owner
            .iter()
            .position(|owner| owner.load(Ordering::Relaxed) == 0)
        else {
            return false;
        };
        self.spectrum_slot_owner[slot].store(index as u32 + 1, Ordering::Relaxed);
        self.clear_slot(slot);
        self.spectrum_enabled[index].store(slot as u32 + 1, Ordering::Relaxed);
        true
    }

    pub fn spectrum_enabled(&self, target: usize, stage: usize) -> bool {
        self.slot_of(target, stage).is_some()
    }

    /// Publish a normalized log-frequency level vector from the audio thread.
    pub fn publish_spectrum(&self, target: usize, stage: usize, levels: &[f32; SPECTRUM_BINS]) {
        let Some(slot) = self.slot_of(target, stage) else {
            return;
        };
        let base = slot * SPECTRUM_BINS;
        for (cell, level) in self.spectrum[base..base + SPECTRUM_BINS].iter().zip(levels) {
            cell.store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// Latest normalized log-frequency levels for a device stage.
    pub fn read_spectrum(&self, target: usize, stage: usize) -> [f32; SPECTRUM_BINS] {
        let mut levels = [0.0; SPECTRUM_BINS];
        if let Some(slot) = self.slot_of(target, stage) {
            let base = slot * SPECTRUM_BINS;
            for (level, cell) in levels.iter_mut().zip(&self.spectrum[base..base + SPECTRUM_BINS]) {
                *level = f32::from_bits(cell.load(Ordering::Relaxed));
            }
        }
        levels
    }

    /// Publish a buffer device's running collision count from the audio
    /// thread. Unconditional: unlike a spectrum, this costs one atomic store
    /// and needs no subscription, and it is what makes a forced return to live
    /// visible while testing.
    pub fn publish_buffer_collisions(&self, target: usize, stage: usize, collisions: u64) {
        if let Some(index) = Self::enabled_index(target, stage) {
            self.buffer_collisions[index].store(collisions as u32, Ordering::Relaxed);
        }
    }

    /// Subscribe or unsubscribe a Buffer stage's waveform, returning whether
    /// it is subscribed afterwards. `false` means the pool was full, which
    /// draws as an empty waveform -- what a picture nobody is taking looks
    /// like. Control thread only, same as the spectrum pool.
    pub fn set_waveform_enabled(&self, target: usize, stage: usize, enabled: bool) -> bool {
        let Some(index) = Self::enabled_index(target, stage) else {
            return false;
        };
        let held = self.waveform_enabled[index].load(Ordering::Relaxed);
        if !enabled {
            self.waveform_enabled[index].store(0, Ordering::Relaxed);
            if held != 0 {
                let slot = held as usize - 1;
                self.waveform_slot_owner[slot].store(0, Ordering::Relaxed);
                self.clear_waveform_slot(slot);
            }
            return false;
        }
        if held != 0 {
            return true;
        }
        let Some(slot) = self
            .waveform_slot_owner
            .iter()
            .position(|owner| owner.load(Ordering::Relaxed) == 0)
        else {
            return false;
        };
        self.waveform_slot_owner[slot].store(index as u32 + 1, Ordering::Relaxed);
        self.clear_waveform_slot(slot);
        self.waveform_enabled[index].store(slot as u32 + 1, Ordering::Relaxed);
        true
    }

    fn waveform_slot_of(&self, target: usize, stage: usize) -> Option<usize> {
        let index = Self::enabled_index(target, stage)?;
        let held = self.waveform_enabled[index].load(Ordering::Relaxed);
        (held != 0).then(|| held as usize - 1)
    }

    fn clear_waveform_slot(&self, slot: usize) {
        let base = slot * WAVEFORM_BINS;
        for cell in &self.waveform[base..base + WAVEFORM_BINS] {
            cell.store(0, Ordering::Relaxed);
        }
    }

    /// Publish a Buffer's picture from the audio thread.
    ///
    /// The marks go out unconditionally -- five stores -- and the peaks only
    /// when something is looking.
    pub fn publish_buffer_display(&self, target: usize, stage: usize, display: &BufferDisplay<'_>) {
        if let Some(index) = Self::enabled_index(target, stage) {
            let base = index * BUFFER_MARKS;
            let mut flags = 0;
            if display.frozen {
                flags |= BUFFER_FLAG_FROZEN;
            }
            if let Some(freeze) = display.armed_freeze {
                flags |= BUFFER_FLAG_ARMED;
                if freeze {
                    flags |= BUFFER_FLAG_ARMED_FREEZE;
                }
            }
            // `-1` for absent, which a fraction of a ring can never be.
            let head = display.head.unwrap_or(-1.0);
            let (start, end) = display.window.unwrap_or((-1.0, -1.0));
            self.buffer_marks[base].store(head.to_bits(), Ordering::Relaxed);
            self.buffer_marks[base + 1].store(display.write.to_bits(), Ordering::Relaxed);
            self.buffer_marks[base + 2].store(start.to_bits(), Ordering::Relaxed);
            self.buffer_marks[base + 3].store(end.to_bits(), Ordering::Relaxed);
            self.buffer_marks[base + 4].store(flags, Ordering::Relaxed);
        }
        let Some(slot) = self.waveform_slot_of(target, stage) else {
            return;
        };
        let base = slot * WAVEFORM_BINS;
        for (cell, peak) in self.waveform[base..base + WAVEFORM_BINS]
            .iter()
            .zip(display.peaks)
        {
            cell.store(peak.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// Latest peaks for a Buffer stage, oldest ring index first. All zero
    /// when nothing is subscribed.
    pub fn read_waveform(&self, target: usize, stage: usize) -> Vec<f32> {
        let mut peaks = vec![0.0; WAVEFORM_BINS];
        if let Some(slot) = self.waveform_slot_of(target, stage) {
            let base = slot * WAVEFORM_BINS;
            for (peak, cell) in peaks
                .iter_mut()
                .zip(&self.waveform[base..base + WAVEFORM_BINS])
            {
                *peak = f32::from_bits(cell.load(Ordering::Relaxed));
            }
        }
        peaks
    }

    /// Latest head, writer, window and state for a Buffer stage.
    pub fn read_buffer_marks(&self, target: usize, stage: usize) -> BufferMarks {
        let Some(index) = Self::enabled_index(target, stage) else {
            return BufferMarks::default();
        };
        let base = index * BUFFER_MARKS;
        let read = |offset: usize| {
            f32::from_bits(self.buffer_marks[base + offset].load(Ordering::Relaxed))
        };
        let head = read(0);
        let start = read(2);
        let end = read(3);
        let flags = self.buffer_marks[base + 4].load(Ordering::Relaxed);
        BufferMarks {
            head: (head >= 0.0).then_some(head),
            write: read(1),
            window: (start >= 0.0 && end >= 0.0).then_some((start, end)),
            frozen: flags & BUFFER_FLAG_FROZEN != 0,
            armed_freeze: (flags & BUFFER_FLAG_ARMED != 0)
                .then_some(flags & BUFFER_FLAG_ARMED_FREEZE != 0),
        }
    }

    /// Collisions counted by the device in this stage since it was installed.
    pub fn read_buffer_collisions(&self, target: usize, stage: usize) -> u32 {
        Self::enabled_index(target, stage).map_or(0, |index| {
            self.buffer_collisions[index].load(Ordering::Relaxed)
        })
    }

    /// Clear subscriptions and retained display data before a complete graph
    /// replacement. The UI re-subscribes the new project after it is visible.
    pub fn clear_spectra(&self) {
        for cell in &self.waveform_enabled {
            cell.store(0, Ordering::Relaxed);
        }
        for cell in &self.waveform_slot_owner {
            cell.store(0, Ordering::Relaxed);
        }
        for cell in &self.waveform {
            cell.store(0, Ordering::Relaxed);
        }
        for cell in &self.buffer_marks {
            cell.store(0, Ordering::Relaxed);
        }
        for cell in &self.spectrum_enabled {
            cell.store(0, Ordering::Relaxed);
        }
        for cell in &self.spectrum_slot_owner {
            cell.store(0, Ordering::Relaxed);
        }
        for cell in &self.spectrum {
            cell.store(0, Ordering::Relaxed);
        }
        for cell in &self.buffer_collisions {
            cell.store(0, Ordering::Relaxed);
        }
    }
}

impl DeviceMeters {
    const TARGETS: usize = MAX_CHANNELS + MAX_BUSES;
    const STAGES: usize = MAX_EFFECTS_PER_CHANNEL + 1;
    // Input L/R, output L/R, detector level, gain-reduction depth.
    const VALUES_PER_STAGE: usize = 6;
    /// Cell offset of the dynamics pair within a stage.
    const DETECTOR: usize = 4;
    const REDUCTION: usize = 5;

    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cells: (0..Self::TARGETS * Self::STAGES * Self::VALUES_PER_STAGE)
                .map(|_| AtomicU32::new(0))
                .collect(),
        })
    }

    fn base(target: usize, stage: usize) -> Option<usize> {
        (target < Self::TARGETS && stage < Self::STAGES)
            .then_some((target * Self::STAGES + stage) * Self::VALUES_PER_STAGE)
    }

    pub fn publish_input(&self, target: usize, stage: usize, left: f32, right: f32) {
        if let Some(base) = Self::base(target, stage) {
            self.cells[base].fetch_max(left.max(0.0).to_bits(), Ordering::Relaxed);
            self.cells[base + 1].fetch_max(right.max(0.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn publish_output(&self, target: usize, stage: usize, left: f32, right: f32) {
        if let Some(base) = Self::base(target, stage) {
            self.cells[base + 2].fetch_max(left.max(0.0).to_bits(), Ordering::Relaxed);
            self.cells[base + 3].fetch_max(right.max(0.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// Raise a gain-reducing device's held display state.
    ///
    /// Both values are stored as non-negative magnitudes so the same
    /// `fetch_max` hold as the peak meters is correct for them: the detector
    /// keeps its linear level, and the reduction keeps its *depth* in dB
    /// (the negation of the frame's reduction). That also makes the
    /// swap-to-zero on read mean exactly "silent, and reducing nothing",
    /// which is the right thing for a stage the GUI has stopped watching.
    pub fn publish_dynamics(&self, target: usize, stage: usize, frame: DynamicsFrame) {
        let Some(base) = Self::base(target, stage) else {
            return;
        };
        // A silent block reports `-inf` dB, which converts to the zero the
        // held cell already rests at; anything else is an ordinary level.
        let detector = if frame.detector_db.is_finite() {
            db_to_lin(frame.detector_db).max(0.0)
        } else {
            0.0
        };
        let depth_db = (-frame.reduction_db).max(0.0);
        self.cells[base + Self::DETECTOR].fetch_max(detector.to_bits(), Ordering::Relaxed);
        self.cells[base + Self::REDUCTION].fetch_max(depth_db.to_bits(), Ordering::Relaxed);
    }

    /// Read and clear a device's held dynamics state, as `(detector level,
    /// gain reduction dB)` with the reduction back in its natural sign.
    pub fn take_dynamics(&self, target: usize, stage: usize) -> (f32, f32) {
        Self::base(target, stage)
            .map(|base| {
                let read =
                    |index: usize| f32::from_bits(self.cells[index].swap(0, Ordering::Relaxed));
                (
                    read(base + Self::DETECTOR),
                    -read(base + Self::REDUCTION),
                )
            })
            .unwrap_or((0.0, 0.0))
    }

    /// Empty every cell of one target's stages in one pass.
    ///
    /// **A cell nobody reads is a `fetch_max` hold, and nothing else empties
    /// it.** The pump drains `take`/`take_dynamics` for the one chain the rack
    /// is showing, so every other channel's and bus's stage cells keep
    /// whatever their loudest block ever was -- and switching the rack to a
    /// channel last viewed ten minutes ago drew that ten-minute maximum for
    /// one 8 ms tick before the next read cleared it. A transient nobody can
    /// hear, on a meter that has been dark since it was last looked at.
    ///
    /// The whole-array alternative is what this exists to avoid: draining
    /// every target every tick is `TARGETS * STAGES * VALUES_PER_STAGE`
    /// swaps at 125 Hz, which is the cost the spectrum pool exists to dodge
    /// in the analogous case. Draining the *outgoing* target once, when the
    /// rack moves, is `STAGES * VALUES_PER_STAGE` swaps on a gesture a user
    /// makes by hand.
    pub fn clear_target(&self, target: usize) {
        let Some(base) = Self::base(target, 0) else {
            return;
        };
        let end = base + Self::STAGES * Self::VALUES_PER_STAGE;
        for cell in &self.cells[base..end] {
            cell.store(0, Ordering::Relaxed);
        }
    }

    pub fn take(&self, target: usize, stage: usize) -> ((f32, f32), (f32, f32)) {
        let read = |index: usize| f32::from_bits(self.cells[index].swap(0, Ordering::Relaxed));
        Self::base(target, stage)
            .map(|base| {
                (
                    (read(base), read(base + 1)),
                    (read(base + 2), read(base + 3)),
                )
            })
            .unwrap_or(((0.0, 0.0), (0.0, 0.0)))
    }
}

impl BusMeters {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cells: (0..MAX_BUSES * 2).map(|_| AtomicU32::new(0)).collect(),
            reduction: (0..MAX_BUSES).map(|_| AtomicU32::new(0)).collect(),
        })
    }

    /// Raise `bus`'s held peak. Called on the audio thread, once per block.
    ///
    /// `fetch_max` on the raw bit pattern is a correct maximum here because
    /// peaks are non-negative and IEEE-754 orders non-negative floats the same
    /// way their bit patterns order as integers.
    pub fn publish(&self, bus: usize, peak_l: f32, peak_r: f32) {
        let base = bus * 2;
        if let Some(cell) = self.cells.get(base) {
            cell.fetch_max(peak_l.max(0.0).to_bits(), Ordering::Relaxed);
        }
        if let Some(cell) = self.cells.get(base + 1) {
            cell.fetch_max(peak_r.max(0.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// Raise `bus`'s held gain reduction, given the reduction in dB the way
    /// the rest of the codebase states it: negative, or zero for none.
    pub fn publish_reduction(&self, bus: usize, reduction_db: f32) {
        if let Some(cell) = self.reduction.get(bus) {
            cell.fetch_max((-reduction_db).max(0.0).to_bits(), Ordering::Relaxed);
        }
    }

    /// Read and clear `bus`'s held gain reduction, as a positive number of
    /// decibels. Called on the GUI thread.
    pub fn take_reduction(&self, bus: usize) -> f32 {
        self.reduction
            .get(bus)
            .map(|cell| f32::from_bits(cell.swap(0, Ordering::Relaxed)))
            .unwrap_or(0.0)
    }

    /// Read and clear `bus`'s held peak. Called on the GUI thread.
    pub fn take(&self, bus: usize) -> (f32, f32) {
        let base = bus * 2;
        let read = |index: usize| {
            self.cells
                .get(index)
                .map(|cell| f32::from_bits(cell.swap(0, Ordering::Relaxed)))
                .unwrap_or(0.0)
        };
        (read(base), read(base + 1))
    }
}

const VOICES_PER_CHANNEL: usize = MAX_SAMPLER_VOICES as usize;

/// Live per-voice sample playback position for every channel, for a UI
/// playhead. Same shape as `BusMeters`/`DeviceMeters` and for the same
/// reason: the GUI only ever wants the latest position, never a backlog, so
/// a wait-free array of atomics fits better than the event ring.
///
/// Unlike the peak meters this is a plain store/load, not a held-until-read
/// swap: a moving position has no "missed transient" to protect, the GUI
/// just wants whatever the audio thread last published. An inactive voice
/// publishes `f32::NAN`, which `read` filters out.
pub struct PlayheadMeters {
    cells: Vec<AtomicU32>,
}

impl PlayheadMeters {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cells: (0..MAX_CHANNELS * VOICES_PER_CHANNEL)
                .map(|_| AtomicU32::new(f32::NAN.to_bits()))
                .collect(),
        })
    }

    fn base(channel: usize) -> Option<usize> {
        (channel < MAX_CHANNELS).then_some(channel * VOICES_PER_CHANNEL)
    }

    /// Publish `channel`'s voice positions, overwriting every slot. Called
    /// on the audio thread, once per block.
    pub fn publish(&self, channel: usize, positions: &[f32; VOICES_PER_CHANNEL]) {
        let Some(base) = Self::base(channel) else {
            return;
        };
        for (offset, position) in positions.iter().enumerate() {
            self.cells[base + offset].store(position.to_bits(), Ordering::Relaxed);
        }
    }

    /// Every currently-active voice position for `channel`. Called on the
    /// GUI thread.
    pub fn read(&self, channel: usize) -> Vec<f32> {
        let Some(base) = Self::base(channel) else {
            return Vec::new();
        };
        (base..base + VOICES_PER_CHANNEL)
            .map(|index| f32::from_bits(self.cells[index].load(Ordering::Relaxed)))
            .filter(|position| !position.is_nan())
            .collect()
    }
}

/// Each channel's modulator outputs as of the last control tick of the most
/// recent block, so the UI can draw what modulation is doing right now.
///
/// Latest-value rather than peak-held, for the same reason `PlayheadMeters`
/// is: a moving LFO has no transient to protect, and a held peak would make
/// every assigned knob's arc stick at its excursion instead of animating. The
/// GUI publishes nothing back, so a plain relaxed store/load pair is enough.
///
/// Only the last tick is published. A block is a few milliseconds and the UI
/// repaints far slower than that, so the intermediate ticks would be
/// overwritten unseen -- the audio thread should not pay to store them.
pub struct ModulatorMeters {
    cells: Vec<AtomicU32>,
}

impl ModulatorMeters {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cells: (0..MAX_CHANNELS * CONTROL_SOURCE_SLOTS)
                .map(|_| AtomicU32::new(0.0f32.to_bits()))
                .collect(),
        })
    }

    fn base(channel: usize) -> Option<usize> {
        (channel < MAX_CHANNELS).then_some(channel * CONTROL_SOURCE_SLOTS)
    }

    /// Publish `channel`'s control sources. Called on the audio thread, once
    /// per block.
    ///
    /// The whole row, modulator slots and generator outlets alike, because
    /// this is what the view resolves a knob's live modulation offset from:
    /// carrying only the rack would leave a knob driven by an outlet sitting
    /// still while it audibly moves.
    pub fn publish(&self, channel: usize, outputs: &[f32; CONTROL_SOURCE_SLOTS]) {
        let Some(base) = Self::base(channel) else {
            return;
        };
        for (offset, value) in outputs.iter().enumerate() {
            self.cells[base + offset].store(value.to_bits(), Ordering::Relaxed);
        }
    }

    /// `channel`'s latest control sources. Called on the GUI thread.
    pub fn read(&self, channel: usize) -> [f32; CONTROL_SOURCE_SLOTS] {
        let mut out = [0.0; CONTROL_SOURCE_SLOTS];
        let Some(base) = Self::base(channel) else {
            return out;
        };
        for (offset, value) in out.iter_mut().enumerate() {
            *value = f32::from_bits(self.cells[base + offset].load(Ordering::Relaxed));
        }
        out
    }
}

#[cfg(test)]
mod tests {

    /// A slot is taken on subscribe and given back on unsubscribe, so the
    /// pool does not leak across a session of opening and closing EQ faces.
    #[test]
    fn a_spectrum_slot_is_returned_when_the_stage_unsubscribes() {
        let telemetry = DeviceTelemetry::new();
        for stage in 0..SPECTRUM_SLOTS {
            assert!(telemetry.set_spectrum_enabled(0, stage, true), "stage {stage}");
        }
        // Full: one more gets no slot, and says so rather than pretending.
        assert!(!telemetry.set_spectrum_enabled(1, 0, true));

        for stage in 0..SPECTRUM_SLOTS {
            telemetry.set_spectrum_enabled(0, stage, false);
        }
        assert!(telemetry.set_spectrum_enabled(1, 0, true));
    }

    /// Two subscribed stages get different slots, so one analyzer's spectrum
    /// is not another's. A pool that shared them would be worse than the
    /// array it replaced.
    #[test]
    fn two_stages_do_not_share_a_spectrum() {
        let telemetry = DeviceTelemetry::new();
        assert!(telemetry.set_spectrum_enabled(0, 1, true));
        assert!(telemetry.set_spectrum_enabled(0, 2, true));

        let mut loud = [0.0; SPECTRUM_BINS];
        loud[3] = 1.0;
        telemetry.publish_spectrum(0, 1, &loud);

        assert_eq!(telemetry.read_spectrum(0, 1)[3], 1.0);
        assert_eq!(telemetry.read_spectrum(0, 2)[3], 0.0);
    }

    /// An unsubscribed stage reads zeros and its publishes go nowhere, which
    /// is what lets a stage that could not get a slot look exactly like one
    /// whose analyzer is switched off.
    #[test]
    fn an_unsubscribed_stage_publishes_nothing() {
        let telemetry = DeviceTelemetry::new();
        let mut loud = [0.0; SPECTRUM_BINS];
        loud[7] = 1.0;
        telemetry.publish_spectrum(0, 5, &loud);
        assert_eq!(telemetry.read_spectrum(0, 5)[7], 0.0);
    }

    /// Re-subscribing an already-subscribed stage keeps its slot rather than
    /// taking a second one. `sync_effect_spectrum_subscriptions` re-asserts
    /// every enabled analyzer on each project install, so this is the common
    /// path rather than an edge case -- and getting it wrong would drain the
    /// pool on the fourth project load.
    #[test]
    fn re_subscribing_does_not_take_a_second_slot() {
        let telemetry = DeviceTelemetry::new();
        for _ in 0..SPECTRUM_SLOTS * 2 {
            assert!(telemetry.set_spectrum_enabled(0, 1, true));
        }
        assert!(telemetry.set_spectrum_enabled(0, 2, true));
    }
    use super::*;
    use mooloop_dsp::dynamics::lin_to_db;

    #[test]
    fn a_peak_is_held_until_it_is_read() {
        let meters = BusMeters::new();
        meters.publish(3, 0.5, 0.25);
        // A quieter block must not erase the transient the GUI has not seen.
        meters.publish(3, 0.1, 0.1);
        assert_eq!(meters.take(3), (0.5, 0.25));
        // Reading clears, so a silent bus reads silent next time.
        assert_eq!(meters.take(3), (0.0, 0.0));
    }

    #[test]
    fn buses_do_not_share_cells() {
        let meters = BusMeters::new();
        meters.publish(0, 1.0, 1.0);
        assert_eq!(meters.take(1), (0.0, 0.0));
        assert_eq!(meters.take(0), (1.0, 1.0));
    }

    #[test]
    fn out_of_range_buses_are_ignored_rather_than_panicking() {
        let meters = BusMeters::new();
        meters.publish(MAX_BUSES + 5, 1.0, 1.0);
        assert_eq!(meters.take(MAX_BUSES + 5), (0.0, 0.0));
    }

    #[test]
    fn spectrum_telemetry_is_latest_value_and_can_be_disabled() {
        let telemetry = DeviceTelemetry::new();
        let levels = std::array::from_fn(|index| index as f32 / SPECTRUM_BINS as f32);
        telemetry.set_spectrum_enabled(2, 1, true);
        assert!(telemetry.spectrum_enabled(2, 1));
        telemetry.publish_spectrum(2, 1, &levels);
        assert_eq!(telemetry.read_spectrum(2, 1), levels);
        telemetry.set_spectrum_enabled(2, 1, false);
        assert!(!telemetry.spectrum_enabled(2, 1));
        assert_eq!(telemetry.read_spectrum(2, 1), [0.0; SPECTRUM_BINS]);
    }

    #[test]
    fn dynamics_telemetry_holds_the_deepest_reduction_until_it_is_read() {
        let meters = DeviceMeters::new();
        meters.publish_dynamics(
            0,
            1,
            DynamicsFrame {
                detector_db: -20.0,
                reduction_db: -9.0,
            },
        );
        // A calmer block must not erase the squeeze the GUI has not seen.
        meters.publish_dynamics(
            0,
            1,
            DynamicsFrame {
                detector_db: -40.0,
                reduction_db: -1.0,
            },
        );
        let (detector, reduction_db) = meters.take_dynamics(0, 1);
        assert!(
            (lin_to_db(detector) + 20.0).abs() < 0.01,
            "held detector read {} dB",
            lin_to_db(detector)
        );
        assert!(
            (reduction_db + 9.0).abs() < 0.01,
            "held reduction read {reduction_db} dB"
        );
        // Reading clears, so a stage nobody is driving reads as at rest.
        assert_eq!(meters.take_dynamics(0, 1), (0.0, 0.0));
    }

    #[test]
    fn a_silent_dynamics_frame_rests_at_zero_rather_than_a_residual_level() {
        let meters = DeviceMeters::new();
        meters.publish_dynamics(0, 1, DynamicsFrame::SILENT);
        assert_eq!(meters.take_dynamics(0, 1), (0.0, 0.0));
    }

    #[test]
    fn dynamics_telemetry_does_not_disturb_the_peak_meters() {
        let meters = DeviceMeters::new();
        meters.publish_input(2, 3, 0.5, 0.25);
        meters.publish_output(2, 3, 0.4, 0.2);
        meters.publish_dynamics(
            2,
            3,
            DynamicsFrame {
                detector_db: -6.0,
                reduction_db: -3.0,
            },
        );
        assert_eq!(meters.take(2, 3), ((0.5, 0.25), (0.4, 0.2)));
        // ...and the peaks' own read did not clear the dynamics pair.
        assert!(meters.take_dynamics(2, 3).1 < 0.0);
    }

    #[test]
    fn playhead_positions_start_and_stay_empty_until_published() {
        let meters = PlayheadMeters::new();
        assert!(meters.read(0).is_empty());
    }

    #[test]
    fn playhead_read_reports_only_active_voices() {
        let meters = PlayheadMeters::new();
        let mut positions = [f32::NAN; VOICES_PER_CHANNEL];
        positions[0] = 0.25;
        positions[3] = 0.75;
        meters.publish(2, &positions);
        assert_eq!(meters.read(2), vec![0.25, 0.75]);
        assert!(meters.read(1).is_empty());
    }

    #[test]
    fn playhead_publish_overwrites_every_slot_including_now_inactive_ones() {
        let meters = PlayheadMeters::new();
        let mut active = [f32::NAN; VOICES_PER_CHANNEL];
        active[0] = 0.5;
        meters.publish(0, &active);
        assert_eq!(meters.read(0), vec![0.5]);

        // The voice released: the next block's publish must clear the old
        // reading, not leave a stale playhead behind (unlike a held peak).
        let idle = [f32::NAN; VOICES_PER_CHANNEL];
        meters.publish(0, &idle);
        assert!(meters.read(0).is_empty());
    }

    #[test]
    fn out_of_range_playhead_channels_are_ignored_rather_than_panicking() {
        let meters = PlayheadMeters::new();
        meters.publish(MAX_CHANNELS + 5, &[0.5; VOICES_PER_CHANNEL]);
        assert!(meters.read(MAX_CHANNELS + 5).is_empty());
    }
}

#[cfg(test)]
mod clear_target_tests {
    use super::*;

    /// **A chain nobody is looking at does not keep its loudest block.**
    ///
    /// Device meter cells are `fetch_max` holds and only a read empties one.
    /// The pump drains the one chain the rack is showing, so every other
    /// target held whatever its loudest block ever was -- and switching the
    /// rack to a channel last viewed ten minutes ago drew that ten-minute
    /// maximum for one 8 ms tick before the next read cleared it.
    ///
    /// Both halves are asserted, because the peaks and the dynamics live in
    /// the same stage and are read by two different methods: an earlier
    /// attempt at this in the bus loop wrote zeros through `publish`, which
    /// is a `fetch_max` and so cannot lower a cell at all.
    #[test]
    fn clearing_a_target_empties_its_peaks_and_its_dynamics() {
        let meters = DeviceMeters::new();
        meters.publish_input(3, 0, 0.9, 0.8);
        meters.publish_output(3, 1, 0.7, 0.6);
        meters.publish_dynamics(
            3,
            1,
            DynamicsFrame {
                detector_db: -6.0,
                reduction_db: -4.0,
            },
        );
        // A different target, to prove the clear is not a reset.
        meters.publish_output(4, 1, 0.25, 0.25);

        meters.clear_target(3);

        assert_eq!(meters.take(3, 0), ((0.0, 0.0), (0.0, 0.0)));
        assert_eq!(meters.take(3, 1), ((0.0, 0.0), (0.0, 0.0)));
        assert_eq!(meters.take_dynamics(3, 1), (0.0, 0.0));
        assert_eq!(
            meters.take(4, 1),
            ((0.0, 0.0), (0.25, 0.25)),
            "clearing one target emptied another"
        );
    }

    /// The last stage of a target is inside the clear and the first stage of
    /// the next target is outside it. Written because the range is computed
    /// from `STAGES * VALUES_PER_STAGE` and an off-by-one there would be
    /// invisible in the test above.
    #[test]
    fn the_clear_covers_exactly_one_targets_stages() {
        let meters = DeviceMeters::new();
        let last = MAX_EFFECTS_PER_CHANNEL;
        meters.publish_output(3, last, 0.9, 0.9);
        meters.publish_output(4, 0, 0.9, 0.9);

        meters.clear_target(3);

        assert_eq!(meters.take(3, last), ((0.0, 0.0), (0.0, 0.0)));
        assert_eq!(meters.take(4, 0), ((0.0, 0.0), (0.9, 0.9)));
    }

    /// An out-of-range target is ignored rather than clearing the wrong one
    /// or panicking, matching every other method here.
    #[test]
    fn an_out_of_range_target_clears_nothing() {
        let meters = DeviceMeters::new();
        meters.publish_output(0, 0, 0.5, 0.5);
        meters.clear_target(MAX_CHANNELS + MAX_BUSES + 7);
        assert_eq!(meters.take(0, 0), ((0.0, 0.0), (0.5, 0.5)));
    }
}
