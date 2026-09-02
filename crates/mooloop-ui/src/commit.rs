//! Freezing a sampler's live stretch into an ordinary buffer.
//!
//! WSOLA only runs forwards, so the stretch work inherited a rule from the
//! #32 spike: reverse and ping-pong are refused while stretching. Slicing
//! needs reverse-per-slice, and a slicer that silently loses reverse the
//! moment a loop is tempo-fitted is not a groovebox. Committing resolves it
//! without making the stretcher run backwards:
//!
//! > Stretch is **live** for forward pitched playback. For reverse or slice
//! > mode, **commit** it.
//!
//! A commit is a render, not a new engine. The source stays authoritative
//! here on the UI thread; the render becomes what is published, displayed,
//! and edited, so the waveform, the markers, and the start/end fractions all
//! live in one coordinate system rather than two. Reverting restores the
//! stashed pre-commit values, and re-committing always renders from the
//! source, so repeated tempo changes cannot accumulate drift.
//!
//! Nothing here is realtime, and nothing here is persisted but the spec: a
//! project stores six numbers and re-renders on load.

use std::sync::Arc;

use mooloop_core::{clamp01, LoopMode, SampleCommit, SamplerParams, SliceMap};
use mooloop_dsp::interpolate::{Region, RegionEdge};
use mooloop_dsp::{render_stretched, SampleData, Sampler};

/// Everything a commit produces, ready to be installed on a channel.
pub struct CommittedSample {
    /// The rendered buffer. This is what gets published and drawn.
    pub sample: Arc<SampleData>,
    /// The markers carried across the render, in the buffer's own frames.
    pub slices: SliceMap,
    /// The patch with `stretch_enabled` cleared and its bounds remapped.
    pub params: SamplerParams,
    /// What was baked, and what the editor looked like before it.
    pub commit: SampleCommit,
}

/// The playback region of `source`, in source frames.
fn playback_region(params: SamplerParams, len: usize) -> (f64, f64) {
    let len_f = len.max(1) as f64;
    let start = f64::from(clamp01(params.start)) * len_f;
    let end = (f64::from(clamp01(params.end)) * len_f)
        .max(start + 1.0)
        .min(len_f);
    (start.min(end - 1.0), end)
}

/// Bake `source`'s stretch into a buffer.
///
/// `bpm` is only read when the patch is fitting to tempo, and the ratio it
/// resolves is stored rather than the tempo: a committed loop is baked at a
/// fixed tempo, and the UI marks the fit stale rather than silently
/// re-rendering when the project moves.
///
/// Returns `None` when there is nothing to render -- an empty sample or a
/// degenerate region.
pub fn commit_stretch(
    source: &SampleData,
    params: SamplerParams,
    slices: &SliceMap,
    bpm: f64,
) -> Option<CommittedSample> {
    if source.frames.is_empty() {
        return None;
    }
    let len = source.frames.len();
    let (region_start, region_end) = playback_region(params, len);

    // The live stretcher's own derivation, not a second copy of it. Measured
    // in the source's frames at unity playback rate, which is what
    // `effective_ratio` reduces to when the sample and the device agree: the
    // committed loop is one bar at the pitch it was recorded at, and
    // transposing it afterwards is an ordinary sampler transposition.
    let ratio = Sampler::effective_ratio(params, len, source.sample_rate, bpm, 1.0);

    let render = render_stretched(
        &source.frames,
        Region {
            start: region_start,
            end: region_end,
            edge: RegionEdge::Silent,
        },
        params.stretch_mode,
        u32::from(params.stretch_grain),
        ratio,
        source.sample_rate,
    );
    if render.is_empty() {
        return None;
    }
    let rendered_len = render.len() as f64;

    // Markers and bounds cross through the trace rather than through the
    // nominal ratio: at a search window's worth of error a break's slices
    // flam audibly.
    let mut committed_slices = SliceMap::new();
    for marker in slices.markers() {
        committed_slices.add(render.output_frame_of(f64::from(marker.frame)).round() as u32);
    }

    let to_fraction = |fraction: f32| {
        let source_frame = f64::from(clamp01(fraction)) * len.max(1) as f64;
        (render.output_frame_of(source_frame) / rendered_len).clamp(0.0, 1.0) as f32
    };
    let commit = SampleCommit {
        mode: params.stretch_mode,
        ratio: ratio as f32,
        grain: params.stretch_grain,
        source_markers: slices.markers().iter().map(|marker| marker.frame).collect(),
        source_start: params.start,
        source_end: params.end,
        source_loop_start: params.loop_start,
        source_loop_end: params.loop_end,
    };

    let mut params = params;
    // The rendered buffer *is* the region, so the region is now all of it.
    let (loop_start, loop_end) = if params.loop_mode == LoopMode::Off {
        (0.0, 1.0)
    } else {
        (to_fraction(commit.source_loop_start), to_fraction(commit.source_loop_end))
    };
    params.start = 0.0;
    params.end = 1.0;
    params.loop_start = loop_start;
    params.loop_end = loop_end.max(loop_start);
    // The stretch is in the audio now. Leaving the switch on would stretch an
    // already-stretched buffer.
    params.stretch_enabled = false;

    Some(CommittedSample {
        sample: Arc::new(SampleData {
            frames: render.frames,
            sample_rate: source.sample_rate,
            root_note: source.root_note,
        }),
        slices: committed_slices,
        params,
        commit,
    })
}

