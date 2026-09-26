//! Oscillators and noise sources for the synth voices.
//!
//! Saw and pulse use PolyBLEP correction so they stay band-limited enough for
//! musical use without per-sample oversampling. Sine reads a lookup table
//! (see [`table_sine`]) rather than calling `sin` per sample; triangle is
//! still naive arithmetic. Both alias negligibly at these frequencies.
//! Everything is allocation-free state advanced one sample at a time.

use core::f32::consts::TAU;
use std::sync::OnceLock;

use mooloop_core::OscWave;

use crate::scale::clamp_param;

/// Entries per cycle in [`sine_table`]. Large enough that linear
/// interpolation between neighbouring entries holds a 440 Hz table sine
/// under -90 dB THD (`sine_table_thd_is_below_90_db`), small enough that the
/// table (16 KB, an `f32` pair an entry) and the one-time build cost are both
/// trivial. Built once, process-wide, the way
/// [`crate::interpolate::SincTable`] is.
const SINE_TABLE_LEN: usize = 2048;

/// Each entry and the step to the next one (`[sin, next - sin]`), so a read
/// is one load and a multiply-add where it was two loads and a subtract
/// (MOO-264). The step is the same `f32` subtraction the read used to make,
/// done once here, so the read is the same to the bit.
static SINE_TABLE: OnceLock<[[f32; 2]; SINE_TABLE_LEN]> = OnceLock::new();

/// The process-wide sine table, built on first call.
///
/// [`Osc::new`] forces this off the audio thread — the same reason
/// `SincTable::shared` is forced at device construction — so no
/// `next_sample`/`next_step` call on the audio thread is ever the one that
/// builds it.
fn sine_table() -> &'static [[f32; 2]; SINE_TABLE_LEN] {
    SINE_TABLE.get_or_init(|| {
        let sine: [f32; SINE_TABLE_LEN] =
            std::array::from_fn(|index| ((index as f32 / SINE_TABLE_LEN as f32) * TAU).sin());
        std::array::from_fn(|index| {
            let next = sine[(index + 1) & (SINE_TABLE_LEN - 1)];
            [sine[index], next - sine[index]]
        })
    })
}

/// One cycle of a sine, read at `phase` (cycles, any real value — folded to
/// `[0, 1)`) by linear interpolation between the two nearest table entries.
///
/// A deliberate approximation of `(phase * TAU).sin()`, not a bit-exact
/// stand-in for it (`reports/fable-2026-09-22.md` finding 2): the error it
/// introduces is bounded by `sine_table_thd_is_below_90_db` rather than
/// argued from the table size alone.
#[inline]
fn table_sine(phase: f32) -> f32 {
    table_sine_wrapped(wrap_unit(phase))
}

/// [`table_sine`] for a phase already folded by [`wrap_unit`] (or
/// [`wrap_near`], its equal): `[0, 1]`, or `-0.0`.
///
/// Folding it again changed nothing but `1.0`, which it turned into `0.0`,
/// and both read entry 0 at a fraction of zero, so the second fold every
/// oscillator's sine paid is gone (MOO-264).
/// `a_folded_phase_reads_the_table_as_a_refolded_one_bit_for_bit` pins it.
#[inline(always)]
fn table_sine_wrapped(phase: f32) -> f32 {
    let table = sine_table();
    // In [0, SINE_TABLE_LEN] (the top when a tiny negative phase rounds up to
    // a whole cycle), so the truncating cast is its floor and the mask its
    // wrap: one conversion, where `as usize`, `%` and `floor` were three
    // (MOO-255). `the_masked_table_read_is_the_old_one_bit_for_bit` pins it.
    let scaled = phase * SINE_TABLE_LEN as f32;
    let whole = scaled as i32;
    let [value, step] = table[whole as usize & (SINE_TABLE_LEN - 1)];
    let frac = scaled - whole as f32;
    value + step * frac
}

/// One oscillator step: the waveform value, and where inside this sample the
/// phase crossed the end of its cycle.
///
/// The wrap is what makes hard sync possible without an oscillator reaching
/// into another one's state: a master reports where it wrapped, and a slave
/// resets itself at that fraction of the sample. `None` is the ordinary case.
#[derive(Clone, Copy, Debug)]
pub struct OscStep {
    pub value: f32,
    /// Fraction of the sample interval at which the phase wrapped, in
    /// `(0, 1]`. Sub-sample, so a sync reset lands where the master's cycle
    /// actually ended rather than being quantized to the sample grid.
    pub wrap: Option<f32>,
}

/// A single band-limited oscillator.
#[derive(Clone, Copy, Debug)]
pub struct Osc {
    phase: f32,
    /// Phase at the start of the last step. Hard sync needs it: a slave has
    /// already advanced by the time it learns its master wrapped, and the
    /// step height of the reset is measured from where the slave *was*.
    last_phase: f32,
    /// Set by [`Self::sync_reset`] and consumed by the next step.
    ///
    /// A reset leaves the phase inside the cycle-boundary PolyBLEP's window,
    /// where the waveform would otherwise correct a wrap that did not happen
    /// — and correct it for the wrong height, since a natural wrap steps by
    /// the full waveform range and a sync reset steps by whatever the slave
    /// had reached. [`sync_blep`] issues the right correction, so the
    /// oscillator's own must stand down for that one sample.
    after_sync: bool,
}

impl Osc {
    pub fn new() -> Self {
        // Force the shared sine table here, on whatever thread constructs
        // the oscillator, so no `process()` call is ever the first to
        // build it.
        sine_table();
        Self {
            phase: 0.0,
            last_phase: 0.0,
            after_sync: false,
        }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.last_phase = 0.0;
        self.after_sync = false;
    }

