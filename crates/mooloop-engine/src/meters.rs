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
    MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL, MAX_MODULATORS_PER_CHANNEL, MAX_SAMPLER_VOICES,
};
use mooloop_dsp::SPECTRUM_BINS;

/// Peak-hold cells for every bus, left and right interleaved.
pub struct BusMeters {
    cells: Vec<AtomicU32>,
}

/// Held input/output peaks for every visible device. Stage zero is a
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

/// Latest display-oriented data for every device stage.
///
/// This is intentionally a distinct transport from `EngineEvent`: analyzer
/// vectors are continuous observations, so a GUI wants the latest snapshot
/// rather than every historical frame. More display features can be added to
/// this stable device-stage address space without exposing PCM to the UI.
pub struct DeviceTelemetry {
    spectrum_enabled: Vec<AtomicU32>,
    spectrum: Vec<AtomicU32>,
    /// Running count of retained-audio writer/read-head collisions per device
    /// stage. A monotonic counter rather than a flag: the UI compares it
    /// against the value it last saw, so a forced return landing between two
    /// GUI frames is still noticed.
    buffer_collisions: Vec<AtomicU32>,
}

impl DeviceTelemetry {
    const TARGETS: usize = MAX_CHANNELS + MAX_BUSES;
    const STAGES: usize = MAX_EFFECTS_PER_CHANNEL + 1;

    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            spectrum_enabled: (0..Self::TARGETS * Self::STAGES)
                .map(|_| AtomicU32::new(0))
                .collect(),
            spectrum: (0..Self::TARGETS * Self::STAGES * SPECTRUM_BINS)
                .map(|_| AtomicU32::new(0))
                .collect(),
            buffer_collisions: (0..Self::TARGETS * Self::STAGES)
                .map(|_| AtomicU32::new(0))
                .collect(),
        })
    }

    fn base(target: usize, stage: usize) -> Option<usize> {
        (target < Self::TARGETS && stage < Self::STAGES)
            .then_some((target * Self::STAGES + stage) * SPECTRUM_BINS)
    }

    fn enabled_index(target: usize, stage: usize) -> Option<usize> {
        (target < Self::TARGETS && stage < Self::STAGES).then_some(target * Self::STAGES + stage)
    }

    /// Subscribe or unsubscribe a device stage. Disabled stages have no DSP
    /// analysis cost beyond one atomic load at the host boundary.
    pub fn set_spectrum_enabled(&self, target: usize, stage: usize, enabled: bool) {
        if let Some(index) = Self::enabled_index(target, stage) {
            self.spectrum_enabled[index].store(u32::from(enabled), Ordering::Relaxed);
            if !enabled {
                if let Some(base) = Self::base(target, stage) {
                    for cell in &self.spectrum[base..base + SPECTRUM_BINS] {
                        cell.store(0, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    pub fn spectrum_enabled(&self, target: usize, stage: usize) -> bool {
        Self::enabled_index(target, stage)
            .is_some_and(|index| self.spectrum_enabled[index].load(Ordering::Relaxed) != 0)
    }

    /// Publish a normalized log-frequency level vector from the audio thread.
    pub fn publish_spectrum(&self, target: usize, stage: usize, levels: &[f32; SPECTRUM_BINS]) {
        if let Some(base) = Self::base(target, stage) {
            for (cell, level) in self.spectrum[base..base + SPECTRUM_BINS].iter().zip(levels) {
                cell.store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
            }
        }
    }

    /// Latest normalized log-frequency levels for a device stage.
    pub fn read_spectrum(&self, target: usize, stage: usize) -> [f32; SPECTRUM_BINS] {
        let mut levels = [0.0; SPECTRUM_BINS];
        if let Some(base) = Self::base(target, stage) {
            for (level, cell) in levels
                .iter_mut()
                .zip(&self.spectrum[base..base + SPECTRUM_BINS])
            {
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

    /// Collisions counted by the device in this stage since it was installed.
    pub fn read_buffer_collisions(&self, target: usize, stage: usize) -> u32 {
        Self::enabled_index(target, stage).map_or(0, |index| {
            self.buffer_collisions[index].load(Ordering::Relaxed)
        })
    }

    /// Clear subscriptions and retained display data before a complete graph
    /// replacement. The UI re-subscribes the new project after it is visible.
    pub fn clear_spectra(&self) {
        for cell in &self.spectrum_enabled {
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
    const VALUES_PER_STAGE: usize = 4; // input L/R, output L/R

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
            cells: (0..MAX_CHANNELS * MAX_MODULATORS_PER_CHANNEL)
                .map(|_| AtomicU32::new(0.0f32.to_bits()))
                .collect(),
        })
    }

    fn base(channel: usize) -> Option<usize> {
        (channel < MAX_CHANNELS).then_some(channel * MAX_MODULATORS_PER_CHANNEL)
    }

    /// Publish `channel`'s modulator outputs. Called on the audio thread,
    /// once per block.
    pub fn publish(&self, channel: usize, outputs: &[f32; MAX_MODULATORS_PER_CHANNEL]) {
        let Some(base) = Self::base(channel) else {
            return;
        };
        for (offset, value) in outputs.iter().enumerate() {
            self.cells[base + offset].store(value.to_bits(), Ordering::Relaxed);
        }
    }

    /// `channel`'s latest modulator outputs. Called on the GUI thread.
    pub fn read(&self, channel: usize) -> [f32; MAX_MODULATORS_PER_CHANNEL] {
        let mut out = [0.0; MAX_MODULATORS_PER_CHANNEL];
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
    use super::*;

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
