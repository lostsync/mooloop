//! Candidate A: WSOLA (waveform-similarity overlap-add), the transient-oriented
//! time-domain family, with optional onset snapping.
//!
//! Shape of the algorithm as implemented here:
//!
//! - Output is produced in fixed `hop = window / 2` chunks, overlap-added under
//!   a periodic Hann that is COLA at 50%, so no synthesis window is needed.
//! - The analysis pointer advances by `hop / ratio` input frames per output hop.
//!   It is fractional, so duration error never accumulates: the harness asks for
//!   exactly `round(input_len * ratio)` frames and gets them.
//! - Before laying each window down, the segment start is nudged within
//!   `+/- search` frames to the position whose leading `overlap` frames best
//!   match the natural continuation of the previously chosen segment. That is
//!   the whole trick: it keeps waveform phase continuous across the join, which
//!   is what stops the comb-filtered, chorused sound plain SOLA produces.
//! - Alignment is decided on the mid channel only and applied to both, so the
//!   stereo image cannot drift.
//! - With `transient_snap`, a control-side onset table overrides the search
//!   near a detected transient: the segment is placed so the onset lands just
//!   past the crossfade region, and the search is then forbidden from stepping
//!   back across that onset. Without the second half, slowing down re-plays the
//!   attack and produces the classic flam.

use crate::{hann, Source, Stretcher};

#[derive(Clone, Copy)]
pub struct WsolaConfig {
    pub name: &'static str,
    /// OLA window length in frames. `hop` is half of it.
    pub window: usize,
    /// Half-width of the similarity search, in frames.
    pub search: usize,
    /// Decimation of the correlation sum. 1 is exact; 2 halves the cost with
    /// no audible difference at these window sizes.
    pub corr_decim: usize,
    pub transient_snap: bool,
}

impl WsolaConfig {
    /// Short window, no onset table. Cheapest, sharpest, weakest on bass.
    pub const FAST: Self = Self {
        name: "wsola_fast",
        window: 512,
        search: 256,
        corr_decim: 2,
        transient_snap: false,
    };
    /// The proposed default.
    pub const MUSIC: Self = Self {
        name: "wsola_music",
        window: 1024,
        search: 512,
        corr_decim: 2,
        transient_snap: true,
    };
    /// Long window for sustained/tonal material.
    pub const SMOOTH: Self = Self {
        name: "wsola_smooth",
        window: 2048,
        search: 1024,
        corr_decim: 4,
        transient_snap: true,
    };
    /// Intermediate window: between FAST's transient accuracy and NO_SNAP's
    /// tonal stability. Added after the first run showed 512 winning on the
    /// break and 1024 winning on a steady tone.
    pub const BREAK: Self = Self {
        name: "wsola_break",
        window: 768,
        search: 384,
        corr_decim: 2,
        transient_snap: false,
    };
    /// Ablation: MUSIC with the onset table switched off, to isolate how much
    /// of the transient result comes from snapping rather than from WSOLA.
    pub const NO_SNAP: Self = Self {
        name: "wsola_nosnap",
        window: 1024,
        search: 512,
        corr_decim: 2,
        transient_snap: false,
    };
}

pub struct Wsola {
    cfg: WsolaConfig,
    hop: usize,
    overlap: usize,
    window_fn: Vec<f32>,
    acc: Vec<[f32; 2]>,
    head: usize,
    ready: Vec<[f32; 2]>,
    ready_pos: usize,
    ready_len: usize,
    nat: Vec<f32>,
    search_buf: Vec<f32>,
    analysis_pos: f64,
    prev_chosen: i64,
    ratio: f64,
    first_frame: bool,
    next_onset: usize,
    onset_floor: i64,
}

impl Wsola {
    pub fn new(cfg: WsolaConfig, ratio: f64) -> Self {
        let hop = cfg.window / 2;
        let overlap = cfg.window - hop;
        Self {
            cfg,
            hop,
            overlap,
            window_fn: hann(cfg.window),
            acc: vec![[0.0; 2]; cfg.window],
            head: 0,
            ready: vec![[0.0; 2]; hop],
            ready_pos: 0,
            ready_len: 0,
            nat: vec![0.0; overlap],
            search_buf: vec![0.0; 2 * cfg.search + overlap + 1],
            analysis_pos: 0.0,
            prev_chosen: 0,
            ratio,
            first_frame: true,
            next_onset: 0,
            onset_floor: i64::MIN,
        }
    }