    /// Start the next cycle from `phase`. For deterministic per-voice start
    /// phases; a sync reset goes through [`Self::sync_reset`] instead, which
    /// also reports the discontinuity it introduced.
    pub fn reset_to(&mut self, phase: f32) {
        self.phase = phase.rem_euclid(1.0);
        self.last_phase = self.phase;
        self.after_sync = false;
    }

    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// Advance one sample at `freq_hz` and return the waveform value in
    /// `[-1, 1]`. `pulse_width` only applies to [`OscWave::Pulse`].
    pub fn next_sample(
        &mut self,
        freq_hz: f32,
        wave: OscWave,
        pulse_width: f32,
        sample_rate: u32,
    ) -> f32 {
        self.next_step(freq_hz, wave, pulse_width, 0.0, sample_rate).value
    }

    /// Advance one sample with `phase_offset` cycles of phase modulation
    /// added at the read, and report any wrap.
    ///
    /// The offset shifts where the waveform is *read*, not how fast the phase
    /// accumulates, which is what keeps a modulated oscillator's centre pitch
    /// where it was tuned. The wrap is reported from the underlying phase
    /// accumulator rather than from the modulated read position, so a master
    /// being cross-modulated still delivers one sync edge per cycle of its
    /// own tuning instead of a burst of them.
    pub fn next_step(
        &mut self,
        freq_hz: f32,
        wave: OscWave,
        pulse_width: f32,
        phase_offset: f32,
        sample_rate: u32,
    ) -> OscStep {
        let dt = increment(freq_hz, sample_rate);
        let phase = self.phase;
        let advanced = phase + dt;
        self.last_phase = phase;
        self.phase = step_fract(advanced);
        let boundary_dt = if core::mem::take(&mut self.after_sync) {
            0.0
        } else {
            dt
        };
        OscStep {
            value: wave_value(
                wrap_near(phase + phase_offset),
                wave,
                pulse_width,
                dt,
                boundary_dt,
            ),
            wrap: (advanced >= 1.0).then(|| ((1.0 - phase) / dt).clamp(0.0, 1.0)),
        }
    }

    /// Advance one sample reading a crossfade of two waves at the same phase.
    ///
    /// A morph is not two oscillators mixed: they would have to be kept in
    /// lockstep by the caller, and each would pay its own phase accumulator
    /// and PolyBLEP. One phase, read twice, is both cheaper and exactly
    /// continuous — at `mix = 0` and `mix = 1` this is `next_step` on the
    /// respective wave, sample for sample.
    ///
    /// DS-01's tone morph is what wants this: a selector would be a
    /// structural discrete and so ineligible for modulation, and sweeping
    /// timbre across a hit is a percussion gesture.
    pub fn next_step_morph(
        &mut self,
        freq_hz: f32,
        waves: (OscWave, OscWave),
        mix: f32,
        pulse_width: f32,
        phase_offset: f32,
        sample_rate: u32,
    ) -> OscStep {
        let dt = increment(freq_hz, sample_rate);
        let phase = self.phase;
        let advanced = phase + dt;
        self.last_phase = phase;
        self.phase = step_fract(advanced);
        let boundary_dt = if core::mem::take(&mut self.after_sync) {
            0.0
        } else {
            dt
        };
        let read = wrap_near(phase + phase_offset);
        let mix = mix.clamp(0.0, 1.0);
        let a = wave_value(read, waves.0, pulse_width, dt, boundary_dt);
        let value = if mix <= 0.0 {
            a
        } else {
            let b = wave_value(read, waves.1, pulse_width, dt, boundary_dt);
            a + (b - a) * mix
        };
        OscStep {
            value,
            wrap: (advanced >= 1.0).then(|| ((1.0 - phase) / dt).clamp(0.0, 1.0)),
        }
    }

