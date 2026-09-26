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
use mooloop_core::sampler::stretch_pool_voices;
use mooloop_core::GeneratorParams;
use mooloop_dsp::sampler::ZoneAudio;
use mooloop_dsp::{ChannelAudioSnapshot, SampleData, StretchPool};
use mooloop_core::{KeyRange, SampleReference, SampleZone, SamplerState};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use mooloop_engine::StructuralCommand;
use mooloop_core::{
    EngineCommand, NoteEvent, NoteId, SampleCommit, SamplerParams, SliceMap, SliceMarker,
    StretchMode, MAX_PATTERN_STEPS, MAX_SLICES, TICKS_PER_BAR, TICKS_PER_STEP,
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

/// The notes that play a sliced break back as it was cut (MOO-46).
#[derive(Debug, Clone, PartialEq)]
pub struct SlicePattern {
    /// One note per reachable slice, in slice order, ids from `first_id`.
    pub notes: Vec<NoteEvent>,
    /// The pattern length, in steps, that holds every note.
    pub steps: u32,
    /// Slices whose note would be past 127, left out and counted here.
    pub unreachable: usize,
}

/// Where each slice starts, as a note on the tick it falls on (MOO-46).
///
/// The playback region is the break, and it lasts `stretch_bars` bars, the
/// musical length fit-to-tempo already uses. So the notes land where the
/// break's own hits land when it plays at the project's tempo. Slice `i` is
/// note `slice_base_note + i`, the mapping the sampler plays, which gives one
/// addressing scheme, not two. Each note lasts until the next slice starts,
/// and the last one until the break ends.
///
/// `grid` is a quantize step in ticks, or `None` to keep each hit where it
/// really falls, which is most of a break's character. Quantizing can put
/// two slices on one tick. Both are kept, and the earlier one lasts a tick
/// rather than vanishing.
pub fn slice_pattern(
    params: &SamplerParams,
    slices: &SliceMap,
    sample_len: usize,
    grid: Option<u32>,
    first_id: NoteId,
) -> SlicePattern {
    let (start, end) = mooloop_dsp::Sampler::resolve_playback_bounds(*params, sample_len, None);
    let region = (end - start).max(1.0);
    let bars = f64::from(params.stretch_bars.clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS));
    let ticks_per_bar = f64::from(TICKS_PER_BAR);
    let break_ticks = (bars * ticks_per_bar).round().max(1.0) as u32;
    let tick_of = |frame: f64| {
        let tick = ((frame - start) / region * bars * ticks_per_bar).round().max(0.0) as u32;
        match grid {
            Some(step) if step > 0 => ((tick + step / 2) / step) * step,
            _ => tick,
        }
        .min(break_ticks.saturating_sub(1))
    };
    // Slices that begin inside the region, in order, with the note each
    // plays. A marker before the region start plays nothing there.
    let base = u32::from(params.slice_base_note.min(127));
    let mut starts = Vec::new();
    let mut unreachable = 0;
    for (index, marker) in slices.markers().iter().enumerate() {
        let frame = f64::from(marker.frame);
        if frame < start || frame >= end {
            continue;
        }
        let note = base + index as u32;
        if note > 127 {
            unreachable += 1;
            continue;
        }
        starts.push((tick_of(frame), note as u8));
    }
    let mut notes = Vec::with_capacity(starts.len());
    for (n, &(tick, note)) in starts.iter().enumerate() {
        let next = starts.get(n + 1).map_or(break_ticks, |&(next, _)| next);
        let length = next.saturating_sub(tick).max(1);
        notes.push(NoteEvent::new(first_id + n as NoteId, tick, length, note, 100));
    }
    let steps = break_ticks.div_ceil(TICKS_PER_STEP);
    SlicePattern {
        notes,
        steps,
        unreachable,
    }
}

/// What writing a slice pattern did (MOO-46), for the status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceWrite {
    /// Notes written.
    pub notes: usize,
    /// Notes the channel's pattern already held and the write removed.
    pub replaced: usize,
    /// Slices with no note to play them (past MIDI 127), left out.
    pub unreachable: usize,
    /// Slices past the pattern's longest length, or past its note limit,
    /// left out.
    pub dropped: usize,
    /// The pattern length the write left, in steps, if it grew it.
    pub grew_to: Option<usize>,
}

