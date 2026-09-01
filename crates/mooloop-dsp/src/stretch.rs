//! Pitch-independent time stretching for the sampler (#13).
//!
//! WSOLA — waveform-similarity overlap-add — chosen by the #32 spike over an
//! STFT phase vocoder. The spike's harness, measurements, and the rejected
//! alternatives are under `spikes/time-stretch/`; `RESULTS.md` there is the
//! justification for every constant in this file.
//!
//! The algorithm, in one paragraph: advance a fractional analysis pointer by
//! `hop / ratio` input frames per output hop; before laying each window down,
//! nudge its start within `± search` frames to the position whose leading
//! half best matches the natural continuation of the previously chosen
//! segment; overlap-add under a periodic Hann that is COLA at 50%. The
//! similarity search is the whole trick — it keeps waveform phase continuous
//! across the join, which is what stops the comb-filtered, chorused sound
//! that plain SOLA produces.
//!
//! Two properties worth stating because they are contract, not accident:
//!
//! - **Latency is zero.** Output frame 0 corresponds to input frame `start`.
//!   The window and the search are lookahead *into a resident sample*, not
//!   delay in time, so [`Stretcher::latency_frames`] is 0 and there is
//!   nothing to compensate. This is a property of pulling from a resident
//!   sample; a streaming stretcher with a declared analysis latency could not
//!   offer it.
//! - **Duration is exact.** The analysis pointer is fractional, so rounding
//!   never accumulates: ask for `n` output frames and get `n`.
//!
//! Onset snapping is deliberately absent. The spike measured it as helpful
//! with a trustworthy onset table and destructive with a bad one — up to 255
//! cents of pitch error on a held bass note — so it waits for the detector in
//! #33 rather than shipping on top of a throwaway one.

use crate::interpolate::Region;

/// Window length the default quality aims for, in milliseconds.
///
/// **This is the sizing rule the spike produced, and it is load-bearing:** the
/// window must span at least ~1.2 periods of the lowest fundamental that has
/// to survive. 21.3 ms is 1.17x the 18.2 ms period of A1 (55 Hz), and at that
/// width a sustained 55 Hz note comes through 0.4 cents sharp. Halve the
/// window and the similarity search locks onto the wrong period: the same
/// note drifts up to 705 cents and the fundamental is destroyed. Do not turn
/// this into a free knob.
const MUSIC_WINDOW_MS: f64 = 21.333;

/// Percussion window. Half the musical one, which trades the low fundamental
/// away for transient accuracy and extends usable ratios to 2.0 on a break.
const DRUMS_WINDOW_MS: f64 = 10.667;

/// Ratios outside this are refused rather than attempted. 0.5–1.5 is the
/// range the spike measured as clean on a break; the wider clamp exists so a
/// modulated or automated ratio degrades instead of producing nonsense.
pub const MIN_RATIO: f64 = 0.25;
pub const MAX_RATIO: f64 = 4.0;

/// How the window is sized. Two modes, because one window cannot both
/// preserve a 55 Hz fundamental and place a hi-hat accurately.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StretchQuality {
    /// 21.3 ms window. Preserves bass, and the default for anything pitched.
    #[default]
    Music,
    /// 10.7 ms window. Sharper transients and usable to ratio 2.0 on a break,
    /// but it destroys low fundamentals — surface it as percussion-only.
    Drums,
}

impl StretchQuality {
    fn window_ms(self) -> f64 {
        match self {
            Self::Music => MUSIC_WINDOW_MS,
            Self::Drums => DRUMS_WINDOW_MS,
        }
    }
}

/// Window length in frames for a quality at a sample rate.
///
/// Forced even, because the hop is half the window and the Hann is COLA at
/// exactly 50%. An odd window would leave the overlap-add short of unity by a
/// fraction that varies across the window — audible as a periodic amplitude
/// ripple at the hop rate rather than as anything obviously broken.
fn window_frames(quality: StretchQuality, sample_rate: u32) -> usize {
    let raw = (quality.window_ms() / 1000.0 * sample_rate as f64).round() as usize;
    (raw.max(64) + 1) & !1
}

