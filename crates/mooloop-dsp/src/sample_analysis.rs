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

// ---------------------------------------------------------------------------
// Onset detection (MOO-44)
// ---------------------------------------------------------------------------

/// What the slice detector is asked for. Two musical controls, and nothing
/// that names the algorithm inside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnsetSettings {
    /// How small a hit still counts, from 0 (only the loudest) to 1 (every
    /// ghost note).
    pub sensitivity: f32,
    /// The closest two markers may land, in milliseconds. A flam or a roll
    /// faster than this is one slice.
    pub min_spacing_ms: f32,
}

impl Default for OnsetSettings {
    fn default() -> Self {
        Self {
            sensitivity: 0.5,
            min_spacing_ms: 50.0,
        }
    }
}

/// The analysis hop: about 2.7 ms at 48 kHz. A marker lands on the hop its
/// attack begins in.
const ONSET_HOP: usize = 128;

/// The energy window, trailing each hop: four hops, about 11 ms. Long enough
/// that a low tone's cycles do not read as a stream of rises, short enough
/// that one hit does not hide the next.
const ONSET_WINDOW: usize = 4 * ONSET_HOP;

/// How far back from a detected peak the refinement may walk to find where
/// the attack began, in hops.
const ONSET_REFINE_HOPS: usize = 4;