impl Session {
    /// Writes the selected sampler's slices into the current pattern as
    /// notes, one per slice, where each falls in the break (MOO-46).
    ///
    /// `replace` clears this channel's notes in the pattern first; otherwise
    /// the slices are added beside them. A pattern shorter than the break
    /// grows to hold it, up to the longest a pattern can be. The notes are
    /// ordinary pattern data from then on, not linked to the slice table.
    /// Returns the engine commands to send and what was done; `None` when
    /// the channel has no slices to write. The caller records one undo
    /// step.
    pub fn write_slice_pattern(
        &mut self,
        grid: Option<u32>,
        replace: bool,
    ) -> Option<(Vec<EngineCommand>, SliceWrite)> {
        let selected = self.selected;
        let pattern = self.current_pattern;
        let channel = self.channels.get(selected)?;
        if channel.slices.is_empty() {
            return None;
        }
        let len = channel.published_sample()?.frames.len();
        let written = slice_pattern(
            &channel.sampler_params(),
            &channel.slices,
            len,
            grid,
            channel.next_note_id,
        );
        let mut commands = Vec::new();
        // The pattern grows to hold the break, never past its longest.
        let steps = (written.steps as usize).min(usize::from(MAX_PATTERN_STEPS));
        let grew_to = if steps > self.pattern_lengths[pattern] {
            self.set_pattern_length(steps as i32).map(|change| {
                commands.push(EngineCommand::SetPatternLength {
                    pattern: change.pattern as u8,
                    length_steps: change.length as u16,
                });
                change.length
            })
        } else {
            None
        };
        let length_ticks = self.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
        let channel = &mut self.channels[selected];
        let mut replaced = 0;
        if replace {
            for note in channel.notes[pattern].drain(..) {
                commands.push(EngineCommand::RemoveNote {
                    pattern: pattern as u8,
                    channel: selected as u8,
                    id: note.id,
                });
                replaced += 1;
            }
        }
        let mut notes = 0;
        let mut dropped = 0;
        for note in written.notes {
            if note.start_tick >= length_ticks || !channel.has_room_for(pattern, 1) {
                dropped += 1;
                continue;
            }
            let note = NoteEvent {
                duration_ticks: note.duration_ticks.min(length_ticks - note.start_tick),
                ..note
            };
            channel.next_note_id = channel.next_note_id.max(note.id).wrapping_add(1).max(1);
            channel.notes[pattern].push(note);
            commands.push(EngineCommand::UpsertNote {
                pattern: pattern as u8,
                channel: selected as u8,
                note,
            });
            notes += 1;
        }
        channel.notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
        self.mark_dirty();
        Some((
            commands,
            SliceWrite {
                notes,
                replaced,
                unreachable: written.unreachable,
                dropped,
                grew_to,
            },
        ))
    }
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


/// Keeping each sampler's stretch pool the size its patch and lanes ask for
/// (MOO-7).
///
/// The pool follows Voices (`mooloop_core::sampler::stretch_pool_voices`),
/// and Voices has more than one front door on the control thread: the face's
/// stepper, a MIDI-learned binding, a preset or kit load, the STRETCH switch,
/// a commit and a revert. Deriving and diffing once a pump tick is what
/// cannot be forgotten by the next door somebody adds, the reason the
/// compensation, console and solo plans are reconciled the same way. An
/// automation lane is the one path on the audio thread, and the helper sizes
/// a channel with a lane on Voices for every voice.
///
/// An undo or a load installs pools of its own (`ChannelStrip::load_source`
/// sizes them with the same helper), and `replace_project` forgets this
/// mirror, so the first tick after it re-sends each stretching channel's
/// pool once. The engine's install keeps the readers the two pools share, so
/// the duplicate costs an allocation and changes nothing that sounds.
impl Session {
    /// Send a pool (or its removal) for every channel whose wanted size is
    /// not what was last sent, through `send`; a refused send is retried next
    /// tick. On a tick where nothing changed, this is one comparison a
    /// channel and allocates nothing: the lanes are only walked for a sampler
    /// that stretches.
    pub fn sync_sampler_stretch(
        &mut self,
        sample_rate: u32,
        mut send: impl FnMut(StructuralCommand) -> bool,
    ) {
        if self.sampler_stretch_sent.len() != self.channels.len() {
            self.sampler_stretch_sent.resize(self.channels.len(), None);
        }
        for (index, channel) in self.channels.iter().enumerate() {
            let GeneratorParams::Sampler(params) = channel.generator_params() else {
                // Not a sampler: its source was rebuilt without a pool, so
                // there is nothing to take back.
                self.sampler_stretch_sent[index] = Some(0);
                continue;
            };
            let wanted = stretch_pool_voices(&params, &channel.automation);
            let sent = self.sampler_stretch_sent[index];
            if sent == Some(wanted) || (sent.is_none() && wanted == 0) {
                self.sampler_stretch_sent[index] = Some(wanted);
                continue;
            }
            let Ok(channel_index) = u8::try_from(index) else {
                continue;
            };
            let pool = (wanted > 0).then(|| {
                Box::new(StretchPool::new(params.stretch_mode, sample_rate, wanted))
            });
            if send(StructuralCommand::SetSamplerStretch {
                channel: channel_index,
                pool,
            }) {
                self.sampler_stretch_sent[index] = Some(wanted);
            }
        }
    }
}

/// A key zone as the session holds it (MOO-14): the persisted zone, and its
/// decoded buffer beside it, the way [`ChannelState::sample_data`] sits
/// beside `sample_path`. `sample` is `None` for a zone whose audio is not in
/// hand -- a file that is missing or would not decode -- which plays nothing
/// and is reported by [`Session::missing_zones`].
#[derive(Clone)]
pub struct ZoneState {
    pub zone: SampleZone,
    pub sample: Option<Arc<SampleData>>,
}

impl ZoneState {
    /// The zone as a voice reads it.
    pub fn audio(&self) -> ZoneAudio {
        ZoneAudio::new(&self.zone, self.sample.clone())
    }

