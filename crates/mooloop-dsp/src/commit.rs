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
//! A commit is a render, not a new engine, and the render replaces the
//! sample (Adam, 2026-09-30: a committed sample is treated like a rendered
//! one). It renders the whole sample, so every marker has somewhere to go:
//! slices, Start/End and the loop all cross through the render's trace, and
//! none is dropped. A second commit renders the first one's audio, so
//! commits stack. Reverting maps the markers on screen back through every
//! step onto the original.
//!
//! Nothing here is realtime, and nothing here touches the disk.

use std::sync::Arc;

use mooloop_core::{clamp01, CommitStep, SampleCommit, SamplerParams, SliceMap, SliceMarker};

use crate::interpolate::{Region, RegionEdge};
use crate::sampler::{SampleData, Sampler};
use crate::stretch::{render_stretched, StretchRender};

/// How far from 1 the ratio still to bake must be before committing again
/// would change anything, and so before the face calls a commit stale.
pub const STALE_RATIO_TOLERANCE: f64 = 1.0e-3;

/// Everything a commit produces, ready to be installed on a channel.
pub struct CommittedSample {
    /// The rendered buffer, which replaces the sample.
    pub sample: Arc<SampleData>,
    /// Every marker, in the render's frames: same ids, same order, none
    /// dropped, so slice `i` plays the same hit before and after.
    pub slices: SliceMap,
    /// The patch with its bounds and loop mapped onto the render, the live
    /// stretch switched off and a free ratio reset to 1: the stretch is in
    /// the audio now.
    pub params: SamplerParams,
    /// What this commit rendered.
    pub step: CommitStep,
}

/// The ratio a commit of `sample` would bake now: the live stretcher's own
/// derivation, a fit to the tempo or the free ratio, measured in `sample`'s
/// frames at unity playback rate.
///
/// Against the slice map too: a Slices loop grid snaps the loop to the
/// markers, and the voice fits that loop, not the free one.
pub fn pending_ratio(params: SamplerParams, sample: &SampleData, slices: &SliceMap, bpm: f64) -> f64 {
    Sampler::effective_ratio_in(
        params,
        sample.frames.len(),
        sample.sample_rate,
        bpm,
        1.0,
        Some(slices),
    )
}

/// Whether committing `sample` again would stretch it: the tempo, the fitted
/// span or the free ratio has moved since it was committed. `params` as
/// [`params_on_screen`] gives them.
pub fn is_stale(params: SamplerParams, sample: &SampleData, slices: &SliceMap, bpm: f64) -> bool {
    (pending_ratio(params, sample, slices, bpm) - 1.0).abs() > STALE_RATIO_TOLERANCE
}

/// The patch as it applies to the committed sample on screen.
///
/// A commit resets a free ratio to 1, so the knob reads what is still to
/// bake. 0.1.5 did not: a commit it saved (its step has no input length)
/// left the knob at the ratio it baked, so that ratio is divided back out
/// here, or such a song would read stale as it opened and a REBAKE would
/// stretch it twice.
pub fn params_on_screen(params: SamplerParams, commit: &SampleCommit) -> SamplerParams {
    match commit.steps.last() {
        Some(step) if step.frames == 0 && !params.stretch_sync && step.ratio > 0.0 => {
            SamplerParams {
                stretch_ratio: params.stretch_ratio / step.ratio,
                ..params
            }
        }
        _ => params,
    }
}

