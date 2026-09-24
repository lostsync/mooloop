//! Sampler editing: the slice map, and the measurements the editor draws
//! against.
//!
//! Everything here measures against the channel's *published* buffer -- the
//! committed render when there is one, the decoded source otherwise -- so the
//! waveform, the markers and the slice fractions all live in one coordinate
//! system.

use crate::channel::ChannelState;
use crate::sample::{sample_description, sample_duration, waveform_peaks};
use crate::session::{Session, WAVEFORM_BINS};
use mooloop_dsp::sample_analysis::{
    detect_onsets, fraction_from_frame, frame_from_fraction, snap_to_zero_crossing,
    snap_window_frames, OnsetSettings, SnapResult, DEFAULT_SNAP_WINDOW_MS,
};
use mooloop_dsp::SampleData;
use mooloop_core::{
    EngineCommand, SampleCommit, SamplerParams, SliceMap, SliceMarker, StretchMode, MAX_SLICES,
    MAX_STRETCH_BARS, MAX_STRETCH_RATIO, MIN_STRETCH_BARS, MIN_STRETCH_RATIO,
};

/// How a detection preview is accepted (MOO-44).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceAccept {
    /// Keep the markers placed by hand, replace the rest.
    Replace,
    /// Keep every marker, and add the detected ones that aren't beside one.
    Merge,
}

/// Source frames as fractions of the published buffer, for a preview.
pub fn frame_fractions(channel: &ChannelState, frames: &[u32]) -> Vec<f32> {
    let len = channel
        .published_sample()
        .map_or(0, |sample| sample.frames.len());
    if len == 0 {
        return Vec::new();
    }
    frames
        .iter()
        .map(|frame| fraction_from_frame(*frame as usize, len))
        .collect()
}

/// What a slice edit did.
pub enum SliceEdit {
    /// Nothing to act on: no channel, or no audio behind the position.
    Ignored,
    /// The map is full, or a marker is already at that frame.
    Refused,
    /// The new marker positions, normalized against the published buffer.
    Changed(Vec<f32>),
}

/// The nearest zero crossing to `frame`, inside the playback region.
///
/// Markers land wherever a pointer or a division puts them, which is as
/// likely to be mid-waveform as not; snapping is what stops a slice from
/// starting on a click.
pub fn snap_slice_frame(params: &SamplerParams, sample: &SampleData, frame: usize) -> usize {
    let len = sample.frames.len();
    if len < 2 {
        return frame;
    }
    let last = len - 1;
    let bounds = frame_from_fraction(params.start, len).min(last)
        ..=frame_from_fraction(params.end, len).min(last);
    let window = snap_window_frames(DEFAULT_SNAP_WINDOW_MS, sample.sample_rate);
    snap_to_zero_crossing(&sample.frames, frame.min(last), window, bounds).resolved
}

/// Re-derives everything the source pane shows about a channel's audio.
pub fn refresh_sample_view(channel: &mut ChannelState) {
    let Some(sample) = channel.published_sample().cloned() else {
        channel.waveform.clear();
        channel.sample_description.clear();
        channel.sample_duration = 0.0;
        return;
    };
    channel.waveform = waveform_peaks(&sample, WAVEFORM_BINS);
    channel.sample_description = sample_description(&sample);
    channel.sample_duration = sample_duration(&sample);
}

/// The frame a normalized editor position lands on.
pub fn resolve_slice_frame(channel: &ChannelState, position: f32, snap: bool) -> Option<u32> {
    let sample = channel.published_sample()?;
    let len = sample.frames.len();
    if len == 0 {
        return None;
    }
    let frame = frame_from_fraction(position.clamp(0.0, 1.0), len);
    let frame = if snap {
        snap_slice_frame(&channel.sampler_params(), sample, frame)
    } else {
        frame
    };
    Some(frame as u32)
}

/// Slice markers as fractions of the published buffer.
pub fn slice_fractions(channel: &ChannelState) -> Vec<f32> {
    let len = channel
        .published_sample()
        .map_or(0, |sample| sample.frames.len());
    if len == 0 {
        return Vec::new();
    }
    channel
        .slices
        .markers()
        .iter()
        .map(|marker| fraction_from_frame(marker.frame as usize, len))
        .collect()
}