/// One voice's stretcher. All state is allocated in [`Stretcher::new`]; every
/// other method on this type is allocation- and drop-free, which is what lets
/// it live on the audio thread.
pub struct Stretcher {
    window: usize,
    hop: usize,
    overlap: usize,
    /// Half-width of the similarity search. Equal to the hop, which is what
    /// the spike measured; a wider search costs linearly and did not improve
    /// any metric.
    search: usize,
    /// Decimation of the correlation sum. The search still visits every
    /// candidate offset — this only thins the inner product at each one.
    corr_decim: usize,
    window_fn: Vec<f32>,
    /// Overlap-add accumulator, used as a ring so a completed hop can be
    /// drained without shifting the tail down.
    acc: Vec<[f32; 2]>,
    head: usize,
    /// One hop of finished output, drained a frame at a time by `next_frame`.
    ready: Vec<[f32; 2]>,
    ready_pos: usize,
    ready_len: usize,
    /// The natural continuation of the previous segment: what the next window
    /// would have to look like for the join to be seamless.
    nat: Vec<f32>,
    /// Mid-channel candidates for this frame's search, read once so the inner
    /// loop is a flat scan rather than `2 * search + overlap` region lookups.
    search_buf: Vec<f32>,
    analysis_pos: f64,
    prev_chosen: i64,
    ratio: f64,
    first_frame: bool,
}

impl Stretcher {
    pub fn new(quality: StretchQuality, sample_rate: u32) -> Self {
        let window = window_frames(quality, sample_rate);
        let hop = window / 2;
        let overlap = window - hop;
        let search = hop;
        Self {
            window,
            hop,
            overlap,
            search,
            corr_decim: 2,
            window_fn: hann(window),
            acc: vec![[0.0; 2]; window],
            head: 0,
            ready: vec![[0.0; 2]; hop],
            ready_pos: 0,
            ready_len: 0,
            nat: vec![0.0; overlap],
            search_buf: vec![0.0; 2 * search + overlap + 1],
            analysis_pos: 0.0,
            prev_chosen: 0,
            ratio: 1.0,
            first_frame: true,
        }
    }

    /// Algorithmic latency, in output frames. Always zero — see the module
    /// header. Present so the node contract has something honest to report
    /// rather than callers assuming it.
    pub fn latency_frames(&self) -> usize {
        0
    }

    /// Frames past the nominal analysis position the stretcher may read.
    ///
    /// This is a region bound, not latency: it says how close to the end of a
    /// non-looping region the analysis pointer can get before the search
    /// starts finding silence rather than material.
    pub fn lookahead_frames(&self) -> usize {
        self.window + self.search
    }

    /// Heap bytes held per voice. Reported so the polyphony budget can be
    /// checked against something measured rather than estimated.
    pub fn state_bytes(&self) -> usize {
        self.window_fn.capacity() * 4
            + self.acc.capacity() * 8
            + self.ready.capacity() * 8
            + self.nat.capacity() * 4
            + self.search_buf.capacity() * 4
    }

    /// Output frames per input frame. `1.5` is longer and slower.
    ///
    /// Takes effect at the next overlap-add hop rather than the next frame:
    /// a window already being laid down is finished under the ratio it
    /// started with. The spike measured live ratio changes as click-free, so
    /// there is deliberately no crossfade or declick here.
    pub fn set_ratio(&mut self, ratio: f64) {
        if ratio.is_finite() {
            self.ratio = ratio.clamp(MIN_RATIO, MAX_RATIO);
        }
    }

    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// Restart at an absolute input frame. Allocation- and drop-free, so a
    /// note-on can call it.
    pub fn reset(&mut self, start_frame: f64) {
        for frame in self.acc.iter_mut() {
            *frame = [0.0, 0.0];
        }
        self.head = 0;
        self.ready_pos = 0;
        self.ready_len = 0;
        self.analysis_pos = start_frame;
        self.prev_chosen = start_frame as i64;
        self.first_frame = true;
    }

