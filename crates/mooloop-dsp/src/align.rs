//! Dry-path latency alignment for the engine's effect container.
//!
//! The container around every effect node blends the node's output with the
//! unprocessed signal (its wet/dry control). When a node reports a nonzero
//! `AudioNode::latency_frames`, the dry half of that blend must be delayed by
//! the same amount or the two paths comb-filter against each other. `DryAlign`
//! is that delay: a plain stereo integer-sample ring, allocated once next to
//! the node it aligns to.

/// Stereo integer-sample delay matching one node's reported latency.
///
/// Construction allocates, so instances are built on the non-realtime side
/// (with the node they align to) and shipped through the same ownership
/// channel. `process` itself performs no allocation.
pub struct DryAlign {
    left: Vec<f32>,
    right: Vec<f32>,
    write: usize,
}

impl DryAlign {
    /// A dry-path delay of `latency` frames. Returns `None` for zero latency
    /// so the container can skip the work entirely for the common case.
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
            self.write = (self.write + 1) % frames;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_latency_is_no_align() {
        assert!(DryAlign::new(0).is_none());
    }

    #[test]
    fn an_impulse_comes_out_exactly_latency_frames_later() {
        let mut align = DryAlign::new(3).unwrap();
        let mut left = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut right = [0.5, 0.0, 0.0, 0.0, 0.0, 0.0];
        align.process(&mut left, &mut right);
        assert_eq!(left, [0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(right, [0.0, 0.0, 0.0, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn the_ring_carries_state_across_blocks() {
        let mut align = DryAlign::new(2).unwrap();
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
