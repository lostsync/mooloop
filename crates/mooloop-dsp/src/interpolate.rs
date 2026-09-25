//! Band-limited sample-rate conversion for sample playback.
//!
//! This is playback-rate conversion, not time stretching: pitch and duration
//! stay coupled. What it replaces is two-point linear interpolation, whose
//! stopband is poor enough that pitching a sample audibly acquires foldback
//! on the way up and dullness on the way down.
//!
//! The kernel is a windowed sinc sampled densely into a prototype table and
//! read at whatever spacing the current playback rate calls for, which is how
//! one table serves every ratio:
//!
//! - **Pitching down or unity** reads the prototype at its natural spacing.
//!   At unity rate with an integer read position the kernel collapses to a
//!   single unit tap, because `sinc` is zero at every non-zero integer, so
//!   playback is sample-exact rather than merely close.
//! - **Pitching up** narrows the kernel's cutoff by the rate and widens its
//!   support to match, which is the part linear interpolation cannot do at
//!   all: the source content above the new Nyquist has to be filtered out
//!   before it folds back. The widening is capped by [`MAX_STRETCH`] so the
//!   per-sample cost stays bounded however far a note is transposed.
//!
//! Nothing here allocates, locks, or does I/O after the table is built, and
//! the table is built once off the audio thread — [`SincTable::shared`] is
//! forced during device construction so no `process()` call is the first to
//! touch it.

use std::sync::OnceLock;

/// Half the kernel's width in frames at unity rate: 8 zero crossings a side,
/// so 16 taps. Enough window to put the stopband far below the noise floor of
/// the material a sampler plays, and cheap enough to run on every voice.
const HALF_TAPS: usize = 8;

/// Prototype samples per frame of kernel support. The read interpolates
/// linearly between neighbouring prototype samples, so this only has to be
/// fine enough that the residual is negligible against the kernel itself.
const DENSITY: usize = 256;

/// Furthest the kernel is allowed to widen when pitching up, and so the
/// bound on per-sample work: 4x support, 64 taps, two octaves of transposition
/// with a fully band-limited kernel. Past that the kernel stops narrowing and
/// foldback returns gradually, which is a better failure than an unbounded
/// read on the audio thread.
const MAX_STRETCH: f64 = 4.0;

/// Widest the kernel reaches on either side of its centre, in frames, at any
/// rate it will accept. Published because anything feeding the reader from a
/// windowed view of a longer stream -- the time stretcher's scratch, for one
/// -- has to keep this much valid material on both sides of the read
/// position, or the kernel silently folds against the window's edge instead
/// of the region's.
pub const MAX_HALF_TAPS: usize = HALF_TAPS * MAX_STRETCH as usize;

/// Prototype table length, plus one so the linear read always has a right
/// neighbour to reach for.
const TABLE_LEN: usize = HALF_TAPS * DENSITY + 2;

/// Where a read that reaches past the active region finds its frames.
///
/// The kernel is 16 frames wide at unity and wider when pitching up, so it
/// routinely overhangs a loop point that the read head itself has not
/// crossed yet. What it sees there is part of the playback contract rather
/// than an implementation detail: a forward loop has to see the material it
/// is about to wrap into, or the crossing is filtered against silence and
/// clicks exactly where the loop was supposed to be seamless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionEdge {
    /// Nothing plays past this region. The kernel sees silence beyond it,
    /// which is what a one-shot's ends genuinely are.
    Silent,
    /// A forward loop: material past `end` is the material at `start`, and
    /// material before `start` is the material at `end`.
    Wrap,
    /// A ping-pong turnaround: material past an edge is the region reflected
    /// back into itself, which is what the read head is about to play.
    Mirror,
    /// A forward loop whose seam is crossfaded (MOO-43): it folds exactly as
    /// [`RegionEdge::Wrap`] does, and its last `fade` frames are prepared so
    /// the seam has nothing to click on.
    ///
    /// Where the sample has material before the loop's start (down to
    /// `floor`, the playback region's own start), the loop's end blends,
    /// equal-power, into that material, the classic sampler loop crossfade.
    /// The frame at `end - 1` is wholly the frame at `start - 1`, so the last
    /// frame and the first are neighbours. The loop keeps its length and its
    /// start is untouched. If there's less pre-roll than `fade`, the blend
    /// shortens to what there is.
    ///
    /// Where there is none, because the loop starts at the region's first
    /// frame, the loop's end fades out to silence over `fade` frames and its
    /// start fades back in over `head`. Blending into the loop's own opening
    /// played backwards was measured and rejected: on a loop that starts on
    /// a kick, it put a full-level reversed kick in front of the downbeat.
    ///
    /// A reverse voice crosses either one the other way round, so it needs no
    /// rule of its own. A fade is never more than half the loop.
    Crossfade {
        /// Frames of the blend, at the end of the loop. At least one.
        fade: u32,
        /// The first frame the pre-roll may read.
        floor: i64,
        /// Frames the loop's start fades in over, when there is no pre-roll.
        head: u32,
    },
}

