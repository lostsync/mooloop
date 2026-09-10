//! Console summing: a non-linearity at every point a strip leaves, and its
//! exact inverse at every point signals arrive.
//!
//! In the spirit of the Airwindows Console series rather than a port of it.
//! The idea is the part worth taking: put a curve on the way *out* of a strip
//! and its inverse on the way *in* to the summing point. The pair cancels for
//! one strip on its own and leaves a real interaction between several, so the
//! character is entirely in the summing -- there is no "sound" to a channel
//! that is alone, which is what stops this being a saturator wearing a
//! mixer's clothes.
//!
//! [`encode`] is `sin` and [`decode`] is `asin`, unscaled.
//!
//! # What it does to a sum
//!
//! `sin(x) < x` and `asin(y) > y`, so a sum of encoded strips decodes to
//! *more* than the linear sum while the total is modest, and runs into a
//! hard ceiling when it is not. At sources calibrated to mooloop's -12 dBFS
//! operating level:
//!
//! ```text
//!   sources    console      linear
//!         2      0.518       0.500     +0.3 dB   separation
//!         4      1.427       1.000     +3.1 dB
//!         8      1.571       2.000     -2.1 dB   at the ceiling
//!        16      1.571       4.000     -8.1 dB
//! ```
//!
//! (Coincident peaks, which real music does not have -- the density at which
//! a mix reaches the ceiling is higher than this table implies.) That shape
//! is the whole point: it separates a sparse mix and refuses to let a dense
//! one keep growing.
//!
//! # The ceiling, and where it actually is
//!
//! `docs/GAIN_STRUCTURE.md` otherwise says *"nothing bounds a sample in the
//! live path"*, and this is the exception, on purpose. Adam's ruling,
//! 2026-09-09: *"you can do a perfectly clean mix on it if you want but you
//! could also drive it and get something nice in return."* Console-off stays
//! that clean path, stays the default, and stays bit-identical to a tree
//! without this module.
//!
//! **The bound is `PI/2`, which is +3.92 dBFS, not 0 dBFS** -- the plan said
//! 0 and the plan was wrong. Normalizing the pair to put the knee at full
//! scale (`sin(x * PI/2)` and `asin(y) / (PI/2)`) was tried and rejected for
//! two reasons. It clips two sources at -8 dBFS, which is a limiter rather
//! than a summing law. And it is numerically worst exactly where music is
//! loudest: the round trip's conditioning is `1 / cos`, and the normalized
//! curve's `cos` goes to zero at full scale, so `decode(encode(0.999))` comes
//! back wrong in the fifth decimal place. The unscaled pair has `cos >= 0.54`
//! everywhere below full scale, so the null holds to a few `f32` ULPs across
//! the whole useful range.
//!
//! Both functions clamp. In [`decode`] the clamp is `asin`'s domain and there
//! is no choice about it; in [`encode`] it is a *monotone extension* of the
//! curve past its quarter period, and the alternative is worse than a bound
//! -- `sin` turns over there, so an unclamped encode would fold a hot strip
//! back down through itself, which is not saturation, it is garbage.

use std::f32::consts::FRAC_PI_2;

/// Encode a sample on its way out of a strip.
///
/// Monotone everywhere, saturating above `PI/2` rather than folding.
#[inline]
pub fn encode(sample: f32) -> f32 {
    sample.clamp(-FRAC_PI_2, FRAC_PI_2).sin()
}

/// Decode a sample at the point encoded strips converge.
///
/// The inverse of [`encode`] over `[-PI/2, PI/2]`, which is what makes one
/// strip on its own null against console-off.
#[inline]
pub fn decode(sample: f32) -> f32 {
    sample.clamp(-1.0, 1.0).asin()
}

/// The largest magnitude a decode point can produce: `PI/2`, or +3.92 dBFS.
pub const DECODE_CEILING: f32 = FRAC_PI_2;

/// Encode a block in place.
pub fn encode_block(left: &mut [f32], right: &mut [f32]) {
    for sample in left.iter_mut().chain(right.iter_mut()) {
        *sample = encode(*sample);
    }
}

