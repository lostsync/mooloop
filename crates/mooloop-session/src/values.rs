//! Reading and writing the numbers a control shows.
//!
//! Knob travel, typed entry, and the tempo measurement that seeds a stretch.
//! None of it needs a widget: the mappings are the parameter's, and the view
//! only decides which control they are attached to.

use mooloop_core::{
    SamplerParams, MAX_STRETCH_BARS, MAX_STRETCH_GRAIN, MAX_STRETCH_RATIO, MIN_STRETCH_BARS,
    MIN_STRETCH_GRAIN, MIN_STRETCH_RATIO,
};

/// Parse a number a user typed into a value field.
///
/// Tolerant of the units the field itself displays, because the obvious thing
/// to do with a box reading "2.00x" is to type "4x". Anything unparseable
/// returns `None` and the field snaps back to the authoritative text rather
/// than committing a guess.
pub fn parse_typed_value(text: &str) -> Option<f32> {
    // The *leading* number, not every digit in the string. Stripping
    // non-digits and concatenating what is left looks equivalent and is not:
    // the grain field reads "256 fr / 375 Hz", so clicking into it and
    // pressing Enter unchanged would have committed 256375.
    let text = text.trim();
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        let numeric = ch.is_ascii_digit()
            || (ch == '.' && !text[..index].contains('.'))
            || (matches!(ch, '-' | '+') && index == 0);
        if !numeric {
            break;
        }
        end = index + ch.len_utf8();
    }
    text[..end]
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
}

/// Bar counts read as "2 bar" or "0.5 bar" rather than "2.000000": the
/// values that matter here are powers of two, and trailing zeros make a
/// snapped length look like an arbitrary one.
pub fn format_bars(bars: f32) -> String {
    if (bars - bars.round()).abs() < 1.0e-4 {
        format!("{} bar", bars.round() as i32)
    } else {
        format!("{bars:.3} bar")
    }
}

pub fn stretch_bars_to_norm(bars: f32) -> f32 {
    let bars = bars.clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS);
    (bars / MIN_STRETCH_BARS).log2() / (MAX_STRETCH_BARS / MIN_STRETCH_BARS).log2()
}

pub fn stretch_bars_from_norm(norm: f32) -> f32 {
    let span = (MAX_STRETCH_BARS / MIN_STRETCH_BARS).log2();
    (MIN_STRETCH_BARS * (norm.clamp(0.0, 1.0) * span).exp2())
        .clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS)
}

/// How many bars a channel's loop currently measures, at the project tempo.
///
/// The loop is what gets fitted when there is one, matching what the DSP
/// derives its ratio from; otherwise the playback region is.
pub fn measured_loop_bars(params: SamplerParams, frames: usize, sample_rate: u32, bpm: f64) -> f32 {
    let len = frames.max(1) as f32;
    // The same span fit-to-tempo will derive its ratio from, asked of the
    // sampler rather than re-decided here: a guess measured against one
    // region and a ratio derived from another is a seed that is wrong by
    // construction.
    let (start, end) = if mooloop_dsp::Sampler::fits_the_playback_region(params) {
        (params.start, params.end)
    } else {
        (params.loop_start, params.loop_end)
    };
    let region = ((end - start).max(0.0) * len).max(1.0);
    let per_bar = mooloop_core::frames_per_bar(sample_rate, bpm) as f32;
    region / per_bar.max(1.0)
}

/// Map the stretch ratio onto a knob's 0..1 travel, and back.
///
/// Logarithmic, matching the parameter's own `Exponential` curve: the band
/// that stays clean sits just around unity while the ceiling is deliberately
/// far past it, so linear travel would spend almost the whole knob on
/// extremes. Unity lands at about a third of the way round.
pub fn stretch_ratio_to_norm(ratio: f32) -> f32 {
    let ratio = ratio.clamp(MIN_STRETCH_RATIO, MAX_STRETCH_RATIO);
    (ratio / MIN_STRETCH_RATIO).log2() / (MAX_STRETCH_RATIO / MIN_STRETCH_RATIO).log2()
}

