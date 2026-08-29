//! Stereo audio buses — the buffers that flow between nodes.
//!
//! Everything in mooloop is planar stereo (separate L/R), matching JACK's
//! per-port model. A [`StereoBus`] is preallocated to [`MAX_BLOCK_SIZE`] at
//! engine startup; nodes operate in place on whatever slice the current block
//! needs, so the realtime thread never allocates.
//!
//! The engine owns one bus per channel strip plus the master bus. Keeping
//! buffers out of the nodes means future routing (bus sends, sidechains) is
//! purely an engine-side concern: nodes never need to know where their
//! buffer came from or goes to.

/// Hard upper bound on a JACK cycle's frame count. PipeWire's default max
/// quantum is 8192; cycles larger than this are clamped (see `Graph`).
pub const MAX_BLOCK_SIZE: usize = 8192;

/// A preallocated planar stereo buffer.
pub struct StereoBus {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl StereoBus {
    /// Allocate a bus that can hold any block up to `capacity` frames.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            l: vec![0.0; capacity],
            r: vec![0.0; capacity],
        }
    }

    pub fn capacity(&self) -> usize {
        self.l.len()
    }

    /// Zero the first `frames` samples (the active region for this block).
    pub fn clear(&mut self, frames: usize) {
        for s in &mut self.l[..frames] {
            *s = 0.0;
        }
        for s in &mut self.r[..frames] {
            *s = 0.0;
        }
    }

    /// Sum another bus into this one (unity gain), first `frames` samples.
    pub fn add_from(&mut self, other: &StereoBus, frames: usize) {
        for i in 0..frames {
            self.l[i] += other.l[i];
            self.r[i] += other.r[i];
        }
    }

    /// Scale L and R by independent gains (used for gain/pan staging).
    pub fn apply_stereo_gain(&mut self, gain_l: f32, gain_r: f32, frames: usize) {
        self.apply_stereo_gain_range(gain_l, gain_r, 0, frames);
    }

    /// Apply gain to `start..end` only. The strip's output stage uses this to
    /// step gain and pan at the control rate when a source or a lane is
    /// driving them, instead of stamping one value across the whole block.
    pub fn apply_stereo_gain_range(&mut self, gain_l: f32, gain_r: f32, start: usize, end: usize) {
        for i in start..end {
            self.l[i] *= gain_l;
            self.r[i] *= gain_r;
        }
    }

    /// Peak absolute amplitude over the first `frames` samples, `(l, r)`.
    pub fn peak(&self, frames: usize) -> (f32, f32) {
        let mut pl = 0.0f32;
        let mut pr = 0.0f32;
        for i in 0..frames {
            pl = pl.max(self.l[i].abs());
            pr = pr.max(self.r[i].abs());
        }
        (pl, pr)
    }
}

/// Historical constant-power channel pan law. `pan` is in `[-1, 1]`; centre
/// sends -3 dB to each side and the endpoints are unity. Retaining this law
/// preserves existing project gain staging.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * core::f32::consts::FRAC_PI_4;
    (angle.cos(), angle.sin())
}

/// Level-neutral balance for an already-stereo bus. Centre is `(1, 1)`; moving
/// toward an endpoint attenuates only the opposite side and never boosts the
/// surviving side. This is deliberately distinct from panning a source.
pub fn balance_gains(balance: f32) -> (f32, f32) {
    let balance = balance.clamp(-1.0, 1.0);
    if balance < 0.0 {
        (1.0, (-balance * core::f32::consts::FRAC_PI_2).cos())
    } else {
        ((balance * core::f32::consts::FRAC_PI_2).cos(), 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_ops() {
        let mut a = StereoBus::with_capacity(64);
        let mut b = StereoBus::with_capacity(64);
        b.l[..64].fill(0.5);
        b.r[..64].fill(0.25);
        a.add_from(&b, 64);
        assert_eq!(a.peak(64), (0.5, 0.25));
        a.apply_stereo_gain(2.0, 4.0, 64);
        assert_eq!(a.peak(64), (1.0, 1.0));
        a.clear(64);
        assert_eq!(a.peak(64), (0.0, 0.0));
    }

    #[test]
    fn channel_pan_retains_the_historical_constant_power_law() {
        let (l, r) = pan_gains(0.0);
        assert!((l - r).abs() < 1e-6);
        assert!((l * l + r * r - 1.0).abs() < 1e-5);
        for pan in [-1.0, -0.5, 0.0, 0.37, 1.0] {
            let (l, r) = pan_gains(pan);
            assert!((l * l + r * r - 1.0).abs() < 1e-5, "power moved at {pan}");
        }
        let (l, r) = pan_gains(-1.0);
        assert!((l - 1.0).abs() < 1e-6 && r < 1e-6);
        let (l, r) = pan_gains(1.0);
        assert!(l < 1e-6 && (r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn stereo_balance_is_unity_at_centre_without_endpoint_boost() {
        assert_eq!(balance_gains(0.0), (1.0, 1.0));
        let (left, right) = balance_gains(-1.0);
        assert!((left - 1.0).abs() < 1e-6 && right.abs() < 1e-6);
        let (left, right) = balance_gains(1.0);
        assert!(left.abs() < 1e-6 && (right - 1.0).abs() < 1e-6);
    }
}