/// The span of frames a voice is currently reading, and what happens at its
/// edges.
///
/// Carried as one named value rather than three loose numbers because the
/// region a read head is working in has musical meaning — it is the loop, or
/// the one-shot's trimmed extent — and the interpolator, the crossfade in
/// #31, and the UI all have to agree on it.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    /// First frame of the region, inclusive.
    pub start: f64,
    /// One past the last frame of the region.
    pub end: f64,
    pub edge: RegionEdge,
}

impl Region {
    /// The whole of a sample, with nothing playing past either end.
    pub fn whole(len: usize) -> Self {
        Self {
            start: 0.0,
            end: len as f64,
            edge: RegionEdge::Silent,
        }
    }

    /// Resolve an integer frame index the kernel asked for into an index
    /// inside the region, or `None` where nothing plays.
    ///
    /// `Wrap` and `Mirror` fold repeatedly rather than once, because a kernel
    /// widened by [`MAX_STRETCH`] can overhang a short loop several times
    /// over. Folding in a loop keeps that correct instead of reading whatever
    /// happened to be adjacent in the sample.
    pub(crate) fn resolve(&self, index: i64, len: usize) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let start = self.start.floor() as i64;
        let end = (self.end.ceil() as i64).max(start + 1);
        let span = end - start;

        let folded = match self.edge {
            RegionEdge::Silent => {
                if index < start || index >= end {
                    return None;
                }
                index
            }
            RegionEdge::Wrap | RegionEdge::Crossfade { .. } => {
                start + (index - start).rem_euclid(span)
            }
            RegionEdge::Mirror => {
                // Reflect into `[0, span)` through a period of `2 * span`:
                // the material past an edge is the region played backwards.
                let period = span * 2;
                let offset = (index - start).rem_euclid(period);
                let reflected = if offset < span {
                    offset
                } else {
                    period - 1 - offset
                };
                start + reflected
            }
        };
        usize::try_from(folded).ok().filter(|frame| *frame < len)
    }

    /// The frames `lo..hi` that [`Region::frame`] hands back untouched, as
    /// `frames[index]`: inside the region, inside the sample, and clear of a
    /// crossfaded seam's blend and fade-in. A run of frames wholly inside it
    /// can be read straight from the slice and is bit for bit what `frame`
    /// would give, without folding each index (MOO-248).
    pub(crate) fn plain_span(&self, len: usize) -> (i64, i64) {
        let start = floor_i64(self.start);
        let end = ceil_i64(self.end).max(start + 1);
        let span = end - start;
        let (lo, hi) = match self.edge {
            RegionEdge::Crossfade { fade, floor, head } => {
                let pre_roll = (start - floor).max(0);
                if pre_roll == 0 {
                    let fade = i64::from(fade).min(span / 2);
                    let head = i64::from(head).min(fade);
                    (
                        if head >= 1 { start + head } else { start },
                        if fade >= 1 { end - fade } else { end },
                    )
                } else {
                    let fade = i64::from(fade).min(span / 2).min(pre_roll);
                    (start, if fade >= 1 { end - fade } else { end })
                }
            }
            _ => (start, end),
        };
        let lo = lo.max(0);
        (lo, hi.min(len as i64).max(lo))
    }

    /// The stereo frame the kernel sees at `index`: [`Region::resolve`]'s
    /// frame, blended across a crossfaded seam, or `None` where nothing plays.
    ///
    /// Every edge but [`RegionEdge::Crossfade`] returns the resolved frame
    /// untouched, so a region without a fade reads bit for bit what it did
    /// before the seam existed.
    pub(crate) fn frame(&self, frames: &[[f32; 2]], index: i64) -> Option<[f32; 2]> {
        let resolved = self.resolve(index, frames.len())?;
        let source = frames[resolved];
        let RegionEdge::Crossfade { fade, floor, head } = self.edge else {
            return Some(source);
        };
        let start = self.start.floor() as i64;
        let end = (self.end.ceil() as i64).max(start + 1);
        let span = end - start;
        let pre_roll = (start - floor).max(0);
        let at = resolved as i64;
        if pre_roll == 0 {
            // Out and back in: the end to silence, the start up from it.
            let fade = i64::from(fade).min(span / 2);
            let head = i64::from(head).min(fade);
            let into = at - (end - fade);
            if fade >= 1 && into >= 0 {
                let (gain, _) = crate::smooth::equal_power((into + 1) as f32 / fade as f32);
                return Some([source[0] * gain, source[1] * gain]);
            }
            let from_start = at - start;
            if head >= 1 && from_start < head {
                let (_, gain) =
                    crate::smooth::equal_power((from_start + 1) as f32 / (head + 1) as f32);
                return Some([source[0] * gain, source[1] * gain]);
            }
            return Some(source);
        }
        let fade = i64::from(fade).min(span / 2).min(pre_roll);
        let into = at - (end - fade);
        if fade < 1 || into < 0 {
            return Some(source);
        }
        // `into + 1` so the last frame of the loop is wholly the pre-roll and
        // the first frame of the fade is already a step into it: the frame
        // before the fade has weight zero, the frame at the seam weight one.
        let phase = (into + 1) as f32 / fade as f32;
        let (out_gain, in_gain) = crate::smooth::equal_power(phase);
        let before = usize::try_from(at - span)
            .ok()
            .filter(|frame| *frame < frames.len())
            .map_or([0.0, 0.0], |frame| frames[frame]);
        Some([
            source[0] * out_gain + before[0] * in_gain,
            source[1] * out_gain + before[1] * in_gain,
        ])
    }
}

