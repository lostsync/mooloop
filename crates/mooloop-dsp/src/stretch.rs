//! Pitch-independent time stretching for the sampler (#13).
//!
//! WSOLA — waveform-similarity overlap-add — chosen by the #32 spike over an
//! STFT phase vocoder. The spike's harness, measurements, and the rejected
//! alternatives are under `spikes/time-stretch/`; `RESULTS.md` there is the
//! justification for the transparent modes' constants.
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
//! # The artifact is a feature
//!
//! [`StretchMode::Grain`] exists because the spike optimized for the wrong
//! thing for half of this instrument's use. It graded candidates on
//! transparency and called ratio 2.0 "falls apart"; the rattling, woodblock
//! character of a break stretched far past musical range is a sound people
//! reach for deliberately, and it is one of the two things this engine is
//! wanted for.
//!
//! What the search does is place every splice where the waveform continues
//! phase-coherently. Without it, each splice lands wherever the analysis
//! pointer happens to be, so the overlap-add joins two segments at arbitrary
//! relative phase — a discontinuity, once per hop, at a fixed rate. That is
//! the buzz: sidebands at `sample_rate / hop` around whatever the material
//! was, growing as the ratio rises and each grain is laid down more times.
//! Measurably, on a sustained tone at ratio 8, the searching modes hold
//! about 0.9 of their energy in the fundamental and `Grain` holds well under
//! half. So transparency and rattle are separate modes rather than two ends
//! of one quality knob, and `Grain` is the mode that declines to search.
//!
//! Note what is *not* claimed: that the search removes the repetition. At a
//! high ratio every mode replays the same material many times — that is what
//! stretching is. The search changes how the repeats are joined, not that
//! they repeat. Whether the result sounds like the record Adam is after is a
//! listening question, and nothing here has been judged by ear.
//!
//! Because the buzz sits at the hop rate, the grain size is a timbral control
//! rather than a quality setting: 1024 frames at 48 kHz rattles at 94 Hz,
//! 256 at 375 Hz, 128 at 750 Hz. It is deliberately free and continuous so it
//! can be swept and modulated.
//!
//! Onset snapping is absent in every mode. The spike measured it as helpful
//! with a trustworthy onset table and destructive with a bad one — up to 255
//! cents on a held bass note — so it waits for the detector in #33.

use crate::interpolate::{Region, RegionEdge, SincTable, MAX_HALF_TAPS};
use mooloop_core::{
    StretchMode, MAX_STRETCH_GRAIN, MAX_STRETCH_RATIO, MIN_STRETCH_GRAIN, MIN_STRETCH_RATIO,
};

/// Window length the default mode aims for, in milliseconds.
///
/// **This is the sizing rule the spike produced, and it is load-bearing:** the
/// window must span at least ~1.2 periods of the lowest fundamental that has
/// to survive. 21.3 ms is 1.17x the 18.2 ms period of A1 (55 Hz), and at that
/// width a sustained 55 Hz note comes through 0.4 cents sharp. Halve the
/// window and the similarity search locks onto the wrong period: the same
/// note drifts up to 705 cents and the fundamental is destroyed. Do not turn
/// this into a free knob — [`StretchMode::Grain`] is where free window sizes
/// live, and it has no fundamental to protect because it is not trying to be
/// transparent.
const MUSIC_WINDOW_MS: f64 = 21.333;

/// Percussion window. Half the musical one, which trades the low fundamental
/// away for transient accuracy and extends usable ratios to 2.0 on a break.
const DRUMS_WINDOW_MS: f64 = 10.667;

/// Grain window bounds, in frames rather than milliseconds.
///
/// Frames, because this control's meaning is the repetition rate it produces
/// — `sample_rate / (grain / 2)` — and a user sweeping it is chasing a pitch,
/// not a duration. At 48 kHz the range buzzes from about 23 Hz to 1.5 kHz.
pub const GRAIN_MIN_FRAMES: u32 = MIN_STRETCH_GRAIN as u32;
pub const GRAIN_MAX_FRAMES: u32 = MAX_STRETCH_GRAIN as u32;
pub const GRAIN_DEFAULT_FRAMES: u32 = 1024;

/// Ratio bounds. Output frames per input frame, so above 1.0 is slower.
///
/// The ceiling is high on purpose. An earlier draft clamped near the top of
/// the spike's *clean* range, which was the wrong instinct: extreme
/// slow-down is a destination here, not a failure, and CPU does not care —
/// cost is per output hop and the output hop rate is fixed however slowly the
/// analysis pointer crawls. The floor is where speeding up stops resembling
/// the source at all.
pub const MIN_RATIO: f64 = MIN_STRETCH_RATIO as f64;
pub const MAX_RATIO: f64 = MAX_STRETCH_RATIO as f64;

/// Resolution of the shared Hann prototype. Read with linear interpolation at
/// whatever the active window length is, so changing grain size mid-render
/// costs a different stride through this table rather than rebuilding a
/// window — which would mean thousands of `cos` calls on the audio thread.
const HANN_TABLE: usize = 4096;

/// Whether a mode hunts for the best splice point. The one structural
/// difference between transparency and rattle.
///
/// A free function rather than a method because [`StretchMode`] is
/// `mooloop_core`'s -- it is serialized into projects and addressed by
/// descriptor, so it belongs with the other device parameters rather than
/// here, and this crate cannot hang inherent methods on it.
fn searches(mode: StretchMode) -> bool {
    !matches!(mode, StretchMode::Grain)
}

/// Window length in frames for a mode at a sample rate.
///
/// Forced even, because the hop is half the window and the Hann is COLA at
/// exactly 50%. An odd window would leave the overlap-add short of unity by a
/// fraction that varies across the window — audible as a periodic amplitude
/// ripple at the hop rate rather than as anything obviously broken. In
/// `Grain` that ripple is the point, but it should come from the splice
/// placement, not from a rounding error nobody chose.
fn window_frames(mode: StretchMode, sample_rate: u32, grain_frames: u32) -> usize {
    let raw = match mode {
        StretchMode::Grain => {
            grain_frames.clamp(GRAIN_MIN_FRAMES, GRAIN_MAX_FRAMES) as usize
        }
        _ => {
            let ms = if mode == StretchMode::Music {
                MUSIC_WINDOW_MS
            } else {
                DRUMS_WINDOW_MS
            };
            (ms / 1000.0 * sample_rate as f64).round() as usize
        }
    };
    (raw.max(GRAIN_MIN_FRAMES as usize) + 1) & !1
}

/// Largest window this stretcher may ever be asked for, and therefore what
/// its buffers are sized to.
///
/// Mode and grain size are live controls, so the buffers cannot be sized to
/// the *current* window — they are sized once to the worst case and a shorter
/// window uses a prefix. That is what makes changing either of them on the
/// audio thread allocation-free.
fn capacity_frames(sample_rate: u32) -> usize {
    window_frames(StretchMode::Music, sample_rate, 0).max(GRAIN_MAX_FRAMES as usize)
}

/// How often, in output frames, [`Stretcher::next_frame`] pays down the
/// next hop's search. A power of two, so the check is a mask. At 16 a
/// 128-frame block pays eight shares, and a Music hop of 512 frames is split
/// into 32.
const PACE: usize = 16;

/// Candidates scored together. The same sums in the same order per
/// candidate as one at a time, so the choice is bit for bit the same, but
/// eight independent chains instead of one dependent chain, which the
/// compiler turns into vector adds.
const LANES: usize = 8;

/// The next hop's splice search, begun as soon as this hop is laid down and
/// paid for a share at a time while this hop drains (MOO-248).
///
/// Everything the search reads is known the moment a hop is chosen: where
/// the next one nominally starts (the analysis pointer has already moved)
/// and what it must continue (the chosen segment's natural continuation).
/// Doing it all at the hop boundary put a whole search in whichever
/// callback the boundary fell in, and voices started together fell in the
/// same one. The plan records what it was begun from, and the boundary uses
/// it only if all of that still holds; otherwise it searches from scratch
/// there, as before, so a plan can never change what is chosen.
#[derive(Clone, Copy)]
struct Plan {
    live: bool,
    mode: StretchMode,
    window: usize,
    nominal: i64,
    nat_start: i64,
    region_start: u64,
    region_end: u64,
    edge: RegionEdge,
    frames_ptr: usize,
    frames_len: usize,
    nat_done: usize,
    buf_done: usize,
    next_candidate: usize,
    best_offset: usize,
    best_score: f32,
    units_total: usize,
    units_done: usize,
}

impl Plan {
    fn idle() -> Self {
        Self {
            live: false,
            mode: StretchMode::Music,
            window: 0,
            nominal: 0,
            nat_start: 0,
            region_start: 0,
            region_end: 0,
            edge: RegionEdge::Silent,
            frames_ptr: 0,
            frames_len: 0,
            nat_done: 0,
            buf_done: 0,
            next_candidate: 0,
            best_offset: 0,
            best_score: f32::NEG_INFINITY,
            units_total: 0,
            units_done: 0,
        }
    }

    /// Whether this plan was begun on the same sample and region.
    fn reads(&self, frames: &[[f32; 2]], region: Region) -> bool {
        self.frames_ptr == frames.as_ptr() as usize
            && self.frames_len == frames.len()
            && self.region_start == region.start.to_bits()
            && self.region_end == region.end.to_bits()
            && self.edge == region.edge
    }
}

/// One voice's stretcher. All state is allocated in [`Stretcher::new`]; every
/// other method on this type is allocation- and drop-free, which is what lets
/// it live on the audio thread.
pub struct Stretcher {
    sample_rate: u32,
    mode: StretchMode,
    grain_frames: u32,
    /// Active geometry, re-derived only when mode or grain size changes, and
    /// only at a hop boundary.
    window: usize,
    hop: usize,
    overlap: usize,
    /// Half-width of the similarity search. Equal to the hop, which is what
    /// the spike measured; a wider search costs linearly and did not improve
    /// any metric. Zero in `Grain`, which is the mode's whole definition.
    search: usize,
    /// Decimation of the correlation sum. The search still visits every
    /// candidate offset — this only thins the inner product at each one.
    corr_decim: usize,
    /// Pending geometry, applied at the next hop. Changing the window
    /// mid-window would leave the accumulator holding half of one envelope
    /// and half of another, which clicks.
    pending: Option<(StretchMode, u32)>,
    hann: Vec<f32>,
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
    /// Mid-channel candidates for this hop's search, read once so the inner
    /// loop is a flat scan rather than `2 * search + overlap` region lookups.
    search_buf: Vec<f32>,
    analysis_pos: f64,
    prev_chosen: i64,
    ratio: f64,
    first_frame: bool,
    plan: Plan,
    /// Whether the search is spread over the hop. Off only in tests and
    /// the cost measurement, to compare against the search at the boundary.
    spread: bool,
}

