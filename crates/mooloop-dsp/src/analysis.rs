//! Allocation-free display analysis shared by device hosts.
//!
//! This intentionally produces compact, log-frequency level vectors rather
//! than handing PCM to the UI. The audio thread owns the rolling window; a
//! device host publishes its latest semantic display data through the engine.
//!
//! # Two things about the band grid that are easy to miss
//!
//! **The grid moves with the sample rate.** Bands span 20 Hz to
//! `0.45 * sample_rate`, so at 96 kHz the display reaches 43 kHz and spends
//! about seven of its forty-eight bands above hearing. The bands are not the
//! same frequencies at two rates, which is why
//! `every_band_captures_the_cycles_it_asked_for` asks about cycles rather
//! than samples.
//!
//! **Its resolution is not the window's.** The bands are log-spaced and the
//! window is not, so one window length is finer than the spacing at the top
//! and coarser at the bottom. [`SPECTRUM_PERIODS`] is how that is answered.

use crate::bus::StereoBus;
use crate::scale::hz_from_normalized;

/// Number of logarithmically spaced spectrum bands intended for a compact UI.
pub const SPECTRUM_BINS: usize = 48;

/// The longest window any band uses, and so the length of the ring.
///
/// 42.7 ms at 48 kHz. See [`SPECTRUM_PERIODS`] for why the low bands want it
/// and why this is where the wanting stops.
const SPECTRUM_WINDOW: usize = 2_048;

/// The shortest window any band uses.
///
/// Above about 750 Hz a band is already wider than the resolution this buys,
/// so a longer window would cost samples and tell nobody anything -- and a
/// short window is what keeps the top of the display quick.
const SPECTRUM_MIN_WINDOW: usize = 512;

/// Cycles of a band's *own* frequency wanted inside its window.
///
/// **The window is per band, not per analyzer, and that is the whole point.**
/// The bands are log-spaced at about 16% each, so the gap between two of them
/// at 100 Hz is 16 Hz while the gap at 10 kHz is 1.6 kHz -- but one window
/// length resolves the same number of hertz everywhere. At the 512 samples
/// this used to run, that was about 94 Hz, which is finer than the spacing
/// above 600 Hz and coarser than it below: **sixteen of the forty-eight
/// bands were showing the same smear.**
///
/// A fixed number of cycles asks each band for the resolution its own
/// spacing needs, and falls out of the sample rate rather than being a table
/// to re-pick at 96 kHz. Eight is enough for a stable Goertzel estimate and
/// puts the 2048-sample ceiling at 375 Hz.
///
/// This costs what it sounds like it costs -- about 2.4x the multiply-
/// accumulates of a flat 512 -- and it is affordable for the reason the
/// analyzer was always affordable: it runs once a hop, and only for a device
/// whose display is subscribed.
const SPECTRUM_PERIODS: f32 = 8.0;

const SPECTRUM_HOP: usize = 2_048;
/// Level a band reads as zero. Public because a consumer that *compares* two
/// spectra has to undo the normalization to get back to decibels, and
/// spelling -84 a second time to do it is how the two would drift apart.
pub const SPECTRUM_FLOOR_DB: f32 = -84.0;

/// Rolling, low-rate spectrum analyzer. It is deliberately inexpensive:
/// values are calculated once per hop, with a fixed Goertzel bank over a
/// mono sum, and only when a device display subscribes to it.
///
/// **Each band reads its own depth into the ring.** That is free here in a
/// way it would not be under an FFT: a Goertzel bin is an independent pass
/// over whatever samples it is given, so multi-resolution analysis is a
/// per-band loop bound rather than three transforms and a splice. The
/// coefficient does not depend on the window length either, so only the
/// summation and its normalization vary.
pub struct SpectrumAnalyzer {
    samples: [f32; SPECTRUM_WINDOW],
    coefficients: [f32; SPECTRUM_BINS],
    /// Each band's own window length, in samples, ending at the newest one.
    windows: [usize; SPECTRUM_BINS],
    write: usize,
    filled: usize,
    since_publish: usize,
    sample_rate: u32,
}