/// Bake `input`'s stretch into a buffer: the whole of `input`, at the ratio
/// [`pending_ratio`] gives now.
///
/// `input` is the sample on screen, which after an earlier commit is that
/// commit's render. The ratio is stored rather than the tempo: a committed
/// loop is baked at a fixed tempo, and the face marks it stale rather than
/// re-rendering when the project moves.
///
/// Returns `None` when there is nothing to render, an empty sample.
pub fn commit_stretch(
    input: &SampleData,
    params: SamplerParams,
    slices: &SliceMap,
    bpm: f64,
) -> Option<CommittedSample> {
    if input.frames.is_empty() {
        return None;
    }
    let len = input.frames.len();
    // Rounded to the `f32` the spec stores *before* rendering, not after.
    // `render_stretched` is length-determined by its ratio and steps its
    // analysis pointer by `hop / ratio`, so a render at the `f64` and a
    // re-render at the stored `f32` would be two slightly different renders.
    let ratio = pending_ratio(params, input, slices, bpm) as f32;
    let step = CommitStep {
        mode: params.stretch_mode,
        ratio,
        grain: params.stretch_grain,
        start: 0.0,
        end: 1.0,
        frames: u32::try_from(len).unwrap_or(u32::MAX),
    };
    let render = render_step(&input.frames, input.sample_rate, &step);
    if render.is_empty() {
        return None;
    }
    let rendered_len = render.len();

    // Through the trace rather than the nominal ratio: at a search window's
    // worth of error a break's slices flam audibly.
    let slices = map_slices(slices, rendered_len, |frame| render.output_frame_of(frame));
    let to_fraction = |fraction: f32| {
        let frame = f64::from(clamp01(fraction)) * len as f64;
        (render.output_frame_of(frame) / rendered_len as f64).clamp(0.0, 1.0) as f32
    };

    let mut params = params;
    params.start = to_fraction(params.start);
    params.end = to_fraction(params.end);
    // Mapped whatever the loop mode: points parked with looping off are
    // still the user's.
    params.loop_start = to_fraction(params.loop_start);
    params.loop_end = to_fraction(params.loop_end).max(params.loop_start);
    // The stretch is in the audio now. Left on, it would stretch the render
    // again; a free ratio is reset for the same reason, so the knob reads
    // what is still to bake and a commit at once is not stale.
    params.stretch_enabled = false;
    if !params.stretch_sync {
        params.stretch_ratio = 1.0;
    }

    Some(CommittedSample {
        sample: Arc::new(SampleData {
            frames: render.frames,
            sample_rate: input.sample_rate,
            root_note: input.root_note,
        }),
        slices,
        params,
        step,
    })
}

/// Render one step from its input, at the span it names.
fn render_step(input: &[[f32; 2]], sample_rate: u32, step: &CommitStep) -> StretchRender {
    let len = input.len().max(1) as f64;
    let start = f64::from(clamp01(step.start)) * len;
    let end = (f64::from(clamp01(step.end)) * len).max(start + 1.0).min(len);
    render_stretched(
        input,
        Region {
            start: start.min(end - 1.0),
            end,
            edge: RegionEdge::Silent,
        },
        step.mode,
        u32::from(step.grain),
        f64::from(step.ratio),
        sample_rate,
    )
}

/// Every step's render from `original`, each made from the one before, with
/// its trace. `None` when a step renders nothing.
fn render_chain(original: &SampleData, steps: &[CommitStep]) -> Option<Vec<StretchRender>> {
    let mut chain: Vec<StretchRender> = Vec::with_capacity(steps.len());
    for step in steps {
        let render = {
            let input = chain.last().map_or(&original.frames, |render| &render.frames);
            render_step(input, original.sample_rate, step)
        };
        if render.is_empty() {
            return None;
        }
        chain.push(render);
    }
    Some(chain)
}

/// Re-make an unstored commit's render from the channel's sample, for an
/// install.
///
/// `None` for a commit whose render is stored: the sample *is* the render
/// then. An unstored commit is how 0.1.5 saved one, from its playback
/// region, and those still come back as they were baked:
/// `render_stretched` is length-determined by the spec.
pub fn rerender_commit(source: &SampleData, commit: &SampleCommit) -> Option<Arc<SampleData>> {
    if !commit.is_unstored() || source.frames.is_empty() {
        return None;
    }
    let render = render_chain(source, &commit.steps)?.pop()?;
    Some(Arc::new(SampleData {
        frames: render.frames,
        sample_rate: source.sample_rate,
        root_note: source.root_note,
    }))
}