    /// The file this zone names, if it names one.
    pub fn path(&self) -> Option<&Path> {
        match &self.zone.sample {
            SampleReference::File { path, .. } => Some(path),
            SampleReference::Builtin { .. } | SampleReference::Empty => None,
        }
    }

    /// A file the zone names whose audio is not in hand.
    pub fn is_missing(&self) -> bool {
        self.path().is_some() && self.sample.is_none()
    }
}

/// Every key-zone buffer the session has decoded, by the path the document
/// names it by (MOO-14).
///
/// **Why a table, and why by path.** The undo history holds whole documents
/// and each channel's *base* sample (`ProjectSnapshot::samples`), not zone
/// audio, and a document names a zone's audio only by its file. So when an
/// undo brings back a zone the session has since dropped, this is where its
/// buffer is found again -- without decoding on the UI thread, which a
/// restore must never do. Entries are the same `Arc`s the channels hold, not
/// copies.
///
/// **What it assumes:** a file does not change its contents under the same
/// path while a song is open. That holds for what the app writes itself: a
/// recorded take gets a new name, `<date>-<time>-<channel>.wav` to the
/// second (`take.rs`, `take_file_name`), and a song's embedded copies get
/// fresh `NN-` names (`mooloop-project`, `unclaimed_name`). A file edited in
/// another program while the song is open plays its old audio until the song
/// is reopened, which is what the base sample does too.
///
/// **Its lifetime is the document's.** Opening or creating a document
/// replaces the whole table with that document's decode
/// ([`Session::admit_zone_audio`]); a kit or preset merged into the open song
/// adds to it; undo and redo never touch it. So a long session holds the open
/// song's zone buffers and the ones its history can reach, not every song's.
#[derive(Default)]
pub struct ZoneAudioTable {
    entries: HashMap<PathBuf, Arc<SampleData>>,
}

impl ZoneAudioTable {
    pub fn get(&self, path: &Path) -> Option<&Arc<SampleData>> {
        self.entries.get(path)
    }