impl SpectrumAnalyzer {
    pub const fn new() -> Self {
        Self {
            samples: [0.0; SPECTRUM_WINDOW],
            coefficients: [0.0; SPECTRUM_BINS],
            windows: [SPECTRUM_MIN_WINDOW; SPECTRUM_BINS],
            write: 0,
            filled: 0,
            since_publish: SPECTRUM_HOP,
            sample_rate: 0,
        }
    }

    pub fn reset(&mut self) {
        self.samples.fill(0.0);
        self.write = 0;
        self.filled = 0;
        self.since_publish = SPECTRUM_HOP;
    }

    fn configure(&mut self, sample_rate: u32) {
        if self.sample_rate == sample_rate {
            return;
        }
        self.sample_rate = sample_rate;
        self.reset();
        let max_frequency = sample_rate as f32 * 0.45;
        for index in 0..SPECTRUM_BINS {
            let position = index as f32 / (SPECTRUM_BINS - 1) as f32;
            let frequency = hz_from_normalized(position, max_frequency);
            self.coefficients[index] =
                2.0 * (core::f32::consts::TAU * frequency / sample_rate as f32).cos();
            // Rounded up to a power of two so the ring's wrap stays a mask
            // and the tiers are few enough to reason about.
            let wanted = SPECTRUM_PERIODS * sample_rate as f32 / frequency.max(1.0);
            self.windows[index] = (wanted.ceil() as usize)
                .next_power_of_two()
                .clamp(SPECTRUM_MIN_WINDOW, SPECTRUM_WINDOW);
        }
    }

    /// Ingest one device input block. Returns a fresh normalized display vector
    /// at most once every [`SPECTRUM_HOP`] samples after the window fills.
    pub fn push(
        &mut self,
        sample_rate: u32,
        bus: &StereoBus,
        frames: usize,
    ) -> Option<[f32; SPECTRUM_BINS]> {
        self.prepare(sample_rate);
        let frames = frames.min(bus.capacity());
        for frame in 0..frames {
            self.write((bus.l[frame] + bus.r[frame]) * 0.5);
        }
        self.take()
    }

    /// Ready the bank for a block at `sample_rate`. Call before [`Self::write`].
    pub fn prepare(&mut self, sample_rate: u32) {
        self.configure(sample_rate);
    }

    /// Ingest one mono sample.
    ///
    /// The sample-at-a-time half of [`Self::push`], for a device that cannot
    /// hand over a bus because it has already overwritten it. The preamp is
    /// the case: it reads dry and writes wet into the same buffer, so its dry
    /// signal exists only inside the sample loop.
    #[inline]
    pub fn write(&mut self, sample: f32) {
        self.samples[self.write] = sample;
        self.write = (self.write + 1) % SPECTRUM_WINDOW;
        self.filled = (self.filled + 1).min(SPECTRUM_WINDOW);
        self.since_publish = self.since_publish.saturating_add(1);
    }

    /// A fresh display vector, if the window is full and a hop has passed.
    pub fn take(&mut self) -> Option<[f32; SPECTRUM_BINS]> {
        if self.filled < SPECTRUM_WINDOW || self.since_publish < SPECTRUM_HOP {
            return None;
        }
        self.since_publish = 0;
        Some(self.analyze())
    }