/// What a revert hands back: the original's markers and patch.
pub struct Reverted {
    pub params: SamplerParams,
    pub slices: SliceMap,
    /// The original no longer renders to the length on screen -- it changed
    /// on disk since the commit -- so the markers were mapped by proportion
    /// at the last step.
    pub original_changed: bool,
}

/// Go back to `original` from a commit, carrying the markers on screen with
/// it: every slice (ids kept, none dropped), Start/End and the loop are
/// mapped back through each step's trace, newest first. The traces are
/// re-made from `original` and the steps' specs, which reproduce them.
///
/// `published_len` is the length of the render on screen, which the markers
/// are in. The live stretch goes back on, and a free ratio is multiplied
/// back up by what the commits baked, so the patch asks for what it sounded
/// like.
///
/// `None` when the original renders nothing.
pub fn revert_commit(
    original: &SampleData,
    commit: &SampleCommit,
    published_len: usize,
    params: SamplerParams,
    slices: &SliceMap,
) -> Option<Reverted> {
    let original_len = original.frames.len();
    if original_len == 0 {
        return None;
    }
    let chain = render_chain(original, &commit.steps)?;
    let remade_len = chain.last().map_or(published_len, StretchRender::len);
    let original_changed = remade_len != published_len
        || commit
            .steps
            .first()
            .is_some_and(|step| step.frames != 0 && step.frames as usize != original_len);
    let scale = remade_len as f64 / published_len.max(1) as f64;
    let back = |frame: f64| {
        chain
            .iter()
            .rev()
            .fold(frame * scale, |frame, render| render.source_frame_of(frame))
    };
    let to_fraction = |fraction: f32| {
        let frame = f64::from(clamp01(fraction)) * published_len as f64;
        (back(frame) / original_len as f64).clamp(0.0, 1.0) as f32
    };

    let mut params = params_on_screen(params, commit);
    params.start = to_fraction(params.start);
    params.end = to_fraction(params.end);
    params.loop_start = to_fraction(params.loop_start);
    params.loop_end = to_fraction(params.loop_end).max(params.loop_start);
    params.stretch_enabled = true;
    if !params.stretch_sync {
        params.stretch_ratio = ((f64::from(params.stretch_ratio) * commit.total_ratio()) as f32)
            .clamp(mooloop_core::MIN_STRETCH_RATIO, mooloop_core::MAX_STRETCH_RATIO);
    }
    Some(Reverted {
        params,
        slices: map_slices(slices, original_len, back),
        original_changed,
    })
}