/// `x.floor() as i64`, without the library call `floor` is on x86-64's
/// baseline, which has no rounding instruction (MOO-247). Truncation is
/// floor for everything at or above zero and one more below it for a
/// negative non-integer; the saturating ends and NaN come out as the
/// `as` cast of `floor` gives them. Read positions and region edges go
/// through this once or twice a frame per voice.
#[inline]
fn floor_i64(x: f64) -> i64 {
    let truncated = x as i64;
    if (truncated as f64) > x {
        truncated.saturating_sub(1)
    } else {
        truncated
    }
}

/// `x.ceil() as i64`, the same way as [`floor_i64`].
#[inline]
fn ceil_i64(x: f64) -> i64 {
    let truncated = x as i64;
    if (truncated as f64) < x {
        truncated.saturating_add(1)
    } else {
        truncated
    }
}

/// The shared windowed-sinc prototype.
///
/// One table serves every playback rate, so this is built once for the
/// process rather than per voice or per device.
pub struct SincTable {
    /// `h(t)` for `t` in `[0, HALF_TAPS]`, sampled `DENSITY` times per unit.
    /// Symmetric, so only one side is stored.
    prototype: [f32; TABLE_LEN],
}

static SHARED: OnceLock<SincTable> = OnceLock::new();

impl SincTable {
    /// The process-wide table, built on first call.
    ///
    /// Call this once from device construction so the build never lands on
    /// the audio thread; every later call is an atomic load.
    pub fn shared() -> &'static SincTable {
        SHARED.get_or_init(SincTable::build)
    }

    fn build() -> Self {
        let mut prototype = [0.0f32; TABLE_LEN];
        for (index, tap) in prototype.iter_mut().enumerate() {
            let t = index as f64 / DENSITY as f64;
            *tap = (sinc(t) * blackman(t / HALF_TAPS as f64)) as f32;
        }
        Self { prototype }
    }

    /// Read the prototype at `t` frames from the kernel's centre, linearly
    /// between neighbouring samples. Zero past the kernel's support.
    ///
    /// The index goes through `i32` rather than `usize` (MOO-247): x86-64
    /// has no single instruction for a float to or from an unsigned 64-bit
    /// integer. The values are the same: past `i32::MAX` saturates, which
    /// is past the table as before.
    #[inline]
    fn tap(&self, t: f64) -> f32 {
        let scaled = t * DENSITY as f64;
        if scaled < 0.0 {
            return 0.0;
        }
        let index = scaled as i32;
        if index as usize + 1 >= TABLE_LEN {
            return 0.0;
        }
        let frac = (scaled - f64::from(index)) as f32;
        let index = index as usize;
        let low = self.prototype[index];
        low + (self.prototype[index + 1] - low) * frac
    }

    /// One stereo frame at fractional position `pos`, read at `rate` frames
    /// of source per frame of output.
    ///
    /// `rate` is the voice's playback rate and only ever narrows the kernel:
    /// reading slower than the source needs no extra band limiting, so
    /// pitching down and unity share the natural-width path.
    pub fn read(
        &self,
        frames: &[[f32; 2]],
        pos: f64,
        rate: f64,
        region: Region,
    ) -> [f32; 2] {
        let len = frames.len();
        // `rate` is guarded here rather than clamped below because `clamp`
        // propagates NaN, and a NaN rate would otherwise reach the kernel
        // width and turn the read loop's bounds into nonsense.
        if len == 0 || !pos.is_finite() || !rate.is_finite() {
            return [0.0, 0.0];
        }

        // Narrow the cutoff by the rate when pitching up, and widen support
        // to match. `ratio` is both the kernel's time scale and its gain
        // correction, since a wider kernel sums more taps.
        let stretch = rate.abs().clamp(1.0, MAX_STRETCH);
        let ratio = 1.0 / stretch;
        let half_width = HALF_TAPS as f64 * stretch;

        let centre = floor_i64(pos) as f64;
        let (first, last) = if stretch == 1.0 {
            // Whole numbers either side of a whole number: no rounding to do.
            (centre as i64 - HALF_TAPS as i64, centre as i64 + HALF_TAPS as i64)
        } else {
            (
                ceil_i64(centre - half_width),
                floor_i64(centre + half_width),
            )
        };

        // A whole frame at the source's own rate (a one-shot at its root
        // note, on a sample at the project rate) is that frame (MOO-247). The
        // kernel's centre tap is exactly 1 and every other tap lands on a
        // zero of the sinc, which the table holds as about 4e-17 rather than
        // 0, so this differs from the full sum only below about -300 dB of
        // the frame's neighbours: bit for bit wherever the frame itself is
        // above about -180 dBFS.
        if rate.abs() == 1.0 && pos == centre {
            return region.frame(frames, pos as i64).unwrap_or([0.0, 0.0]);
        }

        let mut left = 0.0f32;
        let mut right = 0.0f32;
        let (lo, hi) = region.plain_span(len);
        if first >= lo && last < hi && stretch == 1.0 {
            // The usual case at or below the source's rate (MOO-247): the
            // kernel is 17 taps, all inside the region, and every tap's
            // table position is a whole number of table steps from the
            // centre's plus one of two fractions. Inside a region the read
            // position is at least 8, so `pos - centre` is exact, and so is
            // every `m ± f` a tap is at: the fractions below are the very
            // values `tap` would compute per tap, found once per frame. The
            // same coefficients, the same sums in the same order, so bit for
            // bit what the edge path gives on any finite sample.
            let centre_index = centre as usize;
            let f = pos - centre;
            let before = f * DENSITY as f64;
            // Through `i32`: both are in `0..=256`, and a float to or from
            // an unsigned 64-bit integer is a sequence on x86-64, not one
            // instruction.
            let a = before as i32;
            let before_frac = (before - f64::from(a)) as f32;
            let after = DENSITY as f64 - before;
            let b = after as i32;
            let after_frac = (after - f64::from(b)) as f32;
            let (a, b) = (a as usize, b as usize);
            let p = &self.prototype;
            let lerp = |index: usize, frac: f32| p[index] + (p[index + 1] - p[index]) * frac;
            // `centre - 8` is past the kernel's support, a zero tap the edge
            // path skips, so the taps start at `centre - 7`.
            let taps = &frames[centre_index - 7..=centre_index + 8];
            for (m, source) in (0..HALF_TAPS).rev().zip(&taps[..HALF_TAPS]) {
                let coeff = lerp(m * DENSITY + a, before_frac);
                if coeff == 0.0 {
                    continue;
                }
                left += source[0] * coeff;
                right += source[1] * coeff;
            }
            for (m, source) in (1..=HALF_TAPS).zip(&taps[HALF_TAPS..]) {
                let coeff = lerp((m - 1) * DENSITY + b, after_frac);
                if coeff == 0.0 {
                    continue;
                }
                left += source[0] * coeff;
                right += source[1] * coeff;
            }
        } else if first >= lo && last < hi {
            // Pitching up: the kernel is wider and its steps are not whole
            // table steps, so each tap's position is worked out as the edge
            // path does. Still no folding, and the same sums in the same
            // order: bit for bit what the edge path gives.
            let taps = &frames[first as usize..=last as usize];
            for (index, source) in (first..).zip(taps) {
                let coeff = self.tap((pos - index as f64).abs() * ratio);
                if coeff == 0.0 {
                    continue;
                }
                left += source[0] * coeff;
                right += source[1] * coeff;
            }
        } else {
            for index in first..=last {
                let coeff = self.tap((pos - index as f64).abs() * ratio);
                if coeff == 0.0 {
                    continue;
                }
                if let Some(source) = region.frame(frames, index) {
                    left += source[0] * coeff;
                    right += source[1] * coeff;
                }
            }
        }

        let gain = ratio as f32;
        [left * gain, right * gain]
    }
}

