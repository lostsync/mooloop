//! A stereo integer-sample delay, for putting two signal paths back in time
//! with each other.
//!
//! Two things need exactly this and nothing more, which is why it is one type:
//!
//! - **A dry path inside an effect container.** The container blends a node's
//!   output with the unprocessed signal (its wet/dry control), so when the
//!   node reports a nonzero `AudioNode::latency_frames` the dry half must be
//!   delayed by the same amount or the two comb-filter against each other.
//! - **A producer's compensation before it sums.** A channel whose chain is
//!   shorter than its neighbour's arrives early at the bus they share, so it
//!   waits by the difference. `mooloop_core::compile_latency` decides by how
//!   much; this is what carries it out
//!   (`docs/plans/latency-compensation/04-preallocated-delays.md`).
//!
//! Integer samples, not the fractional reads `delayline::DelayLine` offers:
//! latency is a whole number of frames, and interpolating it would low-pass a
//! signal that is only meant to wait.

/// Stereo integer-sample delay of a fixed length.
///
/// Construction allocates, so instances are built on the non-realtime side
/// (with whatever they align) and shipped through the same ownership channel.
/// `process` itself performs no allocation.
pub struct IntegerDelay {
    left: Vec<f32>,
    right: Vec<f32>,
    write: usize,
}

impl IntegerDelay {
    /// A delay of `latency` frames. Returns `None` for zero, so a caller with
    /// nothing to align skips the work entirely — which is the common case at
    /// both call sites and is why this is an `Option` rather than a
    /// zero-length ring.
    pub fn new(latency: u32) -> Option<Self> {
        let frames = latency as usize;
        if frames == 0 {
            return None;
        }
        Some(Self {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            write: 0,
        })
    }

    /// The ring's length, which is also the delay it applies.
    ///
    /// A caller that has fed it silence for at least this many frames knows
    /// every slot holds a zero, and so knows that freezing it changes
    /// nothing — which is what lets a resting bus stop running it.
    pub fn len(&self) -> usize {
        self.left.len()
    }

    /// Empty the ring without changing its length.
    ///
    /// For a producer that stopped producing — a muted channel renders
    /// nothing, so anything still in its compensation ring is audio from
    /// before the mute that would be emitted on unmute. Cheap: the ring is a
    /// latency's worth of frames, not a block's.
    pub fn reset(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
    }

    /// Delay both channels in place by the constructed latency. The ring's
    /// length *is* the delay, so the slot being overwritten always holds the
    /// sample from exactly `latency` frames ago.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        debug_assert_eq!(left.len(), right.len());
        let frames = self.left.len();
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let delayed_l = self.left[self.write];
            let delayed_r = self.right[self.write];
            self.left[self.write] = *l;
            self.right[self.write] = *r;
            *l = delayed_l;
            *r = delayed_r;
            // Wrapped by comparison rather than by `%`. The head is always
            // inside the ring, so one past the end is the only case there is,
            // and a remainder by a length the compiler cannot see is an
            // integer division -- tens of cycles, per sample, on every bus
            // carrying compensation.
            self.write += 1;
            if self.write == frames {
                self.write = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_latency_is_no_align() {
        assert!(IntegerDelay::new(0).is_none());
    }

    #[test]
    fn an_impulse_comes_out_exactly_latency_frames_later() {
        let mut align = IntegerDelay::new(3).unwrap();
        let mut left = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut right = [0.5, 0.0, 0.0, 0.0, 0.0, 0.0];
        align.process(&mut left, &mut right);
        assert_eq!(left, [0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(right, [0.0, 0.0, 0.0, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn the_ring_carries_state_across_blocks() {
        let mut align = IntegerDelay::new(2).unwrap();
        let mut left = [1.0];
        let mut right = [1.0];
        align.process(&mut left, &mut right);
        assert_eq!(left, [0.0]);
        let mut left = [0.0];
        let mut right = [0.0];
        align.process(&mut left, &mut right);
        align.process(&mut left, &mut right);
        assert_eq!(left, [1.0]);
        assert_eq!(right, [1.0]);
    }
}
