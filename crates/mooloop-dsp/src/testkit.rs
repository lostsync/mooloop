//! The shared measurement kit for DSP tests: frequency response, alias level,
//! THD, discontinuity, and the four sample rates every primitive is checked
//! at.
//!
//! Until 2026-09-22 each test module carried its own copy of these -- twenty
//! or so private `rms`, `peak`, `db`, `max_step`, `tone_energy` and
//! `response_db` helpers, each slightly different, and every one of them run
//! at 48 kHz only (`reports/teams-2026-09-22.md`, F10; MOO-117). A primitive
//! checked against its own private probe at one rate is checked against very
//! little: the SVF, the ladders and the cutoff mapping all behaved differently
//! at 96 kHz and nothing could have noticed.
//!
//! So measure through here, and measure at [`RATES`]. Every probe states what
//! it measures and how, and none of them is tuned to one device: a test that
//! needs a different measurement adds it here, where the next test can use it
//! too.
//!
//! Test support only. Nothing on the audio thread calls into this module, and
//! most of it allocates. It is public so that other crates' tests (the
//! engine's "control changes are continuous" family, MOO-104) can measure the
//! same way rather than growing a second copy.

use core::f64::consts::TAU;

/// The sample rates every primitive is checked at. 44.1 and 48 kHz are what
/// JACK usually runs; 96 and 192 kHz are where a coefficient written as if
/// the rate were fixed shows itself.
pub const RATES: [u32; 4] = [44_100, 48_000, 96_000, 192_000];

/// A ratio in decibels. Floored at -240 dB so silence reads as a very small
/// number rather than `-inf`, which an assertion can still compare.
pub fn db(ratio: f32) -> f32 {
    20.0 * ratio.max(1.0e-12).log10()
}

