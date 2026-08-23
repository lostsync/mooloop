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
use mooloop_core::{MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL};

/// Peak-hold cells for every bus, left and right interleaved.
pub struct BusMeters {
    cells: Vec<AtomicU32>,
}

/// Held input/output peaks for every visible channel device. Stage zero is a
/// source (its input is deliberately never published); later stages are the
/// effect slots. This gives a device face a truthful pair of meters instead
/// of borrowing the master meter for every rectangle in the rack.
pub struct DeviceMeters {
    cells: Vec<AtomicU32>,
}

impl DeviceMeters {
    const STAGES: usize = MAX_EFFECTS_PER_CHANNEL + 1;
    const VALUES_PER_STAGE: usize = 4; // input L/R, output L/R

    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cells: (0..MAX_CHANNELS * Self::STAGES * Self::VALUES_PER_STAGE)
                .map(|_| AtomicU32::new(0))
                .collect(),
        })
    }

    fn base(channel: usize, stage: usize) -> Option<usize> {
        (channel < MAX_CHANNELS && stage < Self::STAGES)
            .then_some((channel * Self::STAGES + stage) * Self::VALUES_PER_STAGE)
    }

    pub fn publish_input(&self, channel: usize, stage: usize, left: f32, right: f32) {
        if let Some(base) = Self::base(channel, stage) {
            self.cells[base].fetch_max(left.max(0.0).to_bits(), Ordering::Relaxed);
            self.cells[base + 1].fetch_max(right.max(0.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn publish_output(&self, channel: usize, stage: usize, left: f32, right: f32) {
        if let Some(base) = Self::base(channel, stage) {
            self.cells[base + 2].fetch_max(left.max(0.0).to_bits(), Ordering::Relaxed);
            self.cells[base + 3].fetch_max(right.max(0.0).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn take(&self, channel: usize, stage: usize) -> ((f32, f32), (f32, f32)) {
        let read = |index: usize| f32::from_bits(self.cells[index].swap(0, Ordering::Relaxed));
        Self::base(channel, stage)
            .map(|base| ((read(base), read(base + 1)), (read(base + 2), read(base + 3))))
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
}