/// The read as it was before MOO-247's fast paths: every tap through the
/// region's edge policy. Kept for the tests that pin the fast paths to it and
/// for the cost measurement.
#[cfg(test)]
impl SincTable {
    pub(crate) fn read_through_edges(
        &self,
        frames: &[[f32; 2]],
        pos: f64,
        rate: f64,
        region: Region,
    ) -> [f32; 2] {
        let len = frames.len();
        if len == 0 || !pos.is_finite() || !rate.is_finite() {
            return [0.0, 0.0];
        }
        let stretch = rate.abs().clamp(1.0, MAX_STRETCH);
        let ratio = 1.0 / stretch;
        let half_width = HALF_TAPS as f64 * stretch;
        let centre = pos.floor();
        let first = (centre - half_width).ceil() as i64;
        let last = (centre + half_width).floor() as i64;
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        for index in first..=last {
            let coeff = self.tap((pos - index as f64).abs() * ratio);
            if coeff == 0.0 {
                continue;
            }
            if let Some(source) = region.frame(frames, index) {
                left += source[0] * coeff;
                right += source[1] * coeff;
            }
        }
        let gain = ratio as f32;
        [left * gain, right * gain]
    }
}

/// Normalized sinc, `sin(pi t) / (pi t)`, with the removable singularity at
/// zero filled in. Zero at every non-zero integer, which is what makes an
/// unshifted read reproduce its input exactly.
fn sinc(t: f64) -> f64 {
    if t.abs() < 1.0e-9 {
        return 1.0;
    }
    let x = std::f64::consts::PI * t;
    x.sin() / x
}