/// Whether a committed render no longer matches the parameters it was baked
/// from, so the editor's stale badge should show.
pub fn commit_is_stale(channel: &ChannelState, commit: &SampleCommit, bpm: f64) -> bool {
    let params = channel.sampler_params();
    if commit.mode != params.stretch_mode {
        return true;
    }
    if params.stretch_mode == StretchMode::Grain && commit.grain != params.stretch_grain {
        return true;
    }
    if !params.stretch_sync {
        return (commit.ratio - params.stretch_ratio).abs() > 1.0e-3;
    }
    let Some(source) = channel.sample_data.as_ref() else {
        return false;
    };
    let params = SamplerParams {
        start: commit.source_start,
        end: commit.source_end,
        loop_start: commit.source_loop_start,
        loop_end: commit.source_loop_end,
        ..channel.sampler_params()
    };
    let now = mooloop_dsp::Sampler::effective_ratio(
        params,
        source.frames.len(),
        source.sample_rate,
        bpm,
        1.0,
    );
    (now - f64::from(commit.ratio)).abs() > 1.0e-3
}

/// What fit-to-tempo is doing, in words for the face (MOO-39): the fitted
/// span's own length and the tempo that makes it `stretch_bars` bars, and
/// what it lasts at the project's tempo.
#[derive(Debug, Clone, PartialEq)]
pub struct FitReadout {
    /// The span fit-to-tempo measures, in seconds of the sample's own time.
    pub source_seconds: f64,
    /// The tempo at which that span is `stretch_bars` bars long.
    pub source_bpm: f64,
    /// How long it lasts fitted, at the project tempo.
    pub fitted_seconds: f64,
    /// The ratio the root key runs.
    pub ratio: f32,
    /// Why the fit can't be trusted, if it can't: a readable sentence for the
    /// status bar, and the face shows the line in the warning colour.
    pub doubt: Option<String>,
}

/// The tempo band a loop's own tempo is believed in. Outside it, the bar
/// count is more likely wrong than the loop is that fast or slow.
const PLAUSIBLE_BPM: std::ops::RangeInclusive<f64> = 60.0..=200.0;

/// Reads a sampler's fit, or `None` with no audio.
pub fn fit_readout(
    params: &SamplerParams,
    sample: &SampleData,
    slices: Option<&SliceMap>,
    bpm: f64,
) -> Option<FitReadout> {
    let len = sample.frames.len();
    if len == 0 || sample.sample_rate == 0 {
        return None;
    }
    let (start, end) = mooloop_dsp::Sampler::fitted_span_in(*params, len, slices);
    let rate = f64::from(sample.sample_rate);
    let source_seconds = (end - start) / rate;
    let bars = f64::from(params.stretch_bars.clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS));
    // A bar at 1 BPM, in seconds, from the one function that knows how many
    // beats a bar has.
    let bar_at_one = mooloop_core::frames_per_bar(sample.sample_rate, 1.0) / rate;
    let source_bpm = bars * bar_at_one / source_seconds;
    let fitted_seconds = bars * bar_at_one / bpm;
    let ratio = mooloop_dsp::Sampler::synced_ratio(*params, sample, bpm, slices);
    let doubt = if ratio <= MIN_STRETCH_RATIO || ratio >= MAX_STRETCH_RATIO {
        Some(format!(
            "Fitting {bars} bars to {bpm:.0} BPM needs more stretch than the sampler has; check the bar count"
        ))
    } else if !PLAUSIBLE_BPM.contains(&source_bpm) {
        // The nearest power-of-two bar count, either way, that lands in
        // the band.
        let suggestion = [0.5, 2.0, 0.25, 4.0, 0.125, 8.0]
            .into_iter()
            .map(|factor| bars * factor)
            .find(|candidate| PLAUSIBLE_BPM.contains(&(candidate * bar_at_one / source_seconds)));
        Some(match suggestion {
            Some(better) => format!(
                "{bars} bars makes this loop {source_bpm:.0} BPM; {better} bars would be {:.0}",
                better * bar_at_one / source_seconds
            ),
            None => format!("{bars} bars makes this loop {source_bpm:.0} BPM"),
        })
    } else {
        None
    };
    Some(FitReadout {
        source_seconds,
        source_bpm,
        fitted_seconds,
        ratio,
        doubt,
    })
}

