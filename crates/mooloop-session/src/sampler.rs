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
    fraction_from_frame, frame_from_fraction, snap_to_zero_crossing, snap_window_frames,
    SnapResult, DEFAULT_SNAP_WINDOW_MS,
};
use mooloop_dsp::SampleData;
use mooloop_core::{EngineCommand, SampleCommit, SamplerParams, SliceMarker, StretchMode, MAX_SLICES};

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
        snap_slice_frame(&channel.params, sample, frame)
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
    let params = channel.params;
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
        ..channel.params
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
        self.dirty = true;
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
        self.dirty = true;
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
        self.dirty = true;
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
        let start = frame_from_fraction(channel.params.start, len) as u32;
        let end = frame_from_fraction(channel.params.end, len) as u32;
        channel
            .slices
            .divide_evenly(count.max(1) as usize, start, end);
        if snap {
            let params = channel.params;
            let snapped: Vec<SliceMarker> = channel
                .slices
                .markers()
                .iter()
                .map(|marker| SliceMarker {
                    id: marker.id,
                    frame: snap_slice_frame(&params, &sample, marker.frame as usize) as u32,
                })
                .collect();
            channel.slices.rebuild(snapped);
        }
        let markers = slice_fractions(channel);
        self.dirty = true;
        Some(markers)
    }

    /// Empties the slice map.
    pub fn clear_slices(&mut self) -> Option<Vec<f32>> {
        let (_, channel) = self.sliced_channel()?;
        channel.slices.clear();
        self.dirty = true;
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
            Some(commit) => mooloop_dsp::commit::revert_commit(channel.params, commit),
            None => (channel.params, channel.slices.clone()),
        };
        let Some(committed) = mooloop_dsp::commit::commit_stretch(&source, params, &slices, bpm)
        else {
            return Err(false);
        };
        let ratio = committed.commit.ratio;
        channel.params = committed.params;
        channel.slices = committed.slices;
        channel.commit = Some(Box::new(committed.commit));
        channel.committed_sample = Some(committed.sample);
        refresh_sample_view(channel);
        let params = channel.params;
        self.dirty = true;
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
        let (params, slices) = mooloop_dsp::commit::revert_commit(channel.params, &commit);
        channel.params = params;
        channel.slices = slices;
        channel.committed_sample = None;
        refresh_sample_view(channel);
        self.dirty = true;
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
            let requested = marker.get(&channel.params);
            let Some((value, result)) = snap_marker(&channel.params, &sample, marker, requested)
            else {
                continue;
            };
            searched += 1;
            if result.moved() {
                moved += 1;
            }
            marker.set(&mut channel.params, value);
            resolved.push((marker, value));
        }
        let params = channel.params;
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
