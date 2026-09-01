//! Oscillators and noise sources for the synth voices.
//!
//! Saw and pulse use PolyBLEP correction so they stay band-limited enough for
//! musical use without per-sample oversampling. Sine and triangle are naive
//! (they alias negligibly at these frequencies). Everything is allocation-free
//! state advanced one sample at a time.

use core::f32::consts::TAU;

use mooloop_core::OscWave;

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
        self.phase = advanced.fract();
        let boundary_dt = if core::mem::take(&mut self.after_sync) {
            0.0
        } else {
            dt
        };
        OscStep {
            value: wave_value(
                (phase + phase_offset).rem_euclid(1.0),
                wave,
                pulse_width,
                dt,
                boundary_dt,
            ),
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
        let at_reset = (self.last_phase + frac * dt).rem_euclid(1.0);
        // Measured on the *naive* waveform, which is what passing `dt = 0`
        // asks for: the PolyBLEP residuals already in `wave_value` correct the
        // oscillator's own wrap, and a reset that lands on one would otherwise
        // read a value that has been corrected once and correct it again.
        let naive = |phase: f32| wave_value(phase, wave, pulse_width, 0.0, 0.0);
        let before = naive((at_reset + phase_offset).rem_euclid(1.0));
        let after = naive(phase_offset.rem_euclid(1.0));
        // The remainder of the sample after the reset, so the slave's next
        // read sits where a continuous-time reset would have left it.
        self.phase = ((1.0 - frac) * dt).fract();
        self.last_phase = 0.0;
        self.after_sync = true;
        after - before
    }
}

/// Per-sample phase increment, with the frequency held inside the band the
/// oscillators are correct over.
fn increment(freq_hz: f32, sample_rate: u32) -> f32 {
    let sr = sample_rate as f32;
    freq_hz.clamp(0.01, sr * 0.45) / sr
}

/// The waveform at an absolute phase.
///
/// `dt` sizes the PolyBLEP residuals, so passing `0` reads the naive waveform
/// — which is what measuring a sync step height wants. `boundary_dt` sizes
/// only the residual at the *cycle boundary*, separately, so a sample that
/// follows a sync reset can decline that one correction while the pulse's
/// width edge keeps its own.
fn wave_value(phase: f32, wave: OscWave, pulse_width: f32, dt: f32, boundary_dt: f32) -> f32 {
    match wave {
        OscWave::Sine => (phase * TAU).sin(),
        OscWave::Triangle => 4.0 * (phase - 0.5).abs() - 1.0,
        OscWave::Saw => 2.0 * phase - 1.0 - polyblep(phase, boundary_dt),
        OscWave::Pulse => {
            let width = pulse_width.clamp(0.05, 0.95);
            let mut value = if phase < width { 1.0 } else { -1.0 };
            value += polyblep(phase, boundary_dt);
            value -= polyblep((phase - width).rem_euclid(1.0), dt);
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
}