/// What the Bars field means by `text` (MOO-39): a bar count, or with "bpm"
/// after it, the loop's own tempo, turned into the bar count that makes the
/// fitted span last that many bars at that tempo. `None` for text that
/// isn't a number, or a tempo with no audio to measure.
pub fn typed_bars(
    params: &SamplerParams,
    sample: Option<&SampleData>,
    slices: Option<&SliceMap>,
    text: &str,
) -> Option<f32> {
    let value = crate::values::parse_typed_value(text)?;
    if !text.to_ascii_lowercase().contains("bpm") {
        return Some(value.clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS));
    }
    let sample = sample?;
    let len = sample.frames.len();
    if len == 0 || sample.sample_rate == 0 || value <= 0.0 {
        return None;
    }
    let (start, end) = mooloop_dsp::Sampler::fitted_span_in(*params, len, slices);
    let rate = f64::from(sample.sample_rate);
    let bar = mooloop_core::frames_per_bar(sample.sample_rate, f64::from(value)) / rate;
    let bars = ((end - start) / rate / bar) as f32;
    Some(bars.clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS))
}

/// One of the four draggable positions on the waveform.
#[derive(Clone, Copy, PartialEq)]
pub enum SampleMarker {
    Start,
    End,
    LoopStart,
    LoopEnd,
}

impl SampleMarker {
    /// Every marker, in the order the editor lists them.
    pub const ALL: [Self; 4] = [Self::Start, Self::End, Self::LoopStart, Self::LoopEnd];

    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::End => "End",
            Self::LoopStart => "Loop start",
            Self::LoopEnd => "Loop end",
        }
    }

    pub fn get(self, params: &SamplerParams) -> f32 {
        match self {
            Self::Start => params.start,
            Self::End => params.end,
            Self::LoopStart => params.loop_start,
            Self::LoopEnd => params.loop_end,
        }
    }

    pub fn set(self, params: &mut SamplerParams, value: f32) {
        match self {
            Self::Start => params.start = value,
            Self::End => params.end = value,
            Self::LoopStart => params.loop_start = value,
            Self::LoopEnd => params.loop_end = value,
        }
    }
}

/// The nearest zero crossing to a marker, within its neighbours.
pub fn snap_marker(
    params: &SamplerParams,
    sample: &SampleData,
    marker: SampleMarker,
    requested: f32,
) -> Option<(f32, SnapResult)> {
    let len = sample.frames.len();
    if len < 2 {
        return None;
    }
    let last = len - 1;
    let frame = |fraction: f32| frame_from_fraction(fraction, len);
    // Each marker is penned in by its neighbours. `saturating_sub` and the
    // `min(last)` guards keep a degenerate region (an empty or inverted one
    // already in the params) from producing a reversed range.
    let bounds = match marker {
        SampleMarker::Start => 0..=frame(params.end).saturating_sub(1),
        SampleMarker::End => (frame(params.start) + 1).min(last)..=last,
        SampleMarker::LoopStart => frame(params.start)..=frame(params.loop_end).saturating_sub(1),
        SampleMarker::LoopEnd => (frame(params.loop_start) + 1).min(last)..=frame(params.end),
    };
    let window = snap_window_frames(DEFAULT_SNAP_WINDOW_MS, sample.sample_rate);
    let result = snap_to_zero_crossing(&sample.frames, frame(requested), window, bounds);
    Some((fraction_from_frame(result.resolved, len), result))
}

/// What one marker's snap did, for the status bar.
pub fn snap_status(marker: SampleMarker, result: SnapResult) -> String {
    if result.moved() {
        format!(
            "{} snapped to frame {} ({:+} frames)",
            marker.label(),
            result.resolved,
            result.offset()
        )
    } else {
        format!(
            "{} kept at frame {}: no zero crossing within {} ms",
            marker.label(),
            result.requested,
            DEFAULT_SNAP_WINDOW_MS as i32
        )
    }
}

/// What snapping every marker at once did.
pub struct SnapAll {
    /// Each marker and where it ended up, for the view to write back.
    pub resolved: Vec<(SampleMarker, f32)>,
    pub moved: usize,
    pub searched: usize,
    pub command: EngineCommand,
}

/// A stretch that was baked into the audio.
pub struct Committed {
    /// The ratio that was baked, for the status bar to report.
    pub ratio: f32,
    pub command: EngineCommand,
}

impl Session {
    /// The slice map a marker edit acts on, plus the channel it belongs to.
    fn sliced_channel(&mut self) -> Option<(usize, &mut ChannelState)> {
        let selected = self.selected;
        Some((selected, self.channels.get_mut(selected)?))
    }