/// Decibels back to a linear ratio.
pub fn from_db(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// Frequency ratio in cents.
pub fn cents(measured_hz: f32, reference_hz: f32) -> f32 {
    1200.0 * (measured_hz / reference_hz).log2()
}

/// Root mean square, accumulated in `f64` so a long render does not lose its
/// quiet tail to rounding. An empty slice is silent.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| s as f64 * s as f64).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Sum of squares: the energy in a stretch of signal, for comparing two
/// stretches of the same length.
pub fn energy(samples: &[f32]) -> f32 {
    samples.iter().map(|&s| s as f64 * s as f64).sum::<f64>() as f32
}

/// The largest absolute sample.
///
/// **NaN-aware.** A NaN anywhere answers NaN, so a poisoned signal fails every
/// comparison instead of reading as silence -- which is what a fold over
/// `f32::max` does, since `max` drops NaN (`reports/teams-2026-09-22.md`, M4).
pub fn peak(samples: &[f32]) -> f32 {
    let mut peak = 0.0_f32;
    for &sample in samples {
        if sample.is_nan() {
            return f32::NAN;
        }
        peak = peak.max(sample.abs());
    }
    peak
}

/// The largest sample-to-sample step: the discontinuity measure. A click is a
/// step far larger than the signal's own slope, so this is what "control
/// changes are continuous" tests compare against the unedited signal.
/// NaN-aware like [`peak`].
pub fn max_step(samples: &[f32]) -> f32 {
    let mut worst = 0.0_f32;
    for pair in samples.windows(2) {
        let step = (pair[1] - pair[0]).abs();
        if step.is_nan() {
            return f32::NAN;
        }
        worst = worst.max(step);
    }
    worst
}

/// Peak over RMS, in dB. A sine reads 3.01 dB.
pub fn crest_db(samples: &[f32]) -> f32 {
    db(peak(samples) / rms(samples).max(1.0e-12))
}

/// Whether every sample is finite.
pub fn all_finite(samples: &[f32]) -> bool {
    samples.iter().all(|sample| sample.is_finite())
}

/// `seconds` of audio at `sample_rate`, in frames.
pub fn frames_for(seconds: f32, sample_rate: u32) -> usize {
    (seconds as f64 * sample_rate as f64).round() as usize
}

/// A sine of `amplitude` at `freq_hz`, starting at phase zero. Phase is
/// computed in `f64` from the frame index, so a long render carries no
/// accumulated phase error into a measurement of it.
pub fn sine(freq_hz: f32, amplitude: f32, sample_rate: u32, frames: usize) -> Vec<f32> {
    let step = TAU * freq_hz as f64 / sample_rate as f64;
    (0..frames)
        .map(|index| (amplitude as f64 * (step * index as f64).sin()) as f32)
        .collect()
}

/// Four-term Blackman-Harris: sidelobes at -92 dB, main lobe four bins either
/// side. Wide, but the kit measures alias components 60 to 90 dB down, and a
/// window whose sidelobes sit above them would measure the window.
fn blackman_harris(len: usize) -> Vec<f64> {
    const A: [f64; 4] = [0.358_75, 0.488_29, 0.141_28, 0.011_68];
    let denominator = (len.max(2) - 1) as f64;
    (0..len)
        .map(|index| {
            let x = TAU * index as f64 / denominator;
            A[0] - A[1] * x.cos() + A[2] * (2.0 * x).cos() - A[3] * (3.0 * x).cos()
        })
        .collect()
}

/// Half-width, in bins, of the region around a harmonic that [`alias_db`]
/// does not count: the window's main lobe plus a margin for scalloping.
const HARMONIC_GUARD_BINS: usize = 6;

/// The amplitude of the component at exactly `freq_hz`.
///
/// A Blackman-Harris-windowed DTFT evaluated at the frequency itself rather
/// than at the nearest FFT bin, so it neither scallops nor needs the render
/// to hold a whole number of cycles. Anything more than four bins
/// (`4 * sample_rate / len` Hz) away is suppressed by at least 92 dB. A sine
/// of amplitude `A` reads `A`.
pub fn tone_amplitude(samples: &[f32], sample_rate: u32, freq_hz: f32) -> f32 {
    let window = blackman_harris(samples.len());
    let step = TAU * freq_hz as f64 / sample_rate as f64;
    let (step_re, step_im) = (step.cos(), -step.sin());
    let (mut rot_re, mut rot_im) = (1.0_f64, 0.0_f64);
    let (mut re, mut im, mut weight) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (&sample, &w) in samples.iter().zip(&window) {
        let x = sample as f64 * w;
        re += x * rot_re;
        im += x * rot_im;
        weight += w;
        let next_re = rot_re * step_re - rot_im * step_im;
        rot_im = rot_re * step_im + rot_im * step_re;
        rot_re = next_re;
    }
    (2.0 * (re * re + im * im).sqrt() / weight.max(1.0e-300)) as f32
}

/// The amplitude of the component at `freq_hz` by unwindowed correlation.
///
/// Exact, with no leakage at all, **only** when the slice holds a whole
/// number of cycles of every component in it -- a render built that way on
/// purpose. Anywhere else use [`tone_amplitude`].
pub fn coherent_amplitude(samples: &[f32], sample_rate: u32, freq_hz: f32) -> f32 {
    let step = TAU * freq_hz as f64 / sample_rate as f64;
    let (mut re, mut im) = (0.0_f64, 0.0_f64);
    for (index, &sample) in samples.iter().enumerate() {
        let phase = step * index as f64;
        re += sample as f64 * phase.cos();
        im += sample as f64 * phase.sin();
    }
    (2.0 * (re * re + im * im).sqrt() / samples.len().max(1) as f64) as f32
}

/// In-place iterative radix-2 FFT. `buffer.len()` must be a power of two.
fn fft(buffer: &mut [(f64, f64)]) {
    let len = buffer.len();
    debug_assert!(len.is_power_of_two());
    let mut j = 0usize;
    for i in 1..len {
        let mut bit = len >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            buffer.swap(i, j);
        }
    }
    let mut span = 2;
    while span <= len {
        let half = span / 2;
        for start in (0..len).step_by(span) {
            for k in 0..half {
                // Twiddles from `sin_cos` each time rather than a rotating
                // product: a few more transcendental calls, and no error that
                // grows across a 64k-point transform.
                let (sin, cos) = (-TAU * k as f64 / span as f64).sin_cos();
                let (ar, ai) = buffer[start + k];
                let (br, bi) = buffer[start + k + half];
                let (tr, ti) = (br * cos - bi * sin, br * sin + bi * cos);
                buffer[start + k] = (ar + tr, ai + ti);
                buffer[start + k + half] = (ar - tr, ai - ti);
            }
        }
        span <<= 1;
    }
}

