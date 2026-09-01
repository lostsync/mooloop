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
            RegionEdge::Wrap => start + (index - start).rem_euclid(span),
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
    fn tap(&self, t: f64) -> f32 {
        let scaled = t * DENSITY as f64;
        if scaled < 0.0 {
            return 0.0;
        }
        let index = scaled as usize;
        if index + 1 >= TABLE_LEN {
            return 0.0;
        }
        let frac = (scaled - index as f64) as f32;
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
            if let Some(frame) = region.resolve(index, len) {
                let source = frames[frame];
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