    /// Adds a marker at a normalized editor position.
    pub fn add_slice(&mut self, position: f32, snap: bool) -> SliceEdit {
        let Some((_, channel)) = self.sliced_channel() else {
            return SliceEdit::Ignored;
        };
        let Some(frame) = resolve_slice_frame(channel, position, snap) else {
            return SliceEdit::Ignored;
        };
        if channel.slices.add(frame).is_none() {
            return SliceEdit::Refused;
        }
        let markers = slice_fractions(channel);
        self.mark_dirty();
        SliceEdit::Changed(markers)
    }

    /// Drags a marker.
    ///
    /// Addressed by id rather than by position, because a drag past a
    /// neighbour reorders the map and the next move frame still means the
    /// marker under the pointer. Not snapped while dragging either: a marker
    /// that jumps to a crossing under the pointer fights the drag, so the
    /// AUTO snap lands it on release instead.
    pub fn move_slice(&mut self, index: i32, position: f32) -> Option<Vec<f32>> {
        let (_, channel) = self.sliced_channel()?;
        let id = channel
            .slices
            .get(index.max(0) as usize)
            .map(|marker| marker.id)?;
        let frame = resolve_slice_frame(channel, position, false)?;
        if !channel.slices.move_to(id, frame) {
            return None;
        }
        let markers = slice_fractions(channel);
        self.mark_dirty();
        Some(markers)
    }

    /// Deletes a marker.
    pub fn remove_slice(&mut self, index: i32) -> Option<Vec<f32>> {
        let (_, channel) = self.sliced_channel()?;
        let id = channel
            .slices
            .get(index.max(0) as usize)
            .map(|marker| marker.id)?;
        channel.slices.remove(id);
        let markers = slice_fractions(channel);
        self.mark_dirty();
        Some(markers)
    }

    /// Replaces the map with `count` evenly spaced markers across the
    /// playback region. `None` means there is no audio to slice.
    ///
    /// Grid divisions land wherever the arithmetic puts them, which is as
    /// likely to be mid-waveform as a hand-placed marker is, so snapping them
    /// is the same reason the trim markers snap, multiplied by the count.
    pub fn divide_slices(&mut self, count: i32, snap: bool) -> Option<Vec<f32>> {
        let (_, channel) = self.sliced_channel()?;
        let sample = channel.published_sample().cloned()?;
        let len = sample.frames.len();
        let start = frame_from_fraction(channel.sampler_params().start, len) as u32;
        let end = frame_from_fraction(channel.sampler_params().end, len) as u32;
        channel
            .slices
            .divide_evenly(count.max(1) as usize, start, end);
        if snap {
            let params = channel.sampler_params();
            let snapped: Vec<SliceMarker> = channel
                .slices
                .markers()
                .iter()
                .map(|marker| SliceMarker {
                    frame: snap_slice_frame(&params, &sample, marker.frame as usize) as u32,
                    ..*marker
                })
                .collect();
            channel.slices.rebuild(snapped);
        }
        let markers = slice_fractions(channel);
        self.mark_dirty();
        Some(markers)
    }

    /// Turns fit-to-tempo on or off for the selected channel (MOO-39).
    ///
    /// Off *freezes* the ratio SYNC was running into the ratio knob (Adam,
    /// 2026-09-24), so the loop keeps sounding the same until the knob is
    /// touched. The caller records the whole change as one undo step. `None`
    /// when nothing changed.
    pub fn set_stretch_sync(&mut self, on: bool, bpm: f64) -> Option<SamplerParams> {
        let (_, channel) = self.sliced_channel()?;
        let before = channel.sampler_params();
        if before.stretch_sync == on {
            return None;
        }
        let frozen = if on {
            None
        } else {
            channel
                .published_sample()
                .map(|sample| {
                    mooloop_dsp::Sampler::synced_ratio(before, sample, bpm, Some(&channel.slices))
                })
        };
        let params = channel.sampler_params_mut()?;
        params.stretch_sync = on;
        if let Some(ratio) = frozen {
            params.stretch_ratio =
                ratio.clamp(mooloop_core::MIN_STRETCH_RATIO, mooloop_core::MAX_STRETCH_RATIO);
        }
        let after = *params;
        self.mark_dirty();
        Some(after)
    }