/// An amplitude spectrum of the **last** power-of-two stretch of `samples`,
/// Blackman-Harris windowed and scaled so that a sine of amplitude `A` on a
/// bin centre reads `A` (off-centre it reads up to 0.83 dB low, the window's
/// scalloping). Bin `k` is centred on [`bin_hz`]`(k, len, sample_rate)`.
///
/// The last stretch rather than the first, because tests render a transient
/// first and measure what it settles to.
pub fn spectrum(samples: &[f32]) -> Vec<f32> {
    let len = fft_len(samples.len());
    let tail = &samples[samples.len() - len..];
    let window = blackman_harris(len);
    let weight: f64 = window.iter().sum();
    let mut buffer: Vec<(f64, f64)> = tail
        .iter()
        .zip(&window)
        .map(|(&sample, &w)| (sample as f64 * w, 0.0))
        .collect();
    fft(&mut buffer);
    buffer[..len / 2]
        .iter()
        .map(|&(re, im)| (2.0 * (re * re + im * im).sqrt() / weight) as f32)
        .collect()
}

/// The transform length [`spectrum`] uses for a slice of `available` samples:
/// the largest power of two that fits.
pub fn fft_len(available: usize) -> usize {
    assert!(available >= 2, "a spectrum needs at least two samples");
    1 << (usize::BITS - 1 - available.leading_zeros())
}

/// The centre frequency of spectrum bin `bin` for a transform of `len`.
pub fn bin_hz(bin: usize, len: usize, sample_rate: u32) -> f32 {
    (bin as f64 * sample_rate as f64 / len as f64) as f32
}

/// The RMS level of everything in `band_hz`, from the spectrum of the last
/// power-of-two stretch of `samples`. A band holding one sine of amplitude
/// `A` reads `A / sqrt(2)`; noise reads its RMS within the band. The measure
/// for a spectral tilt, where a single frequency of a noise signal would read
/// whatever that bin happened to hold.
pub fn band_rms(samples: &[f32], sample_rate: u32, band_hz: (f32, f32)) -> f32 {
    let len = fft_len(samples.len());
    let bins = spectrum(samples);
    let window = blackman_harris(len);
    let sum: f64 = window.iter().sum();
    let sum_sq: f64 = window.iter().map(|w| w * w).sum();
    // The window's equivalent noise bandwidth, in bins: how many bins one
    // sine's power is smeared across in `spectrum`'s amplitude scaling.
    let enbw = len as f64 * sum_sq / (sum * sum);
    let power: f64 = bins
        .iter()
        .enumerate()
        .filter(|(bin, _)| {
            let hz = bin_hz(*bin, len, sample_rate);
            hz >= band_hz.0 && hz <= band_hz.1
        })
        .map(|(_, &amplitude)| amplitude as f64 * amplitude as f64)
        .sum();
    (power / (2.0 * enbw)).sqrt() as f32
}

/// The loudest component in `band_hz` that is **not** a harmonic of
/// `fundamental_hz`, in dB relative to the fundamental.
///
/// This is the alias measure: a band-limited periodic signal has energy only
/// at multiples of its fundamental, so anything else is either aliasing (a
/// harmonic above Nyquist folded back) or noise. Components within
/// [`HARMONIC_GUARD_BINS`] of a harmonic are not counted, and neither is the
/// region around DC, which the window's own leakage owns. The floor of the
/// measurement is the window's -92 dB sidelobes.
///
/// Measures the last power-of-two stretch of `samples` (see [`spectrum`]), so
/// pass at least a quarter of a second of settled signal: resolution is
/// `sample_rate / len` Hz and the guard is six of those.
pub fn alias_db(samples: &[f32], sample_rate: u32, fundamental_hz: f32, band_hz: (f32, f32)) -> f32 {
    let len = fft_len(samples.len());
    let bins = spectrum(samples);
    let tail = &samples[samples.len() - len..];
    let fundamental = tone_amplitude(tail, sample_rate, fundamental_hz);
    let bin_width = sample_rate as f32 / len as f32;
    let guard_hz = HARMONIC_GUARD_BINS as f32 * bin_width;
    let mut loudest = 0.0_f32;
    for (bin, &amplitude) in bins.iter().enumerate().skip(HARMONIC_GUARD_BINS) {
        let hz = bin_hz(bin, len, sample_rate);
        if hz < band_hz.0 || hz > band_hz.1 {
            continue;
        }
        let nearest_harmonic = (hz / fundamental_hz).round().max(1.0) * fundamental_hz;
        if (hz - nearest_harmonic).abs() <= guard_hz {
            continue;
        }
        loudest = loudest.max(amplitude);
    }
    db(loudest / fundamental.max(1.0e-12))
}