    pub fn insert(&mut self, path: PathBuf, sample: Arc<SampleData>) {
        self.entries.insert(path, sample);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A sampler channel's engine snapshot, from the channel as the session
/// holds it: what the per-channel publish sends.
pub fn channel_audio(channel: &ChannelState) -> ChannelAudioSnapshot {
    ChannelAudioSnapshot::for_sampler(
        channel.published_sample().cloned(),
        &channel.slices,
        channel.keys,
        channel.zones.iter().map(ZoneState::audio).collect(),
    )
}

impl Session {
    /// Take in the zone buffers a document's load decoded on its worker.
    ///
    /// `opens` is a song being opened or created: the table is emptied first,
    /// because nothing in the outgoing song's history can be undone into
    /// the new one. A kit or a preset merged into the open song adds to it.
    pub fn admit_zone_audio(&mut self, decoded: Vec<(PathBuf, Arc<SampleData>)>, opens: bool) {
        if opens {
            self.zone_audio.clear();
        }
        for (path, sample) in decoded {
            self.zone_audio.insert(path, sample);
        }
    }

    /// A sampler's zones with their buffers, found in the table by path.
    pub fn resolve_zones(&self, state: &SamplerState) -> Vec<ZoneState> {
        state
            .zones
            .iter()
            .map(|zone| ZoneState {
                zone: zone.clone(),
                sample: match &zone.sample {
                    SampleReference::File { path, .. } => self.zone_audio.get(path).cloned(),
                    SampleReference::Builtin { .. } | SampleReference::Empty => None,
                },
            })
            .collect()
    }

    /// The engine snapshot for a sampler about to be installed from a
    /// document, whose base buffer the caller has in hand. The live install
    /// builds its channels' audio with this before `replace_project` runs,
    /// so the two read the same table.
    pub fn sampler_install_audio(
        &self,
        sample: Option<Arc<SampleData>>,
        state: &SamplerState,
    ) -> ChannelAudioSnapshot {
        ChannelAudioSnapshot::for_sampler(
            sample,
            &state.slices,
            state.keys,
            self.resolve_zones(state).iter().map(ZoneState::audio).collect(),
        )
    }

    /// Every zone that names a file whose audio the session does not have,
    /// by channel. A missing zone is silent, and this is what says so.
    pub fn missing_zones(&self) -> Vec<(usize, PathBuf)> {
        self.channels
            .iter()
            .enumerate()
            .flat_map(|(index, channel)| {
                channel
                    .zones
                    .iter()
                    .filter(|zone| zone.is_missing())
                    .filter_map(move |zone| Some((index, zone.path()?.to_path_buf())))
            })
            .collect()
    }

    /// The per-seat zone buffers of every channel, parallel to each
    /// sampler's `zones`, for an export (MOO-14).
    pub fn zone_sample_snapshots(&self) -> Vec<Vec<Option<Arc<SampleData>>>> {
        self.channels
            .iter()
            .map(|channel| {
                if channel.kind() == mooloop_core::DeviceKind::Sampler {
                    channel.zones.iter().map(|zone| zone.sample.clone()).collect()
                } else {
                    Vec::new()
                }
            })
            .collect()
    }
}

/// The keys a new zone takes (MOO-14): everything above the highest key any
/// zone plays, or, when the keyboard is already covered to the top, the
/// upper half of the highest range, which that range gives up. Returns the
/// new zone's keys and, when a range was split, which one and what it keeps
/// (`None` for the base).
fn keys_for_new_zone(
    base: KeyRange,
    zones: &[ZoneState],
) -> (KeyRange, Option<(Option<usize>, KeyRange)>) {
    let top = zones
        .iter()
        .map(|zone| zone.zone.keys.high)
        .chain(std::iter::once(base.high))
        .max()
        .unwrap_or(127);
    if top < 127 {
        return (KeyRange::new(top + 1, 127), None);
    }
    // The range that reaches the top: the last zone that does, else the base.
    let owner = zones.iter().rposition(|zone| zone.zone.keys.high == 127);
    let range = owner.map_or(base, |index| zones[index].zone.keys);
    if range.low == range.high {
        // One key: nothing to split. The new zone shares it and, being later,
        // is never reached until the user moves one of them.
        return (range, None);
    }
    let middle = range.low + (range.high - range.low).div_ceil(2);
    (
        KeyRange::new(middle, range.high),
        Some((owner, KeyRange::new(range.low, middle - 1))),
    )
}

impl Session {
    /// Add a key zone to `channel` playing `sample`, decoded from `path`
    /// (MOO-14). It takes the keys [`keys_for_new_zone`] finds, rooted at its
    /// lowest key so that key plays the file at its own pitch; its buffer
    /// joins the zone table so an undo past its removal finds it again.
    /// `false` for a channel that is not a sampler.
    pub fn add_zone(&mut self, channel: usize, path: PathBuf, sample: Arc<SampleData>) -> bool {
        let Some(state) = self.channels.get_mut(channel) else {
            return false;
        };
        if state.kind() != mooloop_core::DeviceKind::Sampler {
            return false;
        }
        let (keys, split) = keys_for_new_zone(state.keys, &state.zones);
        match split {
            Some((None, kept)) => state.keys = kept,
            Some((Some(index), kept)) => state.zones[index].zone.keys = kept,
            None => {}
        }
        state.zones.push(ZoneState {
            zone: SampleZone {
                keys,
                root_note: keys.low,
                sample: SampleReference::File {
                    path: path.clone(),
                    embedded: false,
                },
                ..SampleZone::default()
            },
            sample: Some(sample.clone()),
        });
        self.zone_audio.insert(path, sample);
        self.mark_dirty();
        true
    }

    /// Set the base zone's keys; `low` and `high` are put in order.
    pub fn set_base_keys(&mut self, channel: usize, low: u8, high: u8) -> bool {
        let Some(state) = self.channels.get_mut(channel) else {
            return false;
        };
        let keys = KeyRange::new(low, high).repaired();
        if state.keys == keys {
            return false;
        }
        state.keys = keys;
        self.mark_dirty();
        true
    }

    /// Set zone `index`'s keys; `low` and `high` are put in order.
    pub fn set_zone_keys(&mut self, channel: usize, index: usize, low: u8, high: u8) -> bool {
        let Some(zone) = self.zone_mut(channel, index) else {
            return false;
        };
        let keys = KeyRange::new(low, high).repaired();
        if zone.zone.keys == keys {
            return false;
        }
        zone.zone.keys = keys;
        self.mark_dirty();
        true
    }

    /// Set zone `index`'s root key.
    pub fn set_zone_root(&mut self, channel: usize, index: usize, root: u8) -> bool {
        let Some(zone) = self.zone_mut(channel, index) else {
            return false;
        };
        let root = root.min(127);
        if zone.zone.root_note == root {
            return false;
        }
        zone.zone.root_note = root;
        self.mark_dirty();
        true
    }

    /// Remove zone `index`. Its buffer stays in the zone table, which is
    /// what an undo of this finds it in.
    pub fn remove_zone(&mut self, channel: usize, index: usize) -> bool {
        let Some(state) = self.channels.get_mut(channel) else {
            return false;
        };
        if index >= state.zones.len() {
            return false;
        }
        state.zones.remove(index);
        self.mark_dirty();
        true
    }

    fn zone_mut(&mut self, channel: usize, index: usize) -> Option<&mut ZoneState> {
        self.channels.get_mut(channel)?.zones.get_mut(index)
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

    /// A two-bar break sliced into eight: its notes land where each slice
    /// starts, as the base note plus the slice's position, each lasting to
    /// the next, and the pattern that holds them is the break's two bars
    /// (MOO-46).
    #[test]
    fn a_sliced_break_becomes_one_note_per_slice_where_it_falls() {
        let params = SamplerParams {
            play_mode: mooloop_core::PlayMode::Slice,
            stretch_bars: 2.0,
            ..SamplerParams::default()
        };
        let mut map = SliceMap::new();
        map.divide_evenly(8, 0, 96_000);
        let written = slice_pattern(&params, &map, 96_000, None, 10);
        let bar_ticks = TICKS_PER_BAR;
        assert_eq!(written.steps, 2 * mooloop_core::STEPS_PER_BAR);
        assert_eq!(written.unreachable, 0);
        assert_eq!(written.notes.len(), 8);
        for (n, note) in written.notes.iter().enumerate() {
            assert_eq!(note.id, 10 + n as NoteId);
            assert_eq!(note.start_tick, n as u32 * bar_ticks / 4);
            assert_eq!(note.duration_ticks, bar_ticks / 4);
            assert_eq!(note.note, mooloop_core::DEFAULT_SLICE_BASE_NOTE + n as u8);
        }
    }

    /// Unquantized, a hit off the grid stays off it; on a sixteenth grid it
    /// moves to the nearest sixteenth. Two slices landing on one tick both
    /// keep a note, and the earlier lasts a tick.
    #[test]
    fn a_break_off_the_grid_stays_off_it_unless_asked() {
        let params = SamplerParams {
            stretch_bars: 1.0,
            ..SamplerParams::default()
        };
        let mut map = SliceMap::new();
        // One bar of 48,000 frames is 384 ticks, 125 frames a tick.
        for frame in [0u32, 6_100, 6_130, 24_000] {
            map.add(frame);
        }
        let free = slice_pattern(&params, &map, 48_000, None, 1);
        let starts: Vec<u32> = free.notes.iter().map(|note| note.start_tick).collect();
        assert_eq!(starts, [0, 49, 49, 192]);
        assert_eq!(free.notes[1].duration_ticks, 1, "a slice on the same tick lasts one");
        let gridded = slice_pattern(&params, &map, 48_000, Some(TICKS_PER_STEP), 1);
        let starts: Vec<u32> = gridded.notes.iter().map(|note| note.start_tick).collect();
        assert_eq!(starts, [0, 48, 48, 192]);
    }

    /// A slice whose note would be past 127 has no note to play it: it is
    /// left out and counted, not wrapped or clamped onto another slice.
    #[test]
    fn slices_past_the_note_range_are_counted_not_played() {
        let params = SamplerParams {
            slice_base_note: 120,
            ..SamplerParams::default()
        };
        let mut map = SliceMap::new();
        map.divide_evenly(12, 0, 48_000);
        let written = slice_pattern(&params, &map, 48_000, None, 1);
        assert_eq!(written.notes.len(), 8, "notes 120 to 127");
        assert_eq!(written.unreachable, 4);
        assert!(written.notes.iter().all(|note| note.note <= 127));
    }

    /// Writing replaces this channel's notes in the pattern when asked and
    /// adds beside them otherwise, grows a short pattern to the break, and
    /// hands back the commands that tell the engine.
    #[test]
    fn writing_a_slice_pattern_replaces_or_adds_and_grows_the_pattern() {
        let mut session = session_with_audio();
        if let Some(params) = session.channels[0].sampler_params_mut() {
            params.play_mode = mooloop_core::PlayMode::Slice;
            params.stretch_bars = 2.0;
        }
        session.divide_slices(4, false).expect("audio");
        session.pattern_lengths[0] = 16;
        session.channels[0].create_note(0, 0, 24, 60).expect("room");

        let (commands, write) = session.write_slice_pattern(None, false).expect("slices");
        assert_eq!(write.notes, 4);
        assert_eq!(write.replaced, 0);
        assert_eq!(write.grew_to, Some(32), "two bars is 32 steps");
        assert_eq!(session.pattern_lengths[0], 32);
        assert_eq!(session.channels[0].notes[0].len(), 5, "added beside the old note");
        assert!(matches!(commands[0], EngineCommand::SetPatternLength { length_steps: 32, .. }));

        let (commands, write) = session.write_slice_pattern(None, true).expect("slices");
        assert_eq!(write.replaced, 5);
        assert_eq!(write.notes, 4);
        assert_eq!(session.channels[0].notes[0].len(), 4);
        let removes = commands
            .iter()
            .filter(|command| matches!(command, EngineCommand::RemoveNote { .. }))
            .count();
        assert_eq!(removes, 5);
        let ids: std::collections::BTreeSet<NoteId> =
            session.channels[0].notes[0].iter().map(|note| note.id).collect();
        assert_eq!(ids.len(), 4, "note ids stay unique");
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

#[cfg(test)]
mod stretch_pool_tests {
    use super::*;
    use mooloop_core::{
        ControlBinding, ControlSource, ControlTarget, EffectTarget, MidiChannelFilter, MidiKind,
        MidiMessage, MidiPortFilter, MidiPortId, MidiPortInfo, ParamAddr, ParamOwner,
    };

    /// What the reconciler sent this tick, as pool sizes (`None` for a
    /// removal) by channel.
    fn tick(session: &mut Session) -> Vec<(u8, Option<usize>)> {
        let mut sent = Vec::new();
        session.sync_sampler_stretch(48_000, |command| {
            if let StructuralCommand::SetSamplerStretch { channel, pool } = command {
                sent.push((channel, pool.map(|pool| pool.len())));
            }
            true
        });
        sent
    }

    fn stretching(session: &mut Session, voices: u8) {
        let params = session.channels[0]
            .sampler_params_mut()
            .expect("the default channel is a sampler");
        params.stretch_enabled = true;
        params.polyphony = voices;
    }

    /// The pool follows Voices (MOO-7): twice the count, resent only when it
    /// changes, and taken back when stretching stops. A tick with nothing
    /// changed sends nothing.
    #[test]
    fn the_pool_follows_voices_and_only_when_it_changes() {
        let mut session = Session::default();
        assert!(tick(&mut session).is_empty(), "a sampler that does not stretch got a pool");
        stretching(&mut session, 1);
        assert_eq!(tick(&mut session), vec![(0, Some(2))]);
        assert!(tick(&mut session).is_empty(), "an unchanged tick sent a pool");
        stretching(&mut session, 4);
        assert_eq!(tick(&mut session), vec![(0, Some(8))]);
        session.channels[0]
            .sampler_params_mut()
            .expect("a sampler")
            .stretch_enabled = false;
        assert_eq!(tick(&mut session), vec![(0, None)]);
    }

    /// A refused send is not recorded as sent, so the next tick tries again.
    #[test]
    fn a_refused_send_is_retried() {
        let mut session = Session::default();
        stretching(&mut session, 2);
        session.sync_sampler_stretch(48_000, |_| false);
        assert_eq!(tick(&mut session), vec![(0, Some(4))]);
    }

    /// An undo or a load replaces the engine's sources wholesale, so the
    /// mirror is forgotten and the next tick re-sends what the song wants.
    #[test]
    fn replacing_the_project_resends_the_pool() {
        let mut session = Session::default();
        stretching(&mut session, 3);
        assert_eq!(tick(&mut session), vec![(0, Some(6))]);
        let project = session.project_snapshot(120, 0);
        session.replace_project(&project, &[]);
        assert_eq!(tick(&mut session), vec![(0, Some(6))]);
    }

    /// A MIDI-learned binding on Voices moves it on the control thread,
    /// through the session, and the pool follows it like any other edit.
    #[test]
    fn a_midi_learned_voices_binding_resizes_the_pool() {
        let mut session = Session::default();
        stretching(&mut session, 1);
        assert_eq!(tick(&mut session), vec![(0, Some(2))]);
        let voices = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::source(mooloop_core::DeviceKind::Sampler),
            param: mooloop_core::SAMPLER_PARAM_POLYPHONY,
        };
        session.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Param(session.param_key(voices).expect("channel 0 has an identity")),
        ));
        let ports = vec![MidiPortInfo {
            id: MidiPortId(0),
            name: "keys".to_owned(),
        }];
        session.resolve_control_map(&ports);
        let cc = |value| MidiMessage {
            offset: 0,
            port: MidiPortId(0),
            channel: 0,
            kind: MidiKind::ControlChange {
                controller: 7,
                value,
            },
        };
        // Pickup: the first message catches the control where Voices is,
        // and the second takes it to the top.
        session.apply_control_input(&cc(0), &ports, false);
        session.apply_control_input(&cc(127), &ports, false);
        let voices_now = session.channels[0].sampler_params().polyphony;
        assert!(voices_now > 1, "the binding did not move Voices");
        assert_eq!(
            tick(&mut session),
            vec![(0, Some(stretch_pool_voices(&session.channels[0].sampler_params(), &[])))]
        );
    }

    /// A lane on Voices moves it on the audio thread, where no pool can be
    /// built, so the channel is given one for every voice at once.
    #[test]
    fn a_lane_on_voices_gets_every_voice() {
        let mut session = Session::default();
        stretching(&mut session, 1);
        session.channels[0].automation[0].push(mooloop_core::AutomationLane::new(ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::source(mooloop_core::DeviceKind::Sampler),
            param: mooloop_core::SAMPLER_PARAM_POLYPHONY,
        }));
        assert_eq!(tick(&mut session), vec![(0, Some(16))]);
    }
}

#[cfg(test)]
mod zone_tests {
    use super::*;
    use mooloop_core::{
        ChannelSource, KeyRange, NoteEvent, Project, ProjectChannel, SampleReference, SampleZone,
    };
    use std::sync::Arc;