    /// Hard-sync this oscillator to a master that wrapped `frac` of the way
    /// through the sample just rendered, and return the step height the reset
    /// introduced.
    ///
    /// The caller turns that height into a band-limited correction (see
    /// [`sync_blep`]) rather than this doing it, because the correction spans
    /// two samples and the second one belongs to the caller's next iteration.
    /// A naive reset without it is the classic sync alias.
    pub fn sync_reset(
        &mut self,
        frac: f32,
        freq_hz: f32,
        wave: OscWave,
        pulse_width: f32,
        phase_offset: f32,
        sample_rate: u32,
    ) -> f32 {
        let dt = increment(freq_hz, sample_rate);
        let at_reset = wrap_unit(self.last_phase + frac * dt);
        // Measured on the *naive* waveform, which is what passing `dt = 0`
        // asks for: the PolyBLEP residuals already in `wave_value` correct the
        // oscillator's own wrap, and a reset that lands on one would otherwise
        // read a value that has been corrected once and correct it again.
        let naive = |phase: f32| wave_value(phase, wave, pulse_width, 0.0, 0.0);
        let before = naive(wrap_unit(at_reset + phase_offset));
        let after = naive(wrap_unit(phase_offset));
        // The remainder of the sample after the reset, so the slave's next
        // read sits where a continuous-time reset would have left it.
        self.phase = fast_fract((1.0 - frac) * dt);
        self.last_phase = 0.0;
        self.after_sync = true;
        after - before
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only: run this thread's oscillators the way they ran before
    /// MOO-264, so the new path is pinned against the old one in-process and
    /// the two are timed in one run.
    static BEFORE_MOO264: core::cell::Cell<bool> = const { core::cell::Cell::new(false) };
}

#[cfg(test)]
#[inline(always)]
fn before_moo264() -> bool {
    BEFORE_MOO264.with(core::cell::Cell::get)
}

#[cfg(not(test))]
#[inline(always)]
const fn before_moo264() -> bool {
    false
}

/// The sine read before MOO-264, for [`before_moo264`]: folded a second
/// time, and two loads and a subtract.
#[cfg(test)]
fn old_table_sine(phase: f32) -> f32 {
    let table = sine_table();
    let scaled = wrap_unit(phase) * SINE_TABLE_LEN as f32;
    let whole = scaled as i32;
    let index = whole as usize & (SINE_TABLE_LEN - 1);
    let frac = scaled - whole as f32;
    let next = (index + 1) & (SINE_TABLE_LEN - 1);
    table[index][0] + (table[next][0] - table[index][0]) * frac
}

#[cfg(not(test))]
#[inline(always)]
fn old_table_sine(phase: f32) -> f32 {
    table_sine(phase)
}

/// Past this magnitude every `f32` is a whole number, and below it every
/// whole number fits an `i32`, so a truncating cast is exact.
const EXACT_CAST: f32 = 8_388_608.0;

/// `x.trunc()`, bit for bit, without the call.
///
/// The x86-64 baseline the release build targets has no SSE4.1, so
/// `trunc`, `floor`, `fract` and `rem_euclid` are each a call into libm, and
/// a sine read made four of them a sample (MOO-255). A truncating cast is
/// one instruction each way; `copysign` gives `-0.0` for a negative
/// fraction exactly as `trunc` does. NaN, infinities and anything already
/// whole take the library path.
#[inline(always)]
fn fast_trunc(x: f32) -> f32 {
    if x.abs() < EXACT_CAST {
        (x as i32 as f32).copysign(x)
    } else {
        x.trunc()
    }
}

/// `x.fract()`, bit for bit: `x - x.trunc()`, as the standard library
/// defines it.
#[inline(always)]
fn fast_fract(x: f32) -> f32 {
    x - fast_trunc(x)
}

/// `x.rem_euclid(1.0)`, bit for bit.
///
/// `x % 1.0` is `x - trunc(x)` exactly, carrying the sign of `x` even when
/// it is zero; the `copysign` restores that one case (`-2.0 % 1.0` is
/// `-0.0`, where the subtraction gives `+0.0`). The negative case then adds
/// one, rounded once, as `rem_euclid` does.
#[inline(always)]
fn wrap_unit(x: f32) -> f32 {
    let r = (x - fast_trunc(x)).copysign(x);
    if r < 0.0 {
        r + 1.0
    } else {
        r
    }
}

/// [`fast_fract`], bit for bit, for the `(0, 2)` a phase lands in after one
/// step: a compare and a subtract, where the fraction is two conversions and
/// a sign copy on every oscillator's sample-to-sample chain (MOO-264).
/// Anything else, zero included, takes [`fast_fract`] itself.
#[inline(always)]
fn step_fract(x: f32) -> f32 {
    if before_moo264() {
        return fast_fract(x);
    }
    if x > 0.0 && x < 2.0 {
        // `x - 1.0` is exact over [1, 2), as `x - trunc(x)` is, and over
        // (0, 1) the fraction is `x - 0.0`, which is `x`.
        x - if x >= 1.0 { 1.0 } else { 0.0 }
    } else {
        fast_fract(x)
    }
}

/// [`wrap_unit`], bit for bit, for the `(-1, 2)` a read position lands in
/// unless phase modulation pushes it further: a compare and an add, where
/// the fold is two conversions and two sign copies on the read's chain
/// (MOO-264). Anything else takes [`wrap_unit`] itself.
#[inline(always)]
fn wrap_near(x: f32) -> f32 {
    if before_moo264() {
        return wrap_unit(x);
    }
    if x > -1.0 && x < 2.0 {
        // `wrap_unit`'s own arithmetic on each piece, with its truncation
        // known: over (-1, 0) it is `x + 1.0`, rounded once; over [1, 2)
        // it is `x - 1.0`, exact; and over [0, 1) it is `x`, `-0.0`
        // included.
        if x < 0.0 {
            x + 1.0
        } else if x >= 1.0 {
            x - 1.0
        } else {
            x
        }
    } else {
        wrap_unit(x)
    }
}

/// Per-sample phase increment, with the frequency held inside the band the
/// oscillators are correct over.
fn increment(freq_hz: f32, sample_rate: u32) -> f32 {
    let sr = sample_rate as f32;
    clamp_param(freq_hz, 0.01, sr * 0.45) / sr
}

/// The waveform at an absolute phase.
///
/// `dt` sizes the PolyBLEP residuals, so passing `0` reads the naive waveform
/// — which is what measuring a sync step height wants. `boundary_dt` sizes
/// only the residual at the *cycle boundary*, separately, so a sample that
/// follows a sync reset can decline that one correction while the pulse's
/// width edge keeps its own.
#[inline(always)]
fn wave_value(phase: f32, wave: OscWave, pulse_width: f32, dt: f32, boundary_dt: f32) -> f32 {
    match wave {
        OscWave::Sine if before_moo264() => old_table_sine(phase),
        OscWave::Sine => table_sine_wrapped(phase),
        OscWave::Triangle => 4.0 * (phase - 0.5).abs() - 1.0,
        OscWave::Saw => 2.0 * phase - 1.0 - polyblep(phase, boundary_dt),
        OscWave::Pulse => {
            let width = clamp_param(pulse_width, 0.05, 0.95);
            let mut value = if phase < width { 1.0 } else { -1.0 };
            value += polyblep(phase, boundary_dt);
            value -= polyblep(wrap_near(phase - width), dt);
            value
        }
    }
}

/// The two-sample PolyBLEP correction for a step of height `height` that
/// happened `frac` of the way through the sample just rendered.
///
/// Returns `(now, next)`: add `now` to the sample already computed, and carry
/// `next` into the following one. Both fall out of the same residual the saw
/// and pulse discontinuities use, evaluated at the two sample points either
/// side of the step — `frac` before and `1 - frac` after — and scaled by half
/// the height, which is the convention `wave_value`'s saw already follows.
pub fn sync_blep(height: f32, frac: f32) -> (f32, f32) {
    let half = height * 0.5;
    let after = 1.0 - frac;
    (half * after * after, -half * frac * frac)
}

impl Default for Osc {
    fn default() -> Self {
        Self::new()
    }
}

/// PolyBLEP residual to subtract/add at discontinuities.
#[inline]
fn polyblep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// A tiny xorshift32 white-noise source. Deterministic, allocation-free, and
/// good enough for percussive noise bursts.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    state: u32,
}

impl Noise {
    pub fn new(seed: u32) -> Self {
        Self { state: seed.max(1) }
    }

    pub fn reset(&mut self, seed: u32) {
        self.state = seed.max(1);
    }