/// `slices` carried to a buffer `len` frames long through `map`, keeping
/// every marker: ids and order ride across, and two markers that land on one
/// frame are pushed a frame apart rather than merged, because a slice plays
/// by its index and losing one would move every later slice down a key.
fn map_slices(slices: &SliceMap, len: usize, map: impl Fn(f64) -> f64) -> SliceMap {
    let last = len.saturating_sub(1) as u32;
    let mut frames: Vec<u32> = slices
        .markers()
        .iter()
        .map(|marker| (map(f64::from(marker.frame)).round().max(0.0) as u32).min(last))
        .collect();
    let mut floor = None;
    for frame in frames.iter_mut() {
        if let Some(floor) = floor {
            *frame = (*frame).max(floor);
        }
        floor = Some(frame.saturating_add(1));
    }
    // Pushed past the end only when the markers crowd the last frames; pull
    // those back from the end, which keeps them distinct when they fit.
    let mut ceiling = last;
    for frame in frames.iter_mut().rev() {
        *frame = (*frame).min(ceiling);
        ceiling = frame.saturating_sub(1);
    }
    let mut mapped = SliceMap::new();
    mapped.rebuild(
        slices
            .markers()
            .iter()
            .zip(frames)
            .map(|(marker, frame)| SliceMarker { frame, ..*marker }),
    );
    mapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{LoopMode, PlayMode, StretchMode};

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

    fn stored(steps: Vec<CommitStep>) -> SampleCommit {
        SampleCommit {
            original: Some(mooloop_core::project::SampleReference::Empty),
            steps,
        }
    }

    /// Every marker crosses the commit, the ones at and outside the playback
    /// region included, and keeps its index and id: a commit renders the
    /// whole sample, so each lands at its stretched frame (Adam's example: at
    /// twice the length, 0, 6,000, 12,000 become 0, 12,000, 24,000).
    #[test]
    fn every_marker_keeps_its_key_across_a_commit() {
        let source = ramp(48_000);
        let mut params = stretched_params();
        // The region starts on the third marker and ends before the last.
        params.start = 0.25;
        params.end = 0.75;
        let mut slices = SliceMap::new();
        for frame in [0u32, 6_000, 12_000, 18_000, 24_000, 36_000, 47_000] {
            slices.add(frame);
        }

        let committed = commit_stretch(&source, params, &slices, 120.0).unwrap();
        assert_eq!(committed.sample.frames.len(), 96_000, "the whole sample is rendered");
        assert_eq!(committed.slices.len(), slices.len(), "no marker was dropped");
        let millisecond = 48.0;
        for (before, after) in slices.markers().iter().zip(committed.slices.markers()) {
            assert_eq!(before.id, after.id, "slice {} became a different slice", before.id);
            let expected = f64::from(before.frame) * 2.0;
            assert!(
                (f64::from(after.frame) - expected).abs() < millisecond,
                "a marker at {} landed at {}, expected about {expected}",
                before.frame,
                after.frame
            );
        }
        assert!((committed.params.start - 0.25).abs() < 1.0e-3);
        assert!((committed.params.end - 0.75).abs() < 1.0e-3);
        assert!((committed.params.loop_start - 0.2).abs() < 1.0e-3);
        assert!((committed.params.loop_end - 0.8).abs() < 1.0e-3);
        assert!(!committed.params.stretch_enabled, "the stretch is in the audio now");
        assert_eq!(committed.params.stretch_ratio, 1.0, "nothing left to bake");
        assert!(!is_stale(committed.params, &committed.sample, &committed.slices, 120.0));
    }

    /// Markers closer together than the render can hold apart are pushed a
    /// frame apart, never merged, so the slice count holds even at the
    /// shortest ratio.
    #[test]
    fn markers_that_would_land_on_one_frame_stay_apart() {
        let source = ramp(4_000);
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_ratio: 0.25,
            ..SamplerParams::default()
        };
        let mut slices = SliceMap::new();
        for frame in [1_000u32, 1_001, 1_002, 3_999] {
            slices.add(frame);
        }
        let committed = commit_stretch(&source, params, &slices, 120.0).unwrap();
        assert_eq!(committed.slices.len(), 4);
        let frames: Vec<u32> = committed.slices.markers().iter().map(|m| m.frame).collect();
        assert!(frames.windows(2).all(|pair| pair[0] < pair[1]), "{frames:?}");
        assert!(*frames.last().unwrap() < committed.sample.frames.len() as u32);
    }

    /// Commits stack: a second commit renders the first one's audio, and a
    /// revert maps the markers on screen -- one added after the first commit
    /// included -- back onto the original within a millisecond.
    #[test]
    fn a_revert_maps_the_markers_on_screen_back_through_every_commit() {
        let source = ramp(48_000);
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_ratio: 2.0,
            ..SamplerParams::default()
        };
        let mut slices = SliceMap::new();
        slices.add(12_000);
        let first = commit_stretch(&source, params, &slices, 120.0).unwrap();

        // An edit of the render: a slice at its frame 48,000, which is the
        // original's 24,000.
        let mut edited = first.slices.clone();
        edited.add(48_000);
        let mut again = first.params;
        again.stretch_ratio = 1.5;
        let second = commit_stretch(&first.sample, again, &edited, 120.0).unwrap();
        assert_eq!(second.sample.frames.len(), 144_000, "the render was stretched again");
        assert_eq!(second.slices.len(), 2, "the edit survived the second commit");
        let added = second.slices.markers()[1].frame;
        assert!((i64::from(added) - 72_000).abs() < 48, "at its stretched frame: {added}");

        let commit = stored(vec![first.step, second.step]);
        let reverted = revert_commit(
            &source,
            &commit,
            second.sample.frames.len(),
            second.params,
            &second.slices,
        )
        .unwrap();
        assert!(!reverted.original_changed);
        let frames: Vec<u32> = reverted.slices.markers().iter().map(|m| m.frame).collect();
        assert!((i64::from(frames[0]) - 12_000).abs() < 48, "{frames:?}");
        assert!((i64::from(frames[1]) - 24_000).abs() < 48, "{frames:?}");
        assert_eq!(
            reverted.slices.markers().iter().map(|m| m.id).collect::<Vec<_>>(),
            second.slices.markers().iter().map(|m| m.id).collect::<Vec<_>>(),
        );
        assert!(reverted.params.stretch_enabled, "the live stretch is back");
        assert!(
            (reverted.params.stretch_ratio - 3.0).abs() < 1.0e-4,
            "the free ratio asks for what was baked: {}",
            reverted.params.stretch_ratio
        );
        assert!((reverted.params.end - 1.0).abs() < 1.0e-4);
    }

    /// An original that no longer renders to the length on screen is said
    /// so, and the markers still come back, mapped by proportion.
    #[test]
    fn a_revert_onto_a_changed_original_still_maps_every_marker() {
        let source = ramp(48_000);
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_ratio: 2.0,
            ..SamplerParams::default()
        };
        let mut slices = SliceMap::new();
        slices.add(24_000);
        let committed = commit_stretch(&source, params, &slices, 120.0).unwrap();
        let commit = stored(vec![committed.step]);
        let replaced = ramp(24_000);
        let reverted = revert_commit(
            &replaced,
            &commit,
            committed.sample.frames.len(),
            committed.params,
            &committed.slices,
        )
        .unwrap();
        assert!(reverted.original_changed);
        let frame = reverted.slices.markers()[0].frame;
        assert!((i64::from(frame) - 12_000).abs() < 48, "halfway still: {frame}");
    }

    /// An unstored commit -- 0.1.5's, which rendered only its playback region
    /// -- comes back from its sample exactly as it was baked, and a stored
    /// one is not re-rendered at all.
    #[test]
    fn an_unstored_commit_re_renders_its_region_and_a_stored_one_does_not() {
        let source = ramp(24_000);
        let step = CommitStep {
            mode: StretchMode::Music,
            ratio: 2.0,
            grain: 1024,
            start: 0.1,
            end: 0.9,
            frames: 0,
        };
        let unstored = SampleCommit {
            original: None,
            steps: vec![step],
        };
        let baked = render_stretched(
            &source.frames,
            Region {
                start: 2_400.0,
                end: 21_600.0,
                edge: RegionEdge::Silent,
            },
            StretchMode::Music,
            1024,
            2.0,
            48_000,
        );
        let reloaded = rerender_commit(&source, &unstored).unwrap();
        assert_eq!(reloaded.frames, baked.frames);
        assert!(rerender_commit(&source, &stored(vec![step])).is_none());
    }

    /// The same under a ratio the spec cannot hold exactly. A tempo-fitted
    /// ratio is almost never dyadic, and the spec stores it as an `f32`, so
    /// the bake has to use the number the file will hold.
    #[test]
    fn a_re_made_render_with_an_inexact_ratio_reproduces_the_buffer() {
        let source = SampleData {
            sample_rate: 44_100,
            ..ramp(30_000)
        };
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: 1.0,
            ..SamplerParams::default()
        };
        let committed = commit_stretch(&source, params, &SliceMap::new(), 127.3).unwrap();
        assert_ne!(
            f64::from(committed.step.ratio),
            f64::from(committed.step.ratio).round(),
            "the fixture should exercise a non-integer ratio"
        );
        let unstored = SampleCommit {
            original: None,
            steps: vec![committed.step],
        };
        let reloaded = rerender_commit(&source, &unstored).unwrap();
        assert_eq!(reloaded.frames, committed.sample.frames);
    }

    /// Fit-to-tempo bakes the resolved number, and a tempo change afterwards
    /// makes the commit stale: committing again would stretch the render by
    /// what the tempo moved.
    #[test]
    fn a_tempo_fitted_commit_goes_stale_when_the_tempo_moves() {
        let source = ramp(48_000);
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: 1.0,
            ..SamplerParams::default()
        };
        // One bar at 120 BPM is 96,000 frames; a 48,000-frame region has to
        // be stretched 2x to fill it.
        let committed = commit_stretch(&source, params, &SliceMap::new(), 120.0).unwrap();
        assert!((committed.step.ratio - 2.0).abs() < 1.0e-4);
        assert_eq!(committed.sample.frames.len(), 96_000);
        let slices = SliceMap::new();
        assert!(!is_stale(committed.params, &committed.sample, &slices, 120.0));
        assert!(is_stale(committed.params, &committed.sample, &slices, 100.0));
        let again = pending_ratio(committed.params, &committed.sample, &slices, 60.0);
        assert!((again - 2.0).abs() < 1.0e-3, "half the tempo, twice the render: {again}");
    }

    /// The commit fits the loop the voice plays: with a Slices loop grid that
    /// is the loop snapped to the slice markers, not the whole region.
    #[test]
    fn a_slices_snapped_loop_commits_at_the_ratio_it_sounded() {
        use mooloop_core::sampler::LoopQuantize;

        let source = ramp(96_000);
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: 1.0,
            start: 0.0,
            end: 1.0,
            loop_mode: LoopMode::Forward,
            loop_quantize: LoopQuantize::Slices,
            loop_start: 0.26,
            loop_end: 0.74,
            ..SamplerParams::default()
        };
        let mut slices = SliceMap::new();
        slices.add(24_000);
        slices.add(72_000);

        // The loop snaps to 24,000..72,000 (48,000 frames); one bar at 120
        // BPM is 96,000, so it lasts one bar at ratio 2.
        let live = Sampler::effective_ratio_in(params, 96_000, 48_000, 120.0, 1.0, Some(&slices));
        assert!((live - 2.0).abs() < 1.0e-6, "the live fit moved: {live}");

        let committed = commit_stretch(&source, params, &slices, 120.0).unwrap();
        assert_eq!(committed.step.ratio, live as f32);
        // And the snapped loop fits the render as it is.
        assert!(!is_stale(committed.params, &committed.sample, &committed.slices, 120.0));
    }

    /// A 0.1.5 commit at a free ratio left the knob at that ratio. It opens
    /// not stale, and a revert asks for the ratio it baked, not its square.
    #[test]
    fn a_0_1_5_free_ratio_commit_is_not_stale_and_reverts_to_its_ratio() {
        let source = ramp(24_000);
        let commit = SampleCommit::unstored(StretchMode::Music, 2.0, 1024);
        let render = rerender_commit(&source, &commit).unwrap();
        let params = SamplerParams {
            stretch_ratio: 2.0,
            ..SamplerParams::default()
        };
        let on_screen = params_on_screen(params, &commit);
        assert!(!is_stale(on_screen, &render, &SliceMap::new(), 120.0));
        let reverted =
            revert_commit(&source, &commit, render.frames.len(), params, &SliceMap::new()).unwrap();
        assert!((reverted.params.stretch_ratio - 2.0).abs() < 1.0e-6);
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