    const RATE: u32 = 48_000;

    /// A sine at `hz`, `seconds` long, at `level`.
    fn tone(hz: f64, level: f32, seconds: f64) -> Arc<SampleData> {
        let len = (seconds * f64::from(RATE)) as usize;
        Arc::new(SampleData {
            frames: (0..len)
                .map(|n| {
                    let value =
                        level * (2.0 * std::f64::consts::PI * hz * n as f64 / f64::from(RATE)).sin() as f32;
                    [value, value]
                })
                .collect(),
            sample_rate: RATE,
            root_note: 60,
        })
    }

    fn file(name: &str) -> SampleReference {
        SampleReference::File {
            path: PathBuf::from(format!("/nonexistent/moo-14/{name}.wav")),
            embedded: false,
        }
    }

    /// One sampler: its own sample up to B3 (root C4), and one zone from C4
    /// up (root C5), with a note in each, a bar apart.
    fn split_song() -> Project {
        let mut channel = ProjectChannel::sampler(0, 1);
        let ChannelSource::Sampler(state) = &mut channel.setup.source else {
            unreachable!()
        };
        state.sample = file("base");
        state.params.root_note = 60;
        state.params.attack = 0.0;
        state.params.decay = 8.0;
        state.params.sustain = 1.0;
        state.keys = KeyRange::new(0, 59);
        state.zones = vec![SampleZone {
            keys: KeyRange::new(60, 127),
            root_note: 72,
            sample: file("zone"),
            ..SampleZone::default()
        }];
        let step = mooloop_core::TICKS_PER_STEP;
        channel.notes[0] = vec![
            NoteEvent::new(1, 0, 4 * step, 48, 110),
            NoteEvent::new(2, 8 * step, 4 * step, 84, 110),
        ];
        Project {
            bpm: 120,
            channels: vec![channel],
            pattern_lengths: vec![16],
            ..Project::default()
        }
    }