/// Re-render a commit from its stored spec, for the project-load path.
///
/// This is what lets a project persist six numbers instead of the audio:
/// `render_stretched` is length-determined by the spec, so the buffer that
/// comes back is the one that was baked. The markers are not re-derived --
/// they were saved in the rendered buffer's own coordinates and come back
/// with the project.
pub fn rerender_commit(source: &SampleData, commit: &SampleCommit) -> Option<Arc<SampleData>> {
    if source.frames.is_empty() {
        return None;
    }
    let len = source.frames.len();
    let (region_start, region_end) = playback_region(
        SamplerParams {
            start: commit.source_start,
            end: commit.source_end,
            ..SamplerParams::default()
        },
        len,
    );
    let render = render_stretched(
        &source.frames,
        Region {
            start: region_start,
            end: region_end,
            edge: RegionEdge::Silent,
        },
        commit.mode,
        u32::from(commit.grain),
        f64::from(commit.ratio),
        source.sample_rate,
    );
    (!render.is_empty()).then(|| {
        Arc::new(SampleData {
            frames: render.frames,
            sample_rate: source.sample_rate,
            root_note: source.root_note,
        })
    })
}

/// Undo a commit: the stashed values go back, exactly as they were.
///
/// Exact rather than round-tripped through the trace a second time, which is
/// what `SampleCommit`'s pre-commit half exists for. Mapping markers back
/// through the trace would leave them a fraction of a frame from where the
/// user put them, and doing it twice would show.
pub fn revert_commit(params: SamplerParams, commit: &SampleCommit) -> (SamplerParams, SliceMap) {
    let mut params = params;
    params.start = commit.source_start;
    params.end = commit.source_end;
    params.loop_start = commit.source_loop_start;
    params.loop_end = commit.source_loop_end;
    params.stretch_mode = commit.mode;
    params.stretch_grain = commit.grain;
    // Deliberately not switched back on. Reverting hands back the source
    // buffer and the source bounds; whether to stretch it live again is the
    // next decision, not part of undoing this one.
    let mut slices = SliceMap::new();
    for frame in &commit.source_markers {
        slices.add(*frame);
    }
    (params, slices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{PlayMode, StretchMode};

    fn ramp(len: usize) -> SampleData {
        SampleData {
            frames: (0..len)
                .map(|index| {
                    let value = index as f32 / len as f32;
                    [value, value]
                })
                .collect(),
            sample_rate: 48_000,
            root_note: 60,
        }
    }

    fn stretched_params() -> SamplerParams {
        SamplerParams {
            play_mode: PlayMode::Slice,
            stretch_enabled: true,
            stretch_mode: StretchMode::Music,
            stretch_ratio: 2.0,
            start: 0.1,
            end: 0.9,
            loop_mode: LoopMode::Forward,
            loop_start: 0.2,
            loop_end: 0.8,
            ..SamplerParams::default()
        }
    }

    /// The round trip has to be exact, not approximately exact: a user who
    /// commits, listens, and changes their mind must get their markers back
    /// where they put them rather than a fraction of a frame away.
    #[test]
    fn committing_then_reverting_restores_every_marker_and_bound() {
        let source = ramp(48_000);
        let params = stretched_params();
        let mut slices = SliceMap::new();
        for frame in [5_000u32, 12_000, 20_500, 39_000] {
            slices.add(frame);
        }

        let committed = commit_stretch(&source, params, &slices, 120.0).unwrap();
        assert!(!committed.params.stretch_enabled, "the stretch is in the audio now");
        assert_eq!(committed.slices.len(), slices.len());

        let (reverted, restored) = revert_commit(committed.params, &committed.commit);
        assert_eq!(reverted.start, params.start);
        assert_eq!(reverted.end, params.end);
        assert_eq!(reverted.loop_start, params.loop_start);
        assert_eq!(reverted.loop_end, params.loop_end);
        assert_eq!(
            restored.markers().iter().map(|m| m.frame).collect::<Vec<_>>(),
            slices.markers().iter().map(|m| m.frame).collect::<Vec<_>>()
        );
    }

    /// The commit is length-determined by its spec, which is the property
    /// that lets a project store the spec instead of the audio.
    #[test]
    fn a_reloaded_commit_reproduces_the_buffer_it_was_baked_from() {
        let source = ramp(24_000);
        let committed = commit_stretch(&source, stretched_params(), &SliceMap::new(), 120.0).unwrap();
        let reloaded = rerender_commit(&source, &committed.commit).unwrap();
        assert_eq!(reloaded.frames, committed.sample.frames);
    }

    /// Markers land where the audio moved them. The region runs 0.1..0.9 of a
    /// 48,000-frame ramp at ratio 2, so a marker in the middle of the region
    /// belongs in the middle of a 76,800-frame render.
    #[test]
    fn markers_cross_the_commit_with_the_audio() {
        let source = ramp(48_000);
        let mut slices = SliceMap::new();
        slices.add(4_800); // the region start
        slices.add(24_000); // halfway through the sample, and through the region
        let committed = commit_stretch(&source, stretched_params(), &slices, 120.0).unwrap();

        assert_eq!(committed.sample.frames.len(), 76_800);
        let mapped: Vec<u32> = committed.slices.markers().iter().map(|m| m.frame).collect();
        assert!(mapped[0] < 32, "the region start maps to the buffer start: {mapped:?}");
        assert!(
            (i64::from(mapped[1]) - 38_400).abs() < 48,
            "the region midpoint should land mid-render: {mapped:?}"
        );
    }

    /// Fit-to-tempo bakes the resolved number, so the committed loop is the
    /// length the grid asked for and stops depending on the project tempo.
    #[test]
    fn a_tempo_fitted_commit_bakes_the_resolved_ratio() {
        let source = ramp(48_000);
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: 1.0,
            start: 0.0,
            end: 1.0,
            ..SamplerParams::default()
        };
        // One bar at 120 BPM is 96,000 frames; a 48,000-frame region has to
        // be stretched 2x to fill it.
        let committed = commit_stretch(&source, params, &SliceMap::new(), 120.0).unwrap();
        assert!((committed.commit.ratio - 2.0).abs() < 1.0e-4);
        assert_eq!(committed.sample.frames.len(), 96_000);

        // Half the tempo, twice the bar, twice the render.
        let slower = commit_stretch(&source, params, &SliceMap::new(), 60.0).unwrap();
        assert_eq!(slower.sample.frames.len(), 192_000);
    }

    /// Nothing to render is not an error.
    #[test]
    fn an_empty_sample_commits_to_nothing() {
        let empty = SampleData {
            frames: Vec::new(),
            sample_rate: 48_000,
            root_note: 60,
        };
        assert!(commit_stretch(&empty, stretched_params(), &SliceMap::new(), 120.0).is_none());
    }
}