    /// Where the analysis pointer currently sits, in input frames. This is
    /// what a playhead display should follow: it is the position in the
    /// source that the output is currently speaking from.
    pub fn analysis_pos(&self) -> f64 {
        self.analysis_pos
    }

    /// Produce the next output frame.
    ///
    /// Per-frame rather than per-block because the sampler voice loop is
    /// per-frame — envelopes, the filter, and the shaper all advance around
    /// this call. Output is identical regardless of how the caller groups its
    /// pulls, because a whole overlap-add hop is computed at once and then
    /// drained; block size cannot change the arithmetic.
    pub fn next_frame(&mut self, frames: &[[f32; 2]], region: Region) -> [f32; 2] {
        if frames.is_empty() {
            return [0.0, 0.0];
        }
        if self.ready_pos >= self.ready_len {
            self.produce_hop(frames, region);
        }
        let frame = self.ready[self.ready_pos];
        self.ready_pos += 1;
        frame
    }

    /// Read one frame through the region's edge policy, so the stretcher sees
    /// exactly what the band-limited reader in [`crate::interpolate`] would
    /// see at the same index — a forward loop wraps, a ping-pong mirrors, a
    /// one-shot ends in silence.
    #[inline]
    fn frame_at(frames: &[[f32; 2]], region: Region, index: i64) -> [f32; 2] {
        match region.resolve(index, frames.len()) {
            Some(resolved) => frames[resolved],
            None => [0.0, 0.0],
        }
    }

    #[inline]
    fn mid_at(frames: &[[f32; 2]], region: Region, index: i64) -> f32 {
        let frame = Self::frame_at(frames, region, index);
        0.5 * (frame[0] + frame[1])
    }

    /// Compute one overlap-add hop into `ready`.
    fn produce_hop(&mut self, frames: &[[f32; 2]], region: Region) {
        let window = self.window;
        let hop = self.hop;
        let overlap = self.overlap;
        let search = self.search as i64;

        // Keep the analysis pointer inside a looping region. Without this the
        // pointer walks off the end of a loop and the search reads silence,
        // so a looped stretch would fade out over one pass instead of
        // repeating.
        if let Some(span) = region_span(region) {
            let end = region.end;
            while self.analysis_pos >= end {
                self.analysis_pos -= span;
                self.prev_chosen -= span as i64;
            }
        }

        let nominal = self.analysis_pos.round() as i64;

        // What the previous segment was about to become, had it kept playing.
        // The best candidate is the one that continues this.
        let nat_start = self.prev_chosen + hop as i64;
        for (offset, slot) in self.nat.iter_mut().enumerate() {
            *slot = Self::mid_at(frames, region, nat_start + offset as i64);
        }

        let base = nominal - search;
        for (offset, slot) in self.search_buf.iter_mut().enumerate() {
            *slot = Self::mid_at(frames, region, base + offset as i64);
        }

        let chosen = if self.first_frame {
            // Nothing to continue from yet, and searching would only move the
            // very first frame of playback away from where the caller asked
            // to start.
            nominal
        } else {
            base + self.best_offset() as i64
        };

        // Lay the window down into the accumulator ring. The first hop skips
        // the rising half so a one-shot's initial transient is played at full
        // amplitude rather than faded in from nothing.
        for offset in 0..window {
            let weight = if self.first_frame && offset < overlap {
                1.0
            } else {
                self.window_fn[offset]
            };
            let frame = Self::frame_at(frames, region, chosen + offset as i64);
            let slot = wrap_index(self.head + offset, window);
            self.acc[slot][0] += weight * frame[0];
            self.acc[slot][1] += weight * frame[1];
        }
        self.first_frame = false;

        // Drain the completed hop and clear it, so the ring is zeroed for the
        // window that will overlap into it next time.
        for offset in 0..hop {
            let slot = wrap_index(self.head + offset, window);
            self.ready[offset] = self.acc[slot];
            self.acc[slot] = [0.0, 0.0];
        }
        self.head = wrap_index(self.head + hop, window);
        self.ready_pos = 0;
        self.ready_len = hop;

        self.prev_chosen = chosen;
        // Fractional, so duration error never accumulates.
        self.analysis_pos += hop as f64 / self.ratio;
    }