    /// A session holding `split_song` with both buffers decoded, as a load
    /// leaves it.
    fn split_session(base: &Arc<SampleData>, zone: &Arc<SampleData>) -> Session {
        let mut session = Session::default();
        session.admit_zone_audio(
            vec![(PathBuf::from("/nonexistent/moo-14/zone.wav"), zone.clone())],
            true,
        );
        session.replace_project(&split_song(), &[Some(base.clone())]);
        session
    }

    /// The zone's buffer arrives on the channel from the table, as the same
    /// `Arc`, and the published snapshot carries it with its keys and root.
    #[test]
    fn an_installed_zone_finds_its_audio_and_publishes_it() {
        let (base, zone) = (tone(261.63, 0.5, 1.0), tone(523.25, 0.25, 1.0));
        let session = split_session(&base, &zone);
        let channel = &session.channels[0];
        assert_eq!(channel.keys, KeyRange::new(0, 59));
        assert!(Arc::ptr_eq(channel.zones[0].sample.as_ref().unwrap(), &zone));
        assert!(session.missing_zones().is_empty());
        let audio = channel_audio(channel);
        assert_eq!(audio.keys, KeyRange::new(0, 59));
        assert_eq!(audio.zones.len(), 1);
        assert_eq!(audio.zones[0].root_note, 72);
        assert!(Arc::ptr_eq(audio.zones[0].sample.as_ref().unwrap(), &zone));
        // And the document it saves is the one it installed.
        let song = split_song();
        let saved = session.project_snapshot(120, 0);
        assert_eq!(
            saved.channels[0].setup.sampler_state().unwrap().zones,
            song.channels[0].setup.sampler_state().unwrap().zones
        );
    }