    fn produce_frame(&mut self, src: &Source<'_>) {
        let win = self.cfg.window;
        let hop = self.hop;
        let overlap = self.overlap;
        let search = self.cfg.search as i64;

        // Wrap the analysis pointer inside the loop so the onset table, which
        // is indexed inside one cycle, stays valid on every repeat.
        if let Some((ls, le)) = src.loop_bounds {
            if le > ls {
                let span = (le - ls) as f64;
                while self.analysis_pos >= le as f64 {
                    self.analysis_pos -= span;
                    self.prev_chosen -= span as i64;
                    self.next_onset = 0;
                    self.onset_floor = i64::MIN;
                }
            }
        }

        let nominal = self.analysis_pos.round() as i64;
        let nat_start = self.prev_chosen + hop as i64;
        for (i, slot) in self.nat.iter_mut().enumerate() {
            *slot = src.mid(nat_start + i as i64);
        }
        let base = nominal - search;
        for (i, slot) in self.search_buf.iter_mut().enumerate() {
            *slot = src.mid(base + i as i64);
        }

        let mut chosen: Option<i64> = None;
        if self.cfg.transient_snap && !src.onsets.is_empty() {
            while self
                .next_onset
                .lt(&src.onsets.len())
                .then(|| src.onsets[self.next_onset] as i64)
                .is_some_and(|o| o < self.onset_floor)
            {
                self.next_onset += 1;
            }
            if let Some(&onset) = src.onsets.get(self.next_onset) {
                // Place the attack immediately after the crossfade region, so
                // it is played at full amplitude rather than blended with the
                // decay of the previous segment.
                let target = onset as i64 - overlap as i64;
                if target >= nominal - search && target <= nominal + search {
                    chosen = Some(target);
                    self.onset_floor = onset as i64 + 1;
                    self.next_onset += 1;
                }
            }
        }

        let chosen = chosen.unwrap_or_else(|| {
            if self.first_frame {
                return nominal;
            }
            let lo = if self.onset_floor == i64::MIN {
                0
            } else {
                ((self.onset_floor - overlap as i64) - base).clamp(0, 2 * search) as usize
            };
            let hi = (2 * search) as usize;
            let step = self.cfg.corr_decim.max(1);
            let mut best_k = lo;
            let mut best = f32::NEG_INFINITY;
            for k in lo..=hi {
                let mut num = 0.0f32;
                let mut energy = 1.0e-9f32;
                let mut i = 0;
                while i < overlap {
                    let c = self.search_buf[k + i];
                    num += c * self.nat[i];
                    energy += c * c;
                    i += step;
                }
                let score = num / energy.sqrt();
                if score > best {
                    best = score;
                    best_k = k;
                }
            }
            base + best_k as i64
        });

        // Lay the window down. The first frame skips the rising half so the
        // very first transient of a one-shot is not faded in.
        for i in 0..win {
            let w = if self.first_frame && i < overlap {
                1.0
            } else {
                self.window_fn[i]
            };
            let f = src.frame(chosen + i as i64);
            let a = self.head + i;
            let a = if a >= win { a - win } else { a };
            self.acc[a][0] += w * f[0];
            self.acc[a][1] += w * f[1];
        }
        self.first_frame = false;

        for i in 0..hop {
            let a = self.head + i;
            let a = if a >= win { a - win } else { a };
            self.ready[i] = self.acc[a];
            self.acc[a] = [0.0, 0.0];
        }
        self.head += hop;
        if self.head >= win {
            self.head -= win;
        }
        self.ready_pos = 0;
        self.ready_len = hop;

        self.prev_chosen = chosen;
        self.analysis_pos += hop as f64 / self.ratio;
    }
}

impl Stretcher for Wsola {
    fn name(&self) -> &'static str {
        self.cfg.name
    }

    fn reset(&mut self, start_frame: usize) {
        for f in self.acc.iter_mut() {
            *f = [0.0, 0.0];
        }
        self.head = 0;
        self.ready_pos = 0;
        self.ready_len = 0;
        self.analysis_pos = start_frame as f64;
        self.prev_chosen = start_frame as i64;
        self.first_frame = true;
        self.next_onset = 0;
        self.onset_floor = i64::MIN;
    }

    fn set_ratio(&mut self, ratio: f64) {
        self.ratio = ratio.clamp(0.05, 20.0);
    }

    fn render(&mut self, src: &Source<'_>, out: &mut [[f32; 2]]) {
        let mut written = 0;
        while written < out.len() {
            if self.ready_pos >= self.ready_len {
                self.produce_frame(src);
            }
            let n = (out.len() - written).min(self.ready_len - self.ready_pos);
            out[written..written + n]
                .copy_from_slice(&self.ready[self.ready_pos..self.ready_pos + n]);
            self.ready_pos += n;
            written += n;
        }
    }

    fn latency_frames(&self) -> usize {
        // Zero. Output frame 0 corresponds to input frame `start`; the window
        // and search are lookahead *into a resident sample*, not delay in time.
        0
    }

    fn state_bytes(&self) -> usize {
        self.window_fn.capacity() * 4
            + self.acc.capacity() * 8
            + self.ready.capacity() * 8
            + self.nat.capacity() * 4
            + self.search_buf.capacity() * 4
    }
}

/// Frames of sample the algorithm may read ahead of its nominal analysis
/// position. Not latency, but it bounds how close to the end of a region the
/// pointer can be before it starts reading past it.
pub fn lookahead_frames(cfg: &WsolaConfig) -> usize {
    cfg.window + cfg.search
}
