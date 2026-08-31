//! Control-side analysis over decoded sample data.
//!
//! Distinct from [`crate::analysis`], which runs on the audio thread over a
//! rolling window to feed device displays. Everything here reads whole
//! samples off-thread: it may allocate and take its time, and none of it is
//! allowed anywhere near `process()`.

use core::ops::RangeInclusive;

/// How far either side of the requested frame a snap searches, by default,
/// in milliseconds. Roughly one cycle of a low bass note: far enough to find
/// a crossing in most material, short enough that the marker never lands
/// somewhere the user did not mean.
pub const DEFAULT_SNAP_WINDOW_MS: f32 = 10.0;

/// How close to zero a channel must sit to count as joinable, as a fraction
/// of the search window's own peak. Relative rather than absolute so a quiet
/// passage and a loud one are judged on the same terms.
const LEVEL_TOLERANCE: f32 = 0.05;

/// Where a marker ended up, and where it was asked for. Both are carried so
/// the editor can show the difference: a snap the user cannot see is
/// indistinguishable from marker drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapResult {
    pub requested: usize,
    pub resolved: usize,
}

impl SnapResult {
    /// No acceptable boundary was found, so the marker stays exactly where it
    /// was asked for. This is a normal outcome, not a failure.
    fn unchanged(requested: usize) -> Self {
        Self {
            requested,
            resolved: requested,
        }
    }

    pub fn moved(self) -> bool {
        self.requested != self.resolved
    }

    /// Signed distance the marker travelled, in frames.
    pub fn offset(self) -> i64 {
        self.resolved as i64 - self.requested as i64
    }
}

/// Which way a channel is travelling as it passes zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Rising,
    Falling,
}

/// Convert the search window from milliseconds to frames, never returning
/// zero: a zero-frame window would make every snap a no-op that silently
/// looked like "no crossing found".
pub fn snap_window_frames(window_ms: f32, sample_rate: u32) -> usize {
    ((window_ms.max(0.0) / 1_000.0) * sample_rate as f32)
        .round()
        .max(1.0) as usize
}

/// Resolve `requested` onto the nearest acceptable zero crossing within
/// `window` frames, without leaving `bounds`.
///
/// `bounds` is the inclusive range the result must stay inside. Callers pass
/// the region the marker belongs to — a loop start is bounded below by the
/// play start and above by the loop end — so snapping can never invert or
/// collapse a valid region no matter what the audio does.
///
/// Preference is tiered rather than a weighted score, because the tiers are
/// the actual musical argument and a weight vector is not:
///
/// 1. Both channels cross zero rising. Two markers that both land on rising
///    crossings join continuously in level *and* slope, which is the whole
///    point at a loop boundary.
/// 2. Both channels cross zero in the same direction.
/// 3. Both channels are quiet enough to be joined without a step.
///
/// Within a tier the nearest candidate wins, and a tie goes to the earlier
/// frame so the result is deterministic. If no tier matches, the marker is
/// left alone.
pub fn snap_to_zero_crossing(
    frames: &[[f32; 2]],
    requested: usize,
    window: usize,
    bounds: RangeInclusive<usize>,
) -> SnapResult {
    if frames.len() < 2 || window == 0 || bounds.is_empty() {
        return SnapResult::unchanged(requested);
    }
    let last = frames.len() - 1;
    let low = (*bounds.start()).min(last);
    let high = (*bounds.end()).min(last);
    if low >= high {
        return SnapResult::unchanged(requested);
    }
    let requested = requested.clamp(low, high);

    // A crossing is detected between `i - 1` and `i`, so the first frame can
    // never be a candidate and the search floor is at least 1.
    let first = low.max(1).max(requested.saturating_sub(window));
    let final_frame = high.min(requested.saturating_add(window));
    if first > final_frame {
        return SnapResult::unchanged(requested);
    }

    let mut peak = 0.0f32;
    for frame in &frames[first - 1..=final_frame] {
        peak = peak.max(frame[0].abs()).max(frame[1].abs());
    }
    // Digital silence has no peak to scale against and no crossing to find.
    // Every frame in it is an equally good boundary, so keep the user's.
    if peak <= f32::EPSILON {
        return SnapResult::unchanged(requested);
    }
    let tolerance = peak * LEVEL_TOLERANCE;

    let mut best: Option<(u8, usize, usize)> = None;
    for candidate in first..=final_frame {
        let previous = frames[candidate - 1];
        let current = frames[candidate];
        let Some(tier) = candidate_tier(previous, current, tolerance) else {
            continue;
        };
        let distance = candidate.abs_diff(requested);
        let better = match best {
            None => true,
            Some((best_tier, best_distance, _)) => (tier, distance) < (best_tier, best_distance),
        };
        if better {
            best = Some((tier, distance, candidate));
        }
    }

    match best {
        Some((_, _, resolved)) => SnapResult {
            requested,
            resolved,
        },
        None => SnapResult::unchanged(requested),
    }
}