    fn analyze(&self) -> [f32; SPECTRUM_BINS] {
        let mut levels = [0.0; SPECTRUM_BINS];
        for (bin, level) in levels.iter_mut().enumerate() {
            let coefficient = self.coefficients[bin];
            let window = self.windows[bin];
            // `write` points at the oldest sample, so a window that ends at
            // the newest one starts `window` back from there.
            let start = (self.write + SPECTRUM_WINDOW - window) % SPECTRUM_WINDOW;
            let mut q1 = 0.0;
            let mut q2 = 0.0;
            for offset in 0..window {
                let sample = self.samples[(start + offset) % SPECTRUM_WINDOW];
                let q0 = sample + coefficient * q1 - q2;
                q2 = q1;
                q1 = q0;
            }
            let power =
                (q1 * q1 + q2 * q2 - coefficient * q1 * q2) / (window * window) as f32;
            let db = 10.0 * power.max(1e-12).log10();
            *level = ((db - SPECTRUM_FLOOR_DB) / -SPECTRUM_FLOOR_DB).clamp(0.0, 1.0);
        }
        levels
    }
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_raises_its_nearest_log_band() {
        let sample_rate = 48_000;
        let mut analyzer = SpectrumAnalyzer::new();
        let mut bus = StereoBus::with_capacity(256);
        let mut spectrum = None;
        for block in 0..16 {
            for frame in 0..256 {
                let t = (block * 256 + frame) as f32 / sample_rate as f32;
                let sample = (core::f32::consts::TAU * 1_000.0 * t).sin() * 0.5;
                bus.l[frame] = sample;
                bus.r[frame] = sample;
            }
            spectrum = analyzer.push(sample_rate, &bus, 256).or(spectrum);
        }
        let spectrum = spectrum.expect("analyzer should publish after a hop");
        let nearest = (0..SPECTRUM_BINS)
            .min_by(|a, b| {
                let frequency = |index: usize| {
                    20.0_f32 * (21_600.0_f32 / 20.0_f32).powf(index as f32 / 47.0_f32)
                };
                (frequency(*a) - 1_000.0)
                    .abs()
                    .total_cmp(&(frequency(*b) - 1_000.0).abs())
            })
            .unwrap();
        assert!(
            spectrum[nearest] > 0.45,
            "1 kHz level: {}",
            spectrum[nearest]
        );
    }

    /// The window rule, as the numbers it produces at 48 kHz.
    ///
    /// Pinned rather than described, because the whole change is the claim
    /// that low bands get a longer look and high ones do not.
    #[test]
    fn each_band_asks_for_as_many_cycles_of_its_own_frequency() {
        let mut analyzer = SpectrumAnalyzer::new();
        analyzer.configure(48_000);

        let frequency = |index: usize| {
            20.0_f32 * (21_600.0_f32 / 20.0_f32).powf(index as f32 / 47.0_f32)
        };
        let mut previous = usize::MAX;
        for bin in 0..SPECTRUM_BINS {
            let window = analyzer.windows[bin];
            assert!(
                (SPECTRUM_MIN_WINDOW..=SPECTRUM_WINDOW).contains(&window),
                "band {bin} wants {window} samples, outside the ring"
            );
            assert!(window.is_power_of_two(), "band {bin} window {window}");
            assert!(
                window <= previous,
                "band {bin} at {:.0} Hz wants a longer window than the band below it",
                frequency(bin)
            );
            previous = window;
        }

        // The tiers the constant implies: 8 cycles at 48 kHz reaches the
        // 2048 ceiling below 375 Hz and the 512 floor above 750.
        assert_eq!(analyzer.windows[0], SPECTRUM_WINDOW, "20 Hz");
        assert_eq!(
            analyzer.windows[SPECTRUM_BINS - 1],
            SPECTRUM_MIN_WINDOW,
            "the top band"
        );
        for bin in 0..SPECTRUM_BINS {
            let hz = frequency(bin);
            let expected = if hz < 375.0 {
                SPECTRUM_WINDOW
            } else if hz < 750.0 {
                1_024
            } else {
                SPECTRUM_MIN_WINDOW
            };
            assert_eq!(analyzer.windows[bin], expected, "{hz:.0} Hz");
        }
    }