    /// Next white-noise sample in `[-1, 1]`.
    pub fn next_sample(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        // Map to [-1, 1) using the top 24 bits.
        (x >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{alias_db, cents, coherent_amplitude, db, dominant_hz, frames_for, RATES};

    /// The table read with one conversion and a mask is the one it replaced,
    /// with `rem_euclid`, `as usize`, `%` and `floor`, to the bit (MOO-255):
    /// at every table entry and between them, either side of zero and of a
    /// whole cycle, and far from either.
    #[test]
    fn the_masked_table_read_is_the_old_one_bit_for_bit() {
        fn old(phase: f32) -> f32 {
            let table = sine_table();
            let scaled = phase.rem_euclid(1.0) * SINE_TABLE_LEN as f32;
            let index = scaled as usize % SINE_TABLE_LEN;
            let frac = scaled - scaled.floor();
            let next = (index + 1) % SINE_TABLE_LEN;
            table[index][0] + (table[next][0] - table[index][0]) * frac
        }
        let mut phases = vec![0.0f32, -0.0, 1.0, -1.0, 1.0e-9, -1.0e-9, -1.0e-30, 3.75, -3.75];
        for entry in 0..=SINE_TABLE_LEN * 4 {
            let phase = entry as f32 / (SINE_TABLE_LEN * 4) as f32;
            phases.extend([phase, -phase, phase.next_up(), phase.next_down(), phase + 2.0, phase - 4.0]);
        }
        for phase in phases {
            assert_eq!(table_sine(phase).to_bits(), old(phase).to_bits(), "phase {phase:e}");
        }
    }

    /// The inline wrap, fraction and truncation are the library's, to the
    /// bit, across every magnitude an oscillator sees and past where the
    /// cast would stop being exact (MOO-255).
    /// Every magnitude an oscillator sees, past where the cast would stop
    /// being exact, either side of every whole number near zero, and the
    /// phases an increment lands on.
    fn rounding_cases() -> Vec<f32> {
        let mut values = vec![
            0.0f32, -0.0, 1.0, -1.0, 2.0, -2.0, 0.5, -0.5, 1.0e-9, -1.0e-9, 0.999_999_94,
            -0.999_999_94, 8_388_607.5, -8_388_607.5, 8_388_608.0, -8_388_608.0, 1.0e10,
            -1.0e10, f32::MAX, f32::MIN, f32::MIN_POSITIVE, -f32::MIN_POSITIVE,
            f32::INFINITY, f32::NEG_INFINITY, f32::NAN, 2_147_483_648.0, -2_147_483_648.0,
            4.0e9, -4.0e9, 16_777_216.0, -16_777_217.0,
        ];
        // Just either side of whole numbers, and phases that land exactly
        // on 1.0 (or on a whole number) from an increment.
        for whole in -8i32..=8 {
            let w = whole as f32;
            values.extend([w.next_down(), w, w.next_up()]);
        }
        for step in [0.25f32, 0.1, 1.0 / 3.0, 0.009_166_667] {
            let mut phase = 0.0f32;
            for _ in 0..400 {
                phase += step;
                values.extend([phase, -phase, 1.0 - phase, phase - 1.0]);
            }
        }
        // Every float in a spread of exponents, by bit pattern.
        let mut bits = 0x2000_0000u32;
        while bits < 0x4c00_0000 {
            let x = f32::from_bits(bits);
            values.push(x);
            values.push(-x);
            bits += 0x0000_9e37;
        }
        values
    }

    #[test]
    fn the_inline_rounding_is_the_librarys_bit_for_bit() {
        let values = rounding_cases();
        let same = |a: f32, b: f32| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
        for x in values {
            assert!(same(fast_trunc(x), x.trunc()), "trunc({x:e})");
            assert!(same(fast_fract(x), x.fract()), "fract({x:e})");
            assert!(same(wrap_unit(x), x.rem_euclid(1.0)), "rem_euclid({x:e})");
        }
    }

    #[test]
    fn all_waves_stay_bounded() {
        let sr = 48_000;
        for wave in OscWave::all() {
            let mut osc = Osc::new();
            for i in 0..sr as usize {
                let freq = 20.0 + (i % 5000) as f32;
                let value = osc.next_sample(freq, wave, 0.5, sr);
                assert!(
                    (-1.05..=1.05).contains(&value),
                    "{wave:?} out of range: {value}"
                );
            }
        }
    }

    #[test]
    fn sine_has_expected_frequency() {
        let sr = 48_000;
        let mut osc = Osc::new();
        let mut zero_crossings = 0u32;
        let mut prev = 0.0_f32;
        for _ in 0..sr as usize {
            let value = osc.next_sample(100.0, OscWave::Sine, 0.5, sr);
            if prev <= 0.0 && value > 0.0 {
                zero_crossings += 1;
            }
            prev = value;
        }
        assert_eq!(zero_crossings, 100);
    }

    #[test]
    fn phase_modulation_leaves_the_centre_pitch_alone() {
        // The whole reason the network is phase modulation and not exponential
        // FM: a modulated oscillator still crosses zero at its tuned rate, so
        // no tuning-compensation system is needed behind the XMOD knobs.
        let sr = 48_000;
        for depth in [0.0_f32, 0.5, 2.0] {
            let mut osc = Osc::new();
            let mut modulator = Osc::new();
            let mut wraps = 0u32;
            for _ in 0..sr as usize {
                let offset = modulator.next_sample(37.0, OscWave::Sine, 0.5, sr) * depth;
                if osc
                    .next_step(100.0, OscWave::Sine, 0.5, offset, sr)
                    .wrap
                    .is_some()
                {
                    wraps += 1;
                }
            }
            assert_eq!(wraps, 100, "depth {depth} moved the oscillator's rate");
        }
    }

    #[test]
    fn a_wrap_is_reported_once_per_cycle_at_a_sub_sample_position() {
        let sr = 48_000;
        let mut osc = Osc::new();
        let mut wraps = Vec::new();
        for index in 0..sr as usize {
            if let Some(frac) = osc.next_step(1000.0, OscWave::Saw, 0.5, 0.0, sr).wrap {
                assert!((0.0..=1.0).contains(&frac), "wrap fraction {frac} out of range");
                wraps.push(index);
            }
        }
        // 48 samples a cycle. The count is 999 or 1000 depending on where the
        // last one falls; what matters is that they are one cycle apart and
        // never land twice in one sample.
        assert!((999..=1000).contains(&wraps.len()), "{} wraps", wraps.len());
        for pair in wraps.windows(2) {
            assert_eq!(pair[1] - pair[0], 48);
        }
    }

    /// A hard-synced oscillator is a discontinuity generator, so the question
    /// is how badly the reset aliases.
    ///
    /// It cannot be answered by looking for energy in the wrong places: a
    /// synced oscillator is *exactly* periodic at the master's rate, so every
    /// alias product folds back onto the master's own harmonic grid and no
    /// band of the spectrum is alias-only. What aliasing does instead is get
    /// the harmonic magnitudes wrong, so the test compares them against an
    /// eight-times-oversampled render of the same sync, where nothing within
    /// the audio band has folded.
    ///
    /// The master is chosen so one period is a whole number of samples at both
    /// rates, which makes a single-period DFT exact and removes leakage from
    /// the comparison entirely.
    #[test]
    fn sync_blep_gets_the_harmonics_closer_than_a_naive_reset() {
        const MASTER_HZ: f32 = 375.0; // 128 samples a period at 48 kHz.
        const SLAVE_HZ: f32 = 1400.0;
        const HARMONICS: usize = 63;

        fn render(sr: u32, corrected: bool, samples: usize) -> Vec<f32> {
            let mut master = Osc::new();
            let mut slave = Osc::new();
            let mut carry = 0.0_f32;
            let mut out = vec![0.0_f32; samples];
            for sample in out.iter_mut() {
                let mut value = slave.next_step(SLAVE_HZ, OscWave::Saw, 0.5, 0.0, sr).value + carry;
                carry = 0.0;
                if let Some(frac) = master.next_step(MASTER_HZ, OscWave::Saw, 0.5, 0.0, sr).wrap {
                    let height = slave.sync_reset(frac, SLAVE_HZ, OscWave::Saw, 0.5, 0.0, sr);
                    if corrected {
                        let (now, next) = sync_blep(height, frac);
                        value += now;
                        carry = next;
                    }
                }
                *sample = value;
            }
            out
        }

        /// Magnitudes of harmonics 1..=`HARMONICS` from exactly one period.
        fn harmonics(period: &[f32]) -> [f64; HARMONICS] {
            let n = period.len();
            std::array::from_fn(|index| {
                let bin = index + 1;
                let step = -core::f64::consts::TAU * bin as f64 / n as f64;
                let (mut re, mut im) = (0.0_f64, 0.0_f64);
                for (offset, sample) in period.iter().enumerate() {
                    let angle = step * offset as f64;
                    re += *sample as f64 * angle.cos();
                    im += *sample as f64 * angle.sin();
                }
                (re * re + im * im).sqrt() / n as f64
            })
        }

        // The eighth period of each render, so the comparison is not looking
        // at whatever the first master cycle happens to do.
        let reference = harmonics(&render(48_000 * 8, true, 1024 * 16)[1024 * 8..1024 * 9]);
        let naive = harmonics(&render(48_000, false, 128 * 16)[128 * 8..128 * 9]);
        let corrected = harmonics(&render(48_000, true, 128 * 16)[128 * 8..128 * 9]);

        let error = |measured: &[f64; HARMONICS]| -> f64 {
            measured
                .iter()
                .zip(reference.iter())
                .map(|(a, b)| (a - b).abs())
                .sum()
        };
        let (naive_error, corrected_error) = (error(&naive), error(&corrected));
        println!("sync harmonic error: naive {naive_error:.4}, blep {corrected_error:.4}");
        assert!(
            corrected_error < naive_error * 0.7,
            "correction left {corrected_error:.4} against the naive {naive_error:.4}"
        );
    }

    #[test]
    fn a_synced_oscillator_stays_bounded_across_the_keyboard() {
        let sr = 48_000;
        for slave_hz in [55.0_f32, 440.0, 3520.0, 9000.0] {
            let mut master = Osc::new();
            let mut slave = Osc::new();
            let mut carry = 0.0_f32;
            for _ in 0..sr as usize / 4 {
                let mut value = slave.next_step(slave_hz, OscWave::Saw, 0.5, 0.0, sr).value + carry;
                carry = 0.0;
                if let Some(frac) = master.next_step(220.0, OscWave::Saw, 0.5, 0.0, sr).wrap {
                    let height = slave.sync_reset(frac, slave_hz, OscWave::Saw, 0.5, 0.0, sr);
                    let (now, next) = sync_blep(height, frac);
                    value += now;
                    carry = next;
                }
                assert!(
                    value.is_finite() && value.abs() <= 3.0,
                    "slave at {slave_hz} Hz produced {value}"
                );
            }
        }
    }

    /// The table sine is a deliberate approximation of `(phase * TAU).sin()`
    /// (`reports/fable-2026-09-22.md` finding 2, Plan C step 4), gated on
    /// this bound rather than trusted from the table size alone.
    ///
    /// Exactly 11 whole cycles in 1,200 samples at every rate -- 440 Hz at
    /// 48 kHz, and the frequency that keeps the count whole elsewhere -- so
    /// the kit's unwindowed [`coherent_amplitude`] is exact and there is no
    /// spectral leakage to separate from real distortion, the same trick
    /// `sync_blep_gets_the_harmonics_closer_than_a_naive_reset` uses above.
    /// THD is every harmonic of the fundamental up to Nyquist against the
    /// fundamental itself.
    #[test]
    fn sine_table_thd_is_below_90_db() {
        let n = 1_200usize;
        for sr in RATES {
            let freq = sr as f32 * 11.0 / n as f32;
            let mut osc = Osc::new();
            let samples: Vec<f32> =
                (0..n).map(|_| osc.next_sample(freq, OscWave::Sine, 0.5, sr)).collect();

            let fundamental = coherent_amplitude(&samples, sr, freq);
            assert!(fundamental > 0.9, "the fundamental measured {fundamental}");
            let mut harmonic_power = 0.0_f64;
            let mut harmonic = 2.0_f32;
            while harmonic * freq < sr as f32 * 0.5 {
                let amplitude = coherent_amplitude(&samples, sr, harmonic * freq) as f64;
                harmonic_power += amplitude * amplitude;
                harmonic += 1.0;
            }
            let thd_db = db((harmonic_power.sqrt() / fundamental as f64) as f32);
            assert!(thd_db < -90.0, "{sr} Hz: THD was only {thd_db:.2} dB");
        }
    }

    /// Every wave is on pitch at every rate: the spectrum's strongest peak is
    /// the note, to within a cent.
    #[test]
    fn every_wave_is_on_pitch_at_every_rate() {
        for sr in RATES {
            for wave in OscWave::all() {
                for freq in [55.0_f32, 440.0, 3_520.0] {
                    let mut osc = Osc::new();
                    let samples: Vec<f32> = (0..frames_for(0.5, sr))
                        .map(|_| osc.next_sample(freq, wave, 0.5, sr))
                        .collect();
                    let found = dominant_hz(&samples, sr, 20.0);
                    assert!(
                        cents(found, freq).abs() < 1.0,
                        "{sr} Hz: {wave:?} at {freq} Hz measured {found} Hz"
                    );
                }
            }
        }
    }

    /// **The alias test the oscillators never had (MOO-117).** PolyBLEP is a
    /// two-sample correction, not a band limit: it pushes the folded-back
    /// harmonics down rather than removing them, and by less the higher the
    /// note sits against the rate. So the bound is per wave and stated at the
    /// top of the keyboard's useful range, and the same note at a higher rate
    /// has to alias no worse -- there is more room above it before anything
    /// folds. The triangle is naive (its corners are a slope step, which
    /// aliases at -12 dB/oct rather than the saw's -6), so it gets its own
    /// bound rather than a correction.
    #[test]
    fn band_limited_waves_keep_their_aliases_down_at_every_rate() {
        let band = (20.0, 20_000.0);
        for (wave, bound_db) in [
            (OscWave::Saw, ALIAS_BOUND_SAW_DB),
            (OscWave::Pulse, ALIAS_BOUND_PULSE_DB),
            (OscWave::Triangle, ALIAS_BOUND_TRIANGLE_DB),
        ] {
            for freq in [440.0_f32, 1_234.5, 3_000.0] {
                let mut previous = f32::MAX;
                for sr in RATES {
                    let mut osc = Osc::new();
                    let samples: Vec<f32> = (0..frames_for(0.5, sr))
                        .map(|_| osc.next_sample(freq, wave, 0.5, sr))
                        .collect();
                    let alias = alias_db(&samples, sr, freq, band);
                    println!("{wave:?} {freq} Hz at {sr} Hz: loudest alias {alias:.1} dB");
                    assert!(
                        alias < bound_db,
                        "{sr} Hz: {wave:?} at {freq} Hz aliases at {alias:.1} dB, over {bound_db}"
                    );
                    assert!(
                        alias <= previous + 1.0,
                        "{wave:?} at {freq} Hz aliases worse at {sr} Hz ({alias:.1} dB) than \
                         at the rate below ({previous:.1} dB)"
                    );
                    previous = alias;
                }
            }
        }
    }

    /// Worst non-harmonic component a PolyBLEP saw may have in 20 Hz-20 kHz
    /// at 440 Hz-3 kHz, relative to its fundamental.
    const ALIAS_BOUND_SAW_DB: f32 = -20.0;
    const ALIAS_BOUND_PULSE_DB: f32 = -20.0;
    const ALIAS_BOUND_TRIANGLE_DB: f32 = -20.0;

    /// A NaN or infinite frequency or pulse width must not reach the phase
    /// accumulator, where it would silence the oscillator for good (MOO-117).
    #[test]
    fn a_non_finite_parameter_never_poisons_an_oscillator() {
        for sr in RATES {
            for wave in OscWave::all() {
                for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                    let mut osc = Osc::new();
                    for _ in 0..64 {
                        assert!(osc.next_sample(bad, wave, bad, sr).is_finite());
                    }
                    for _ in 0..64 {
                        assert!(osc.next_sample(440.0, wave, 0.5, sr).is_finite());
                    }
                }
            }
        }
    }

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let mut a = Noise::new(42);
        let mut b = Noise::new(42);
        for _ in 0..1000 {
            let value = a.next_sample();
            assert!((-1.0..1.0).contains(&value));
            assert_eq!(value, b.next_sample());
        }
    }