/// Decode a block in place.
pub fn decode_block(left: &mut [f32], right: &mut [f32]) {
    for sample in left.iter_mut().chain(right.iter_mut()) {
        *sample = decode(*sample);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole design rests on: one strip alone is not
    /// coloured, so console mode cannot be a saturator in disguise.
    ///
    /// Exact would be nicer and is not available: `asin(sin(x))` is two
    /// transcendental functions in `f32`. A tolerance of 1e-6 is about
    /// -120 dBFS, below the floor a 24-bit export is written at, and it holds
    /// over the whole range below full scale -- which is the thing the
    /// normalized variant could not do.
    #[test]
    fn one_strip_alone_is_the_identity() {
        let mut worst = 0.0f32;
        for step in -1000..=1000 {
            let x = step as f32 / 1000.0;
            let error = (decode(encode(x)) - x).abs();
            worst = worst.max(error);
            assert!(error < 1e-6, "decode(encode({x})) was off by {error}");
        }
        // Stated rather than merely asserted: the number is the argument for
        // the unscaled pair, and a change that quietly made it worse should
        // show up as a diff here.
        assert!(worst < 1e-6, "worst round-trip error {worst}");
    }

    /// A sparse sum separates and a dense one is bounded. This is the whole
    /// audible claim, as numbers.
    #[test]
    fn a_sum_is_not_the_linear_sum() {
        // Near zero the curve is nearly straight, so two quiet strips sum
        // almost linearly. That is right: a quiet mix is not meant to be
        // reshaped.
        let quiet = decode(encode(0.05) + encode(0.05));
        assert!((quiet - 0.1).abs() < 0.001, "two quiet strips gave {quiet}");

        // At the operating level two strips separate slightly and four
        // clearly, which is the "not the linear sum" half.
        let two = decode(encode(0.25) * 2.0);
        assert!(two > 0.5 + 0.005, "two strips did not separate: {two}");
        let four = decode(encode(0.25) * 4.0);
        assert!(four > 1.0 + 0.2, "four strips did not separate: {four}");

        // And a dense mix is bounded, which is the other half.
        let eight = decode(encode(0.25) * 8.0);
        assert!(eight < 2.0, "eight strips were not bounded: {eight}");
    }

    /// The ceiling, as a test so it cannot drift without somebody deciding to
    /// move it. `docs/GAIN_STRUCTURE.md` says the same thing in prose.
    #[test]
    fn a_decode_point_bounds_at_the_ceiling() {
        for sources in 1..=64 {
            let sum: f32 = encode(0.9) * sources as f32;
            let out = decode(sum);
            assert!(
                out.abs() <= DECODE_CEILING,
                "{sources} sources decoded to {out}, past the ceiling"
            );
        }
        assert_eq!(decode(2.0), DECODE_CEILING);
        assert_eq!(decode(-2.0), -DECODE_CEILING);
    }

    /// Monotone past its quarter period rather than folding. An unclamped
    /// `sin` would send a strip driven to `PI` back to zero.
    #[test]
    fn a_hot_strip_saturates_instead_of_folding() {
        assert_eq!(encode(3.0), encode(FRAC_PI_2));
        assert_eq!(encode(-3.0), encode(-FRAC_PI_2));
        let mut previous = f32::NEG_INFINITY;
        for step in -400..=400 {
            let value = encode(step as f32 / 100.0);
            assert!(value >= previous, "encode turned over at {step}");
            previous = value;
        }
    }

    /// Nesting needs no special case: a bus that encodes its own output is
    /// decoded by whatever it feeds, exactly as a channel is. This is what
    /// makes a group free in step 04.
    #[test]
    fn nesting_needs_no_special_case() {
        let group_in = decode(encode(0.3) + encode(0.4));
        let master_in = decode(encode(group_in));
        assert!(
            (master_in - group_in).abs() < 1e-6,
            "a lone group through the master was coloured: {master_in} vs {group_in}"
        );
    }
}