    /// Index into `search_buf` of the candidate whose leading `overlap` frames
    /// best continue the previous segment.
    ///
    /// Normalized by the candidate's own energy but not by `nat`'s, since
    /// `nat` is fixed across the scan and cannot change the argmax. Without
    /// the candidate normalization the search would simply pick the loudest
    /// nearby moment rather than the best-matching one.
    fn best_offset(&self) -> usize {
        let overlap = self.overlap;
        let step = self.corr_decim.max(1);
        let last = 2 * self.search;
        let mut best_offset = 0;
        let mut best_score = f32::NEG_INFINITY;
        for candidate in 0..=last {
            let mut correlation = 0.0f32;
            let mut energy = 1.0e-9f32;
            let mut offset = 0;
            while offset < overlap {
                let value = self.search_buf[candidate + offset];
                correlation += value * self.nat[offset];
                energy += value * value;
                offset += step;
            }
            let score = correlation / energy.sqrt();
            if score > best_score {
                best_score = score;
                best_offset = candidate;
            }
        }
        best_offset
    }
}

/// Length of a region the analysis pointer should wrap inside, or `None` for
/// a region it should simply run off the end of.
fn region_span(region: Region) -> Option<f64> {
    match region.edge {
        // A ping-pong turnaround is not a wrap: folding the analysis pointer
        // as if it were would replay the region forwards instead of
        // reversing it. Reverse and ping-pong under stretch are out of scope
        // for v1 (#13), and the UI disables stretch for them rather than
        // silently producing this.
        crate::interpolate::RegionEdge::Wrap => {
            let span = region.end - region.start;
            (span > 0.0).then_some(span)
        }
        _ => None,
    }
}

#[inline]
fn wrap_index(index: usize, window: usize) -> usize {
    if index >= window {
        index - window
    } else {
        index
    }
}

