#![allow(clippy::needless_range_loop)]
// Spectral code indexes several parallel arrays by bin number at once. The
// iterator rewrite clippy suggests needs a zip chain per loop and reads worse
// than `for k in 0..bins`, so the lint is off for this throwaway crate.

//! Time-stretch spike harness for issue #32.
//!
//! Throwaway comparison code. Nothing here is meant to ship; the point is to
//! produce numbers and renders that justify the algorithm choice for #13.
//!
//! The candidates share one interface deliberately shaped like the sampler's
//! real situation: an immutable, fully-resident `SampleData`-like buffer, a
//! region, an optional loop, and a pull-style `render` that fills whatever
//! block the executor asked for. That is what a sampler voice actually needs,
//! and it is where the two families differ most.

pub mod fixtures;
pub mod metrics;
pub mod pvoc;
pub mod wsola;

/// Immutable source material plus the control-side analysis a stretcher may
/// rely on. Everything in here is prepared off the audio thread.
pub struct Source<'a> {
    pub frames: &'a [[f32; 2]],
    /// Playback region, in frames.
    pub start: usize,
    pub end: usize,
    /// Loop bounds inside the region. `None` means play once to `end`.
    pub loop_bounds: Option<(usize, usize)>,
    /// Onset positions in frames, ascending. Prepared by `metrics::onsets`.
    pub onsets: &'a [usize],
}

impl<'a> Source<'a> {
    pub fn whole(frames: &'a [[f32; 2]], onsets: &'a [usize]) -> Self {
        Self {
            frames,
            start: 0,
            end: frames.len(),
            loop_bounds: None,
            onsets,
        }
    }

    pub fn looped(frames: &'a [[f32; 2]], onsets: &'a [usize]) -> Self {
        Self {
            frames,
            start: 0,
            end: frames.len(),
            loop_bounds: Some((0, frames.len())),
            onsets,
        }
    }

    /// Read one frame at an absolute index, applying loop wrap or end clamp.
    /// This is the only place the region/loop policy lives, so both candidates
    /// see identical material.
    #[inline]
    pub fn frame(&self, index: i64) -> [f32; 2] {
        let len = self.frames.len() as i64;
        if len == 0 {
            return [0.0, 0.0];
        }
        let index = match self.loop_bounds {
            Some((ls, le)) if le > ls => {
                let ls = ls as i64;
                let le = le as i64;
                let span = le - ls;
                if index >= le || index < ls {
                    ls + (index - ls).rem_euclid(span)
                } else {
                    index
                }
            }
            _ => index,
        };
        if index < 0 || index >= len {
            [0.0, 0.0]
        } else {
            self.frames[index as usize]
        }
    }

    /// Mid-channel read, used for every alignment and analysis decision so a
    /// stereo pair never drifts apart.
    #[inline]
    pub fn mid(&self, index: i64) -> f32 {
        let f = self.frame(index);
        0.5 * (f[0] + f[1])
    }

    /// Length of the material that will be consumed once, in frames.
    pub fn playable_len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

/// The realtime surface every candidate must satisfy: fixed state allocated in
/// `new`, a pull-style `render`, a declared latency, and a declared footprint.
pub trait Stretcher {
    /// Name used in the report.
    fn name(&self) -> &'static str;
    /// Start (or restart) at an absolute input frame. Must not allocate.
    fn reset(&mut self, start_frame: usize);
    /// Change the stretch ratio (output frames per input frame). Must not
    /// allocate and must be safe to call between any two blocks.
    fn set_ratio(&mut self, ratio: f64);
    /// Fill `out` with the next block. Must not allocate.
    fn render(&mut self, src: &Source<'_>, out: &mut [[f32; 2]]);
    /// Algorithmic latency in output frames.
    fn latency_frames(&self) -> usize;
    /// Heap bytes held by this instance, per voice.
    fn state_bytes(&self) -> usize;
}

/// Render `out_len` frames in one call. Used as the reference against which
/// every block-size run is compared.
pub fn render_all(
    stretcher: &mut dyn Stretcher,
    src: &Source<'_>,
    start_frame: usize,
    out_len: usize,
) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; out_len];
    stretcher.reset(start_frame);
    stretcher.render(src, &mut out);
    out
}

/// Render `out_len` frames in fixed blocks, the way the executor would.
pub fn render_blocked(
    stretcher: &mut dyn Stretcher,
    src: &Source<'_>,
    start_frame: usize,
    out_len: usize,
    block: usize,
) -> Vec<[f32; 2]> {
    let mut out = vec![[0.0f32; 2]; out_len];
    stretcher.reset(start_frame);
    let mut pos = 0;
    while pos < out_len {
        let n = block.min(out_len - pos);
        stretcher.render(src, &mut out[pos..pos + n]);
        pos += n;
    }
    out
}

/// Hann window, periodic form (COLA-correct for hop = N/2 and N/4).
pub fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = core::f32::consts::TAU * i as f32 / n as f32;
            0.5 - 0.5 * x.cos()
        })
        .collect()
}

/// Deterministic PRNG so every fixture is byte-reproducible on any machine.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in `[-1, 1)`.
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / 8_388_608.0 - 1.0
    }
}