    /// Detects the onsets in the selected channel's playback region, as
    /// source frames, for the editor to preview (MOO-44). Nothing changes
    /// until [`Self::accept_slices`]; `None` with no audio.
    ///
    /// Runs on the calling thread: a two-bar break is a few milliseconds of
    /// arithmetic, and nothing here is near the audio thread.
    pub fn detect_slices(&self, settings: OnsetSettings) -> Option<Vec<u32>> {
        let channel = self.channels.get(self.selected)?;
        let sample = channel.published_sample()?;
        let len = sample.frames.len();
        let params = channel.sampler_params();
        let start = frame_from_fraction(params.start, len);
        let end = frame_from_fraction(params.end, len);
        Some(
            detect_onsets(&sample.frames, sample.sample_rate, start, end, settings)
                .into_iter()
                .map(|frame| frame as u32)
                .collect(),
        )
    }

    /// Accepts a detection preview (MOO-44). Replace keeps the markers
    /// placed by hand and replaces the rest, and Merge adds beside every
    /// marker. A detected frame within `settings.min_spacing_ms` of a kept
    /// marker is left out. Returns the map as fractions and how many markers
    /// were added; the caller records it as one undo step.
    pub fn accept_slices(
        &mut self,
        frames: &[u32],
        settings: OnsetSettings,
        how: SliceAccept,
    ) -> Option<(Vec<f32>, usize)> {
        let (_, channel) = self.sliced_channel()?;
        let rate = channel.published_sample()?.sample_rate;
        let gap = (settings.min_spacing_ms.max(0.0) / 1_000.0 * rate as f32) as u32;
        let added = match how {
            SliceAccept::Replace => channel.slices.replace_detected(frames, gap),
            SliceAccept::Merge => channel.slices.merge_detected(frames, gap),
        };
        let markers = slice_fractions(channel);
        self.mark_dirty();
        Some((markers, added))
    }

    /// Empties the slice map.
    pub fn clear_slices(&mut self) -> Option<Vec<f32>> {
        let (_, channel) = self.sliced_channel()?;
        channel.slices.clear();
        self.mark_dirty();
        Some(Vec::new())
    }

    /// Bakes the pending stretch into the channel's audio.
    ///
    /// Always rendered from the *source*, never from a buffer that has
    /// already been baked: re-committing at a new tempo has to be a fresh
    /// render, or repeated tempo changes accumulate stretch on stretch.
    ///
    /// `Err(true)` means there is no sample at all; `Err(false)` means there
    /// is nothing the current parameters would change.
    pub fn commit_stretch(&mut self, bpm: f64) -> Result<Committed, bool> {
        let selected = self.selected;
        let Some(channel) = self.channels.get_mut(selected) else {
            return Err(true);
        };
        let Some(source) = channel.sample_data.clone() else {
            return Err(true);
        };
        let (params, slices) = match channel.commit.as_ref() {
            Some(commit) => mooloop_dsp::commit::revert_commit(channel.sampler_params(), commit),
            None => (channel.sampler_params(), channel.slices.clone()),
        };
        let Some(committed) = mooloop_dsp::commit::commit_stretch(&source, params, &slices, bpm)
        else {
            return Err(false);
        };
        let ratio = committed.commit.ratio;
        if let Some(p) = channel.sampler_params_mut() {
            *p = committed.params;
        }
        channel.slices = committed.slices;
        channel.commit = Some(Box::new(committed.commit));
        channel.committed_sample = Some(committed.sample);
        refresh_sample_view(channel);
        let params = channel.sampler_params();
        self.mark_dirty();
        Ok(Committed {
            ratio,
            command: EngineCommand::SetChannelSamplerParams {
                channel: selected as u8,
                params,
            },
        })
    }

    /// Throws away a committed render and goes back to the source.
    ///
    /// Returns the restored parameters, which the caller needs to build the
    /// live stretcher the patch is asking for again.
    pub fn revert_stretch(&mut self) -> Option<(SamplerParams, EngineCommand)> {
        let selected = self.selected;
        let channel = self.channels.get_mut(selected)?;
        let commit = channel.commit.take()?;
        let (params, slices) = mooloop_dsp::commit::revert_commit(channel.sampler_params(), &commit);
        if let Some(p) = channel.sampler_params_mut() {
            *p = params;
        }
        channel.slices = slices;
        channel.committed_sample = None;
        refresh_sample_view(channel);
        self.mark_dirty();
        Some((
            params,
            EngineCommand::SetChannelSamplerParams {
                channel: selected as u8,
                params,
            },
        ))
    }