/// Blackman window over `[-1, 1]`, evaluated on `|t|`. Chosen for its
/// stopband rather than its main-lobe width: a sampler's problem is foldback
/// that lands in the middle of the music, not a fraction of a dB at Nyquist.
fn blackman(t: f64) -> f64 {
    if t.abs() >= 1.0 {
        return 0.0;
    }
    let x = std::f64::consts::PI * (t.abs() + 1.0);
    0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(len: usize) -> Vec<[f32; 2]> {
        (0..len)
            .map(|index| {
                let value = index as f32 / len as f32;
                [value, value * 0.5]
            })
            .collect()
    }

    fn sine(len: usize, cycles_per_frame: f64) -> Vec<[f32; 2]> {
        (0..len)
            .map(|index| {
                let value = (std::f64::consts::TAU * cycles_per_frame * index as f64).sin() as f32;
                [value, value]
            })
            .collect()
    }

    /// The property the whole kernel design turns on: at unity rate an
    /// integer position must return its frame untouched, not a filtered
    /// approximation of it. `sinc` being zero at every non-zero integer is
    /// what buys this, and it is why the prototype is not cut below Nyquist.
    #[test]
    fn an_unshifted_read_reproduces_its_input() {
        let table = SincTable::shared();
        let frames = ramp(256);
        let region = Region::whole(frames.len());
        for index in 32..200 {
            let read = table.read(&frames, index as f64, 1.0, region);
            assert!(
                (read[0] - frames[index][0]).abs() < 1.0e-6,
                "frame {index}: read {} vs source {}",
                read[0],
                frames[index][0]
            );
            assert!((read[1] - frames[index][1]).abs() < 1.0e-6);
        }
    }

    /// Level correctness at unity: a constant signal has to come back at its
    /// own value, which is the kernel summing to one.
    #[test]
    fn a_constant_survives_a_fractional_read_at_its_own_level() {
        let table = SincTable::shared();
        let frames = vec![[0.5f32, -0.25]; 256];
        let region = Region::whole(frames.len());
        for step in 0..16 {
            let pos = 128.0 + step as f64 / 16.0;
            let read = table.read(&frames, pos, 1.0, region);
            assert!((read[0] - 0.5).abs() < 1.0e-3, "at {pos}: {}", read[0]);
            assert!((read[1] + 0.25).abs() < 1.0e-3, "at {pos}: {}", read[1]);
        }
    }

    /// The point of the exercise: reading a bright source faster than it was
    /// recorded has to fold back less than linear interpolation does.
    #[test]
    fn pitching_up_folds_back_less_than_linear_interpolation() {
        let table = SincTable::shared();
        // A tone near Nyquist is what folds worst when read faster.
        let frames = sine(4096, 0.4);
        let region = Region::whole(frames.len());
        let rate = 1.5;

        let (mut sinc_energy, mut linear_energy) = (0.0f64, 0.0f64);
        for step in 0..2048 {
            let pos = 512.0 + step as f64 * rate;
            let banded = table.read(&frames, pos, rate, region);

            let index = pos.floor() as usize;
            let frac = (pos - index as f64) as f32;
            let linear = frames[index][0] + (frames[index + 1][0] - frames[index][0]) * frac;

            sinc_energy += f64::from(banded[0]) * f64::from(banded[0]);
            linear_energy += f64::from(linear) * f64::from(linear);
        }
        // The band-limited path filters the content that would have folded,
        // so it carries materially less energy than the linear path, which
        // keeps that energy as alias.
        assert!(
            sinc_energy < linear_energy * 0.75,
            "band-limited {sinc_energy:.1} vs linear {linear_energy:.1}"
        );
    }

    /// A kernel overhanging a forward loop must read the material it is about
    /// to wrap into, not silence.
    #[test]
    fn a_forward_loop_reads_across_its_wrap_rather_than_into_silence() {
        let table = SincTable::shared();
        let frames = vec![[1.0f32, 1.0]; 512];
        let region = Region {
            start: 64.0,
            end: 192.0,
            edge: RegionEdge::Wrap,
        };
        // Sitting right on the loop end, most of the kernel hangs past it.
        let read = table.read(&frames, 191.5, 1.0, region);
        assert!(
            (read[0] - 1.0).abs() < 1.0e-3,
            "the wrap read {} instead of the constant it loops over",
            read[0]
        );
    }

    /// The same read against a one-shot's end, where silence is the truth.
    #[test]
    fn a_silent_edge_does_not_invent_material_past_the_region() {
        let table = SincTable::shared();
        let frames = vec![[1.0f32, 1.0]; 512];
        let region = Region {
            start: 64.0,
            end: 192.0,
            edge: RegionEdge::Silent,
        };
        let read = table.read(&frames, 191.5, 1.0, region);
        assert!(
            read[0] < 0.75,
            "a silent edge returned {}, so it read past the region",
            read[0]
        );
    }

    /// Every path stays finite and in bounds, including rates far past the
    /// stretch cap and positions outside the sample entirely.
    #[test]
    fn extreme_rates_and_positions_stay_finite_and_in_bounds() {
        let table = SincTable::shared();
        let frames = sine(64, 0.25);
        for edge in [RegionEdge::Silent, RegionEdge::Wrap, RegionEdge::Mirror] {
            let region = Region {
                start: 8.0,
                end: 40.0,
                edge,
            };
            for rate in [0.001, 0.5, 1.0, 8.0, 64.0, 1000.0] {
                for pos in [-500.0, -1.0, 0.0, 8.0, 39.9, 63.0, 500.0] {
                    let read = table.read(&frames, pos, rate, region);
                    assert!(
                        read[0].is_finite() && read[1].is_finite(),
                        "{edge:?} at rate {rate} pos {pos} produced {read:?}"
                    );
                    assert!(read[0].abs() <= 2.0, "{edge:?} {rate} {pos}: {read:?}");
                }
            }
        }
    }

    /// A loop shorter than the kernel folds many times over. The read must
    /// still land inside the region rather than walking off the sample.
    fn noise(len: usize) -> Vec<[f32; 2]> {
        let mut state = 0x9e37_79b9_u32;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let a = (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let b = (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
                [a, b]
            })
            .collect()
    }

    fn test_regions() -> Vec<Region> {
        vec![
            Region::whole(4_000),
            Region { start: 100.5, end: 3_000.25, edge: RegionEdge::Wrap },
            Region { start: 200.0, end: 230.0, edge: RegionEdge::Wrap },
            Region { start: 300.0, end: 3_500.0, edge: RegionEdge::Mirror },
            Region {
                start: 1_000.0,
                end: 3_000.0,
                edge: RegionEdge::Crossfade { fade: 300, floor: 500, head: 0 },
            },
            Region {
                start: 0.0,
                end: 2_000.0,
                edge: RegionEdge::Crossfade { fade: 300, floor: 0, head: 120 },
            },
            Region { start: 50.0, end: 5_000.0, edge: RegionEdge::Silent },
        ]
    }

    /// MOO-247's fast paths read the same taps in the same order as the
    /// edge-resolving loop, so every read is bit for bit what it was: every
    /// edge, positions near and far from them, whole and fractional, at
    /// rates that narrow the kernel and ones that do not.
    #[test]
    fn the_fast_reads_are_bit_identical_to_reading_through_the_edges() {
        let table = SincTable::shared();
        let frames = noise(4_000);
        let (mut unity_reads, mut unity_identical) = (0usize, 0usize);
        for region in test_regions() {
            for rate in [0.25, 0.5, 0.749, 1.0, 1.0001, 1.5, 2.0, 3.3, 4.0, 6.0] {
                let mut pos = -40.0f64;
                while pos < 4_060.0 {
                    let fast = table.read(&frames, pos, rate, region);
                    let slow = table.read_through_edges(&frames, pos, rate, region);
                    if rate == 1.0 && pos.fract() == 0.0 {
                        // The unity path: the frame itself, which the full
                        // sum is to within the table's sinc zeros.
                        unity_reads += 1;
                        for channel in 0..2 {
                            assert!(
                                (fast[channel] - slow[channel]).abs() < 1.0e-12,
                                "{region:?} unity at {pos}: {fast:?} vs {slow:?}"
                            );
                        }
                        if fast == slow {
                            unity_identical += 1;
                        }
                    } else {
                        assert_eq!(
                            (fast[0].to_bits(), fast[1].to_bits()),
                            (slow[0].to_bits(), slow[1].to_bits()),
                            "{region:?} rate {rate} at {pos}"
                        );
                    }
                    pos += if pos.fract() == 0.0 { 0.371 } else { 0.629 };
                }
                // And every whole position, which the stepping above
                // mostly misses: the unity path at 1.0.
                for whole in -40..4_060 {
                    let pos = f64::from(whole);
                    let fast = table.read(&frames, pos, rate, region);
                    let slow = table.read_through_edges(&frames, pos, rate, region);
                    if rate == 1.0 {
                        unity_reads += 1;
                        for channel in 0..2 {
                            assert!(
                                (fast[channel] - slow[channel]).abs() < 1.0e-12,
                                "{region:?} unity at {pos}: {fast:?} vs {slow:?}"
                            );
                        }
                        if fast == slow {
                            unity_identical += 1;
                        }
                    } else {
                        assert_eq!(
                            (fast[0].to_bits(), fast[1].to_bits()),
                            (slow[0].to_bits(), slow[1].to_bits()),
                            "{region:?} rate {rate} at {pos}"
                        );
                    }
                }
            }
        }
        // Inside a region, a unity read is the frame to the bit; the only
        // differences are past a silent edge, where the full sum picks up
        // the zeros' residue from the frames inside.
        assert!(unity_reads > 1_000);
        assert!(
            unity_identical * 100 >= unity_reads * 97,
            "{unity_identical} of {unity_reads} unity reads identical"
        );
    }

    /// What one voice's reads cost per 128-frame block, before and after the
    /// fast paths, in one run: each variant's fastest of 9 passes.
    ///
    /// ```sh
    /// cargo test -p mooloop-dsp --release --lib sinc_read_cost -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "measures wall time; run deliberately in release"]
    fn sinc_read_cost() {
        use std::time::Instant;
        let table = SincTable::shared();
        let frames = noise(96_000);
        let region = Region { start: 0.0, end: 96_000.0, edge: RegionEdge::Silent };
        let looped = Region { start: 1_000.0, end: 90_000.0, edge: RegionEdge::Wrap };
        println!();
        println!("  case                         edges µs/block   fast µs/block");
        for (label, rate, start, reg) in [
            ("unity, whole frames", 1.0, 100.0, region),
            ("unity, whole frames, loop", 1.0, 1_100.0, looped),
            ("half rate", 0.5, 100.25, region),
            ("0.7491 (a fifth down)", 0.7491, 100.0, region),
            ("1.4983 (a fifth up)", 1.4983, 100.0, region),
            ("2.0", 2.0, 100.0, region),
            ("3.0", 3.0, 100.0, region),
        ] {
            let blocks = 400usize;
            let mut best = [f64::MAX; 2];
            for _ in 0..9 {
                for (variant, slot) in best.iter_mut().enumerate() {
                    let mut pos: f64 = start;
                    let started = Instant::now();
                    for _ in 0..blocks * 128 {
                        let frame = if variant == 0 {
                            table.read_through_edges(&frames, pos, rate, reg)
                        } else {
                            table.read(&frames, pos, rate, reg)
                        };
                        std::hint::black_box(frame);
                        pos += rate;
                    }
                    let per_block = started.elapsed().as_nanos() as f64 / blocks as f64 / 1e3;
                    *slot = slot.min(per_block);
                }
            }
            println!("  {label:<28} {:>14.2}  {:>14.2}", best[0], best[1]);
        }
    }

    #[test]
    fn a_loop_shorter_than_the_kernel_still_resolves_inside_itself() {
        let region = Region {
            start: 10.0,
            end: 13.0,
            edge: RegionEdge::Wrap,
        };
        for index in -200..200 {
            if let Some(frame) = region.resolve(index, 64) {
                assert!(
                    (10..13).contains(&frame),
                    "index {index} resolved to {frame}, outside the loop"
                );
            }
        }
        let mirror = Region {
            start: 10.0,
            end: 13.0,
            edge: RegionEdge::Mirror,
        };
        for index in -200..200 {
            if let Some(frame) = mirror.resolve(index, 64) {
                assert!(
                    (10..13).contains(&frame),
                    "index {index} mirrored to {frame}, outside the loop"
                );
            }
        }
    }
}