    /// A zone whose audio is not in hand installs silent and is reported,
    /// never installed as if it had audio.
    #[test]
    fn a_zone_with_no_audio_in_hand_is_reported_missing() {
        let mut session = Session::default();
        session.replace_project(&split_song(), &[Some(tone(261.63, 0.5, 1.0))]);
        assert!(session.channels[0].zones[0].sample.is_none());
        assert_eq!(
            session.missing_zones(),
            vec![(0, PathBuf::from("/nonexistent/moo-14/zone.wav"))]
        );
        assert!(channel_audio(&session.channels[0]).zones[0].sample.is_none());
    }

    /// **Undo brings a removed zone back with its audio.** Removing a zone
    /// and undoing it restore documents, not audio; the table is what finds
    /// the buffer again, without a decode.
    #[test]
    fn undoing_a_zone_removal_brings_its_audio_back() {
        let (base, zone) = (tone(261.63, 0.5, 1.0), tone(523.25, 0.25, 1.0));
        let mut session = split_session(&base, &zone);
        let before = session.project_snapshot(120, 0);
        let mut removed = before.clone();
        removed.channels[0].setup.sampler_state_mut().unwrap().zones.clear();
        session.replace_project(&removed, &[Some(base.clone())]);
        assert!(session.channels[0].zones.is_empty());
        // Undo.
        session.replace_project(&before, &[Some(base.clone())]);
        assert!(Arc::ptr_eq(session.channels[0].zones[0].sample.as_ref().unwrap(), &zone));
        // And it plays: the note above the split, in the bar's second half,
        // sounds through the real executor.
        let project = session.project_snapshot(120, 0);
        let played = mooloop_engine::live_check::play_audio_through_executor(
            &project,
            vec![channel_audio(&session.channels[0])],
            RATE,
            96_000,
            256,
        );
        let upper = played[96_000..120_000].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(upper > 0.05, "the restored zone is silent: peak {upper}");
    }