/// Rank one candidate frame, lower being better, or `None` if it is not an
/// acceptable boundary at all.
fn candidate_tier(previous: [f32; 2], current: [f32; 2], tolerance: f32) -> Option<u8> {
    let left = channel_crossing(previous[0], current[0]);
    let right = channel_crossing(previous[1], current[1]);
    let quiet = |value: f32| value.abs() <= tolerance;
    let both_quiet = quiet(current[0]) && quiet(current[1]);

    // A channel that is already quiet here can be joined whatever it is
    // doing, which is what makes hard-panned and mono-in-stereo material
    // snappable: the silent side never crosses because it never moves.
    let agrees = |direction: Direction, crossing: Option<Direction>, value: f32| {
        crossing == Some(direction) || quiet(value)
    };

    for (tier, direction) in [(0u8, Direction::Rising), (1, Direction::Falling)] {
        let real_crossing = left == Some(direction) || right == Some(direction);
        if real_crossing
            && agrees(direction, left, current[0])
            && agrees(direction, right, current[1])
        {
            return Some(tier);
        }
    }
    both_quiet.then_some(2)
}

/// The direction a channel passes through zero between two frames, if it
/// does. Equality with zero counts as the crossing frame rather than being
/// skipped, so a signal that touches zero exactly is still a candidate.
fn channel_crossing(previous: f32, current: f32) -> Option<Direction> {
    if previous <= 0.0 && current > 0.0 {
        Some(Direction::Rising)
    } else if previous >= 0.0 && current < 0.0 {
        Some(Direction::Falling)
    } else {
        None
    }
}

/// Convert a stored normalized marker to a frame index.
///
/// Markers persist as `f32` fractions of the sample length, so the frame a
/// fraction denotes is only exact while the length fits the mantissa: below
/// 2^23 frames (about 175 seconds at 48 kHz) `frame -> fraction -> frame`
/// round-trips exactly, and above it a marker can land a frame or more away
/// from where it was resolved. Loops, the material this exists for, sit well
/// inside that. Long-sample marker precision needs a representation change,
/// not a rounding change.
pub fn frame_from_fraction(fraction: f32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    ((fraction.clamp(0.0, 1.0) * len as f32).round() as usize).min(len - 1)
}