/// Total harmonic distortion: the RMS sum of harmonics 2 and up (every one
/// below Nyquist), over the fundamental. A ratio; pass it to [`db`] for dB.
pub fn thd(samples: &[f32], sample_rate: u32, fundamental_hz: f32) -> f32 {
    let fundamental = tone_amplitude(samples, sample_rate, fundamental_hz);
    let nyquist = sample_rate as f32 * 0.5;
    let mut sum = 0.0_f64;
    let mut harmonic = 2.0_f32;
    while harmonic * fundamental_hz < nyquist {
        let amplitude = tone_amplitude(samples, sample_rate, harmonic * fundamental_hz) as f64;
        sum += amplitude * amplitude;
        harmonic += 1.0;
    }
    (sum.sqrt() / fundamental.max(1.0e-12) as f64) as f32
}

/// The frequency of the strongest spectral peak above `min_hz`, refined
/// between bins by a parabola through the log magnitudes. Taken over from
/// `spikes/time-stretch/src/metrics.rs`'s probe of the same name.
pub fn dominant_hz(samples: &[f32], sample_rate: u32, min_hz: f32) -> f32 {
    let len = fft_len(samples.len());
    let bins = spectrum(samples);
    let first = ((min_hz as f64 * len as f64 / sample_rate as f64).ceil() as usize).max(1);
    let mut best = first;
    for bin in first..bins.len() - 1 {
        if bins[bin] > bins[best] {
            best = bin;
        }
    }
    let best = best.clamp(1, bins.len() - 2);
    let (a, b, c) = (
        (bins[best - 1].max(1.0e-12) as f64).ln(),
        (bins[best].max(1.0e-12) as f64).ln(),
        (bins[best + 1].max(1.0e-12) as f64).ln(),
    );
    let denominator = a - 2.0 * b + c;
    let offset = if denominator.abs() > 1.0e-12 {
        0.5 * (a - c) / denominator
    } else {
        0.0
    };
    ((best as f64 + offset) * sample_rate as f64 / len as f64) as f32
}

/// How a sine is put through a device for a steady-state measurement.
///
/// A probe renders `settle_s` of the sine and throws it away -- the
/// transient, and any follower or envelope finding its level -- then renders
/// `measure_s` more and measures that. Both are in seconds, so a probe means
/// the same thing at every rate in [`RATES`]. The defaults (a quarter second
/// each) settle anything that rings for less than about 50 ms; a filter at
/// high resonance and a low corner rings for longer, so give it
/// [`Probe::settle`].
#[derive(Clone, Copy, Debug)]
pub struct Probe {
    pub sample_rate: u32,
    pub amplitude: f32,
    pub settle_s: f32,
    pub measure_s: f32,
}