    // --- MOO-264: the cheaper folds and table read, against the old ones --

    /// Run `f` with this thread's oscillators on the path before MOO-264.
    fn before<T>(f: impl FnOnce() -> T) -> T {
        BEFORE_MOO264.with(|flag| flag.set(true));
        let out = f();
        BEFORE_MOO264.with(|flag| flag.set(false));
        out
    }

    /// The one-step fraction and the near fold are the full ones, to the
    /// bit, everywhere: inside the ranges they shortcut and outside them.
    #[test]
    fn the_near_folds_are_the_full_ones_bit_for_bit() {
        let same = |a: f32, b: f32| a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan());
        let mut values = rounding_cases();
        for edge in [-1.0f32, 0.0, 1.0, 2.0] {
            values.extend([edge.next_down(), edge, edge.next_up(), -edge]);
        }
        for x in values {
            assert!(same(step_fract(x), fast_fract(x)), "step_fract({x:e})");
            assert!(same(wrap_near(x), wrap_unit(x)), "wrap_near({x:e})");
        }
    }

    /// A phase folded once reads the table as the phase folded twice did,
    /// with the old two loads and a subtract, to the bit: every table entry
    /// and between them, either side of zero and of a whole cycle, and far
    /// from either.
    #[test]
    fn a_folded_phase_reads_the_table_as_a_refolded_one_bit_for_bit() {
        let mut phases = rounding_cases();
        for entry in 0..=SINE_TABLE_LEN * 4 {
            let phase = entry as f32 / (SINE_TABLE_LEN * 4) as f32;
            phases.extend([phase, -phase, phase.next_up(), phase.next_down(), phase + 2.0, phase - 4.0]);
        }
        for phase in phases.into_iter().filter(|phase| phase.is_finite()) {
            let old = before(|| old_table_sine(phase));
            assert_eq!(table_sine_wrapped(wrap_near(phase)).to_bits(), old.to_bits(), "phase {phase:e}");
            assert_eq!(table_sine(phase).to_bits(), old.to_bits(), "phase {phase:e}");
        }
    }

    /// Every way an oscillator is driven, as a sample list: each wave at
    /// every rate, across the keyboard and past both clamps, with phase
    /// modulation from none to far past a cycle, pulse widths to both
    /// stops, the morph, and hard sync from a master at another pitch.
    fn drive_everything() -> Vec<f32> {
        let mut out = Vec::new();
        let waves = [OscWave::Sine, OscWave::Triangle, OscWave::Saw, OscWave::Pulse];
        let mut rng = Noise::new(0x264);
        for sr in RATES {
            for (w, &wave) in waves.iter().enumerate() {
                for freq in [0.0f32, 8.0, 55.0, 440.0, 3_520.0, 17_000.0, sr as f32] {
                    for depth in [0.0f32, 0.3, 1.7, 6.0] {
                        let mut osc = Osc::new();
                        let mut master = Osc::new();
                        let mut slave = Osc::new();
                        osc.reset_to(-1.0e-9);
                        for n in 0..600 {
                            let offset = depth * rng.next_sample();
                            let width = 0.5 + 0.5 * rng.next_sample();
                            let step = osc.next_step(freq, wave, width, offset, sr);
                            out.push(step.value);
                            out.push(step.wrap.unwrap_or(-1.0));
                            let morph = osc.next_step_morph(
                                freq * 0.5,
                                (wave, waves[(w + 1) % 4]),
                                (n % 7) as f32 / 6.0,
                                width,
                                offset,
                                sr,
                            );
                            out.push(morph.value);
                            let wrap = master.next_step(freq * 0.37 + 1.0, OscWave::Saw, 0.5, 0.0, sr);
                            let own = slave.next_step(freq, wave, width, offset, sr);
                            out.push(own.value);
                            if let Some(frac) = wrap.wrap {
                                out.push(slave.sync_reset(frac, freq, wave, width, offset, sr));
                            }
                            out.push(slave.phase());
                        }
                    }
                }
            }
        }
        out
    }

    /// An oscillator on the new folds and table read is the old one to the
    /// bit, however it is driven.
    #[test]
    fn every_oscillator_path_is_the_old_one_bit_for_bit() {
        let old = before(drive_everything);
        let new = drive_everything();
        assert_eq!(old.len(), new.len());
        for (index, (a, b)) in old.iter().zip(&new).enumerate() {
            assert_eq!(a.to_bits(), b.to_bits(), "sample {index}: {a} became {b}");
        }
    }

    use crate::bus::StereoBus;
    use crate::event::{Event, EventList, TimedEvent};
    use crate::node::{AudioNode, ProcessContext};

    /// Every device built on [`Osc`], with the patches that reach every
    /// wave, phase modulation and sync: ML-P8's bank at its own unison and
    /// at X8, ML-M1's bank, DS-01's bank and starter kit, and the v1 poly,
    /// mono and drum synths with each wave.
    fn oscillator_devices(sr: u32) -> Vec<(String, Box<dyn AudioNode>)> {
        use crate::drumsynth::DrumSynth;
        use crate::ds01::Ds01;
        use crate::mlm1::MlM1;
        use crate::mlp8::MlP8;
        use crate::monosynth::MonoSynth;
        use crate::polysynth::PolySynth;
        use mooloop_core::{DrumMode, DrumSynthParams, MlP8Unison, MonoSynthParams, PolySynthParams};
        let mut devices: Vec<(String, Box<dyn AudioNode>)> = Vec::new();
        for patch in mooloop_core::mlp8_factory::patches() {
            devices.push((format!("ML-P8 {}", patch.name), Box::new(MlP8::new(patch.params, sr))));
            let wide = mooloop_core::MlP8Params { unison: MlP8Unison::X8, ..patch.params };
            devices.push((format!("ML-P8 {} X8", patch.name), Box::new(MlP8::new(wide, sr))));
        }
        for patch in mooloop_core::mlm1_factory::patches() {
            devices.push((format!("ML-M1 {}", patch.name), Box::new(MlM1::new(patch.params, sr))));
        }
        for patch in mooloop_core::ds01_factory::patches() {
            devices.push((format!("DS-01 {}", patch.name), Box::new(Ds01::new(patch.params, sr))));
        }
        for (name, patch) in mooloop_core::ds01_factory::starter_kit() {
            devices.push((format!("DS-01 kit {name}"), Box::new(Ds01::new(patch.params, sr))));
        }
        for wave in [OscWave::Sine, OscWave::Triangle, OscWave::Saw, OscWave::Pulse] {
            let mut poly = PolySynthParams::default();
            let mut mono = MonoSynthParams::default();
            for (index, osc) in poly.osc.iter_mut().chain(mono.osc.iter_mut()).enumerate() {
                osc.wave = wave;
                osc.level = 0.5;
                osc.semitones = (index % 3) as f32 * 7.0;
                osc.pulse_width = 0.3;
            }
            devices.push((format!("PolySynth {wave:?}"), Box::new(PolySynth::new(poly, sr))));
            devices.push((format!("MonoSynth {wave:?}"), Box::new(MonoSynth::new(mono, sr))));
        }
        for mode in [DrumMode::Kick, DrumMode::Snare, DrumMode::Hat] {
            let params = DrumSynthParams { mode, ..DrumSynthParams::default() };
            devices.push((format!("DrumSynth {mode:?}"), Box::new(DrumSynth::new(params, sr))));
        }
        devices
    }

    /// Three notes struck, held for most of `frames` and released, in
    /// 128-frame blocks; both channels.
    fn render_device(device: &mut dyn AudioNode, sr: u32, frames: usize) -> Vec<f32> {
        let block = 128;
        let held = frames * 3 / 4;
        let notes = [36u8, 57, 64];
        let mut bus = StereoBus::with_capacity(block);
        let mut out = Vec::with_capacity(frames * 2);
        let mut rendered = 0;
        while rendered < frames {
            let len = block.min(frames - rendered);
            let mut events = EventList::empty();
            for (index, &note) in notes.iter().enumerate() {
                let id = index as u64 + 1;
                if rendered == 0 {
                    events.push(TimedEvent { offset: index as u32 * 5, event: Event::NoteOn { id, note, velocity: 100 } });
                }
                if rendered <= held && held < rendered + len {
                    events.push(TimedEvent { offset: (held - rendered) as u32, event: Event::NoteOff { id, note } });
                }
            }
            bus.l[..len].fill(0.0);
            bus.r[..len].fill(0.0);
            let ctx = ProcessContext {
                sample_rate: sr,
                frames: len,
                playing: true,
                bpm: 120.0,
                position_ticks: 0.0,
                position_frames: rendered as u64,
            };
            device.process(&ctx, &mut bus, &events, None);
            out.extend_from_slice(&bus.l[..len]);
            out.extend_from_slice(&bus.r[..len]);
            rendered += len;
        }
        out
    }

    /// Every device built on the oscillator sounds the same to the bit on
    /// the new folds and table read, at every rate. A change to a primitive
    /// changes every device on it, so each one is rendered, not argued.
    #[test]
    fn every_oscillator_device_is_the_old_one_bit_for_bit() {
        for sr in RATES {
            let frames = sr as usize / 3;
            let old: Vec<_> = before(|| {
                oscillator_devices(sr)
                    .into_iter()
                    .map(|(name, mut device)| (name, render_device(device.as_mut(), sr, frames)))
                    .collect()
            });
            let new = oscillator_devices(sr);
            assert_eq!(old.len(), new.len());
            for ((name, old), (_, mut device)) in old.into_iter().zip(new) {
                assert!(old.iter().any(|sample| *sample != 0.0), "{name} at {sr}: silence proves nothing");
                let now = render_device(device.as_mut(), sr, frames);
                let moved = old.iter().zip(&now).position(|(a, b)| a.to_bits() != b.to_bits());
                assert_eq!(moved, None, "{name} at {sr} Hz: MOO-264 moved a sample");
            }
        }
    }

    /// **What MOO-264's oscillator changes save**, in microseconds a
    /// 128-frame block at 48 kHz, old and new interleaved in one run so a
    /// shared machine's noise lands on both: every pass renders every
    /// (device, path) pair once, and each block's cost is its fastest over
    /// the passes. One note at Unison X8 for ML-P8, as `device_cost` plays
    /// it, held three quarters of two seconds. The test build carries the
    /// path switch, so these read a little above `device_cost`.
    ///
    /// ```sh
    /// REPS=15 cargo test -p mooloop-dsp --release oscillator_path_cost -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "measures wall time; run deliberately in release"]
    fn oscillator_path_cost() {
        use crate::mlm1::MlM1;
        use crate::mlp8::MlP8;
        use mooloop_core::{MlP8Params, MlP8Unison};
        use std::time::Instant;
        let sr = 48_000;
        let reps = std::env::var("REPS").ok().and_then(|reps| reps.parse().ok()).unwrap_or(7usize).max(1);
        let block = 128;
        let blocks = 2 * sr as usize / block;
        let held_blocks = blocks * 3 / 4;
        let make = |name: &str| -> Box<dyn AudioNode> {
            if let Some(patch) = mooloop_core::mlp8_factory::patches().into_iter().find(|p| p.name == name) {
                return Box::new(MlP8::new(MlP8Params { unison: MlP8Unison::X8, ..patch.params }, sr));
            }
            let patch = mooloop_core::mlm1_factory::patches().into_iter().find(|p| p.name == name).expect("a factory patch");
            Box::new(MlM1::new(patch.params, sr))
        };
        let names = ["Cold Metal", "Init Saw", "Servo Pad", "Furnace Stab", "Wide Machine", "Round Bass"];
        let mut fastest = vec![[vec![f64::MAX; blocks], vec![f64::MAX; blocks]]; names.len()];
        let mut bus = StereoBus::with_capacity(block);
        for _ in 0..reps {
            for (d, name) in names.iter().enumerate() {
                for (path, old) in [(0usize, true), (1, false)] {
                    BEFORE_MOO264.with(|flag| flag.set(old));
                    let mut device = make(name);
                    for (index, slot) in fastest[d][path].iter_mut().enumerate() {
                        let mut events = EventList::empty();
                        if index == 0 {
                            events.push(TimedEvent { offset: 0, event: Event::NoteOn { id: 1, note: 60, velocity: 100 } });
                        }
                        if index == held_blocks {
                            events.push(TimedEvent { offset: 0, event: Event::NoteOff { id: 1, note: 60 } });
                        }
                        bus.l[..block].fill(0.0);
                        bus.r[..block].fill(0.0);
                        let ctx = ProcessContext {
                            sample_rate: sr,
                            frames: block,
                            playing: true,
                            bpm: 120.0,
                            position_ticks: 0.0,
                            position_frames: (index * block) as u64,
                        };
                        let start = Instant::now();
                        device.process(&ctx, &mut bus, &events, None);
                        let spent = start.elapsed().as_secs_f64() * 1.0e6;
                        std::hint::black_box(&bus);
                        *slot = slot.min(spent);
                    }
                }
            }
        }
        BEFORE_MOO264.with(|flag| flag.set(false));
        println!("us per 128-frame block, fastest of {reps} passes, mean over {blocks} blocks (ML-P8 at Unison X8)");
        println!("{:<22} {:>9} {:>9} {:>7}", "device", "before", "now", "saved");
        for (d, name) in names.iter().enumerate() {
            let mean = |path: usize| fastest[d][path].iter().sum::<f64>() / blocks as f64;
            let (old, new) = (mean(0), mean(1));
            println!("{name:<22} {old:>9.1} {new:>9.1} {:>6.1}%", 100.0 * (old - new) / old);
        }
    }
}