impl Stretcher {
    pub fn new(mode: StretchMode, sample_rate: u32) -> Self {
        let capacity = capacity_frames(sample_rate);
        let window = window_frames(mode, sample_rate, GRAIN_DEFAULT_FRAMES);
        let hop = window / 2;
        let mut stretcher = Self {
            sample_rate,
            mode,
            grain_frames: GRAIN_DEFAULT_FRAMES,
            window,
            hop,
            overlap: window - hop,
            search: if searches(mode) { hop } else { 0 },
            corr_decim: 2,
            pending: None,
            hann: hann_table(),
            acc: vec![[0.0; 2]; capacity],
            head: 0,
            ready: vec![[0.0; 2]; capacity / 2],
            ready_pos: 0,
            ready_len: 0,
            nat: vec![0.0; capacity / 2],
            search_buf: vec![0.0; capacity + capacity / 2 + 1],
            analysis_pos: 0.0,
            prev_chosen: 0,
            ratio: 1.0,
            first_frame: true,
            plan: Plan::idle(),
            spread: true,
        };
        stretcher.apply_geometry(mode, GRAIN_DEFAULT_FRAMES);
        stretcher
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

    /// Heap bytes held per voice.
    ///
    /// Sized to the worst-case window rather than the active one, because
    /// mode and grain size are live controls. That is why this is several
    /// times the figure in #13's original budget: the budget was written when
    /// the window was fixed at construction.
    pub fn state_bytes(&self) -> usize {
        self.hann.capacity() * 4
            + self.acc.capacity() * 8
            + self.ready.capacity() * 8
            + self.nat.capacity() * 4
            + self.search_buf.capacity() * 4
    }

    pub fn mode(&self) -> StretchMode {
        self.mode
    }

    pub fn grain_frames(&self) -> u32 {
        self.grain_frames
    }

    /// Active window length in frames. In `Grain` this is what sets the
    /// repetition rate, at `sample_rate / (window / 2)`.
    pub fn window(&self) -> usize {
        self.window
    }

    /// Frequency of the grain repetition, in Hz. Meaningless in the
    /// transparent modes, where the search is actively suppressing it.
    pub fn rattle_hz(&self) -> f64 {
        self.sample_rate as f64 / self.hop as f64
    }

    /// Switch mode. Takes effect at the next hop boundary.
    pub fn set_mode(&mut self, mode: StretchMode) {
        self.queue_geometry(mode, self.target().1);
    }

    /// Set the grain window, in frames. Free and continuous by design: this
    /// is a timbre, and sweeping it is the point. Clamped to
    /// [`GRAIN_MIN_FRAMES`]..=[`GRAIN_MAX_FRAMES`], and ignored by the
    /// transparent modes, whose window sizing is a correctness rule rather
    /// than a preference.
    pub fn set_grain_frames(&mut self, frames: u32) {
        self.queue_geometry(self.target().0, frames);
    }

    /// The geometry the stretcher is heading for: whatever is queued, or the
    /// active geometry if nothing is. Both setters read through this so that
    /// changing one control cannot silently discard a change to the other
    /// that has not landed yet -- a mode switch and a grain sweep arriving in
    /// the same block is the normal case, not an edge case.
    fn target(&self) -> (StretchMode, u32) {
        self.pending.unwrap_or((self.mode, self.grain_frames))
    }

    fn queue_geometry(&mut self, mode: StretchMode, grain_frames: u32) {
        let grain_frames = grain_frames.clamp(GRAIN_MIN_FRAMES, GRAIN_MAX_FRAMES);
        if mode == self.mode && grain_frames == self.grain_frames {
            self.pending = None;
            return;
        }
        self.pending = Some((mode, grain_frames));
    }

    fn apply_geometry(&mut self, mode: StretchMode, grain_frames: u32) {
        let window = window_frames(mode, self.sample_rate, grain_frames);
        self.mode = mode;
        self.grain_frames = grain_frames;
        self.window = window;
        self.hop = window / 2;
        self.overlap = window - self.hop;
        self.search = if searches(mode) { self.hop } else { 0 };
        self.pending = None;
        self.plan.live = false;
    }

    /// Search each hop whole at its boundary, as before MOO-248, instead of
    /// spreading it. For comparisons only; the output is the same.
    #[cfg(test)]
    pub(crate) fn set_spread(&mut self, spread: bool) {
        self.spread = spread;
        self.plan.live = false;
    }

    /// Output frames per input frame. `1.5` is longer and slower.
    ///
    /// Takes effect at the next overlap-add hop rather than the next frame:
    /// a window already being laid down is finished under the ratio it
    /// started with. The spike measured live ratio changes as click-free, so
    /// there is deliberately no crossfade or declick here. The hop
    /// quantization means an automated ratio moves in steps of one hop, which
    /// is also a gentle lowpass on a fast sweep.
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
        if let Some((mode, grain)) = self.pending.take() {
            self.apply_geometry(mode, grain);
        }
        self.head = 0;
        self.ready_pos = 0;
        self.ready_len = 0;
        self.analysis_pos = start_frame;
        self.prev_chosen = start_frame as i64;
        self.first_frame = true;
        self.plan.live = false;
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
    /// drained, and the next hop's search is paid for by frame count, not by
    /// call; block size cannot change the arithmetic.
    pub fn next_frame(&mut self, frames: &[[f32; 2]], region: Region) -> [f32; 2] {
        if frames.is_empty() {
            return [0.0, 0.0];
        }
        if self.ready_pos >= self.ready_len {
            self.produce_hop(frames, region);
        }
        let frame = self.ready[self.ready_pos];
        self.ready_pos += 1;
        if self.plan.live && self.ready_pos & (PACE - 1) == 0 {
            if self.plan.reads(frames, region) {
                // Due by the hop's last frame, in proportion to how much of
                // the hop has been drained.
                let due = self.plan.units_total * self.ready_pos / self.ready_len;
                self.advance_plan(frames, region, due);
            } else {
                // The sample or region moved under the plan: the boundary
                // searches from scratch, as it always did.
                self.plan.live = false;
            }
        }
        frame
    }

    /// Read one frame through the region's edge policy, so the stretcher sees
    /// exactly what the band-limited reader in [`crate::interpolate`] would
    /// see at the same index — a forward loop wraps, a ping-pong mirrors, a
    /// one-shot ends in silence.
    #[inline]
    fn frame_at(frames: &[[f32; 2]], region: Region, index: i64) -> [f32; 2] {
        region.frame(frames, index).unwrap_or([0.0, 0.0])
    }

    #[inline]
    fn mid_at(frames: &[[f32; 2]], region: Region, index: i64) -> f32 {
        let frame = Self::frame_at(frames, region, index);
        0.5 * (frame[0] + frame[1])
    }

    /// `out[k] = mid_at(at + k)`, read straight from the slice when the run
    /// is wholly inside the region's plain span, which is the usual case.
    fn fill_mid(out: &mut [f32], frames: &[[f32; 2]], region: Region, at: i64) {
        let (lo, hi) = region.plain_span(frames.len());
        if at >= lo && at + out.len() as i64 <= hi {
            let from = at as usize;
            let count = out.len();
            for (slot, frame) in out.iter_mut().zip(&frames[from..from + count]) {
                *slot = 0.5 * (frame[0] + frame[1]);
            }
        } else {
            for (offset, slot) in out.iter_mut().enumerate() {
                *slot = Self::mid_at(frames, region, at + offset as i64);
            }
        }
    }

    /// Begin the search for a hop that nominally starts at `nominal` and
    /// must continue the material at `nat_start`, under the active geometry.
    fn begin_plan(&mut self, frames: &[[f32; 2]], region: Region, nominal: i64, nat_start: i64) {
        let overlap = self.overlap;
        let candidates = 2 * self.search + 1;
        let per_candidate = overlap.div_ceil(self.corr_decim.max(1));
        self.plan = Plan {
            live: true,
            mode: self.mode,
            window: self.window,
            nominal,
            nat_start,
            region_start: region.start.to_bits(),
            region_end: region.end.to_bits(),
            edge: region.edge,
            frames_ptr: frames.as_ptr() as usize,
            frames_len: frames.len(),
            nat_done: 0,
            buf_done: 0,
            next_candidate: 0,
            best_offset: 0,
            best_score: f32::NEG_INFINITY,
            // One unit per frame read and one per multiply-add pair, which
            // is close enough to proportional for pacing.
            units_total: overlap + (candidates + overlap) + candidates * per_candidate,
            units_done: 0,
        };
    }

    /// Whether the live plan is the search the boundary is about to need.
    fn plan_fits(&self, frames: &[[f32; 2]], region: Region, nominal: i64, nat_start: i64) -> bool {
        self.plan.live
            && self.plan.mode == self.mode
            && self.plan.window == self.window
            && self.plan.nominal == nominal
            && self.plan.nat_start == nat_start
            && self.plan.reads(frames, region)
    }

    /// Do the plan's work until `due` units are done, or all of it. Reads
    /// the natural continuation, then the candidates, then scores them in
    /// order, keeping the first best exactly as a single pass would.
    fn advance_plan(&mut self, frames: &[[f32; 2]], region: Region, due: usize) {
        let overlap = self.overlap;
        let span = 2 * self.search + overlap + 1;
        let last = 2 * self.search;
        let per_candidate = overlap.div_ceil(self.corr_decim.max(1));
        while self.plan.units_done < due {
            let budget = due - self.plan.units_done;
            if self.plan.nat_done < overlap {
                let from = self.plan.nat_done;
                let count = (overlap - from).min(budget);
                let at = self.plan.nat_start + from as i64;
                Self::fill_mid(&mut self.nat[from..from + count], frames, region, at);
                self.plan.nat_done += count;
                self.plan.units_done += count;
            } else if self.plan.buf_done < span {
                let from = self.plan.buf_done;
                let count = (span - from).min(budget);
                let at = self.plan.nominal - self.search as i64 + from as i64;
                Self::fill_mid(&mut self.search_buf[from..from + count], frames, region, at);
                self.plan.buf_done += count;
                self.plan.units_done += count;
            } else if self.plan.next_candidate <= last {
                let first = self.plan.next_candidate;
                let scored = if first + LANES - 1 <= last {
                    let scores = self.score_lanes(first);
                    for (lane, score) in scores.into_iter().enumerate() {
                        self.keep_if_best(first + lane, score);
                    }
                    LANES
                } else {
                    let score = self.score(first);
                    self.keep_if_best(first, score);
                    1
                };
                self.plan.next_candidate += scored;
                self.plan.units_done += scored * per_candidate;
            } else {
                self.plan.units_done = self.plan.units_total;
                break;
            }
        }
    }

    #[inline]
    fn keep_if_best(&mut self, candidate: usize, score: f32) {
        if score > self.plan.best_score {
            self.plan.best_score = score;
            self.plan.best_offset = candidate;
        }
    }

    /// Hann weight at `offset` within a window of `window` frames, read from
    /// the shared prototype. Linear interpolation between table points; the
    /// prototype is fine enough that the residual is far below the COLA
    /// tolerance the overlap-add needs.
    #[inline]
    fn window_weight(&self, offset: usize, window: usize) -> f32 {
        let position = offset as f32 / window as f32 * HANN_TABLE as f32;
        let index = position as usize;
        let fraction = position - index as f32;
        let low = self.hann[index];
        let high = self.hann[index + 1];
        low + (high - low) * fraction
    }

    /// Compute one overlap-add hop into `ready`.
    fn produce_hop(&mut self, frames: &[[f32; 2]], region: Region) {
        // A queued mode or grain change lands here, between windows. Applying
        // it mid-window would leave the accumulator holding half of one
        // envelope and half of another.
        if let Some((mode, grain)) = self.pending.take() {
            self.apply_geometry(mode, grain);
        }

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

        let chosen = if self.first_frame || !searches(self.mode) {
            // `Grain` never searches: the splice lands wherever the analysis
            // pointer says, which is what makes the repetition periodic and
            // the rattle pitched. On the first frame there is also nothing to
            // continue from, and searching would only move the very first
            // frame of playback away from where the caller asked to start.
            nominal
        } else {
            // What the previous segment was about to become, had it kept
            // playing. The best candidate is the one that continues this.
            let nat_start = self.prev_chosen + hop as i64;
            if !self.plan_fits(frames, region, nominal, nat_start) {
                self.begin_plan(frames, region, nominal, nat_start);
            }
            // Whatever the drained hop left unpaid, or the whole search.
            self.advance_plan(frames, region, usize::MAX);
            nominal - search + self.plan.best_offset as i64
        };
        self.plan.live = false;

        // Lay the window down into the accumulator ring. The first hop skips
        // the rising half so a one-shot's initial transient is played at full
        // amplitude rather than faded in from nothing.
        let (lo, hi) = region.plain_span(frames.len());
        let plain = chosen >= lo && chosen + window as i64 <= hi;
        for offset in 0..window {
            let weight = if self.first_frame && offset < overlap {
                1.0
            } else {
                self.window_weight(offset, window)
            };
            let frame = if plain {
                frames[chosen as usize + offset]
            } else {
                Self::frame_at(frames, region, chosen + offset as i64)
            };
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

        // Begin the next hop's search now, from what the next boundary will
        // compute: the same wrap of the same pointer in the same region. A
        // queued geometry change will change the search, so it waits.
        if self.spread && searches(self.mode) && self.pending.is_none() {
            let mut pos = self.analysis_pos;
            let mut prev = self.prev_chosen;
            if let Some(span) = region_span(region) {
                while pos >= region.end {
                    pos -= span;
                    prev -= span as i64;
                }
            }
            self.begin_plan(frames, region, pos.round() as i64, prev + self.hop as i64);
        }
    }

    /// How well the candidate at `candidate` in `search_buf` continues the
    /// previous segment: its correlation with `nat` over its leading
    /// `overlap` frames.
    ///
    /// Normalized by the candidate's own energy but not by `nat`'s, since
    /// `nat` is fixed across the scan and cannot change the argmax. Without
    /// the candidate normalization the search would simply pick the loudest
    /// nearby moment rather than the best-matching one.
    fn score(&self, candidate: usize) -> f32 {
        let step = self.corr_decim.max(1);
        let mut correlation = 0.0f32;
        let mut energy = 1.0e-9f32;
        let mut offset = 0;
        while offset < self.overlap {
            let value = self.search_buf[candidate + offset];
            correlation += value * self.nat[offset];
            energy += value * value;
            offset += step;
        }
        correlation / energy.sqrt()
    }

    /// [`Stretcher::score`] for `LANES` neighbouring candidates at once: per
    /// lane the same products summed in the same order, so the same bits.
    fn score_lanes(&self, first: usize) -> [f32; LANES] {
        let step = self.corr_decim.max(1);
        let mut correlation = [0.0f32; LANES];
        let mut energy = [1.0e-9f32; LANES];
        let mut offset = 0;
        while offset < self.overlap {
            let at = first + offset;
            let values: &[f32; LANES] = self.search_buf[at..at + LANES]
                .try_into()
                .expect("a slice of LANES");
            let natural = self.nat[offset];
            for lane in 0..LANES {
                correlation[lane] += values[lane] * natural;
                energy[lane] += values[lane] * values[lane];
            }
            offset += step;
        }
        let mut scores = [0.0f32; LANES];
        for lane in 0..LANES {
            scores[lane] = correlation[lane] / energy[lane].sqrt();
        }
        scores
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
        crate::interpolate::RegionEdge::Wrap | crate::interpolate::RegionEdge::Crossfade { .. } => {
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

/// Periodic Hann prototype, one period across `HANN_TABLE` points plus a
/// right neighbour so the interpolated read always has one. COLA at 50%
/// overlap, which is why the overlap-add needs no synthesis window and no
/// normalization pass.
fn hann_table() -> Vec<f32> {
    (0..=HANN_TABLE + 1)
        .map(|index| {
            let phase = core::f32::consts::TAU * index as f32 / HANN_TABLE as f32;
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}

/// Frames of stretched output held at once. Only ever a sliding window on a
/// stream that is produced and consumed strictly forwards, so it is small --
/// it exists to give the resampling kernel material on both sides of its read
/// position, not to buffer anything.
const SCRATCH: usize = 256;

/// Valid stretched material kept on each side of the read position.
///
/// Derived from [`MAX_HALF_TAPS`] rather than written as a number, because
/// the number is only correct as long as the kernel's reach does not change.
/// Double it, so the window still slides in useful strides instead of
/// re-shifting on nearly every frame.
const MARGIN: i64 = MAX_HALF_TAPS as i64 * 2;

/// How far the window slides when the read position runs out of room. One
/// `copy_within` per `SHIFT` output frames at unity, so about one frame
/// copied per frame rendered.
const SHIFT: usize = MARGIN as usize;

/// Time stretching and transposition composed: [`Stretcher`] produces at the
/// source's own pitch, and the band-limited reader from [`crate::interpolate`]
/// transposes its output.
///
/// The two stages are deliberately separate. Stretch ratio and playback rate
/// are different musical ideas -- #20's product rules keep pitch shifting,
/// time stretching, tempo fit and slicing distinct -- and fusing them would
/// have run the similarity search on untransposed material while the output
/// was transposed, which is not something the spike measured. Kept separate,
/// each is exactly what it was measured as.
///
/// Composing them is also how pitch shift falls out for free: set the ratio
/// and the rate to the same number and duration returns to the original while
/// the pitch moves. The spike listed that as plausible but never measured it;
/// here it is a test.
///
/// **Latency is still zero.** Read position 0 is stretched frame 0, which is
/// input frame `start`. The kernel reaches backwards into frames that predate
/// the start and finds silence there, exactly as a one-shot's opening does
/// today. Nothing has to fill before output begins.
pub struct StretchReader {
    stretcher: Stretcher,
    /// A window on the stretched stream. `scratch[0]` is stretched frame
    /// `base`.
    scratch: Vec<[f32; 2]>,
    base: i64,
    /// Next stretched frame the stretcher has yet to hand over.
    produced: i64,
    /// Fractional read position in the stretched stream.
    pos: f64,
    /// Where in the *source* the frame being handed out right now came from.
    ///
    /// Not the same as the stretcher's analysis pointer, which is the
    /// production frontier: it runs ahead by up to a whole hop plus the
    /// scratch fill, because a hop is computed before any of it is consumed.
    /// Using the frontier as a playhead puts the cursor ahead of what is
    /// audible, and -- worse -- using it for end-of-region detection ends a
    /// one-shot early and drops its tail. So this integrates at *consumption*
    /// time instead: each output frame eats `rate` stretched frames, and each
    /// stretched frame is `1 / ratio` of a source frame.
    source_pos: f64,
}

impl StretchReader {
    pub fn new(mode: StretchMode, sample_rate: u32) -> Self {
        let mut reader = Self {
            stretcher: Stretcher::new(mode, sample_rate),
            scratch: vec![[0.0; 2]; SCRATCH],
            base: 0,
            produced: 0,
            pos: 0.0,
            source_pos: 0.0,
        };
        reader.reset(0.0);
        reader
    }

    pub fn stretcher(&self) -> &Stretcher {
        &self.stretcher
    }

    pub fn stretcher_mut(&mut self) -> &mut Stretcher {
        &mut self.stretcher
    }

    /// Zero, and for the same reason the stretcher's is. See the type docs.
    pub fn latency_frames(&self) -> usize {
        0
    }

    pub fn state_bytes(&self) -> usize {
        self.stretcher.state_bytes() + self.scratch.capacity() * 8
    }

    /// The stretcher's production frontier. Ahead of what is sounding; use
    /// [`Self::source_pos`] for anything the listener or the user sees.
    pub fn analysis_pos(&self) -> f64 {
        self.stretcher.analysis_pos()
    }

    /// Where in the source the frame just handed out came from. This is the
    /// playhead, and it is what end-of-region detection must compare.
    pub fn source_pos(&self) -> f64 {
        self.source_pos
    }

    /// Restart at an absolute input frame. Allocation- and drop-free.
    ///
    /// The window is placed so the read position starts `MARGIN` frames into
    /// it, leaving the kernel room to reach backwards into the zeroed frames
    /// that precede the start.
    pub fn reset(&mut self, start_frame: f64) {
        self.stretcher.reset(start_frame);
        for frame in self.scratch.iter_mut() {
            *frame = [0.0, 0.0];
        }
        self.base = -MARGIN;
        self.produced = 0;
        self.pos = 0.0;
        self.source_pos = start_frame;
    }

    /// Produce one output frame at `rate`, where 1.0 is the source's own
    /// pitch and 2.0 is an octave up.
    pub fn read(&mut self, frames: &[[f32; 2]], region: Region, rate: f64) -> [f32; 2] {
        if frames.is_empty() || !rate.is_finite() || rate <= 0.0 {
            // Reverse under stretch is not supported and the UI disables it;
            // producing silence is better than letting a negative rate walk
            // the window backwards past material already discarded.
            return [0.0, 0.0];
        }
        self.ensure(self.pos.ceil() as i64 + MARGIN, frames, region);
        let local = self.pos - self.base as f64;
        let frame = SincTable::shared().read(
            &self.scratch,
            local,
            rate,
            Region::whole(SCRATCH),
        );
        self.pos += rate;
        self.source_pos += rate / self.stretcher.ratio();
        if let Some(span) = region_span(region) {
            while self.source_pos >= region.end {
                self.source_pos -= span;
            }
        }
        frame
    }

    /// Slide and refill the window so stretched frames up to `upto` are valid.
    fn ensure(&mut self, upto: i64, frames: &[[f32; 2]], region: Region) {
        while upto >= self.base + SCRATCH as i64 {
            self.scratch.copy_within(SHIFT.., 0);
            for slot in self.scratch[SCRATCH - SHIFT..].iter_mut() {
                *slot = [0.0, 0.0];
            }
            self.base += SHIFT as i64;
        }
        // Only as far as the read needs, not to the end of the window: the
        // kernel never looks past `upto`, and filling the whole window at a
        // note-on paid for half a hop's splice search in the first block
        // (MOO-248). Which frames are produced is unchanged, only when.
        while self.produced <= upto {
            let frame = self.stretcher.next_frame(frames, region);
            let slot = self.produced - self.base;
            // Frames that fell behind the window as it slid are simply
            // dropped: the read position never goes backwards.
            if (0..SCRATCH as i64).contains(&slot) {
                self.scratch[slot as usize] = frame;
            }
            self.produced += 1;
        }
    }
}

/// Every voice's stretch state for one sampler, allocated as a unit.
///
/// This lives on the *device*, not the voice, and it is why: a `StretchReader`
/// is about 100 KB, `RenderState` builds a `ChannelStrip` for all 256
/// addressable channels at startup, and each of those eagerly constructs a
/// `Sampler` with 16 voices. A reader per voice would therefore allocate
/// roughly 390 MiB before a project is even loaded, almost all of it for
/// channels that hold nothing. Voices are small and must stay small.
///
/// So the pool is `None` until a sampler actually stretches, it is built on
/// the control thread, and it is handed to the realtime thread already
/// allocated -- the same ownership round trip installed effect nodes make.
/// Turning stretch on is consequently a *structural* change rather than a
/// parameter: `Sampler::set_params` runs on the realtime command drain and
/// cannot allocate. Ratio, mode, and grain size stay ordinary parameters and
/// remain modulatable; only the existence of the state crosses threads.
pub struct StretchPool {
    readers: Box<[StretchReader]>,
}

impl StretchPool {
    /// Build a pool covering `voices` voices. Control thread only -- this is
    /// the allocation the whole design exists to keep off the audio thread.
    pub fn new(mode: StretchMode, sample_rate: u32, voices: usize) -> Self {
        Self {
            readers: (0..voices)
                .map(|_| StretchReader::new(mode, sample_rate))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.readers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.readers.is_empty()
    }

    pub fn reader_mut(&mut self, voice: usize) -> Option<&mut StretchReader> {
        self.readers.get_mut(voice)
    }

    pub fn readers_mut(&mut self) -> impl Iterator<Item = &mut StretchReader> {
        self.readers.iter_mut()
    }

    /// Exchange two voices' readers, for a sampler moving a voice between
    /// slots. A swap in place: realtime-safe. Out of range is a no-op.
    pub fn swap_readers(&mut self, a: usize, b: usize) {
        if a < self.readers.len() && b < self.readers.len() {
            self.readers.swap(a, b);
        }
    }

    /// Take over `displaced`'s readers for every voice both pools cover,
    /// giving it this pool's fresh ones in exchange (MOO-7).
    ///
    /// A resize arrives while voices are sounding, and a fresh reader under a
    /// sounding voice has no history: the voice would jump. Swapping the
    /// structs moves their heap pointers and nothing else, so this is
    /// realtime-safe, and `displaced` still leaves holding a pool's worth of
    /// readers for the control thread to drop.
    pub fn adopt_readers(&mut self, displaced: &mut StretchPool) {
        for (mine, theirs) in self.readers.iter_mut().zip(displaced.readers.iter_mut()) {
            std::mem::swap(mine, theirs);
        }
    }

    /// Total heap held, so the memory cost of enabling stretch on a sampler
    /// is a number someone can look up rather than estimate.
    pub fn state_bytes(&self) -> usize {
        self.readers.iter().map(StretchReader::state_bytes).sum()
    }
}

/// One output frame in every this many is recorded in a
/// [`StretchRender::trace`].
///
/// 256 keeps an 8-second loop's trace to about 1,500 entries while holding
/// the interpolation error far below anything audible: the analysis pointer
/// is fractional and advances at a fixed rate within a span, so a
/// piecewise-linear read inside 256 frames is a fraction of a frame out.
pub const TRACE_INTERVAL: usize = 256;

/// A stretched region, frozen. What "commit the stretch to the buffer" means.
pub struct StretchRender {
    pub frames: Vec<[f32; 2]>,
    /// A coarse, monotonic map from source frame to output frame, sampled
    /// every [`TRACE_INTERVAL`] output frames, with a final entry at the end
    /// of the render so the whole range is interpolable.
    ///
    /// This is what carries slice markers and region fractions across the
    /// commit. Mapping them by the nominal ratio instead would be out by up
    /// to one search window -- about 10.7 ms, an audible flam on a break.
    /// The trace is used at commit time and thrown away; a project stores the
    /// render *spec*, never the trace and never the audio.
    pub trace: Vec<(u32, u32)>,
}

impl StretchRender {
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Where a source frame ended up in the render, in output frames.
    ///
    /// Linear inside a trace span, and clamped to the render at both ends: a
    /// marker outside the region that was rendered has no position inside it,
    /// and the nearest edge is the only honest answer.
    pub fn output_frame_of(&self, source_frame: f64) -> f64 {
        let Some(first) = self.trace.first() else {
            return 0.0;
        };
        if source_frame <= f64::from(first.0) {
            return f64::from(first.1);
        }
        let last = self.trace.last().copied().unwrap_or(*first);
        if source_frame >= f64::from(last.0) {
            return f64::from(last.1);
        }
        let at = self
            .trace
            .partition_point(|(source, _)| f64::from(*source) <= source_frame);
        let (low_source, low_output) = self.trace[at - 1];
        let (high_source, high_output) = self.trace[at];
        let span = f64::from(high_source) - f64::from(low_source);
        if span <= 0.0 {
            return f64::from(low_output);
        }
        let fraction = (source_frame - f64::from(low_source)) / span;
        f64::from(low_output) + fraction * (f64::from(high_output) - f64::from(low_output))
    }
}

/// Render a region of `source` through the stretcher, once, off the realtime
/// thread.
///
/// This is a render, not a new engine: it drives the same
/// [`Stretcher::next_frame`] a sounding voice drives, from a plain loop. That
/// is the whole reason committing a stretch is cheap to build and cheap to
/// trust -- there is no second stretching implementation to keep in agreement
/// with the first.
///
/// The length is decided by the output count, `round(region_len * ratio)`,
/// rather than by watching the analysis pointer reach the region end. That
/// makes the result reproducible from the spec alone, which is what lets a
/// project store six numbers instead of the audio.
pub fn render_stretched(
    source: &[[f32; 2]],
    region: Region,
    mode: StretchMode,
    grain: u32,
    ratio: f64,
    sample_rate: u32,
) -> StretchRender {
    let region_len = region.end - region.start;
    let ratio = if ratio.is_finite() {
        ratio.clamp(MIN_RATIO, MAX_RATIO)
    } else {
        1.0
    };
    if source.is_empty() || !region_len.is_finite() || region_len <= 0.0 {
        return StretchRender {
            frames: Vec::new(),
            trace: Vec::new(),
        };
    }
    let out_len = (region_len * ratio).round().max(1.0) as usize;

    // Driven through a `StretchReader` at unity rate rather than through the
    // bare `Stretcher`, for the trace's sake: the reader reports where the
    // frame it just handed out *came from*, while the stretcher only exposes
    // its production frontier, which runs ahead by up to a hop. Tracing the
    // frontier put every marker several milliseconds early. This way the
    // commit maps markers through the same playhead a sounding voice
    // reports, so the two cannot disagree.
    let mut reader = StretchReader::new(mode, sample_rate);
    {
        let stretcher = reader.stretcher_mut();
        stretcher.set_mode(mode);
        stretcher.set_grain_frames(grain);
        stretcher.set_ratio(ratio);
    }
    reader.reset(region.start);

    let mut frames = Vec::with_capacity(out_len);
    let mut trace = Vec::with_capacity(out_len / TRACE_INTERVAL + 2);
    for index in 0..out_len {
        if index % TRACE_INTERVAL == 0 {
            trace.push((reader.source_pos().max(0.0) as u32, index as u32));
        }
        frames.push(reader.read(source, region, 1.0));
    }
    // A closing entry so the last partial span is interpolable rather than
    // clamped: without it every marker past the final sampled point would
    // collapse onto it.
    trace.push((reader.source_pos().max(0.0) as u32, out_len as u32));

    StretchRender { frames, trace }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpolate::RegionEdge;

    const SR: u32 = 48_000;

    fn tone(len: usize, hz: f64) -> Vec<[f32; 2]> {
        (0..len)
            .map(|index| {
                let phase = core::f64::consts::TAU * hz * index as f64 / SR as f64;
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

    /// Fraction of the output's energy still sitting in `hz`, by Goertzel.
    /// 1.0 is a pure tone; splice discontinuities scatter energy into
    /// sidebands and drive it down.
    ///
    /// This replaced a block-RMS ripple measure that looked reasonable and
    /// measured nothing: at 64 frames a block spans under a third of a 220 Hz
    /// period, so it read the tone's own phase rather than any artifact, and
    /// scored the two modes within 7% of each other.
    fn tonal_purity(out: &[[f32; 2]], hz: f64) -> f32 {
        let omega = core::f64::consts::TAU * hz / SR as f64;
        let coeff = 2.0 * omega.cos();
        let (mut previous, mut older) = (0.0f64, 0.0f64);
        let mut energy = 0.0f64;
        for frame in out {
            let sample = frame[0] as f64;
            let current = sample + coeff * previous - older;
            older = previous;
            previous = current;
            energy += sample * sample;
        }
        let power = previous * previous + older * older - coeff * previous * older;
        let n = out.len() as f64;
        (power / (energy * n / 2.0).max(1.0e-12)) as f32
    }

    /// The sizing rule from the spike, as an executable claim: the default
    /// window is at least 1.2 periods of A1, and the percussion window is
    /// deliberately not.
    #[test]
    fn the_music_window_spans_a_low_fundamental_and_the_drum_window_does_not() {
        let a1_period = SR as f64 / 55.0;
        let music = window_frames(StretchMode::Music, SR, 0) as f64;
        let drums = window_frames(StretchMode::Drums, SR, 0) as f64;
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
    fn the_transparent_windows_are_1024_and_512_at_48k() {
        assert_eq!(window_frames(StretchMode::Music, SR, 0), 1024);
        assert_eq!(window_frames(StretchMode::Drums, SR, 0), 512);
    }

    /// Every window must be even, or the Hann stops summing to unity across
    /// the hop. Grain sizes are user-chosen, so odd requests have to be
    /// rounded rather than trusted.
    #[test]
    fn every_window_is_even_whatever_is_asked_for() {
        for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
            for mode in [StretchMode::Music, StretchMode::Drums] {
                assert_eq!(window_frames(mode, rate, 0) % 2, 0);
            }
        }
        for grain in [0, 1, 63, 65, 127, 333, 1023, 4095, 99_999] {
            let window = window_frames(StretchMode::Grain, SR, grain);
            assert_eq!(window % 2, 0, "grain {grain} gave window {window}");
            assert!((GRAIN_MIN_FRAMES as usize..=GRAIN_MAX_FRAMES as usize + 1)
                .contains(&window));
        }
    }

    /// The COLA property the overlap-add depends on, checked through the
    /// interpolated table read rather than an ideal Hann, since the table is
    /// what the overlap-add actually uses.
    #[test]
    fn the_interpolated_window_sums_to_unity_across_the_hop() {
        for window in [128usize, 512, 1024, 1366, 4096] {
            let stretcher = Stretcher::new(StretchMode::Music, SR);
            let hop = window / 2;
            for offset in 0..hop {
                let sum = stretcher.window_weight(offset, window)
                    + stretcher.window_weight(offset + hop, window);
                assert!(
                    (sum - 1.0).abs() < 1.0e-4,
                    "window {window} offset {offset} summed to {sum}"
                );
            }
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

        let mut one_shot = Stretcher::new(StretchMode::Music, SR);
        one_shot.set_ratio(1.37);
        one_shot.reset(0.0);
        let reference = render(&mut one_shot, &source, region, count);

        for block in [1usize, 32, 64, 128, 480, 512, 1024] {
            let mut blocked = Stretcher::new(StretchMode::Music, SR);
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

    /// A stretched sustained tone must keep its pitch in the transparent
    /// modes. Measured as zero crossings, which is crude but entirely
    /// sufficient to catch the failure this test exists for -- a window too
    /// short for the fundamental locks onto the wrong period and the pitch
    /// moves by hundreds of cents.
    #[test]
    fn a_sustained_tone_keeps_its_pitch_when_stretched() {
        let hz = 220.0;
        let source = tone(96_000, hz);
        let region = Region::whole(source.len());

        for ratio in [0.75, 1.25, 1.5] {
            let mut stretcher = Stretcher::new(StretchMode::Music, SR);
            stretcher.set_ratio(ratio);
            stretcher.reset(0.0);
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

    /// The reason `Grain` exists, stated as a measurement. With the search
    /// on, splices are phase-coherent and a stretched tone stays a tone. With
    /// it off, every splice is a phase discontinuity at the hop rate, and the
    /// energy scatters into sidebands -- the buzz.
    ///
    /// This is the mechanism claim from the module header, and it is the only
    /// part of the artifact that is measurable here. Whether it sounds like
    /// the intended record is a listening question this cannot answer.
    #[test]
    fn grain_scatters_a_tone_that_the_searching_modes_keep_intact() {
        let source = tone(200_000, 220.0);
        let region = Region::whole(source.len());
        let ratio = 8.0;

        let mut grain = Stretcher::new(StretchMode::Grain, SR);
        grain.set_ratio(ratio);
        grain.reset(0.0);
        let grainy = render(&mut grain, &source, region, 48_000);

        let mut music = Stretcher::new(StretchMode::Music, SR);
        music.set_ratio(ratio);
        music.reset(0.0);
        let searched = render(&mut music, &source, region, 48_000);

        let grain_purity = tonal_purity(&grainy[4_096..], 220.0);
        let music_purity = tonal_purity(&searched[4_096..], 220.0);
        assert!(
            music_purity > 0.8,
            "the searching mode should keep a stretched tone intact, \
             got {music_purity:.3}"
        );
        assert!(
            grain_purity < music_purity * 0.6,
            "grain should scatter the tone: \
             grain {grain_purity:.3} vs music {music_purity:.3}"
        );
    }

    /// The grain window is a pitch control, and this is the mapping: the
    /// repetition sits at `sample_rate / hop`, so halving the window doubles
    /// the rattle.
    #[test]
    fn the_grain_window_sets_the_rattle_frequency() {
        let mut stretcher = Stretcher::new(StretchMode::Grain, SR);
        stretcher.set_grain_frames(1024);
        stretcher.reset(0.0);
        assert!((stretcher.rattle_hz() - 93.75).abs() < 0.01);

        stretcher.set_grain_frames(256);
        stretcher.reset(0.0);
        assert!((stretcher.rattle_hz() - 375.0).abs() < 0.01);

        stretcher.set_grain_frames(128);
        stretcher.reset(0.0);
        assert!((stretcher.rattle_hz() - 750.0).abs() < 0.01);
    }

    /// The grain window is meant to be swept while sound is coming out. A
    /// change lands at a hop boundary rather than mid-window, and must not
    /// produce anything non-finite on the way.
    #[test]
    fn sweeping_the_grain_window_mid_render_stays_finite_and_takes_effect() {
        let source = tone(200_000, 220.0);
        let region = Region::whole(source.len());

        let mut stretcher = Stretcher::new(StretchMode::Grain, SR);
        stretcher.set_ratio(6.0);
        stretcher.reset(0.0);

        let mut out = Vec::new();
        for step in 0..64 {
            // Sweep from a low rattle to a high one across the render.
            let frames = 2048 - step * 30;
            stretcher.set_grain_frames(frames.max(64) as u32);
            out.extend(render(&mut stretcher, &source, region, 512));
        }
        assert!(out.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
        assert!(stretcher.window() < 1024, "the sweep should have taken effect");

        let rms =
            (out.iter().map(|f| f[0] * f[0]).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.05, "sweep went silent: rms {rms}");
    }

    /// Both controls queue into the same slot, so setting one must not throw
    /// away an unlanded change to the other. Setting the mode used to read
    /// the *active* grain size and re-queue it, discarding a grain sweep that
    /// had not reached a hop boundary yet.
    #[test]
    fn queueing_a_mode_change_does_not_discard_a_queued_grain_change() {
        let source = tone(20_000, 220.0);
        let region = Region::whole(source.len());

        let mut stretcher = Stretcher::new(StretchMode::Music, SR);
        stretcher.reset(0.0);
        stretcher.set_grain_frames(256);
        stretcher.set_mode(StretchMode::Grain);
        // Nothing has been pulled yet, so neither change has landed.
        assert_eq!(stretcher.window(), 1024);

        render(&mut stretcher, &source, region, 1);
        assert_eq!(stretcher.mode(), StretchMode::Grain);
        assert_eq!(
            stretcher.grain_frames(),
            256,
            "the grain change was discarded by the mode change"
        );
        assert_eq!(stretcher.window(), 256);
    }

    /// Switching between transparency and rattle is a performance gesture, so
    /// it has to survive being done mid-note.
    #[test]
    fn switching_mode_mid_render_stays_finite() {
        let source = tone(200_000, 220.0);
        let region = Region::whole(source.len());

        let mut stretcher = Stretcher::new(StretchMode::Music, SR);
        stretcher.set_ratio(4.0);
        stretcher.reset(0.0);

        let mut out = Vec::new();
        for step in 0..16 {
            stretcher.set_mode(if step % 2 == 0 {
                StretchMode::Grain
            } else {
                StretchMode::Music
            });
            out.extend(render(&mut stretcher, &source, region, 2_000));
        }
        assert!(out.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
    }

    /// Extreme slow-down is a destination, not a failure. It must keep
    /// producing sound rather than starving, and cost nothing extra -- the
    /// analysis pointer crawls but the output hop rate does not change.
    #[test]
    fn extreme_slow_down_keeps_producing() {
        let source = tone(200_000, 110.0);
        let region = Region::whole(source.len());

        let mut stretcher = Stretcher::new(StretchMode::Grain, SR);
        stretcher.set_ratio(MAX_RATIO);
        stretcher.reset(0.0);
        let out = render(&mut stretcher, &source, region, 96_000);

        let rms =
            (out.iter().map(|f| f[0] * f[0]).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.1, "16x slow-down went quiet: rms {rms}");
        // 96k output frames at ratio 16 should have consumed only ~6k input.
        assert!(
            stretcher.analysis_pos() < 8_000.0,
            "pointer ran to {} at ratio 16",
            stretcher.analysis_pos()
        );
    }

    /// Duration is exact by construction, and this is the property that makes
    /// it so: the analysis pointer is fractional, so over thousands of hops
    /// it lands where arithmetic says it should rather than drifting.
    #[test]
    fn the_analysis_pointer_advances_without_accumulating_error() {
        let source = tone(400_000, 110.0);
        let region = Region::whole(source.len());
        let ratio = 1.37;

        let mut stretcher = Stretcher::new(StretchMode::Music, SR);
        stretcher.set_ratio(ratio);
        stretcher.reset(0.0);
        let produced = 200_000;
        render(&mut stretcher, &source, region, produced);

        let expected = produced as f64 / ratio;
        let hop = stretcher.window() as f64 / 2.0;
        let drift = stretcher.analysis_pos() - expected;
        assert!(
            (-1.0..=hop + 1.0).contains(&drift),
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

        let mut stretcher = Stretcher::new(StretchMode::Music, SR);
        stretcher.set_ratio(1.25);
        stretcher.reset(0.0);
        let out = render(&mut stretcher, &source, region, 60_000);

        let tail = &out[50_000..];
        let rms =
            (tail.iter().map(|f| f[0] * f[0]).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 0.3, "loop went quiet after several passes: tail rms {rms}");
    }

    /// Ratios are clamped rather than trusted. A modulated or automated ratio
    /// can arrive at any value, including one that would send the analysis
    /// pointer somewhere meaningless.
    #[test]
    fn an_out_of_range_or_nonfinite_ratio_cannot_take_hold() {
        let mut stretcher = Stretcher::new(StretchMode::Music, SR);
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

        let mut stretcher = Stretcher::new(StretchMode::Music, SR);
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

    /// Buffers are sized to the worst case so mode and grain changes never
    /// allocate. This pins the cost of that decision, which is what the
    /// polyphony budget has to be rewritten against.
    #[test]
    fn the_per_voice_footprint_covers_the_largest_window() {
        let music = Stretcher::new(StretchMode::Music, SR);
        let grain = Stretcher::new(StretchMode::Grain, SR);
        // Same allocation either way: the mode is a live control.
        assert_eq!(music.state_bytes(), grain.state_bytes());
        assert!(
            music.state_bytes() < 128 * 1024,
            "footprint {} exceeds the budget headroom",
            music.state_bytes()
        );
    }

    /// Zero latency is contract, not an implementation detail: the node layer
    /// reports it and nothing downstream compensates for it.
    #[test]
    fn the_stretcher_declares_no_latency() {
        let stretcher = Stretcher::new(StretchMode::Music, SR);
        assert_eq!(stretcher.latency_frames(), 0);
        assert_eq!(stretcher.lookahead_frames(), 1_536);
    }

    /// Pitch of a rendered buffer, by zero crossings. Crude, and entirely
    /// sufficient for claims measured in semitones rather than cents.
    fn measured_hz(out: &[[f32; 2]]) -> f64 {
        let crossings = out
            .windows(2)
            .filter(|pair| pair[0][0] <= 0.0 && pair[1][0] > 0.0)
            .count();
        crossings as f64 * SR as f64 / out.len() as f64
    }

    fn read_all(
        reader: &mut StretchReader,
        frames: &[[f32; 2]],
        region: Region,
        rate: f64,
        count: usize,
    ) -> Vec<[f32; 2]> {
        (0..count).map(|_| reader.read(frames, region, rate)).collect()
    }

    /// With both stages at unity the composition must be transparent -- if it
    /// is not, everything measured about either stage separately is moot.
    #[test]
    fn the_composed_reader_is_transparent_at_unity() {
        let source = tone(40_000, 220.0);
        let region = Region::whole(source.len());

        let mut reader = StretchReader::new(StretchMode::Music, SR);
        reader.reset(0.0);
        let out = read_all(&mut reader, &source, region, 1.0, 20_000);

        let error: f32 = out[2_048..]
            .iter()
            .zip(source[2_048..].iter())
            .map(|(played, expected)| (played[0] - expected[0]).abs())
            .sum::<f32>()
            / (out.len() - 2_048) as f32;
        assert!(error < 0.08, "mean absolute error at unity was {error}");
    }

    /// Stretching alone changes duration and leaves pitch where it was.
    #[test]
    fn stretching_without_transposing_holds_the_pitch() {
        let hz = 220.0;
        let source = tone(200_000, hz);
        let region = Region::whole(source.len());

        let mut reader = StretchReader::new(StretchMode::Music, SR);
        reader.stretcher_mut().set_ratio(1.5);
        reader.reset(0.0);
        let out = read_all(&mut reader, &source, region, 1.0, 48_000);

        let cents = 1200.0 * (measured_hz(&out[4_096..]) / hz).log2();
        assert!(cents.abs() < 25.0, "stretch moved the pitch by {cents:.1} cents");
    }

    /// Transposing alone moves the pitch and leaves the stretcher at unity.
    #[test]
    fn transposing_without_stretching_moves_the_pitch() {
        let hz = 220.0;
        let source = tone(200_000, hz);
        let region = Region::whole(source.len());

        let mut reader = StretchReader::new(StretchMode::Music, SR);
        reader.reset(0.0);
        let out = read_all(&mut reader, &source, region, 2.0, 40_000);

        let cents = 1200.0 * (measured_hz(&out[4_096..]) / (hz * 2.0)).log2();
        assert!(
            cents.abs() < 25.0,
            "an octave up landed {cents:.1} cents off"
        );
    }

    /// The composition's payoff, and the thing the spike called plausible but
    /// never measured: equal ratio and rate is a pitch shift at the original
    /// duration. The stretcher makes the stream `n` times longer, the reader
    /// consumes it `n` times faster, and what is left is the transposition.
    #[test]
    fn equal_ratio_and_rate_is_a_pitch_shift_at_the_original_duration() {
        let hz = 220.0;
        let source = tone(200_000, hz);
        let region = Region::whole(source.len());

        for shift in [1.5, 2.0] {
            let mut reader = StretchReader::new(StretchMode::Music, SR);
            reader.stretcher_mut().set_ratio(shift);
            reader.reset(0.0);
            let produced = 40_000;
            let out = read_all(&mut reader, &source, region, shift, produced);

            let cents = 1200.0 * (measured_hz(&out[4_096..]) / (hz * shift)).log2();
            assert!(
                cents.abs() < 30.0,
                "shift {shift} landed {cents:.1} cents off"
            );

            // Duration: the source is consumed at its own rate, so `produced`
            // output frames should have eaten about `produced` input frames.
            let consumed = reader.analysis_pos();
            let drift = (consumed - produced as f64).abs() / produced as f64;
            assert!(
                drift < 0.05,
                "shift {shift} consumed {consumed} input for {produced} output"
            );
        }
    }

    /// The window slides under the read position as it advances. At a high
    /// rate it slides several times per frame, and the kernel must never see
    /// the seam.
    #[test]
    fn the_scratch_window_slides_without_seams_at_any_rate() {
        let source = tone(400_000, 110.0);
        let region = Region::whole(source.len());

        for rate in [0.25, 1.0, 2.0, 4.0, 11.7] {
            let mut reader = StretchReader::new(StretchMode::Music, SR);
            reader.reset(0.0);
            let out = read_all(&mut reader, &source, region, rate, 20_000);
            assert!(
                out.iter().all(|f| f[0].is_finite() && f[1].is_finite()),
                "rate {rate} produced a non-finite frame"
            );
            let rms =
                (out.iter().map(|f| f[0] * f[0]).sum::<f32>() / out.len() as f32).sqrt();
            assert!(rms > 0.2, "rate {rate} went quiet: rms {rms}");
        }
    }

    /// Same determinism requirement as the stretcher, now through both
    /// stages: an offline render and the realtime path must agree bit for
    /// bit whatever block size the executor picked.
    #[test]
    fn the_composed_output_is_identical_however_it_is_grouped() {
        let source = tone(60_000, 220.0);
        let region = Region::whole(source.len());
        let count = 8_000;

        let mut one_shot = StretchReader::new(StretchMode::Music, SR);
        one_shot.stretcher_mut().set_ratio(1.37);
        one_shot.reset(0.0);
        let reference = read_all(&mut one_shot, &source, region, 1.19, count);

        for block in [1usize, 32, 128, 480, 1024] {
            let mut blocked = StretchReader::new(StretchMode::Music, SR);
            blocked.stretcher_mut().set_ratio(1.37);
            blocked.reset(0.0);
            let mut produced = Vec::with_capacity(count);
            while produced.len() < count {
                let take = block.min(count - produced.len());
                produced.extend(read_all(&mut blocked, &source, region, 1.19, take));
            }
            assert_eq!(produced, reference, "block {block} diverged");
        }
    }

    /// Reverse under stretch is out of scope and the UI disables it. If a
    /// negative rate arrives anyway it must produce silence rather than walk
    /// the window backwards past material already discarded.
    #[test]
    fn a_reverse_or_nonfinite_rate_produces_silence_rather_than_nonsense() {
        let source = tone(40_000, 220.0);
        let region = Region::whole(source.len());

        let mut reader = StretchReader::new(StretchMode::Music, SR);
        reader.reset(0.0);
        for rate in [-1.0, 0.0, f64::NAN] {
            let out = read_all(&mut reader, &source, region, rate, 128);
            assert!(
                out.iter().all(|frame| frame == &[0.0, 0.0]),
                "rate {rate} produced sound"
            );
        }
    }

    /// The grain mode has to survive the composition too -- it is the one
    /// people will run at extreme settings.
    #[test]
    fn grain_survives_extreme_slow_down_with_transposition() {
        let source = tone(200_000, 110.0);
        let region = Region::whole(source.len());

        let mut reader = StretchReader::new(StretchMode::Grain, SR);
        reader.stretcher_mut().set_ratio(12.0);
        reader.stretcher_mut().set_grain_frames(180);
        reader.reset(0.0);
        let out = read_all(&mut reader, &source, region, 3.0, 48_000);

        assert!(out.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
        let rms =
            (out.iter().map(|f| f[0] * f[0]).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms > 0.05, "extreme grain settings went silent: rms {rms}");
    }

    /// The reason the pool exists. A reader is large; a voice is not. If a
    /// reader ever ends up inside `Voice`, 256 channels x 16 voices of eager
    /// construction turns into hundreds of megabytes at startup for an empty
    /// project. This pins the per-reader cost so that regression is visible
    /// as a number rather than as a memory graph.
    #[test]
    fn a_reader_is_far_too_large_to_live_in_a_voice() {
        let reader = StretchReader::new(StretchMode::Music, SR);
        let bytes = reader.state_bytes();
        assert!(
            (96_000..=110_000).contains(&bytes),
            "reader footprint moved to {bytes}"
        );
        // What that would have cost per voice across the addressable channels.
        let naive = bytes * 256 * 16;
        assert!(
            naive > 300 * 1024 * 1024,
            "the eager-per-voice cost this design avoids is {naive}"
        );
    }

    /// A pool covers the sampler's voices and is built in one place, on the
    /// thread that is allowed to allocate.
    #[test]
    fn a_pool_covers_every_voice_it_was_asked_for() {
        let mut pool = StretchPool::new(StretchMode::Music, SR, 16);
        assert_eq!(pool.len(), 16);
        assert!(pool.reader_mut(15).is_some());
        assert!(pool.reader_mut(16).is_none());
        // ~1.6 MB for a sampler that is actually stretching, which is the
        // trade the device-level pool buys.
        assert!(pool.state_bytes() < 2 * 1024 * 1024);
    }

    /// An empty sample must not panic or read out of bounds -- a voice can be
    /// rendered in the window between a sample being cleared and the voice
    /// noticing.
    #[test]
    fn an_empty_sample_renders_silence() {
        let mut stretcher = Stretcher::new(StretchMode::Music, SR);
        stretcher.reset(0.0);
        let out = render(&mut stretcher, &[], Region::whole(0), 512);
        assert!(out.iter().all(|frame| frame == &[0.0, 0.0]));
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::interpolate::RegionEdge;

    const SR: u32 = 48_000;

    fn tone(len: usize, hz: f64) -> Vec<[f32; 2]> {
        (0..len)
            .map(|index| {
                let phase = core::f64::consts::TAU * hz * index as f64 / SR as f64;
                let value = phase.sin() as f32;
                [value, value]
            })
            .collect()
    }

    fn whole(len: usize) -> Region {
        Region {
            start: 0.0,
            end: len as f64,
            edge: RegionEdge::Silent,
        }
    }

    /// Committing at unity has to be a no-op on the audio, or "commit" would
    /// be a destructive edit disguised as a bookkeeping one. WSOLA is only
    /// *nearly* sample-exact at unity -- the search picks offset zero and the
    /// Hann pair is COLA at 50%, so the interior reproduces the source to
    /// well under a bit of 16-bit resolution.
    #[test]
    fn rendering_at_unity_reproduces_the_source_region() {
        let source = tone(20_000, 220.0);
        let render = render_stretched(&source, whole(source.len()), StretchMode::Music, 1024, 1.0, SR);

        assert_eq!(render.len(), source.len());
        // The first window has nothing to overlap with, exactly as a
        // one-shot's opening does today; measure past it.
        let worst = source[2_048..18_000]
            .iter()
            .zip(&render.frames[2_048..18_000])
            .map(|(want, got)| (want[0] - got[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1.0e-3, "unity render drifted from the source by {worst}");
    }

    /// The length is decided by the spec, not by watching the analysis
    /// pointer, so a project can store six numbers instead of the audio.
    #[test]
    fn a_render_has_exactly_the_output_length_the_ratio_asks_for() {
        let source = tone(10_000, 220.0);
        for ratio in [0.5, 1.0, 1.5, 2.0, 4.0] {
            let render =
                render_stretched(&source, whole(source.len()), StretchMode::Music, 1024, ratio, SR);
            let expected = (10_000.0 * ratio).round() as usize;
            assert_eq!(render.len(), expected, "ratio {ratio}");
        }

        // A sub-region is measured by the region, not by the sample.
        let region = Region {
            start: 2_000.0,
            end: 6_000.0,
            edge: RegionEdge::Silent,
        };
        let render = render_stretched(&source, region, StretchMode::Music, 1024, 2.0, SR);
        assert_eq!(render.len(), 8_000);
    }

    /// Reproducibility is the property that lets a project persist the spec
    /// rather than the rendered audio, so it is pinned rather than assumed.
    #[test]
    fn two_renders_from_the_same_spec_are_identical() {
        let source = tone(8_000, 180.0);
        let spec = |()| {
            render_stretched(&source, whole(source.len()), StretchMode::Drums, 512, 2.5, SR)
        };
        let first = spec(());
        let second = spec(());
        assert_eq!(first.frames, second.frames);
        assert_eq!(first.trace, second.trace);
    }

    /// What the trace is for: carrying a slice marker across the commit. A
    /// marker three quarters of the way through a region has to land three
    /// quarters of the way through the render, to well inside a millisecond
    /// -- a break's slices are what would flam otherwise.
    #[test]
    fn the_trace_maps_a_marker_back_to_within_a_millisecond() {
        let source = tone(40_000, 220.0);
        let ratio = 2.5;
        let render =
            render_stretched(&source, whole(source.len()), StretchMode::Music, 1024, ratio, SR);

        let millisecond = SR as f64 / 1_000.0;
        for fraction in [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
            let source_frame = 40_000.0 * fraction;
            let expected = source_frame * ratio;
            let mapped = render.output_frame_of(source_frame);
            assert!(
                (mapped - expected).abs() < millisecond,
                "a marker at {fraction} mapped to {mapped}, expected {expected}"
            );
        }

        // The trace is monotonic, which is what makes the interpolation above
        // meaningful at all.
        assert!(render
            .trace
            .windows(2)
            .all(|pair| pair[0].0 <= pair[1].0 && pair[0].1 < pair[1].1));
    }

    /// Nothing to render is not an error. An empty sample or an inverted
    /// region hands back an empty render rather than panicking or looping.
    #[test]
    fn a_degenerate_region_renders_nothing() {
        let source = tone(1_000, 220.0);
        assert!(render_stretched(&[], whole(0), StretchMode::Music, 1024, 2.0, SR).is_empty());
        let inverted = Region {
            start: 500.0,
            end: 100.0,
            edge: RegionEdge::Silent,
        };
        assert!(render_stretched(&source, inverted, StretchMode::Music, 1024, 2.0, SR).is_empty());
    }
}


/// The stretcher and its reader exactly as they were before MOO-248
/// (8e570c4f), kept so the tests can hold the new ones to them sample for
/// sample in the same process. Pinned hashes would not do: the Hann table
/// and any test tone come from the platform's `cos` and `sin`, which differ
/// in the last bit between x86-64 and aarch64.
#[cfg(test)]
#[allow(dead_code, unused_imports, clippy::all)]
mod before_moo248 {
    use super::{
        capacity_frames, hann_table, region_span, searches, window_frames, wrap_index,
        GRAIN_DEFAULT_FRAMES, GRAIN_MAX_FRAMES, GRAIN_MIN_FRAMES, HANN_TABLE, MARGIN, MAX_RATIO,
        MIN_RATIO, SCRATCH, SHIFT,
    };
    use crate::interpolate::{Region, SincTable};
    use mooloop_core::StretchMode;

    pub struct Stretcher {
        sample_rate: u32,
        mode: StretchMode,
        grain_frames: u32,
        /// Active geometry, re-derived only when mode or grain size changes, and
        /// only at a hop boundary.
        window: usize,
        hop: usize,
        overlap: usize,
        /// Half-width of the similarity search. Equal to the hop, which is what
        /// the spike measured; a wider search costs linearly and did not improve
        /// any metric. Zero in `Grain`, which is the mode's whole definition.
        search: usize,
        /// Decimation of the correlation sum. The search still visits every
        /// candidate offset — this only thins the inner product at each one.
        corr_decim: usize,
        /// Pending geometry, applied at the next hop. Changing the window
        /// mid-window would leave the accumulator holding half of one envelope
        /// and half of another, which clicks.
        pending: Option<(StretchMode, u32)>,
        hann: Vec<f32>,
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
        /// Mid-channel candidates for this hop's search, read once so the inner
        /// loop is a flat scan rather than `2 * search + overlap` region lookups.
        search_buf: Vec<f32>,
        analysis_pos: f64,
        prev_chosen: i64,
        ratio: f64,
        first_frame: bool,
    }

    impl Stretcher {
        pub fn new(mode: StretchMode, sample_rate: u32) -> Self {
            let capacity = capacity_frames(sample_rate);
            let window = window_frames(mode, sample_rate, GRAIN_DEFAULT_FRAMES);
            let hop = window / 2;
            let mut stretcher = Self {
                sample_rate,
                mode,
                grain_frames: GRAIN_DEFAULT_FRAMES,
                window,
                hop,
                overlap: window - hop,
                search: if searches(mode) { hop } else { 0 },
                corr_decim: 2,
                pending: None,
                hann: hann_table(),
                acc: vec![[0.0; 2]; capacity],
                head: 0,
                ready: vec![[0.0; 2]; capacity / 2],
                ready_pos: 0,
                ready_len: 0,
                nat: vec![0.0; capacity / 2],
                search_buf: vec![0.0; capacity + capacity / 2 + 1],
                analysis_pos: 0.0,
                prev_chosen: 0,
                ratio: 1.0,
                first_frame: true,
            };
            stretcher.apply_geometry(mode, GRAIN_DEFAULT_FRAMES);
            stretcher
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

        /// Heap bytes held per voice.
        ///
        /// Sized to the worst-case window rather than the active one, because
        /// mode and grain size are live controls. That is why this is several
        /// times the figure in #13's original budget: the budget was written when
        /// the window was fixed at construction.
        pub fn state_bytes(&self) -> usize {
            self.hann.capacity() * 4
                + self.acc.capacity() * 8
                + self.ready.capacity() * 8
                + self.nat.capacity() * 4
                + self.search_buf.capacity() * 4
        }

        pub fn mode(&self) -> StretchMode {
            self.mode
        }

        pub fn grain_frames(&self) -> u32 {
            self.grain_frames
        }

        /// Active window length in frames. In `Grain` this is what sets the
        /// repetition rate, at `sample_rate / (window / 2)`.
        pub fn window(&self) -> usize {
            self.window
        }

        /// Frequency of the grain repetition, in Hz. Meaningless in the
        /// transparent modes, where the search is actively suppressing it.
        pub fn rattle_hz(&self) -> f64 {
            self.sample_rate as f64 / self.hop as f64
        }

        /// Switch mode. Takes effect at the next hop boundary.
        pub fn set_mode(&mut self, mode: StretchMode) {
            self.queue_geometry(mode, self.target().1);
        }

        /// Set the grain window, in frames. Free and continuous by design: this
        /// is a timbre, and sweeping it is the point. Clamped to
        /// [`GRAIN_MIN_FRAMES`]..=[`GRAIN_MAX_FRAMES`], and ignored by the
        /// transparent modes, whose window sizing is a correctness rule rather
        /// than a preference.
        pub fn set_grain_frames(&mut self, frames: u32) {
            self.queue_geometry(self.target().0, frames);
        }

        /// The geometry the stretcher is heading for: whatever is queued, or the
        /// active geometry if nothing is. Both setters read through this so that
        /// changing one control cannot silently discard a change to the other
        /// that has not landed yet -- a mode switch and a grain sweep arriving in
        /// the same block is the normal case, not an edge case.
        fn target(&self) -> (StretchMode, u32) {
            self.pending.unwrap_or((self.mode, self.grain_frames))
        }

        fn queue_geometry(&mut self, mode: StretchMode, grain_frames: u32) {
            let grain_frames = grain_frames.clamp(GRAIN_MIN_FRAMES, GRAIN_MAX_FRAMES);
            if mode == self.mode && grain_frames == self.grain_frames {
                self.pending = None;
                return;
            }
            self.pending = Some((mode, grain_frames));
        }

        fn apply_geometry(&mut self, mode: StretchMode, grain_frames: u32) {
            let window = window_frames(mode, self.sample_rate, grain_frames);
            self.mode = mode;
            self.grain_frames = grain_frames;
            self.window = window;
            self.hop = window / 2;
            self.overlap = window - self.hop;
            self.search = if searches(mode) { self.hop } else { 0 };
            self.pending = None;
        }

        /// Output frames per input frame. `1.5` is longer and slower.
        ///
        /// Takes effect at the next overlap-add hop rather than the next frame:
        /// a window already being laid down is finished under the ratio it
        /// started with. The spike measured live ratio changes as click-free, so
        /// there is deliberately no crossfade or declick here. The hop
        /// quantization means an automated ratio moves in steps of one hop, which
        /// is also a gentle lowpass on a fast sweep.
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
            if let Some((mode, grain)) = self.pending.take() {
                self.apply_geometry(mode, grain);
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
            region.frame(frames, index).unwrap_or([0.0, 0.0])
        }

        #[inline]
        fn mid_at(frames: &[[f32; 2]], region: Region, index: i64) -> f32 {
            let frame = Self::frame_at(frames, region, index);
            0.5 * (frame[0] + frame[1])
        }

        /// Hann weight at `offset` within a window of `window` frames, read from
        /// the shared prototype. Linear interpolation between table points; the
        /// prototype is fine enough that the residual is far below the COLA
        /// tolerance the overlap-add needs.
        #[inline]
        fn window_weight(&self, offset: usize, window: usize) -> f32 {
            let position = offset as f32 / window as f32 * HANN_TABLE as f32;
            let index = position as usize;
            let fraction = position - index as f32;
            let low = self.hann[index];
            let high = self.hann[index + 1];
            low + (high - low) * fraction
        }

        /// Compute one overlap-add hop into `ready`.
        fn produce_hop(&mut self, frames: &[[f32; 2]], region: Region) {
            // A queued mode or grain change lands here, between windows. Applying
            // it mid-window would leave the accumulator holding half of one
            // envelope and half of another.
            if let Some((mode, grain)) = self.pending.take() {
                self.apply_geometry(mode, grain);
            }

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

            let chosen = if self.first_frame || !searches(self.mode) {
                // `Grain` never searches: the splice lands wherever the analysis
                // pointer says, which is what makes the repetition periodic and
                // the rattle pitched. On the first frame there is also nothing to
                // continue from, and searching would only move the very first
                // frame of playback away from where the caller asked to start.
                nominal
            } else {
                // What the previous segment was about to become, had it kept
                // playing. The best candidate is the one that continues this.
                let nat_start = self.prev_chosen + hop as i64;
                for offset in 0..overlap {
                    self.nat[offset] =
                        Self::mid_at(frames, region, nat_start + offset as i64);
                }
                let base = nominal - search;
                let span = 2 * self.search + overlap + 1;
                for offset in 0..span {
                    self.search_buf[offset] =
                        Self::mid_at(frames, region, base + offset as i64);
                }
                base + self.best_offset() as i64
            };

            // Lay the window down into the accumulator ring. The first hop skips
            // the rising half so a one-shot's initial transient is played at full
            // amplitude rather than faded in from nothing.
            for offset in 0..window {
                let weight = if self.first_frame && offset < overlap {
                    1.0
                } else {
                    self.window_weight(offset, window)
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

    pub struct StretchReader {
        stretcher: Stretcher,
        /// A window on the stretched stream. `scratch[0]` is stretched frame
        /// `base`.
        scratch: Vec<[f32; 2]>,
        base: i64,
        /// Next stretched frame the stretcher has yet to hand over.
        produced: i64,
        /// Fractional read position in the stretched stream.
        pos: f64,
        /// Where in the *source* the frame being handed out right now came from.
        ///
        /// Not the same as the stretcher's analysis pointer, which is the
        /// production frontier: it runs ahead by up to a whole hop plus the
        /// scratch fill, because a hop is computed before any of it is consumed.
        /// Using the frontier as a playhead puts the cursor ahead of what is
        /// audible, and -- worse -- using it for end-of-region detection ends a
        /// one-shot early and drops its tail. So this integrates at *consumption*
        /// time instead: each output frame eats `rate` stretched frames, and each
        /// stretched frame is `1 / ratio` of a source frame.
        source_pos: f64,
    }

    impl StretchReader {
        pub fn new(mode: StretchMode, sample_rate: u32) -> Self {
            let mut reader = Self {
                stretcher: Stretcher::new(mode, sample_rate),
                scratch: vec![[0.0; 2]; SCRATCH],
                base: 0,
                produced: 0,
                pos: 0.0,
                source_pos: 0.0,
            };
            reader.reset(0.0);
            reader
        }

        pub fn stretcher(&self) -> &Stretcher {
            &self.stretcher
        }

        pub fn stretcher_mut(&mut self) -> &mut Stretcher {
            &mut self.stretcher
        }

        /// Zero, and for the same reason the stretcher's is. See the type docs.
        pub fn latency_frames(&self) -> usize {
            0
        }

        pub fn state_bytes(&self) -> usize {
            self.stretcher.state_bytes() + self.scratch.capacity() * 8
        }

        /// The stretcher's production frontier. Ahead of what is sounding; use
        /// [`Self::source_pos`] for anything the listener or the user sees.
        pub fn analysis_pos(&self) -> f64 {
            self.stretcher.analysis_pos()
        }

        /// Where in the source the frame just handed out came from. This is the
        /// playhead, and it is what end-of-region detection must compare.
        pub fn source_pos(&self) -> f64 {
            self.source_pos
        }

        /// Restart at an absolute input frame. Allocation- and drop-free.
        ///
        /// The window is placed so the read position starts `MARGIN` frames into
        /// it, leaving the kernel room to reach backwards into the zeroed frames
        /// that precede the start.
        pub fn reset(&mut self, start_frame: f64) {
            self.stretcher.reset(start_frame);
            for frame in self.scratch.iter_mut() {
                *frame = [0.0, 0.0];
            }
            self.base = -MARGIN;
            self.produced = 0;
            self.pos = 0.0;
            self.source_pos = start_frame;
        }

        /// Produce one output frame at `rate`, where 1.0 is the source's own
        /// pitch and 2.0 is an octave up.
        pub fn read(&mut self, frames: &[[f32; 2]], region: Region, rate: f64) -> [f32; 2] {
            if frames.is_empty() || !rate.is_finite() || rate <= 0.0 {
                // Reverse under stretch is not supported and the UI disables it;
                // producing silence is better than letting a negative rate walk
                // the window backwards past material already discarded.
                return [0.0, 0.0];
            }
            self.ensure(self.pos.ceil() as i64 + MARGIN, frames, region);
            let local = self.pos - self.base as f64;
            let frame = SincTable::shared().read(
                &self.scratch,
                local,
                rate,
                Region::whole(SCRATCH),
            );
            self.pos += rate;
            self.source_pos += rate / self.stretcher.ratio();
            if let Some(span) = region_span(region) {
                while self.source_pos >= region.end {
                    self.source_pos -= span;
                }
            }
            frame
        }

        /// Slide and refill the window so stretched frames up to `upto` are valid.
        fn ensure(&mut self, upto: i64, frames: &[[f32; 2]], region: Region) {
            while upto >= self.base + SCRATCH as i64 {
                self.scratch.copy_within(SHIFT.., 0);
                for slot in self.scratch[SCRATCH - SHIFT..].iter_mut() {
                    *slot = [0.0, 0.0];
                }
                self.base += SHIFT as i64;
            }
            while self.produced < self.base + SCRATCH as i64 {
                let frame = self.stretcher.next_frame(frames, region);
                let slot = self.produced - self.base;
                // Frames that fell behind the window as it slid are simply
                // dropped: the read position never goes backwards.
                if (0..SCRATCH as i64).contains(&slot) {
                    self.scratch[slot as usize] = frame;
                }
                self.produced += 1;
            }
        }
    }
}

/// MOO-248 holds the stretcher to its output from before the splice search
/// was spread over the hop: the search and the overlap-add do the same
/// arithmetic on the same frames, only earlier, so every case below renders
/// sample for sample what [`before_moo248`] renders, in the same process.
#[cfg(test)]
mod bit_identity {
    use super::*;
    use crate::interpolate::RegionEdge;

    /// What a case needs of a stretcher, so one script drives the new one
    /// and the one from before MOO-248 alike.
    trait Stretches {
        fn make(mode: StretchMode, sample_rate: u32) -> Self;
        fn ratio_to(&mut self, ratio: f64);
        fn reset_at(&mut self, frame: f64);
        fn current_mode(&self) -> StretchMode;
        fn mode_to(&mut self, mode: StretchMode);
        fn grain_to(&mut self, frames: u32);
        fn next(&mut self, frames: &[[f32; 2]], region: Region) -> [f32; 2];
    }

    macro_rules! stretches {
        ($type:ty) => {
            impl Stretches for $type {
                fn make(mode: StretchMode, sample_rate: u32) -> Self {
                    <$type>::new(mode, sample_rate)
                }
                fn ratio_to(&mut self, ratio: f64) {
                    self.set_ratio(ratio)
                }
                fn reset_at(&mut self, frame: f64) {
                    self.reset(frame)
                }
                fn current_mode(&self) -> StretchMode {
                    self.mode()
                }
                fn mode_to(&mut self, mode: StretchMode) {
                    self.set_mode(mode)
                }
                fn grain_to(&mut self, frames: u32) {
                    self.set_grain_frames(frames)
                }
                fn next(&mut self, frames: &[[f32; 2]], region: Region) -> [f32; 2] {
                    self.next_frame(frames, region)
                }
            }
        };
    }
    stretches!(Stretcher);
    stretches!(before_moo248::Stretcher);

    const SR: u32 = 48_000;

    /// Tones plus deterministic noise, so the search has something real to
    /// choose between and a changed choice cannot hide.
    fn source(len: usize) -> Vec<[f32; 2]> {
        let mut seed = 0x2545_f491_u32;
        (0..len)
            .map(|index| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                let t = index as f32 / SR as f32;
                let tone = (t * 110.0 * core::f32::consts::TAU).sin() * 0.4
                    + (t * 587.0 * core::f32::consts::TAU).sin() * 0.2;
                [tone + noise * 0.2, tone * 0.8 - noise * 0.1]
            })
            .collect()
    }

    fn hash(out: &[[f32; 2]]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for frame in out {
            for sample in frame {
                hash ^= u64::from(sample.to_bits());
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        hash
    }

    fn region(start: f64, end: f64, edge: RegionEdge) -> Region {
        Region { start, end, edge }
    }

    /// One case: a mode, a ratio, a region, and what changes mid-render
    /// (every `change` frames, by `step`'s index), pulled in blocks of 128.
    fn run_with<S: Stretches>(mode: StretchMode, ratio: f64, reg: Region, changes: u32) -> u64 {
        let frames = source(60_000);
        let mut stretcher = S::make(mode, SR);
        stretcher.ratio_to(ratio);
        stretcher.reset_at(reg.start.max(0.0) + 17.0);
        let mut out = Vec::with_capacity(40_000);
        let mut reg = reg;
        for block in 0..(40_000 / 128) {
            if changes & 1 != 0 && block % 23 == 22 {
                stretcher.ratio_to(0.4 + (block % 7) as f64 * 0.6);
            }
            if changes & 2 != 0 && block % 41 == 40 {
                let next = match stretcher.current_mode() {
                    StretchMode::Music => StretchMode::Drums,
                    StretchMode::Drums => StretchMode::Grain,
                    _ => StretchMode::Music,
                };
                stretcher.mode_to(next);
            }
            if changes & 4 != 0 && block % 5 == 4 {
                stretcher.grain_to(200 + (block as u32 * 37) % 1500);
            }
            if changes & 8 != 0 && block % 17 == 16 {
                // A loop end that moves, as a modulated loop does.
                reg.end = 30_000.0 + (block % 9) as f64 * 1_234.5;
            }
            for _ in 0..128 {
                out.push(stretcher.next(&frames, reg));
            }
        }
        hash(&out)
    }

    /// A case's output from the new stretcher and from the one before
    /// MOO-248, hashed.
    fn run(mode: StretchMode, ratio: f64, reg: Region, changes: u32) -> (u64, u64) {
        (
            run_with::<Stretcher>(mode, ratio, reg, changes),
            run_with::<before_moo248::Stretcher>(mode, ratio, reg, changes),
        )
    }

    fn reader(mode: StretchMode, ratio: f64, rate: f64, reg: Region) -> (u64, u64) {
        let frames = source(60_000);
        let mut reader = StretchReader::new(mode, SR);
        reader.stretcher_mut().set_ratio(ratio);
        reader.reset(reg.start);
        let new: Vec<_> = (0..30_000).map(|_| reader.read(&frames, reg, rate)).collect();
        let mut reader = before_moo248::StretchReader::new(mode, SR);
        reader.stretcher_mut().set_ratio(ratio);
        reader.reset(reg.start);
        let old: Vec<_> = (0..30_000).map(|_| reader.read(&frames, reg, rate)).collect();
        (hash(&new), hash(&old))
    }

    fn cases() -> Vec<(&'static str, (u64, u64))> {
        let whole = region(0.0, 60_000.0, RegionEdge::Silent);
        let wrap = region(1_000.0, 30_000.0, RegionEdge::Wrap);
        let short = region(2_000.0, 3_100.0, RegionEdge::Wrap);
        let mirror = region(4_000.5, 21_000.25, RegionEdge::Mirror);
        let fade_pre = region(
            5_000.0,
            25_000.0,
            RegionEdge::Crossfade { fade: 900, floor: 1_000, head: 0 },
        );
        let fade_none = region(
            0.0,
            25_000.0,
            RegionEdge::Crossfade { fade: 700, floor: 0, head: 300 },
        );
        vec![
            ("music x2 whole", run(StretchMode::Music, 2.0, whole, 0)),
            ("music x0.5 whole", run(StretchMode::Music, 0.5, whole, 0)),
            ("drums x1.37 wrap", run(StretchMode::Drums, 1.37, wrap, 0)),
            ("grain x3 wrap sweep", run(StretchMode::Grain, 3.0, wrap, 4)),
            ("music x8 short loop", run(StretchMode::Music, 8.0, short, 0)),
            ("music x1.5 mirror", run(StretchMode::Music, 1.5, mirror, 0)),
            ("music x1.5 fade pre-roll", run(StretchMode::Music, 1.5, fade_pre, 0)),
            ("drums x2 fade head", run(StretchMode::Drums, 2.0, fade_none, 0)),
            ("music ratio changes", run(StretchMode::Music, 1.2, wrap, 1)),
            ("mode switches and sweeps", run(StretchMode::Music, 1.7, wrap, 1 | 2 | 4)),
            ("moving loop end", run(StretchMode::Music, 2.5, wrap, 8)),
            ("everything moves", run(StretchMode::Drums, 0.7, fade_pre, 15)),
            ("reader music x2 rate 1.5", reader(StretchMode::Music, 2.0, 1.5, wrap)),
            ("reader drums x0.6 rate 0.7", reader(StretchMode::Drums, 0.6, 0.7, whole)),
        ]
    }

    /// The fast reads trust `plain_span`: every frame inside it must be what
    /// `Region::frame` returns, for every edge, including a crossfaded seam
    /// with and without pre-roll and a region that overhangs the sample.
    #[test]
    fn a_region_plain_span_is_exactly_where_a_frame_is_read_untouched() {
        let frames = source(10_000);
        let regions = [
            region(0.0, 10_000.0, RegionEdge::Silent),
            region(100.4, 9_000.6, RegionEdge::Wrap),
            region(300.0, 12_000.0, RegionEdge::Mirror),
            region(2_000.0, 6_000.0, RegionEdge::Crossfade { fade: 500, floor: 1_800, head: 0 }),
            region(2_000.0, 6_000.0, RegionEdge::Crossfade { fade: 500, floor: 0, head: 0 }),
            region(0.0, 6_000.0, RegionEdge::Crossfade { fade: 500, floor: 0, head: 200 }),
            region(0.0, 900.0, RegionEdge::Crossfade { fade: 5_000, floor: 0, head: 5_000 }),
            region(0.0, 6_000.0, RegionEdge::Crossfade { fade: 0, floor: 0, head: 0 }),
        ];
        for reg in regions {
            let (lo, hi) = reg.plain_span(frames.len());
            assert!(lo <= hi);
            for index in -50..10_050i64 {
                if (lo..hi).contains(&index) {
                    assert_eq!(
                        reg.frame(&frames, index),
                        Some(frames[index as usize]),
                        "{reg:?} at {index}"
                    );
                }
            }
        }
    }

    /// Spreading the search and doing it whole at the boundary choose the
    /// same splices, sample for sample, in every case above's shape.
    #[test]
    fn spreading_the_search_changes_no_sample() {
        let frames = source(60_000);
        let reg = region(1_000.0, 30_000.0, RegionEdge::Wrap);
        for mode in [StretchMode::Music, StretchMode::Drums, StretchMode::Grain] {
            for ratio in [0.3, 1.0, 2.0, 7.0] {
                let mut outs = Vec::new();
                for spread in [true, false] {
                    let mut stretcher = Stretcher::new(mode, SR);
                    stretcher.set_spread(spread);
                    stretcher.set_ratio(ratio);
                    stretcher.reset(1_234.0);
                    let out: Vec<_> =
                        (0..20_000).map(|_| stretcher.next_frame(&frames, reg)).collect();
                    outs.push(hash(&out));
                }
                assert_eq!(outs[0], outs[1], "{mode:?} x{ratio}");
            }
        }
    }

    #[test]
    fn the_stretcher_output_is_bit_identical_to_before_the_search_was_spread() {
        for (name, (new, old)) in cases() {
            assert_eq!(new, old, "{name} changed");
        }
    }
}