    /// The rule follows the sample rate rather than being a table of hertz.
    ///
    /// What is constant is the **cycles** a band captures, not its sample
    /// count, and the first version of this test asserted the wrong one. The
    /// band grid is itself rate-dependent -- `hz_from_normalized` spans up to
    /// `0.45 * sample_rate`, so band 23 sits at 1.4x the frequency at 96 kHz
    /// that it does at 48 -- and that cancels part of the doubling. Rounding
    /// up to a power of two puts the captured cycles in `[8, 16)` for any
    /// band the ring does not clamp, at any rate.
    #[test]
    fn every_band_captures_the_cycles_it_asked_for() {
        for sample_rate in [44_100, 48_000, 96_000] {
            let mut analyzer = SpectrumAnalyzer::new();
            analyzer.configure(sample_rate);
            let max_frequency = sample_rate as f32 * 0.45;
            for bin in 0..SPECTRUM_BINS {
                let window = analyzer.windows[bin];
                // A clamped band gets what the ring can give, which is the
                // ceiling's whole job; only ask about the ones that are free.
                if window == SPECTRUM_WINDOW || window == SPECTRUM_MIN_WINDOW {
                    continue;
                }
                let position = bin as f32 / (SPECTRUM_BINS - 1) as f32;
                let frequency = hz_from_normalized(position, max_frequency);
                let cycles = window as f32 * frequency / sample_rate as f32;
                assert!(
                    (SPECTRUM_PERIODS..SPECTRUM_PERIODS * 2.0).contains(&cycles),
                    "{sample_rate} Hz band {bin} at {frequency:.0} Hz captures \
                     {cycles:.2} cycles in {window} samples"
                );
            }
        }
    }

    /// A higher rate never shortens a band's window.
    ///
    /// The grid moves up with the rate, which claws back some of the extra
    /// samples a shorter sample period costs -- exactly at the top band,
    /// where the two cancel and the window is unchanged -- but it can never
    /// overtake it.
    #[test]
    fn a_higher_rate_never_shortens_a_window() {
        let mut slow = SpectrumAnalyzer::new();
        slow.configure(48_000);
        let mut fast = SpectrumAnalyzer::new();
        fast.configure(96_000);
        for bin in 0..SPECTRUM_BINS {
            assert!(
                fast.windows[bin] >= slow.windows[bin],
                "band {bin}: {} at 48 kHz, {} at 96",
                slow.windows[bin],
                fast.windows[bin]
            );
        }
    }

    /// What the longer window is *for*: a tone in the bottom two octaves
    /// lands on its own band instead of smearing across its neighbours.
    ///
    /// 60 Hz is where this used to fail. A 512-sample window resolves about
    /// 94 Hz, which is wider than 60 Hz is from zero, so the tone raised the
    /// whole bottom of the display rather than a place in it.
    #[test]
    fn a_low_tone_lands_on_a_band_instead_of_smearing() {
        let sample_rate = 48_000;
        let mut analyzer = SpectrumAnalyzer::new();
        let mut bus = StereoBus::with_capacity(256);
        let mut spectrum = None;
        for block in 0..32 {
            for frame in 0..256 {
                let t = (block * 256 + frame) as f32 / sample_rate as f32;
                let sample = (core::f32::consts::TAU * 60.0 * t).sin() * 0.5;
                bus.l[frame] = sample;
                bus.r[frame] = sample;
            }
            spectrum = analyzer.push(sample_rate, &bus, 256).or(spectrum);
        }
        let spectrum = spectrum.expect("analyzer should publish after a hop");

        let frequency = |index: usize| {
            20.0_f32 * (21_600.0_f32 / 20.0_f32).powf(index as f32 / 47.0_f32)
        };
        let peak = (0..SPECTRUM_BINS)
            .max_by(|a, b| spectrum[*a].total_cmp(&spectrum[*b]))
            .unwrap();
        assert!(
            (frequency(peak) - 60.0).abs() < 12.0,
            "a 60 Hz tone peaked at band {peak}, {:.0} Hz",
            frequency(peak)
        );

        // And it is a peak rather than a plateau: the top of the audible
        // range, which shares nothing with a 60 Hz sine, stays on the floor.
        let top = spectrum[SPECTRUM_BINS - 1];
        assert!(
            top < spectrum[peak] * 0.5,
            "the top band read {top:.3} against the peak's {:.3}",
            spectrum[peak]
        );
    }
}
