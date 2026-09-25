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

    /// Replace the first `frames` samples with another bus's.
    ///
    /// What a send needs and a sum does not: a strip with two outgoing edges
    /// has to keep its own buffer intact while each edge is given its own
    /// level and its own delay, so each one works on a copy.
    pub fn copy_from(&mut self, other: &StereoBus, frames: usize) {
        self.l[..frames].copy_from_slice(&other.l[..frames]);
        self.r[..frames].copy_from_slice(&other.r[..frames]);
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
    ///
    /// **A NaN or an infinity reads as an infinite peak, never as silence.**
    /// `f32::max` returns its other operand when one is NaN, so this used to
    /// drop every NaN sample: a bus full of them metered as silence while it
    /// fed the DAC, and the effect host's sleep check -- which reads this same
    /// peak -- could put the broken device to sleep (MOO-93). Infinity is the
    /// honest reading of a sample with no magnitude, it is above every
    /// threshold anything compares a peak with, and it survives the
    /// `max(0.0)` every meter publish takes. The effect host's non-finite
    /// input check (MOO-176) is this same answer.
    ///
    /// It runs once per effect slot and once per strip every block, so it is
    /// a branch-free integer fold that vectorises (MOO-262); see
    /// [`abs_peak`].
    pub fn peak(&self, frames: usize) -> (f32, f32) {
        (abs_peak(&self.l[..frames]), abs_peak(&self.r[..frames]))
    }
}

/// The largest `|sample|` in `samples`: `0.0` when it is empty, and
/// `INFINITY` when any sample is a NaN or an infinity.
///
/// Folds the **bit patterns**, not the floats. With the sign bit cleared, an
/// IEEE-754 `f32` orders the same as its bits read as an integer, from `+0`
/// through the subnormals and normals to `+inf` at `0x7f80_0000`, and every
/// NaN sits above that. So an integer `max` over the magnitudes' bits finds
/// the largest magnitude exactly -- the same bits the `f32::max` fold of
/// `abs()` returned, for every finite block -- and any NaN pushes the result
/// past infinity's pattern, where it is read back as `INFINITY`. The masked
/// bits never set bit 31, so the fold is a signed `max`, which SSE2 does as a
/// compare and a blend. There is no branch and no NaN rule in the loop to
/// stop LLVM vectorising it, which the NaN-aware float fold MOO-93
/// introduced did.
pub fn abs_peak(samples: &[f32]) -> f32 {
    const MAGNITUDE: u32 = 0x7fff_ffff;
    const INFINITY_BITS: i32 = 0x7f80_0000;
    let mut max = 0i32;
    for sample in samples {
        max = max.max((sample.to_bits() & MAGNITUDE) as i32);
    }
    if max > INFINITY_BITS {
        f32::INFINITY
    } else {
        f32::from_bits(max as u32)
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

    /// Shaped against the unfixed `f32::max` fold, which read both of these
    /// buses as `(0.0, 0.0)`.
    #[test]
    fn a_non_finite_sample_never_reads_as_silence() {
        let mut bus = StereoBus::with_capacity(8);
        bus.l[3] = f32::NAN;
        bus.r[5] = f32::NEG_INFINITY;
        assert_eq!(bus.peak(8), (f32::INFINITY, f32::INFINITY));
        // And a NaN after a louder sample is not outranked by it.
        let mut bus = StereoBus::with_capacity(4);
        bus.l[0] = 0.5;
        bus.l[1] = f32::NAN;
        assert_eq!(bus.peak(4).0, f32::INFINITY);
    }

    /// The fold `peak` used from MOO-93 to MOO-262, kept only to say that
    /// the integer fold answers exactly what it did.
    fn float_fold(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0f32, |peak, &sample| {
            peak.max(if sample.is_nan() { f32::INFINITY } else { sample.abs() })
        })
    }

    /// Bit-identical to the float fold on finite input, at every length up
    /// to 300, over random bit patterns (so subnormals, huge values, both
    /// zeros and both signs turn up), audio-range values, and near-silence.
    #[test]
    fn the_integer_fold_reads_every_finite_block_as_the_float_fold_did() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut block = vec![0.0f32; 300];
        for round in 0..4_000 {
            let len = (next() % 301) as usize;
            for sample in &mut block[..len] {
                let bits = next() as u32;
                *sample = match round % 3 {
                    // Anything finite at all: clearing an exponent bit
                    // turns an infinity or a NaN into a large finite value.
                    0 => {
                        let value = f32::from_bits(bits);
                        if value.is_finite() {
                            value
                        } else {
                            f32::from_bits(bits & 0xbfff_ffff)
                        }
                    }
                    1 => (bits as i32 as f32) / (i32::MAX as f32) * 2.0,
                    _ => match bits % 8 {
                        0 => -0.0,
                        1 => f32::from_bits(bits >> 9),
                        2 => -f32::from_bits(bits >> 9),
                        _ => 0.0,
                    },
                };
            }
            let expected = float_fold(&block[..len]);
            assert!(expected.is_finite());
            let got = abs_peak(&block[..len]);
            assert_eq!(got.to_bits(), expected.to_bits(), "round {round}, {len} samples");
        }
        assert_eq!(abs_peak(&[]).to_bits(), 0.0f32.to_bits());
        assert_eq!(abs_peak(&[-0.0, -0.0]).to_bits(), 0.0f32.to_bits());
        assert_eq!(abs_peak(&[f32::MAX, -f32::MAX]), f32::MAX);
        assert_eq!(abs_peak(&[-f32::from_bits(1)]), f32::from_bits(1));
    }

    /// Every NaN pattern -- quiet, signalling, negative, any payload -- and
    /// both infinities read as `INFINITY`, wherever in the block they fall
    /// and whatever else is in it, as they did through the float fold.
    #[test]
    fn every_nan_pattern_and_infinity_reads_as_an_infinite_peak() {
        let specials = [
            f32::NAN,
            -f32::NAN,
            f32::from_bits(0x7f80_0001),
            f32::from_bits(0xffff_ffff),
            f32::from_bits(0x7fc0_1234),
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];
        for special in specials {
            for at in [0, 1, 63, 127] {
                let mut bus = StereoBus::with_capacity(128);
                bus.l[..128].fill(0.75);
                bus.r[..128].fill(-f32::MAX);
                bus.l[at] = special;
                bus.r[at] = special;
                let bits = special.to_bits();
                assert_eq!(bus.peak(128), (f32::INFINITY, f32::INFINITY), "{bits:#x} at {at}");
                assert_eq!(float_fold(&bus.l[..128]), f32::INFINITY, "{bits:#x} at {at}");
            }
        }
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