    /// Snaps all four markers to zero crossings at once.
    pub fn snap_all_markers(&mut self) -> Option<SnapAll> {
        let selected = self.selected;
        let channel = self.channels.get_mut(selected)?;
        let sample = channel.published_sample().cloned()?;
        let mut resolved = Vec::with_capacity(SampleMarker::ALL.len());
        let (mut moved, mut searched) = (0usize, 0usize);
        for marker in SampleMarker::ALL {
            let requested = marker.get(&channel.sampler_params());
            let Some((value, result)) = snap_marker(&channel.sampler_params(), &sample, marker, requested)
            else {
                continue;
            };
            searched += 1;
            if result.moved() {
                moved += 1;
            }
            if let Some(params) = channel.sampler_params_mut() {
                marker.set(params, value);
            }
            resolved.push((marker, value));
        }
        let params = channel.sampler_params();
        Some(SnapAll {
            resolved,
            moved,
            searched,
            command: EngineCommand::SetChannelSamplerParams {
                channel: selected as u8,
                params,
            },
        })
    }

    /// How many markers the map can still take.
    pub fn slice_headroom(&self) -> usize {
        MAX_SLICES.saturating_sub(
            self.channels
                .get(self.selected)
                .map_or(MAX_SLICES, |channel| channel.slices.len()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A one-second 48 kHz ramp: no zero crossings to snap to except the
    /// first frame, so tests can be explicit about position.
    fn session_with_audio() -> Session {
        let mut session = Session::default();
        let frames: Vec<[f32; 2]> = (0..48_000)
            .map(|i| {
                let v = i as f32 / 48_000.0;
                [v, v]
            })
            .collect();
        session.channels[0].sample_data = Some(Arc::new(SampleData {
            frames,
            sample_rate: 48_000,
            root_note: 60,
        }));
        session
    }

    /// The readout says what the fit is doing: a one-second loop called one
    /// bar is a 240 BPM loop, too fast to believe, so the readout doubts it
    /// and names the bar count that would put it in range. Called half a bar
    /// it is 120 BPM, and fitted to half a bar at the project's tempo.
    #[test]
    fn the_fit_readout_says_what_the_loop_is_and_doubts_a_wild_tempo() {
        let session = session_with_audio();
        let sample = session.channels[0].published_sample().cloned().expect("audio");
        let mut params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: 1.0,
            ..SamplerParams::default()
        };
        let wild = fit_readout(&params, &sample, None, 100.0).expect("a readout");
        assert!((wild.source_seconds - 1.0).abs() < 1.0e-9);
        assert!((wild.source_bpm - 240.0).abs() < 1.0e-6, "{}", wild.source_bpm);
        let doubt = wild.doubt.expect("240 BPM should be doubted");
        assert!(doubt.contains("0.5 bars"), "{doubt}");

        params.stretch_bars = 0.5;
        let sane = fit_readout(&params, &sample, None, 100.0).expect("a readout");
        assert!((sane.source_bpm - 120.0).abs() < 1.0e-6);
        assert!(sane.doubt.is_none(), "{:?}", sane.doubt);
        // Half a bar at 100 BPM is 1.2 s.
        assert!((sane.fitted_seconds - 1.2).abs() < 1.0e-9, "{}", sane.fitted_seconds);
    }

    /// The Bars field takes the loop's own tempo too: a one-second loop at
    /// 120 BPM is half a bar, and a plain number is still a bar count.
    #[test]
    fn a_typed_tempo_becomes_the_bar_count_that_matches_it() {
        let session = session_with_audio();
        let sample = session.channels[0].published_sample().cloned().expect("audio");
        let params = SamplerParams::default();
        assert_eq!(typed_bars(&params, Some(sample.as_ref()), None, "120 bpm"), Some(0.5));
        assert_eq!(typed_bars(&params, Some(sample.as_ref()), None, "60BPM"), Some(0.25));
        assert_eq!(typed_bars(&params, Some(sample.as_ref()), None, "4"), Some(4.0));
        assert_eq!(typed_bars(&params, None, None, "120 bpm"), None, "no audio to measure");
        assert_eq!(typed_bars(&params, Some(sample.as_ref()), None, "fast"), None);
    }

    /// Turning SYNC off freezes the ratio it was running into the knob
    /// (MOO-39), so the stretch the loop plays at doesn't move at the switch;
    /// turning it back on leaves the knob alone.
    #[test]
    fn turning_sync_off_freezes_the_ratio_it_was_running() {
        let mut session = session_with_audio();
        if let Some(params) = session.channels[0].sampler_params_mut() {
            params.stretch_enabled = true;
            params.stretch_sync = true;
            params.stretch_bars = 1.0;
            params.stretch_ratio = 1.0;
        }
        let bpm = 90.0;
        let sample = session.channels[0].published_sample().cloned().expect("audio");
        let running =
            mooloop_dsp::Sampler::synced_ratio(session.channels[0].sampler_params(), &sample, bpm, None);
        // One second of audio as one bar at 90 BPM (2.667 s) is a real
        // stretch, so a frozen 1.0 could not pass by accident.
        assert!((running - 1.0).abs() > 0.5, "the premise: {running}");

        let off = session.set_stretch_sync(false, bpm).expect("a change");
        assert!(!off.stretch_sync);
        assert_eq!(off.stretch_ratio, running);
        assert!(session.dirty);
        assert!(session.set_stretch_sync(false, bpm).is_none(), "already off");

        let on = session.set_stretch_sync(true, 140.0).expect("a change");
        assert!(on.stretch_sync);
        assert_eq!(on.stretch_ratio, running, "turning SYNC on moved the knob");
    }

    /// Markers are reported as fractions of the published buffer, which is
    /// the coordinate system the waveform and every other marker use.
    #[test]
    fn slices_are_reported_as_fractions_of_the_buffer() {
        let mut session = session_with_audio();

        let SliceEdit::Changed(markers) = session.add_slice(0.5, false) else {
            panic!("a slice at the midpoint should have been added");
        };
        assert_eq!(markers.len(), 1);
        assert!((markers[0] - 0.5).abs() < 1.0e-3, "{markers:?}");
        assert!(session.dirty);
    }

    /// With no audio behind it there is nothing a position resolves to, and
    /// that is not the same as a refusal.
    #[test]
    fn a_slice_with_no_audio_is_ignored_rather_than_refused() {
        let mut session = Session::default();
        assert!(matches!(session.add_slice(0.5, false), SliceEdit::Ignored));
        assert!(!session.dirty);
    }

    /// The map has a hard limit, and a full one is a refusal the user is told
    /// about rather than a silent no-op.
    #[test]
    fn a_full_slice_map_refuses() {
        let mut session = session_with_audio();
        session
            .divide_slices(MAX_SLICES as i32, false)
            .expect("there is audio");
        assert_eq!(session.slice_headroom(), 0);
        assert!(matches!(session.add_slice(0.5001, false), SliceEdit::Refused));
    }

    /// A drag past a neighbour reorders the map; addressing by id is what
    /// keeps the next frame of the drag on the same marker.
    #[test]
    fn dragging_a_marker_past_its_neighbour_keeps_hold_of_it() {
        let mut session = session_with_audio();
        session.add_slice(0.25, false);
        session.add_slice(0.75, false);

        // Drag the first marker past the second.
        let markers = session.move_slice(0, 0.9).expect("marker 0 exists");
        assert_eq!(markers.len(), 2);
        let mut sorted = markers.clone();
        sorted.sort_by(f32::total_cmp);
        assert_eq!(markers, sorted, "the map came back out of order");
        assert!((sorted[1] - 0.9).abs() < 1.0e-3, "{sorted:?}");
    }

    #[test]
    fn dividing_lays_markers_across_the_playback_region_and_clearing_empties_it() {
        let mut session = session_with_audio();

        let markers = session.divide_slices(4, false).expect("there is audio");
        assert_eq!(markers.len(), 4);
        assert!((markers[0] - 0.0).abs() < 1.0e-3, "{markers:?}");
        assert!((markers[1] - 0.25).abs() < 1.0e-3, "{markers:?}");

        assert_eq!(session.clear_slices(), Some(Vec::new()));
        assert!(session.channels[0].slices.is_empty());

        // Nothing to divide is a different answer from dividing into nothing.
        let mut empty = Session::default();
        assert!(empty.divide_slices(4, false).is_none());
    }

    #[test]
    fn removing_a_marker_addresses_it_by_position_in_the_map() {
        let mut session = session_with_audio();
        session.add_slice(0.25, false);
        session.add_slice(0.75, false);

        let markers = session.remove_slice(0).expect("marker 0 exists");
        assert_eq!(markers.len(), 1);
        assert!((markers[0] - 0.75).abs() < 1.0e-3);
        assert!(session.remove_slice(5).is_none());
    }
}