    /// A new zone takes the keys above everything, or splits the top range
    /// when the keyboard is covered: a first zone on a full-range sampler
    /// takes the upper half and the sample keeps the lower.
    #[test]
    fn a_new_zone_takes_free_keys_or_splits_the_top_range() {
        let (base, zone) = (tone(261.63, 0.5, 0.1), tone(523.25, 0.25, 0.1));
        let mut session = Session::default();
        session.replace_project(&Project::default(), &[Some(base)]);
        assert!(session.add_zone(0, PathBuf::from("/z1.wav"), zone.clone()));
        assert_eq!(session.channels[0].keys, KeyRange::new(0, 63));
        assert_eq!(session.channels[0].zones[0].zone.keys, KeyRange::new(64, 127));
        assert_eq!(session.channels[0].zones[0].zone.root_note, 64);
        assert!(session.set_zone_keys(0, 0, 90, 64));
        assert_eq!(session.channels[0].zones[0].zone.keys, KeyRange::new(64, 90));
        assert!(session.add_zone(0, PathBuf::from("/z2.wav"), zone.clone()));
        assert_eq!(session.channels[0].zones[1].zone.keys, KeyRange::new(91, 127));
        assert!(session.zone_audio.get(Path::new("/z2.wav")).is_some());
        assert!(session.remove_zone(0, 0));
        assert_eq!(session.channels[0].zones.len(), 1);
        assert!(session.zone_audio.get(Path::new("/z1.wav")).is_some(), "an undo needs it");
    }

    /// Opening a song replaces the table; a merged kit or preset adds to it.
    #[test]
    fn opening_a_song_replaces_the_zone_table_and_a_merge_adds() {
        let mut session = Session::default();
        let one = (PathBuf::from("/a.wav"), tone(100.0, 0.1, 0.01));
        let two = (PathBuf::from("/b.wav"), tone(200.0, 0.1, 0.01));
        session.admit_zone_audio(vec![one.clone()], true);
        session.admit_zone_audio(vec![two.clone()], false);
        assert_eq!(session.zone_audio.len(), 2);
        session.admit_zone_audio(vec![two], true);
        assert_eq!(session.zone_audio.len(), 1);
        assert!(session.zone_audio.get(&one.0).is_none());
    }

    /// **An export of a two-zone song is what playback plays**, sample for
    /// sample: the live install's snapshot through the real executor against
    /// `document::run_export` from the same session.
    #[test]
    fn a_two_zone_export_matches_playback() {
        use mooloop_engine::{
            ExportFormat, ExportProgress, ExportSpec, RenderJob, RenderScope, WavEncoding,
        };
        let (base, zone) = (tone(261.63, 0.5, 1.0), tone(523.25, 0.25, 1.0));
        let session = split_session(&base, &zone);
        let project = session.project_snapshot(120, 0);
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("zones.wav");
        let request = crate::document::ExportRequest {
            project: project.clone(),
            samples: session.sample_snapshots(),
            zones: session.zone_sample_snapshots(),
            job: RenderJob::single(&ExportSpec {
                path: path.clone(),
                scope: RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format: ExportFormat::Wav(WavEncoding::Float32),
            }),
        };
        let result = crate::document::run_export(
            request,
            RATE,
            &ExportProgress::new(),
            std::collections::BTreeMap::new(),
        );
        assert!(
            matches!(result, crate::document::DocumentResult::Exported { .. }),
            "the export failed"
        );
        let exported: Vec<f32> = hound::WavReader::open(&path)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect();

        let audio: Vec<_> = project
            .channels
            .iter()
            .enumerate()
            .map(|(index, channel)| {
                session.sampler_install_audio(
                    session.sample_snapshots()[index].clone(),
                    channel.setup.sampler_state().unwrap(),
                )
            })
            .collect();
        let frames = exported.len() / 2;
        let played = mooloop_engine::live_check::play_audio_through_executor(
            &project, audio, RATE, frames, 256,
        );
        assert_eq!(played.len(), exported.len());
        let worst = played
            .iter()
            .zip(&exported)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "export and playback differ by {worst}");
        // Both zones sound: the base's note in the first half-bar, the
        // zone's in the second.
        let half = frames / 2;
        let loudest = |from: usize, to: usize| {
            exported[from * 2..to * 2].iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        assert!(loudest(0, half / 2) > 0.1, "the base zone is silent");
        assert!(loudest(half, half + half / 2) > 0.05, "the upper zone is silent");
    }

    /// A save's zone paths come back to the session, and the table learns
    /// them for the same buffer, so the next save finds each file where the
    /// last put it and an undo still finds the audio (MOO-14).
    #[test]
    fn a_save_writes_zone_paths_back_and_the_table_follows() {
        let zone = tone(523.25, 0.25, 0.1);
        let mut session = split_session(&tone(261.63, 0.5, 0.1), &zone);
        let saved = PathBuf::from("/song-assets/samples/00-zone.wav");
        let session_ref = &mut session;
        crate::channel::apply_zone_references(
            &mut session_ref.channels,
            &mut session_ref.zone_audio,
            vec![vec![SampleReference::File {
                path: saved.clone(),
                embedded: true,
            }]],
        );
        assert_eq!(session.channels[0].zones[0].path(), Some(saved.as_path()));
        assert!(Arc::ptr_eq(session.zone_audio.get(&saved).unwrap(), &zone));
    }
}