/// Periodic Hann. COLA at 50% overlap, which is why the overlap-add needs no
/// synthesis window and no normalization pass.
fn hann(len: usize) -> Vec<f32> {
    (0..len)
        .map(|index| {
            let phase = core::f32::consts::TAU * index as f32 / len as f32;
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpolate::RegionEdge;

    const SR: u32 = 48_000;

    fn tone(len: usize, hz: f64) -> Vec<[f32; 2]> {
        (0..len)
            .map(|index| {
                let phase =
                    core::f64::consts::TAU * hz * index as f64 / SR as f64;
                let value = phase.sin() as f32;
                [value, value]
            })
            .collect()
    }

    fn render(
        stretcher: &mut Stretcher,
        frames: &[[f32; 2]],
        region: Region,
        count: usize,
    ) -> Vec<[f32; 2]> {
        (0..count)
            .map(|_| stretcher.next_frame(frames, region))
            .collect()
    }

    /// The sizing rule from the spike, as an executable claim: the default
    /// window is at least 1.2 periods of A1, and the percussion window is
    /// deliberately not.
    #[test]
    fn the_music_window_spans_a_low_fundamental_and_the_drum_window_does_not() {
        let a1_period = SR as f64 / 55.0;
        let music = window_frames(StretchQuality::Music, SR) as f64;
        let drums = window_frames(StretchQuality::Drums, SR) as f64;
        assert!(
            music >= a1_period * 1.15,
            "music window {music} must span ~1.2 periods of {a1_period}"
        );
        assert!(
            drums < a1_period,
            "drum window {drums} is expected to be too short for 55 Hz"
        );
    }

    #[test]
    fn the_default_window_is_1024_frames_at_48k() {
        assert_eq!(window_frames(StretchQuality::Music, SR), 1024);
        assert_eq!(window_frames(StretchQuality::Drums, SR), 512);
    }

    /// Every window must be even, or the Hann stops summing to unity across
    /// the hop and the output ripples.
    #[test]
    fn every_supported_sample_rate_yields_an_even_window() {
        for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
            for quality in [StretchQuality::Music, StretchQuality::Drums] {
                let window = window_frames(quality, rate);
                assert_eq!(window % 2, 0, "{quality:?} at {rate} gave {window}");
            }
        }
    }

    /// The COLA property the overlap-add depends on: a periodic Hann summed
    /// against itself half a window later is flat.
    #[test]
    fn the_hann_window_sums_to_unity_across_the_hop() {
        let window = hann(1024);
        for offset in 0..512 {
            let sum = window[offset] + window[offset + 512];
            assert!(
                (sum - 1.0).abs() < 1.0e-5,
                "offset {offset} summed to {sum}"
            );
        }
    }

    /// How the caller groups its pulls must not change a single sample. The
    /// realtime path pulls one frame at a time inside a block of whatever
    /// length the executor chose; an offline render pulls the whole thing.
    /// They have to agree bit for bit, which is what makes an exported render
    /// match what was heard.
    #[test]
    fn output_is_identical_however_the_frames_are_grouped() {
        let source = tone(20_000, 220.0);
        let region = Region::whole(source.len());
        let count = 6_000;

        let mut one_shot = Stretcher::new(StretchQuality::Music, SR);
        one_shot.set_ratio(1.37);
        one_shot.reset(0.0);
        let reference = render(&mut one_shot, &source, region, count);

        for block in [1usize, 32, 64, 128, 480, 512, 1024] {
            let mut blocked = Stretcher::new(StretchQuality::Music, SR);
            blocked.set_ratio(1.37);
            blocked.reset(0.0);
            let mut produced = Vec::with_capacity(count);
            while produced.len() < count {
                let take = block.min(count - produced.len());
                produced.extend(render(&mut blocked, &source, region, take));
            }
            assert_eq!(
                produced, reference,
                "block size {block} diverged from the one-shot render"
            );
        }
    }

    /// A stretched sustained tone must keep its pitch. Measured as zero
    /// crossings, which is crude but entirely sufficient to catch the failure
    /// this test exists for -- a window too short for the fundamental locks
    /// onto the wrong period and the pitch moves by hundreds of cents.
    #[test]
    fn a_sustained_tone_keeps_its_pitch_when_stretched() {
        let hz = 220.0;
        let source = tone(96_000, hz);
        let region = Region::whole(source.len());

        for ratio in [0.75, 1.25, 1.5] {
            let mut stretcher = Stretcher::new(StretchQuality::Music, SR);
            stretcher.set_ratio(ratio);
            stretcher.reset(0.0);
            // Skip the first hops: the flat-topped first window is a
            // deliberate transient exemption, not steady state.
            let out = render(&mut stretcher, &source, region, 48_000);
            let steady = &out[4_096..];

            let crossings = steady
                .windows(2)
                .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
                .count();
            let measured = crossings as f64 * SR as f64 / steady.len() as f64;
            let cents = 1200.0 * (measured / hz).log2();
            assert!(
                cents.abs() < 20.0,
                "ratio {ratio} moved the pitch by {cents:.1} cents \
                 ({measured:.1} Hz vs {hz})"
            );
        }
    }

    /// Duration is exact by construction, and this is the property that makes
    /// it so: the analysis pointer is fractional, so over thousands of hops
    /// it lands where arithmetic says it should rather than drifting.
    #[test]
    fn the_analysis_pointer_advances_without_accumulating_error() {
        let source = tone(400_000, 110.0);
        let region = Region::whole(source.len());
        let ratio = 1.37;

        let mut stretcher = Stretcher::new(StretchQuality::Music, SR);
        stretcher.set_ratio(ratio);
        stretcher.reset(0.0);
        let produced = 200_000;
        render(&mut stretcher, &source, region, produced);

        // Every frame handed out consumed `1 / ratio` input frames. The
        // pointer runs up to one hop ahead of what has been drained, since a
        // hop is computed before it is consumed.
        let expected = produced as f64 / ratio;
        let hop = window_frames(StretchQuality::Music, SR) as f64 / 2.0;
        let drift = stretcher.analysis_pos() - expected;
        assert!(
            drift >= -1.0 && drift <= hop + 1.0,
            "pointer at {} for {produced} frames at ratio {ratio}: drift {drift}",
            stretcher.analysis_pos()
        );
    }

    /// A looping region has to keep repeating under stretch. Before the
    /// analysis pointer was wrapped, it walked past the loop end and the
    /// search found silence, so a stretched loop faded out after one pass.
    #[test]
    fn a_looping_region_keeps_producing_after_several_passes() {
        let source = tone(8_000, 220.0);
        let region = Region {
            start: 0.0,
            end: source.len() as f64,
            edge: RegionEdge::Wrap,
        };

        let mut stretcher = Stretcher::new(StretchQuality::Music, SR);
        stretcher.set_ratio(1.25);
        stretcher.reset(0.0);
        let out = render(&mut stretcher, &source, region, 60_000);

        let tail = &out[50_000..];
        let rms = (tail.iter().map(|f| f[0] * f[0]).sum::<f32>()
            / tail.len() as f32)
            .sqrt();
        assert!(
            rms > 0.3,
            "loop went quiet after several passes: tail rms {rms}"
        );
    }

    /// Ratios are clamped rather than trusted. A modulated or automated ratio
    /// can arrive at any value, including one that would send the analysis
    /// pointer somewhere meaningless.
    #[test]
    fn an_out_of_range_or_nonfinite_ratio_cannot_take_hold() {
        let mut stretcher = Stretcher::new(StretchQuality::Music, SR);
        stretcher.set_ratio(1.25);

        stretcher.set_ratio(f64::NAN);
        assert_eq!(stretcher.ratio(), 1.25, "NaN must leave the ratio alone");

        stretcher.set_ratio(0.0);
        assert_eq!(stretcher.ratio(), MIN_RATIO);

        stretcher.set_ratio(1_000.0);
        assert_eq!(stretcher.ratio(), MAX_RATIO);
    }

    /// Unity is not a special case in the code, so it is worth pinning that
    /// it behaves like one: at ratio 1.0 the output should track the source
    /// closely rather than merely being the right length.
    #[test]
    fn unity_ratio_tracks_the_source() {
        let source = tone(40_000, 220.0);
        let region = Region::whole(source.len());

        let mut stretcher = Stretcher::new(StretchQuality::Music, SR);
        stretcher.set_ratio(1.0);
        stretcher.reset(0.0);
        let out = render(&mut stretcher, &source, region, 20_000);

        let error: f32 = out[2_048..]
            .iter()
            .zip(source[2_048..].iter())
            .map(|(played, expected)| (played[0] - expected[0]).abs())
            .sum::<f32>()
            / (out.len() - 2_048) as f32;
        assert!(error < 0.05, "mean absolute error at unity was {error}");
    }

    /// The footprint the polyphony budget in #13 is written against.
    #[test]
    fn the_per_voice_footprint_matches_the_contract() {
        let music = Stretcher::new(StretchQuality::Music, SR);
        let drums = Stretcher::new(StretchQuality::Drums, SR);
        assert_eq!(music.state_bytes(), 24_580);
        assert_eq!(drums.state_bytes(), 12_292);
    }

    /// Zero latency is contract, not an implementation detail: the node layer
    /// reports it and nothing downstream compensates for it.
    #[test]
    fn the_stretcher_declares_no_latency() {
        let stretcher = Stretcher::new(StretchQuality::Music, SR);
        assert_eq!(stretcher.latency_frames(), 0);
        assert_eq!(stretcher.lookahead_frames(), 1_536);
    }

    /// An empty sample must not panic or read out of bounds -- a voice can be
    /// rendered in the window between a sample being cleared and the voice
    /// noticing.
    #[test]
    fn an_empty_sample_renders_silence() {
        let mut stretcher = Stretcher::new(StretchQuality::Music, SR);
        stretcher.reset(0.0);
        let out = render(&mut stretcher, &[], Region::whole(0), 512);
        assert!(out.iter().all(|frame| frame == &[0.0, 0.0]));
    }
}