impl Probe {
    /// A probe at `sample_rate` with a small-signal amplitude (-34 dBFS).
    /// Small because the slope and corner of a nonlinear filter are
    /// small-signal properties: at full scale this would measure the `tanh`
    /// in a ladder's feedback path instead.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            amplitude: 0.02,
            settle_s: 0.25,
            measure_s: 0.25,
        }
    }

    pub fn amplitude(mut self, amplitude: f32) -> Self {
        self.amplitude = amplitude;
        self
    }

    pub fn settle(mut self, settle_s: f32) -> Self {
        self.settle_s = settle_s;
        self
    }

    pub fn measure(mut self, measure_s: f32) -> Self {
        self.measure_s = measure_s;
        self
    }

    /// Put the sine through `process` and return the measured stretch of its
    /// output.
    pub fn run(&self, mut process: impl FnMut(f32) -> f32, freq_hz: f32) -> Vec<f32> {
        let settle = frames_for(self.settle_s, self.sample_rate);
        let total = settle + frames_for(self.measure_s, self.sample_rate);
        let step = TAU * freq_hz as f64 / self.sample_rate as f64;
        let mut out = Vec::with_capacity(total - settle);
        for index in 0..total {
            let input = (self.amplitude as f64 * (step * index as f64).sin()) as f32;
            let output = process(input);
            if index >= settle {
                out.push(output);
            }
        }
        out
    }

    /// Steady-state gain at `freq_hz`, in dB: the amplitude of the output's
    /// component at the input frequency over the input amplitude. This is the
    /// frequency response proper -- harmonics a nonlinearity adds are not
    /// counted, so it is comparable with a transfer function.
    pub fn gain_db(&self, process: impl FnMut(f32) -> f32, freq_hz: f32) -> f32 {
        db(self.gain(process, freq_hz))
    }

    /// [`Self::gain_db`] as a linear ratio.
    pub fn gain(&self, process: impl FnMut(f32) -> f32, freq_hz: f32) -> f32 {
        let out = self.run(process, freq_hz);
        tone_amplitude(&out, self.sample_rate, freq_hz) / self.amplitude
    }

    /// Steady-state output peak over the input amplitude, in dB: the level a
    /// meter would see, harmonics and all. The measure for a gain bound.
    pub fn peak_gain_db(&self, process: impl FnMut(f32) -> f32, freq_hz: f32) -> f32 {
        let out = self.run(process, freq_hz);
        db(peak(&out) / self.amplitude)
    }
}