pub fn stretch_ratio_from_norm(norm: f32) -> f32 {
    let span = (MAX_STRETCH_RATIO / MIN_STRETCH_RATIO).log2();
    (MIN_STRETCH_RATIO * (norm.clamp(0.0, 1.0) * span).exp2())
        .clamp(MIN_STRETCH_RATIO, MAX_STRETCH_RATIO)
}

/// Same treatment for the grain window, for the same reason: it maps to a
/// repetition frequency, so equal knob travel should be equal musical
/// intervals.
pub fn stretch_grain_to_norm(frames: u16) -> f32 {
    let frames = f32::from(frames.clamp(MIN_STRETCH_GRAIN, MAX_STRETCH_GRAIN));
    (frames / f32::from(MIN_STRETCH_GRAIN)).log2()
        / (f32::from(MAX_STRETCH_GRAIN) / f32::from(MIN_STRETCH_GRAIN)).log2()
}

pub fn stretch_grain_from_norm(norm: f32) -> u16 {
    let span = (f32::from(MAX_STRETCH_GRAIN) / f32::from(MIN_STRETCH_GRAIN)).log2();
    let frames = f32::from(MIN_STRETCH_GRAIN) * (norm.clamp(0.0, 1.0) * span).exp2();
    (frames.round() as i32).clamp(
        i32::from(MIN_STRETCH_GRAIN),
        i32::from(MAX_STRETCH_GRAIN),
    ) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{snap_bars_to_power_of_two, LoopMode, PlayMode};

    /// A knob that reports a different number from the one the engine runs is
    /// worse than no knob. Round-trip both stretch mappings across their
    /// declared ranges.
    #[test]
    fn the_stretch_mappings_round_trip() {
        for ratio in [0.25f32, 0.5, 0.75, 1.0, 1.5, 2.0, 6.5, 16.0] {
            let back = stretch_ratio_from_norm(stretch_ratio_to_norm(ratio));
            assert!(
                (back - ratio).abs() < ratio * 1.0e-4,
                "ratio {ratio} came back as {back}"
            );
        }
        for frames in [64u16, 128, 192, 256, 1024, 2048, 4096] {
            let back = stretch_grain_from_norm(stretch_grain_to_norm(frames));
            assert_eq!(back, frames, "grain {frames} came back as {back}");
        }
    }

    /// Unity is the value a user returns to, so it has to be reachable and it
    /// has to be where the knob's default sits. The Slint default is 0.3333;
    /// this is what pins that number to something real rather than a guess.
    #[test]
    fn unity_stretch_sits_at_the_knobs_default_position() {
        let norm = stretch_ratio_to_norm(1.0);
        assert!(
            (norm - 0.3333).abs() < 0.001,
            "unity maps to {norm}, but the control defaults to 0.3333"
        );
        assert!((stretch_ratio_from_norm(0.3333) - 1.0).abs() < 0.001);

        let grain = stretch_grain_to_norm(1024);
        assert!(
            (grain - 0.6667).abs() < 0.001,
            "the default grain maps to {grain}, but the control defaults to 0.6667"
        );
    }

    /// The bars knob maps like the other two, and unity -- one bar -- has to
    /// be reachable and sit where the Slint control defaults.
    #[test]
    fn the_bars_mapping_round_trips_and_defaults_to_one_bar() {
        for bars in [0.0625f32, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 64.0] {
            let back = stretch_bars_from_norm(stretch_bars_to_norm(bars));
            assert!((back - bars).abs() < bars * 1.0e-4, "{bars} came back {back}");
        }
        let norm = stretch_bars_to_norm(1.0);
        assert!(
            (norm - 0.4).abs() < 0.001,
            "one bar maps to {norm}, but the control defaults to 0.4"
        );
    }

    /// Typed entry has to survive the units the field itself displays: the
    /// obvious thing to do with a box reading "2.00x" is to type "4x".
    #[test]
    fn typed_values_tolerate_the_units_the_field_shows() {
        assert_eq!(parse_typed_value("4"), Some(4.0));
        assert_eq!(parse_typed_value("4x"), Some(4.0));
        assert_eq!(parse_typed_value("2.5 bar"), Some(2.5));
        // The grain field's own text: the leading number is the value, and
        // committing it unchanged has to be a no-op rather than a jump to
        // 256375.
        assert_eq!(parse_typed_value("256 fr / 375 Hz"), Some(256.0));
        assert_eq!(parse_typed_value("-3 st"), Some(-3.0));
        assert_eq!(parse_typed_value(" 1.75 "), Some(1.75));
    }

    /// Anything unparseable must be refused rather than guessed at, so a
    /// half-typed string never reaches the engine.
    #[test]
    fn unparseable_typed_values_are_refused() {
        for text in ["", "   ", "bar", "x", "..", "-", "nan", "inf"] {
            assert_eq!(parse_typed_value(text), None, "{text:?} was accepted");
        }
    }

    /// The auto-snap seed measures the loop against the tempo. At 120 BPM a
    /// bar is 96,000 frames, so a 192,000-frame sample is two bars.
    #[test]
    fn the_seed_measures_the_loop_against_the_tempo() {
        let params = SamplerParams::default();
        let bars = measured_loop_bars(params, 192_000, 48_000, 120.0);
        assert!((bars - 2.0).abs() < 1.0e-3, "measured {bars}");
        assert_eq!(snap_bars_to_power_of_two(bars), 2.0);

        // A slightly long recording still lands on the intended length.
        let sloppy = measured_loop_bars(params, 196_000, 48_000, 120.0);
        assert_eq!(snap_bars_to_power_of_two(sloppy), 2.0);
    }

    /// With a loop set, the loop is what gets measured -- not the whole
    /// sample -- because the loop is the part that repeats against the grid.
    /// Except in slice mode, where the loop is the slice and the global loop
    /// points are hidden: there the whole region is what lies on the grid,
    /// and the seed has to measure what the ratio will be derived from.
    #[test]
    fn the_seed_measures_the_loop_rather_than_the_whole_sample() {
        let params = SamplerParams {
            loop_mode: LoopMode::Forward,
            loop_start: 0.0,
            loop_end: 0.5,
            ..SamplerParams::default()
        };
        let bars = measured_loop_bars(params, 192_000, 48_000, 120.0);
        assert!((bars - 1.0).abs() < 1.0e-3, "measured {bars}");

        let sliced = SamplerParams {
            play_mode: PlayMode::Slice,
            ..params
        };
        let bars = measured_loop_bars(sliced, 192_000, 48_000, 120.0);
        assert!((bars - 2.0).abs() < 1.0e-3, "slice mode measured {bars}");
    }

    /// The frame count is the file's, so the rate has to be the file's too.
    /// Two bars of 44.1 kHz break measured at 48 kHz read as 1.84 bars --
    /// still snapping to two here, but a slightly short two-bar loop would
    /// have been dragged down to one.
    #[test]
    fn the_seed_measures_in_the_samples_own_rate() {
        let params = SamplerParams::default();
        let two_bars_at_44k = 2 * 44_100 * 2;
        let bars = measured_loop_bars(params, two_bars_at_44k, 44_100, 120.0);
        assert!((bars - 2.0).abs() < 1.0e-3, "measured {bars}");
        let misread = measured_loop_bars(params, two_bars_at_44k, 48_000, 120.0);
        assert!(misread < 1.9, "the wrong rate should visibly misread: {misread}");
    }

    /// Out-of-range input is clamped rather than propagated. Both ends of both
    /// controls, because a modulated or automated value can arrive at
    /// anything.
    #[test]
    fn the_stretch_mappings_clamp_rather_than_extrapolate() {
        assert_eq!(stretch_ratio_from_norm(-5.0), MIN_STRETCH_RATIO);
        assert_eq!(stretch_ratio_from_norm(5.0), MAX_STRETCH_RATIO);
        assert_eq!(stretch_grain_from_norm(-5.0), MIN_STRETCH_GRAIN);
        assert_eq!(stretch_grain_from_norm(5.0), MAX_STRETCH_GRAIN);
        assert_eq!(stretch_ratio_to_norm(1_000.0), 1.0);
        assert_eq!(stretch_ratio_to_norm(0.0), 0.0);
    }
}