/// Onsets in `frames[start..end]`, as source frames, earliest first.
///
/// A time-domain detector, because a break's hits are broadband and the
/// sampler has no FFT to lean on. Per hop, it takes the energy of each
/// channel, and of each channel's first difference (which weights the
/// attack's high end, and is what separates a hit from a swell), over a
/// window trailing the hop. The detection function is the rise in log energy
/// from one hop to the next, summed over both channels and both bands. So a
/// transient panned hard to one side counts, and a steady tone, however
/// loud, does not.
///
/// A hop is an onset when its rise is a local maximum and clears a threshold
/// that follows the median of its neighbourhood, so a dense roll and a
/// sparse intro are judged on their own terms. It must also land at least
/// `min_spacing_ms` after the last one kept. When two compete inside the
/// spacing the stronger wins, and a tie goes to the earlier, so the result
/// is deterministic. The kept hop is then walked back to where its energy
/// started climbing, so the marker sits before the attack rather than on its
/// loudest point.
///
/// Allocates, and reads the whole region: control thread only.
pub fn detect_onsets(
    frames: &[[f32; 2]],
    sample_rate: u32,
    start: usize,
    end: usize,
    settings: OnsetSettings,
) -> Vec<usize> {
    let end = end.min(frames.len());
    if end <= start + 2 * ONSET_HOP || sample_rate == 0 {
        return Vec::new();
    }
    // Running sums of each band's energy, so any window is two lookups.
    let span = end - start;
    let mut sums = vec![[0.0f64; 4]; span + 1];
    let mut previous = frames[start];
    for n in 0..span {
        let frame = frames[start + n];
        let (l, r) = (f64::from(frame[0]), f64::from(frame[1]));
        let (dl, dr) = (
            f64::from(frame[0] - previous[0]),
            f64::from(frame[1] - previous[1]),
        );
        previous = frame;
        let before = sums[n];
        sums[n + 1] = [
            before[0] + l * l,
            before[1] + r * r,
            before[2] + dl * dl,
            before[3] + dr * dr,
        ];
    }
    let hops = span / ONSET_HOP;
    let energy = |hop: usize, band: usize| {
        let to = ((hop + 1) * ONSET_HOP).min(span);
        let from = to.saturating_sub(ONSET_WINDOW);
        (sums[to][band] - sums[from][band]).max(0.0)
    };
    // A floor 90 dB under the region's loudest window: below it a rise is
    // noise in the tail, not a hit.
    let mut loudest = 0.0f64;
    for hop in 0..hops {
        for band in 0..4 {
            loudest = loudest.max(energy(hop, band));
        }
    }
    if loudest <= 0.0 || !loudest.is_finite() {
        return Vec::new();
    }
    let floor = loudest * 1.0e-9;
    let log = |value: f64| (value.max(floor) / floor).ln();
    // The region's first hop rises from silence: a region that starts on a
    // hit starts on an onset, which is where a first slice belongs.
    let mut rise = vec![0.0f64; hops];
    for (hop, value) in rise.iter_mut().enumerate() {
        *value = (0..4)
            .map(|band| {
                let before = if hop == 0 { 0.0 } else { energy(hop - 1, band) };
                (log(energy(hop, band)) - log(before)).max(0.0)
            })
            .sum();
    }

    // Sensitivity maps onto the threshold above the local median, in the
    // same log units summed over four bands: 0 asks for a rise of 12, about
    // +13 dB in each band, and 1 for a rise of 2.
    let sensitivity = f64::from(settings.sensitivity.clamp(0.0, 1.0));
    let margin = 12.0 - 10.0 * sensitivity;
    let median_span = (0.25 * f64::from(sample_rate) / ONSET_HOP as f64).round() as usize;
    let spacing_hops = (f64::from(settings.min_spacing_ms.max(0.0)) / 1_000.0
        * f64::from(sample_rate)
        / ONSET_HOP as f64)
        .round()
        .max(1.0) as usize;
    let mut window = Vec::with_capacity(2 * median_span + 1);
    let mut kept: Vec<(usize, f64)> = Vec::new();
    for hop in 0..hops {
        let value = rise[hop];
        let before = if hop == 0 { 0.0 } else { rise[hop - 1] };
        let is_peak = value > before && (hop + 1 >= hops || value >= rise[hop + 1]);
        if !is_peak {
            continue;
        }
        window.clear();
        window.extend_from_slice(
            &rise[hop.saturating_sub(median_span)..(hop + median_span + 1).min(hops)],
        );
        window.sort_by(|a, b| a.total_cmp(b));
        let median = window[window.len() / 2];
        if value < median + margin {
            continue;
        }
        match kept.last_mut() {
            Some((last, strength)) if hop - *last < spacing_hops => {
                if value > *strength {
                    *last = hop;
                    *strength = value;
                }
            }
            _ => kept.push((hop, value)),
        }
    }

    // Walk each back to where its attack began: the earliest hop, within
    // reach and after the previous onset, whose rise is still above a tenth
    // of the peak's.
    let mut onsets = Vec::with_capacity(kept.len());
    let mut after = 0usize;
    for (hop, strength) in kept {
        let mut first = hop;
        while first > after
            && hop - first < ONSET_REFINE_HOPS
            && rise[first - 1] > strength * 0.1
        {
            first -= 1;
        }
        onsets.push(start + first * ONSET_HOP);
        after = hop + 1;
    }
    onsets
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
    // --- Onset detection (MOO-44) ------------------------------------------

    const SR: u32 = 48_000;

    /// A deterministic noise source for the fixtures.
    fn noise(seed: u32) -> impl FnMut() -> f32 {
        let mut state = seed;
        move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }
    }

    /// Adds a hit at `at`: a kick (a falling sine), a snare (noise and a
    /// tone) or a hat (bright noise, short), scaled by `gain`, into the
    /// channels `pan` says ([left, right] gains).
    fn hit(frames: &mut [[f32; 2]], at: usize, kind: u8, gain: f32, pan: [f32; 2]) {
        let mut rng = noise(at as u32 ^ u32::from(kind) << 16);
        let mut phase = 0.0f32;
        let mut previous = 0.0f32;
        let length = match kind {
            0 => SR as usize / 4,
            1 => SR as usize / 6,
            _ => SR as usize / 20,
        };
        for k in 0..length {
            let Some(frame) = frames.get_mut(at + k) else { break };
            let t = k as f32 / SR as f32;
            let value = match kind {
                0 => {
                    phase += std::f32::consts::TAU * (50.0 + 110.0 * (-t / 0.03).exp()) / SR as f32;
                    phase.sin() * (-t / 0.1).exp()
                }
                1 => {
                    0.6 * rng() * (-t / 0.05).exp()
                        + 0.4 * (std::f32::consts::TAU * 190.0 * t).sin() * (-t / 0.07).exp()
                }
                _ => {
                    // A first difference of noise is bright.
                    let n = rng();
                    let bright = n - previous;
                    previous = n;
                    0.5 * bright * (-t / 0.012).exp()
                }
            };
            // Faded out over its last fifth, so the fixture's own cutoff is
            // not a click the detector would rightly find.
            let value = value * ((length - k) as f32 / (length as f32 * 0.2)).min(1.0);
            frame[0] += gain * pan[0] * value;
            frame[1] += gain * pan[1] * value;
        }
    }

    fn silence(seconds: f32) -> Vec<[f32; 2]> {
        vec![[0.0, 0.0]; (seconds * SR as f32) as usize]
    }

    /// Every detected onset within `tolerance` frames before (or a hop
    /// after) a known hit, and every hit found once.
    fn assert_found(found: &[usize], hits: &[usize], what: &str) {
        let tolerance_before = 2 * 128;
        let tolerance_after = 128;
        assert_eq!(found.len(), hits.len(), "{what}: found {found:?} for hits {hits:?}");
        for (&onset, &hit) in found.iter().zip(hits) {
            assert!(
                onset + tolerance_before >= hit && onset <= hit + tolerance_after,
                "{what}: onset {onset} is not at the hit at {hit} ({found:?})"
            );
        }
    }

    /// The headline: a two-bar kick/snare/hat break at 120 BPM gets one
    /// marker per hit at the default settings, each just before its attack.
    #[test]
    fn a_break_gets_one_marker_per_hit() {
        let mut frames = silence(4.0);
        let sixteenth = SR as usize / 8;
        let mut hits = Vec::new();
        for step in 0..32 {
            let at = step * sixteenth;
            let kind = match step % 16 {
                0 | 6 | 10 => Some(0),
                4 | 12 => Some(1),
                s if s % 2 == 0 => Some(2),
                _ => None,
            };
            if let Some(kind) = kind {
                hit(&mut frames, at, kind, 0.8, [1.0, 1.0]);
                hits.push(at);
            }
        }
        let found = detect_onsets(&frames, SR, 0, frames.len(), OnsetSettings::default());
        assert_found(&found, &hits, "the break");
    }

    /// A quiet lead-in (a hat at -30 dB) before the loud break is still a
    /// hit at a high sensitivity, and the break still gets its own markers.
    #[test]
    fn a_quiet_lead_in_is_found_at_high_sensitivity() {
        let mut frames = silence(2.0);
        hit(&mut frames, 4_800, 2, 0.03, [1.0, 1.0]);
        hit(&mut frames, 24_000, 0, 0.9, [1.0, 1.0]);
        hit(&mut frames, 48_000, 1, 0.9, [1.0, 1.0]);
        let settings = OnsetSettings { sensitivity: 0.9, ..OnsetSettings::default() };
        let found = detect_onsets(&frames, SR, 0, frames.len(), settings);
        assert_found(&found, &[4_800, 24_000, 48_000], "the lead-in");
    }

    /// A transient in one channel only is a hit: the channels are measured
    /// separately and summed, so a hard-panned hat is not averaged away.
    #[test]
    fn a_transient_in_one_channel_is_found() {
        let mut frames = silence(1.0);
        // A steady tone in the left channel throughout (its start is an
        // onset of its own), and a snare only in the right.
        for (n, frame) in frames.iter_mut().enumerate() {
            frame[0] = 0.5 * (std::f32::consts::TAU * 110.0 * n as f32 / SR as f32).sin();
        }
        hit(&mut frames, 20_000, 1, 0.8, [0.0, 1.0]);
        let found = detect_onsets(&frames, SR, 0, frames.len(), OnsetSettings::default());
        assert_found(&found, &[0, 20_000], "a right-channel snare over a left-channel tone");
    }

    /// A dense roll: thirty-second-note hats at 120 BPM (62.5 ms apart) are
    /// each a hit at a spacing under their gap, and merge at a spacing over
    /// it.
    #[test]
    fn a_dense_roll_resolves_at_its_spacing() {
        let mut frames = silence(1.2);
        let gap = SR as usize / 16;
        let hits: Vec<usize> = (0..16).map(|n| 2_400 + n * gap).collect();
        for &at in &hits {
            hit(&mut frames, at, 2, 0.6, [1.0, 1.0]);
        }
        let fine = OnsetSettings { min_spacing_ms: 40.0, ..OnsetSettings::default() };
        let found = detect_onsets(&frames, SR, 0, frames.len(), fine);
        assert_found(&found, &hits, "the roll at 40 ms spacing");
        let coarse = OnsetSettings { min_spacing_ms: 100.0, ..OnsetSettings::default() };
        let merged = detect_onsets(&frames, SR, 0, frames.len(), coarse);
        assert!(merged.len() <= hits.len() / 2 + 1, "100 ms spacing kept {} of 16", merged.len());
    }

    /// A steady tone and silence have no onsets, however sensitive.
    #[test]
    fn a_steady_tone_has_no_onsets() {
        let mut frames = silence(1.0);
        for (n, frame) in frames.iter_mut().enumerate().skip(4_800) {
            let value = 0.5 * (std::f32::consts::TAU * 220.0 * n as f32 / SR as f32).sin();
            *frame = [value, value];
        }
        let settings = OnsetSettings { sensitivity: 1.0, ..OnsetSettings::default() };
        let found = detect_onsets(&frames, SR, 0, frames.len(), settings);
        // The tone's own start is an onset; nothing after it is.
        assert!(found.len() <= 1, "a steady tone read as {found:?}");
        assert!(detect_onsets(&silence(1.0), SR, 0, 48_000, settings).is_empty());
    }

    /// More sensitivity never finds fewer onsets, and more spacing never
    /// finds more; the same input always gives the same markers.
    #[test]
    fn sensitivity_and_spacing_are_monotonic_and_deterministic() {
        let mut frames = silence(2.0);
        for (n, gain) in [0.9f32, 0.5, 0.2, 0.08, 0.03, 0.01].into_iter().enumerate() {
            hit(&mut frames, 2_400 + n * 14_000, 2, gain, [1.0, 1.0]);
        }
        let mut last = 0;
        for step in 0..=10 {
            let settings = OnsetSettings { sensitivity: step as f32 / 10.0, ..OnsetSettings::default() };
            let found = detect_onsets(&frames, SR, 0, frames.len(), settings);
            assert!(found.len() >= last, "sensitivity {step}/10 found fewer ({found:?})");
            assert_eq!(found, detect_onsets(&frames, SR, 0, frames.len(), settings));
            last = found.len();
        }
        let mut last = usize::MAX;
        for spacing in [10.0, 30.0, 100.0, 300.0, 1_000.0] {
            let settings = OnsetSettings { min_spacing_ms: spacing, sensitivity: 1.0 };
            let found = detect_onsets(&frames, SR, 0, frames.len(), settings);
            assert!(found.len() <= last, "spacing {spacing} ms found more ({found:?})");
            last = found.len();
        }
    }
}