/// Convert a resolved frame index back to the stored normalized marker.
pub fn fraction_from_frame(frame: usize, len: usize) -> f32 {
    if len == 0 {
        return 0.0;
    }
    (frame as f32 / len as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sine at `period` frames, identical in both channels.
    fn sine(len: usize, period: f32) -> Vec<[f32; 2]> {
        (0..len)
            .map(|i| {
                let s = (core::f32::consts::TAU * i as f32 / period).sin();
                [s, s]
            })
            .collect()
    }

    fn snap(frames: &[[f32; 2]], requested: usize, window: usize) -> SnapResult {
        snap_to_zero_crossing(frames, requested, window, 0..=frames.len() - 1)
    }

    #[test]
    fn a_marker_lands_on_the_rising_crossing_of_a_sine() {
        // Period 100 rises through zero at 0, 100, 200, ... and falls at 50,
        // 150, ... Asked for 104, the rising crossing at 100 is nearest.
        let frames = sine(1_000, 100.0);
        let result = snap(&frames, 104, 20);
        assert_eq!(result.resolved, 100);
        assert!(result.moved());
    }

    #[test]
    fn a_rising_crossing_is_preferred_over_a_nearer_falling_one() {
        // 148 is two frames from the falling crossing at 150 and 48 from the
        // rising one at 100. The tier ordering must still choose 100.
        let frames = sine(1_000, 100.0);
        assert_eq!(snap(&frames, 148, 60).resolved, 100);
    }

    #[test]
    fn a_falling_crossing_is_accepted_when_no_rising_one_is_in_range() {
        // Window of 5 around 148 cannot reach the rising crossing at 100.
        let frames = sine(1_000, 100.0);
        assert_eq!(snap(&frames, 148, 5).resolved, 150);
    }

    #[test]
    fn no_acceptable_crossing_leaves_the_marker_alone() {
        // Constant DC never crosses zero and is never quiet, so there is
        // nothing to snap to and the request must survive untouched.
        let frames = vec![[0.8, 0.8]; 500];
        let result = snap(&frames, 250, 50);
        assert_eq!(result.resolved, 250);
        assert!(!result.moved());
    }

    #[test]
    fn a_channel_still_stepping_disqualifies_the_other_channels_crossing() {
        // Left crosses zero rising at 100; right sits at full scale the whole
        // time. Snapping there would fix the left seam and leave the right
        // one, which is the failure this scoring exists to prevent.
        let mut frames = sine(400, 100.0);
        for frame in frames.iter_mut() {
            frame[1] = 0.9;
        }
        assert!(!snap(&frames, 104, 20).moved());
    }

    #[test]
    fn a_silent_channel_does_not_block_the_other_ones_crossing() {
        // Hard-panned material: the right channel never crosses because it
        // never moves, but it can be joined anywhere without a step.
        let mut frames = sine(400, 100.0);
        for frame in frames.iter_mut() {
            frame[1] = 0.0;
        }
        assert_eq!(snap(&frames, 104, 20).resolved, 100);
    }

    #[test]
    fn bounds_prevent_a_snap_from_inverting_a_region() {
        // The nearest rising crossing is at 100, but a loop end pinned above
        // 120 must not be dragged below it. With 100 excluded and 200 out of
        // the window, the falling crossing at 150 is the correct answer:
        // staying inside the region outranks the preference for rising.
        let frames = sine(1_000, 100.0);
        let result = snap_to_zero_crossing(&frames, 124, 40, 120..=999);
        assert!(result.resolved >= 120);
        assert_eq!(result.resolved, 150);
    }

    #[test]
    fn a_bounded_search_still_reaches_a_rising_crossing_it_can_see() {
        // Same lower bound, but a window wide enough to include 200. The
        // rising crossing must win over the nearer falling one at 150.
        let frames = sine(1_000, 100.0);
        assert_eq!(
            snap_to_zero_crossing(&frames, 124, 100, 120..=999).resolved,
            200
        );
    }

    #[test]
    fn bounds_tighter_than_the_window_cannot_escape_them() {
        let frames = sine(1_000, 100.0);
        let result = snap_to_zero_crossing(&frames, 130, 500, 128..=132);
        assert!((128..=132).contains(&result.resolved));
    }

    #[test]
    fn digital_silence_keeps_the_requested_frame() {
        let frames = vec![[0.0, 0.0]; 500];
        assert!(!snap(&frames, 250, 50).moved());
    }

    #[test]
    fn a_degenerate_sample_is_not_a_panic() {
        assert!(!snap_to_zero_crossing(&[], 0, 10, 0..=0).moved());
        assert!(!snap_to_zero_crossing(&[[0.0, 0.0]], 0, 10, 0..=0).moved());
    }

    #[test]
    fn ties_resolve_to_the_earlier_frame() {
        // Rising crossings at 100 and 200; 150 is exactly between them.
        let frames = sine(1_000, 100.0);
        assert_eq!(snap(&frames, 150, 60).resolved, 100);
    }

    #[test]
    fn markers_round_trip_through_their_stored_fraction() {
        // The precision claim in `frame_from_fraction`'s documentation, held
        // to at a length well inside the mantissa.
        let len = 480_000;
        for frame in [0, 1, 12_345, 240_000, len - 1] {
            let fraction = fraction_from_frame(frame, len);
            assert_eq!(frame_from_fraction(fraction, len), frame);
        }
    }

    #[test]
    fn the_window_never_collapses_to_nothing() {
        assert!(snap_window_frames(0.0, 48_000) >= 1);
        assert_eq!(snap_window_frames(10.0, 48_000), 480);
    }
}