/// Where a falling response crosses `level_db`, found by bisection in log
/// frequency between `low_hz` (where `gain_at` must be above the level) and
/// `high_hz` (where it must be below). `gain_at` is called about twenty
/// times, each with a fresh device.
///
/// For a low-pass corner, pass the passband gain minus 3 dB as the level.
pub fn crossing_hz(
    mut gain_at: impl FnMut(f32) -> f32,
    level_db: f32,
    low_hz: f32,
    high_hz: f32,
) -> f32 {
    let (mut low, mut high) = (low_hz.ln(), high_hz.ln());
    assert!(
        gain_at(low_hz) > level_db && gain_at(high_hz) < level_db,
        "the response does not cross {level_db} dB between {low_hz} and {high_hz} Hz"
    );
    for _ in 0..20 {
        let middle = 0.5 * (low + high);
        if gain_at(middle.exp()) > level_db {
            low = middle;
        } else {
            high = middle;
        }
    }
    (0.5 * (low + high)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kit's own claims, checked against signals whose answer is known,
    /// at every rate -- a probe that is wrong at 192 kHz would pass every
    /// device test built on it there.
    #[test]
    fn a_known_sine_measures_as_itself_at_every_rate() {
        for sr in RATES {
            for (freq, amplitude) in [(50.0_f32, 1.0_f32), (997.0, 0.25), (15_000.0, 0.5)] {
                let signal = sine(freq, amplitude, sr, frames_for(0.25, sr));
                let measured = tone_amplitude(&signal, sr, freq);
                assert!(
                    (db(measured / amplitude)).abs() < 0.01,
                    "{sr} Hz: a {freq} Hz sine of {amplitude} measured {measured}"
                );
                assert!(
                    (rms(&signal) - amplitude / 2.0_f32.sqrt()).abs() < amplitude * 1.0e-3,
                    "{sr} Hz: rms of a {freq} Hz sine"
                );
                assert!((crest_db(&signal) - 3.01).abs() < 0.05);
                let found = dominant_hz(&signal, sr, 20.0);
                assert!(
                    cents(found, freq).abs() < 5.0,
                    "{sr} Hz: dominant {found} Hz for a {freq} Hz sine"
                );
            }
        }
    }

    /// The floor of `alias_db` is the window's -92 dB sidelobes, not `f32`:
    /// a pure sine has to read at or under it.
    #[test]
    fn a_pure_sine_has_no_distortion_and_no_aliases() {
        for sr in RATES {
            let signal = sine(1_234.5, 0.5, sr, frames_for(0.5, sr));
            let thd = db(thd(&signal, sr, 1_234.5));
            assert!(thd < -100.0, "{sr} Hz: THD {thd} dB");
            let alias = alias_db(&signal, sr, 1_234.5, (20.0, 20_000.0));
            assert!(alias < -90.0, "{sr} Hz: alias floor {alias} dB");
        }
    }

    /// A tone that is not a harmonic is reported at its level, which is what
    /// makes `alias_db` an alias measure rather than a noise floor.
    #[test]
    fn a_stray_tone_is_found_at_its_level() {
        for sr in RATES {
            let frames = frames_for(0.5, sr);
            let fundamental = sine(1_000.0, 0.5, sr, frames);
            let stray = sine(3_210.0, 0.5e-3, sr, frames);
            let mixed: Vec<f32> = fundamental.iter().zip(&stray).map(|(a, b)| a + b).collect();
            let measured = alias_db(&mixed, sr, 1_000.0, (20.0, 20_000.0));
            // -60 dB, less at most the window's 0.83 dB scalloping.
            assert!(
                (-61.0..=-59.9).contains(&measured),
                "{sr} Hz: a -60 dB stray tone measured {measured} dB"
            );
        }
    }

    /// A band around one sine reads the sine's RMS; a band away from it reads
    /// nearly nothing.
    #[test]
    fn a_band_reads_the_rms_of_what_is_in_it() {
        for sr in RATES {
            let signal = sine(1_000.0, 0.5, sr, frames_for(0.5, sr));
            let inside = band_rms(&signal, sr, (900.0, 1_100.0));
            assert!(
                (db(inside / (0.5 / 2.0_f32.sqrt()))).abs() < 0.05,
                "{sr} Hz: band around a sine of 0.5 read {inside}"
            );
            assert!(band_rms(&signal, sr, (2_000.0, 4_000.0)) < 1.0e-4);
        }
    }

    /// A 3rd harmonic at a tenth of the fundamental is 10% THD.
    #[test]
    fn thd_counts_the_harmonics() {
        for sr in RATES {
            let frames = frames_for(0.25, sr);
            let signal: Vec<f32> = sine(500.0, 1.0, sr, frames)
                .iter()
                .zip(sine(1_500.0, 0.1, sr, frames))
                .map(|(a, b)| a + b)
                .collect();
            assert!((thd(&signal, sr, 500.0) - 0.1).abs() < 1.0e-3);
        }
    }

    #[test]
    fn a_step_is_the_largest_step_and_nan_is_never_silence() {
        assert_eq!(max_step(&[0.0, 0.1, 0.2, 0.9, 0.95]), 0.7);
        assert!(peak(&[0.0, f32::NAN, 0.5]).is_nan());
        assert!(max_step(&[0.0, f32::NAN, 0.5]).is_nan());
        assert!(!all_finite(&[0.0, f32::INFINITY]));
    }

    /// The probe measures a known transfer: a plain gain of one half is
    /// -6.02 dB at every frequency and rate, and a one-sample delay is 0 dB.
    #[test]
    fn a_probe_measures_a_known_gain() {
        for sr in RATES {
            let probe = Probe::new(sr);
            for freq in [30.0_f32, 1_000.0, 18_000.0] {
                assert!((probe.gain_db(|x| x * 0.5, freq) + 6.0206).abs() < 0.01);
                let mut last = 0.0_f32;
                let delayed = probe.gain_db(
                    |x| {
                        let out = last;
                        last = x;
                        out
                    },
                    freq,
                );
                assert!(delayed.abs() < 0.01, "{sr} Hz, {freq} Hz: {delayed}");
            }
        }
    }

    /// Bisection finds a one-pole's corner where the math puts it.
    #[test]
    fn crossing_finds_a_known_corner() {
        let corner = 1_000.0_f32;
        // |H|^2 = 1 / (1 + (f/fc)^2), analytically; -3.0103 dB at fc.
        let found = crossing_hz(
            |f| -10.0 * (1.0 + (f / corner).powi(2)).log10(),
            -3.0103,
            20.0,
            20_000.0,
        );
        assert!(cents(found, corner).abs() < 1.0, "{found} Hz");
    }
}
