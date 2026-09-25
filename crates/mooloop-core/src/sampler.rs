//! Sampler device parameters. Pure data so the bridge can carry them.

use crate::automation::AutomationLane;
use crate::modulation::ParamOwner;
use crate::time::BEATS_PER_BAR;

pub const MAX_SAMPLER_VOICES: u8 = 16;
pub const MAX_CHOKE_GROUP: u8 = 16;

/// A fresh sampler's output trim: the generator output reference, as gain.
///
/// Loading or replacing a sample never touches this. The other generators
/// calibrate their own default patch to peak at
/// `gain::GENERATOR_OUTPUT_REFERENCE_DBFS`; the sampler cannot, because the
/// audio is whatever the user loaded. Spending that much headroom is the
/// closest honest equivalent -- a normalized, full-scale file then peaks
/// where a default DrumSynth hit peaks, at any pan position. It is
/// predictable headroom, not normalization: nothing measures, matches, or
/// rewrites the audio.
pub fn default_output_gain() -> f32 {
    crate::gain::reference_level_gain()
}

/// How the sampler treats the loop region once the play head reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    /// No looping. Play from `start` to `loop_end` (or sample end), then stop.
    #[default]
    Off,
    /// Loop forward: wrap from `loop_end` back to `loop_start`.
    Forward,
    /// Loop ping-pong: reverse direction at both loop points.
    Pingpong,
}

impl LoopMode {
    pub fn all() -> [LoopMode; 3] {
        [LoopMode::Off, LoopMode::Forward, LoopMode::Pingpong]
    }

    pub fn label(self) -> &'static str {
        match self {
            LoopMode::Off => "Off",
            LoopMode::Forward => "Fwd",
            LoopMode::Pingpong => "Pong",
        }
    }
}

/// What a loop's bounds snap to (MOO-47): nothing, the slice markers, or a
/// division of the bar.
///
/// Applied where the bounds are resolved, so a lane or a modulator moving
/// Loop start steps through the grid audibly instead of sliding through
/// every frame. The bar is the sample's musical length, `stretch_bars` bars
/// over the playback region, which is the same length fit-to-tempo lays
/// against the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopQuantize {
    /// Any frame. What every song saved before this existed does.
    #[default]
    Off,
    /// The nearest slice marker, or the playback region's own ends.
    Slices,
    Bar,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
}

impl LoopQuantize {
    /// In the order the descriptor's positions mean. A saved lane stores the
    /// position, so this order is frozen from the day it shipped (Control,
    /// 2026-09-24): a new grid is appended, and none is ever reordered.
    pub const ALL: [Self; 8] = [
        Self::Off,
        Self::Slices,
        Self::Bar,
        Self::Half,
        Self::Quarter,
        Self::Eighth,
        Self::Sixteenth,
        Self::ThirtySecond,
    ];

    /// Out-of-range input clamps to the nearest end, the `ALL`-table
    /// convention.
    pub fn from_index(index: i32) -> Self {
        Self::ALL[index.clamp(0, Self::ALL.len() as i32 - 1) as usize]
    }

    pub fn to_index(self) -> i32 {
        Self::ALL.iter().position(|q| *q == self).unwrap_or(0) as i32
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Slices => "Slices",
            Self::Bar => "1 bar",
            Self::Half => "1/2",
            Self::Quarter => "1/4",
            Self::Eighth => "1/8",
            Self::Sixteenth => "1/16",
            Self::ThirtySecond => "1/32",
        }
    }

    /// Grid steps per bar, or `None` for Off and Slices.
    pub fn divisions_per_bar(self) -> Option<u32> {
        match self {
            Self::Off | Self::Slices => None,
            Self::Bar => Some(1),
            Self::Half => Some(2),
            Self::Quarter => Some(4),
            Self::Eighth => Some(8),
            Self::Sixteenth => Some(16),
            Self::ThirtySecond => Some(32),
        }
    }
}

/// How a note picks material out of the sample.
///
/// The two are genuinely different instruments, not a quality setting. In
/// `Pitched` the note transposes the whole region, so pitch and duration move
/// together. In `Slice` the note *chooses* a slice and plays it at its
/// original pitch, which is what ReCycle/REX established for fitting a break
/// to a tempo without resynthesising anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMode {
    /// The note transposes the playback region.
    #[default]
    Pitched,
    /// The note selects a slice by ordinal position from `slice_base_note`.
    Slice,
}

impl PlayMode {
    pub fn all() -> [PlayMode; 2] {
        [PlayMode::Pitched, PlayMode::Slice]
    }

    pub fn label(self) -> &'static str {
        match self {
            PlayMode::Pitched => "Pitched",
            PlayMode::Slice => "Slice",
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Slice,
            _ => Self::Pitched,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Pitched => 0,
            Self::Slice => 1,
        }
    }
}

/// The most slices one sample may carry. 128 is the MIDI note range, which is
/// also the most a chromatic keyboard could ever address at once.
pub const MAX_SLICES: usize = 128;

/// The lowest note that plays a slice.
///
/// MIDI 36, which is where Ableton's Simpler and the MPC family put the first
/// slice. Named C2 rather than C1 everywhere in this app, because the UI
/// names middle C as C4 -- the number is the convention, the spelling is
/// ours.
pub const DEFAULT_SLICE_BASE_NOTE: u8 = 36;

/// One slice boundary: a source frame with a stable identity.
///
/// The id is what makes a persisted reference to "this slice" survive its
/// neighbours being inserted or deleted. Note-to-slice mapping deliberately
/// does *not* use it -- a chromatic keyboard means ordinal position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SliceMarker {
    pub id: u64,
    pub frame: u32,
    /// Placed or moved by hand, as opposed to laid down by Divide or by
    /// transient detection (MOO-44). Detection's Replace keeps these and
    /// replaces the rest, so accepting a detection never loses manual work.
    ///
    /// Absent in a song saved before detection existed, and read there as
    /// placed by hand: nothing was detected then, and the careful reading of
    /// an old map is that every marker in it is somebody's.
    #[serde(default = "placed_by_hand")]
    pub hand: bool,
}

fn placed_by_hand() -> bool {
    true
}

/// The slice boundaries of one sample, sorted by frame and unique.
///
/// The invariant lives here rather than in the callers: markers are always
/// sorted ascending, no two share a frame, and there are never more than
/// [`MAX_SLICES`]. Every mutation restores it, so no caller can publish a map
/// the voice would have to defend itself against.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "SliceMapWire")]
pub struct SliceMap {
    markers: Vec<SliceMarker>,
    #[serde(default)]
    next_id: u64,
}

impl SliceMap {
    /// The heap this map owns, for the undo history's memory budget.
    /// `markers` is private, so this lives beside it.
    pub fn heap_bytes(&self) -> usize {
        self.markers.capacity() * std::mem::size_of::<SliceMarker>()
    }
}

/// What a `SliceMap` looks like on disk, before the invariant is imposed.
///
/// Deserialization is the one path into the type that does not go through a
/// mutator, so it is the one path that could otherwise publish an unsorted or
/// duplicated map. Normalizing here keeps "sorted, unique, capped" a property
/// of the type rather than a property of well-formed files, and matches how
/// the rest of the format is read: repair, do not refuse.
#[derive(serde::Deserialize)]
struct SliceMapWire {
    #[serde(default)]
    markers: Vec<SliceMarker>,
    #[serde(default)]
    next_id: u64,
}

impl From<SliceMapWire> for SliceMap {
    fn from(wire: SliceMapWire) -> Self {
        let mut map = Self {
            markers: Vec::new(),
            next_id: wire.next_id,
        };
        map.rebuild(wire.markers);
        map
    }
}

impl SliceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn markers(&self) -> &[SliceMarker] {
        &self.markers
    }

    pub fn len(&self) -> usize {
        self.markers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.markers.is_empty()
    }

    /// Ordinal position of the slice with this id, or `None` if it is gone.
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.markers.iter().position(|marker| marker.id == id)
    }

    pub fn get(&self, index: usize) -> Option<SliceMarker> {
        self.markers.get(index).copied()
    }

    fn mint(&mut self) -> u64 {
        // Ids start at 1 so a zero read out of an uninitialized field is
        // never mistaken for a live slice.
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    /// Add a boundary at `frame`, returning its id. A frame that already has
    /// a marker, or a map already at [`MAX_SLICES`], is refused rather than
    /// silently collapsing two slices into one.
    pub fn add(&mut self, frame: u32) -> Option<u64> {
        if self.markers.len() >= MAX_SLICES {
            return None;
        }
        if self.markers.iter().any(|marker| marker.frame == frame) {
            return None;
        }
        let id = self.mint();
        let at = self
            .markers
            .partition_point(|marker| marker.frame < frame);
        self.markers.insert(
            at,
            SliceMarker {
                id,
                frame,
                hand: true,
            },
        );
        Some(id)
    }

    /// Remove one boundary by id. Its slice merges into the one before it,
    /// which is what deleting a boundary means.
    pub fn remove(&mut self, id: u64) -> bool {
        match self.index_of(id) {
            Some(index) => {
                self.markers.remove(index);
                true
            }
            None => false,
        }
    }

    /// Move one boundary to a new frame, re-sorting so ordinal position
    /// follows the frame rather than the insertion order. A move onto an
    /// occupied frame is refused; the caller's drag simply does not land
    /// there.
    pub fn move_to(&mut self, id: u64, frame: u32) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        if self
            .markers
            .iter()
            .any(|marker| marker.id != id && marker.frame == frame)
        {
            return false;
        }
        self.markers[index].frame = frame;
        // A marker the hand moved is the hand's, wherever it came from.
        self.markers[index].hand = true;
        self.markers.sort_by_key(|marker| marker.frame);
        true
    }

    /// Replace the map with `count` equal slices spanning `[start, end)`.
    ///
    /// The first marker sits on `start`, so slice 0 is the region's own
    /// beginning: a break divided into 8 has its downbeat on the first note,
    /// not a silent lead-in before it.
    pub fn divide_evenly(&mut self, count: usize, start: u32, end: u32) {
        self.markers.clear();
        let count = count.min(MAX_SLICES);
        if count == 0 || end <= start {
            return;
        }
        let span = f64::from(end - start);
        for slice in 0..count {
            let frame = start + (span * slice as f64 / count as f64).round() as u32;
            // `round` can land two low slice counts on the same frame in a
            // very short region; `add` refuses the duplicate rather than
            // producing a zero-length slice.
            if let Some(id) = self.add(frame) {
                // Laid down by arithmetic, not by hand: detection may
                // replace it (MOO-44).
                if let Some(index) = self.index_of(id) {
                    self.markers[index].hand = false;
                }
            }
        }
    }

    /// Keep the markers placed by hand, drop the rest, and add `frames`
    /// (MOO-44's Replace). A detected frame within `min_gap` of a kept
    /// marker is left out, since the hand already put one there. Returns how
    /// many were added; the map's cap is respected, earliest first.
    pub fn replace_detected(&mut self, frames: &[u32], min_gap: u32) -> usize {
        self.markers.retain(|marker| marker.hand);
        self.merge_detected(frames, min_gap)
    }

    /// Add `frames` beside every marker already here (MOO-44's Merge),
    /// leaving out any within `min_gap` of one. Returns how many were added.
    pub fn merge_detected(&mut self, frames: &[u32], min_gap: u32) -> usize {
        let mut added = 0;
        for &frame in frames {
            if self.markers.len() >= MAX_SLICES {
                break;
            }
            let near = self
                .markers
                .iter()
                .any(|marker| marker.frame.abs_diff(frame) < min_gap.max(1));
            if near {
                continue;
            }
            let id = self.mint();
            let at = self.markers.partition_point(|marker| marker.frame < frame);
            self.markers.insert(
                at,
                SliceMarker {
                    id,
                    frame,
                    hand: false,
                },
            );
            added += 1;
        }
        added
    }


    pub fn clear(&mut self) {
        self.markers.clear();
    }

    /// Replace every marker, keeping the ids the caller supplies.
    ///
    /// The one operation that must *not* mint fresh ids. Committing a stretch
    /// moves every marker to a new frame, and rebuilding the map through
    /// `add` would renumber the whole thing -- which is exactly the silent
    /// retargeting the ids exist to prevent. Enforces the same invariant as
    /// every other mutation: sorted, unique by frame, capped.
    pub fn rebuild(&mut self, markers: impl IntoIterator<Item = SliceMarker>) {
        self.markers.clear();
        for marker in markers {
            if self.markers.len() >= MAX_SLICES {
                break;
            }
            if self.markers.iter().any(|held| held.frame == marker.frame) {
                continue;
            }
            let at = self
                .markers
                .partition_point(|held| held.frame < marker.frame);
            self.markers.insert(at, marker);
            // Keep minting clear of anything just adopted, or the next `add`
            // would hand out an id that is already in use.
            self.next_id = self.next_id.max(marker.id);
        }
    }

    /// The source-frame span of slice `index`: its marker to the next one, or
    /// to `region_end` for the last slice. `None` when the index is past the
    /// end of the map.
    pub fn span(&self, index: usize, region_end: f64) -> Option<(f64, f64)> {
        let marker = self.markers.get(index)?;
        let start = f64::from(marker.frame);
        let end = match self.markers.get(index + 1) {
            Some(next) => f64::from(next.frame),
            None => region_end,
        };
        Some((start, end))
    }
}

/// How the time stretcher sizes its window and whether it looks for a splice
/// point.
///
/// `Grain` is not a lower quality than the other two. The similarity search
/// places every join where the waveform continues coherently; declining to
/// search leaves a phase discontinuity once per hop, which is the rattling,
/// woodblock character of a break stretched far past musical range. That is a
/// sound people reach for, so it is a mode rather than a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StretchMode {
    /// ~21 ms window, search on. Transparent, and the only mode that
    /// preserves a low fundamental.
    #[default]
    Music,
    /// ~11 ms window, search on. Sharper transients; destroys bass, so it is
    /// percussion-only.
    Drums,
    /// Free window, no search. The artifact mode.
    Grain,
}

impl StretchMode {
    pub fn all() -> [StretchMode; 3] {
        [StretchMode::Music, StretchMode::Drums, StretchMode::Grain]
    }

    pub fn label(self) -> &'static str {
        match self {
            StretchMode::Music => "Music",
            StretchMode::Drums => "Drums",
            StretchMode::Grain => "Grain",
        }
    }
}

/// The longest loop crossfade, in milliseconds of the sample's own time
/// (MOO-43). A drum loop wants a few; a sustained pad's seam can want tens.
pub const MAX_LOOP_CROSSFADE_MS: f32 = 100.0;

/// Bar-count bounds for tempo-synced stretching.
pub const MIN_STRETCH_BARS: f32 = 0.0625;
pub const MAX_STRETCH_BARS: f32 = 64.0;

/// Snap a length in bars to the nearer power of two.
///
/// The rule, in Adam's words: find the power-of-two bracket the length falls
/// in and take whichever end it is closer to -- one bar or two, two or four,
/// four or eight. The split is the *arithmetic* midpoint, so 1.5 bars rounds
/// up to 2 and 2.9 rounds down to 2, and it generalizes below a bar so a
/// half-bar chop lands on 1/2 rather than being dragged up to 1.
///
/// This is what a freshly enabled fit-to-tempo guesses. A loop is nearly
/// always some power of two of bars, and the length it was recorded at is
/// nearly always slightly off that, so guessing well is the difference
/// between the feature working on the first click and needing a knob turn
/// every time.
pub fn snap_bars_to_power_of_two(bars: f32) -> f32 {
    if !bars.is_finite() || bars <= 0.0 {
        return 1.0;
    }
    let bars = bars.clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS);
    let low = bars.log2().floor().exp2();
    let high = low * 2.0;
    let snapped = if bars >= (low + high) * 0.5 { high } else { low };
    snapped.clamp(MIN_STRETCH_BARS, MAX_STRETCH_BARS)
}

/// Frames in one bar at a tempo.
///
/// [`BEATS_PER_BAR`] beats to the bar, and the buffer device calls this
/// rather than spelling its own: the two being on one grid is now true by
/// construction, where it used to be asserted by this comment and contradicted
/// by `buffer_device.rs`. `Project::beats_per_bar` is project metadata the
/// audio thread is not given, which is why the constant and not the field.
pub fn frames_per_bar(sample_rate: u32, bpm: f64) -> f64 {
    let seconds_per_bar = 60.0 * f64::from(BEATS_PER_BAR) / bpm.max(1.0);
    f64::from(sample_rate) * seconds_per_bar
}

/// Stretch ratio bounds. Output frames per input frame, so above 1.0 is
/// slower. The ceiling is far past the range that stays clean, on purpose --
/// extreme slow-down is a destination, and the cost does not grow with the
/// ratio.
pub const MIN_STRETCH_RATIO: f32 = 0.25;
pub const MAX_STRETCH_RATIO: f32 = 16.0;

/// Grain window bounds, in frames. The repetition sits at
/// `sample_rate / (grain / 2)`, so this is a pitch control: at 48 kHz the
/// range buzzes from about 23 Hz to 1.5 kHz.
pub const MIN_STRETCH_GRAIN: u16 = 64;
pub const MAX_STRETCH_GRAIN: u16 = 4096;

/// What a committed stretch baked, and what the editor looked like before it.
///
/// A commit renders the stretched region and makes the *rendered* buffer what
/// is published, displayed, and edited, so the waveform, the markers, and the
/// start/end fractions all live in one coordinate system rather than two. The
/// source stays authoritative on the UI thread, which is what makes revert
/// and re-commit exact.
///
/// Re-committing at a new ratio always renders from the source using
/// `source_markers`, so repeated tempo changes cannot accumulate drift, and
/// re-rendering on load from this spec is why the audio is never persisted.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SampleCommit {
    // What was baked.
    pub mode: StretchMode,
    /// The *resolved* ratio, so a bar-synced commit reproduces at the tempo
    /// it was baked at rather than at whatever the project is set to now.
    pub ratio: f32,
    pub grain: u16,
    // Pre-commit editor state, so revert and re-commit are exact rather than
    // round-tripped through the trace twice.
    /// The markers as they stood before the commit, ids included.
    ///
    /// Ids and not just frames, so a revert hands back the same slices rather
    /// than fresh ones wearing their frames. Nothing references a slice by id
    /// across a save yet -- per-slice parameters are deferred by #15 -- but
    /// the type's whole reason to carry ids is that this stays true before
    /// something does.
    pub source_markers: Vec<SliceMarker>,
    pub source_start: f32,
    pub source_end: f32,
    pub source_loop_start: f32,
    pub source_loop_end: f32,
}

/// How note-off events affect sample playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMode {
    /// Play the full region. For a looped voice, note-off exits the loop and
    /// lets the remaining sample tail play once.
    #[default]
    OneShot,
    /// Note-off enters the amplitude envelope's release stage.
    Gate,
}

/// How repeated notes of the same pitch use the voice pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetriggerMode {
    /// Replace the oldest active voice on the same pitch.
    #[default]
    Restart,
    /// Allow repeated pitches to overlap up to the polyphony limit.
    Layer,
}

impl LoopMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Forward,
            2 => Self::Pingpong,
            _ => Self::Off,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::Forward => 1,
            Self::Pingpong => 2,
        }
    }
}

impl VoiceMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Gate,
            _ => Self::OneShot,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::OneShot => 0,
            Self::Gate => 1,
        }
    }
}

impl RetriggerMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Layer,
            _ => Self::Restart,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Restart => 0,
            Self::Layer => 1,
        }
    }
}

/// The four stage values an ADSR envelope runs on. Times in seconds, sustain
/// as a level in `[0, 1]`.
///
/// Named as one value because an envelope's shape is a thing a patch has,
/// not four unrelated numbers: the sampler now carries two of them, and
/// copying one into the other is what an old project's migration is. Kept
/// here rather than promoted to a shared type until a second device wants it.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EnvTimes {
    /// Attack time in seconds.
    pub attack: f32,
    /// Decay time in seconds.
    pub decay: f32,
    /// Sustain level in `[0, 1]`.
    pub sustain: f32,
    /// Release time in seconds.
    pub release: f32,
}

/// All sampler parameters, in the units the DSP and UI share. All points are
/// fractions of the sample length in `[0, 1]`; times are seconds.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SamplerParams {
    pub voice_mode: VoiceMode,
    /// Active voice limit in `1..=16`.
    pub polyphony: u8,
    pub retrigger_mode: RetriggerMode,
    /// `0` disables choking; matching non-zero groups choke each other.
    pub choke_group: u8,
    /// Play start point as a fraction of the sample length.
    pub start: f32,
    /// Play end point as a fraction of the sample length.
    pub end: f32,
    /// Play the selected region backwards.
    pub reverse: bool,
    /// Root MIDI note used for keyboard tracking.
    pub root_note: u8,
    /// Coarse tuning offset in semitones.
    pub tune_semitones: f32,
    /// Fine tuning offset in cents.
    pub tune_cents: f32,
    /// Whether a change to `tune_semitones`/`tune_cents` (by hand or by
    /// modulation) is heard on every currently sounding voice, or only on the
    /// next one triggered.
    ///
    /// On is the musically ordinary choice -- it is what makes a tune knob
    /// behave like a tune knob while a note is held, and what a pitch
    /// modulation route needs to be audible at all rather than silently doing
    /// nothing until the next note-on. Off reproduces the sampler's original
    /// behavior, for anyone who was relying on a held note's pitch staying
    /// put under an unrelated tune edit.
    #[serde(default = "default_retune_live")]
    pub retune_live: bool,
    /// Loop start point as a fraction.
    pub loop_start: f32,
    /// Loop end point as a fraction.
    pub loop_end: f32,
    pub loop_mode: LoopMode,
    /// How long a forward loop's seam is crossfaded, in milliseconds of the
    /// sample's own time; 0 is a hard seam (MOO-43).
    ///
    /// Defaulted to 0, so a song saved before the fade existed loads with a
    /// hard seam and renders exactly as it did.
    #[serde(default)]
    pub loop_crossfade_ms: f32,
    /// What the loop's bounds snap to (MOO-47). Defaulted to Off, so a song
    /// saved before the grid existed loops where it always did.
    #[serde(default)]
    pub loop_quantize: LoopQuantize,
    /// Attack time (seconds).
    pub attack: f32,
    /// Decay time (seconds).
    pub decay: f32,
    /// Sustain level in `[0, 1]`.
    pub sustain: f32,
    /// Release time (seconds).
    pub release: f32,
    /// Low-pass cutoff on a perceptual `[0, 1]` scale. `1` bypasses it.
    pub filter_cutoff: f32,
    /// Low-pass resonance in `[0, 1]`.
    pub filter_resonance: f32,
    /// Bipolar filter envelope depth in `[-1, 1]` (up to six octaves).
    pub filter_env_amount: f32,
    /// Soft saturation drive in `[0, 1]`. `0` bypasses it.
    pub drive: f32,
    /// Bit-depth reduction amount in `[0, 1]`. `0` bypasses it.
    pub bit_reduction: f32,
    /// Sample-rate reduction amount in `[0, 1]`. `0` bypasses it.
    pub rate_reduction: f32,
    /// Patch-level output gain, linear, in `[0, MAX_LINEAR_GAIN]` (+12 dB).
    /// This is the sampler's own trim ahead of the channel's inserts, not the
    /// channel fader: a fresh sampler starts at `default_output_gain()`, so a
    /// full-scale commercial sample arrives level with the calibrated
    /// generators instead of well above them.
    #[serde(default = "legacy_output_gain")]
    pub output_gain: f32,
    /// Whether this sampler stretches at all.
    ///
    /// Unlike every other field here, turning this on cannot take effect from
    /// the realtime command drain: the stretch state is about 1.6 MB per
    /// sampler and has to be allocated on the control thread, then installed
    /// structurally. So this records *intent*, and the engine reconciles it by
    /// provisioning or reclaiming the pool. A sampler whose intent is on but
    /// whose pool has not arrived yet simply plays unstretched, which is the
    /// same thing it did the frame before.
    #[serde(default)]
    pub stretch_enabled: bool,
    /// Window sizing and whether the splice point is searched for.
    #[serde(default)]
    pub stretch_mode: StretchMode,
    /// Output frames per input frame. Above 1.0 is slower.
    #[serde(default = "unity_stretch_ratio")]
    pub stretch_ratio: f32,
    /// Grain window in frames, used only by [`StretchMode::Grain`]. Free and
    /// continuous because it is a timbre, not a quality setting.
    #[serde(default = "default_stretch_grain")]
    pub stretch_grain: u16,
    /// Fit the loop to the project tempo instead of using `stretch_ratio`
    /// directly.
    ///
    /// When this is on the ratio is *derived*, not set: the region is made to
    /// last `stretch_bars` bars whatever the tempo and whatever the voice is
    /// transposed to. That last part is the point -- the playback rate enters
    /// the derivation, so pitching a voice up shortens nothing. Pitch and
    /// duration become genuinely independent controls, which is the whole
    /// reason to have a stretcher at all.
    #[serde(default)]
    pub stretch_sync: bool,
    /// How many bars the region should last when `stretch_sync` is on.
    /// Seeded by [`snap_bars_to_power_of_two`] when the feature is switched
    /// on, then free to edit.
    #[serde(default = "default_stretch_bars")]
    pub stretch_bars: f32,
    /// Whether a note transposes the region or selects a slice.
    ///
    /// Defaulted so a project saved before slicing existed loads as `Pitched`
    /// and behaves byte-identically.
    #[serde(default)]
    pub play_mode: PlayMode,
    /// The note that plays slice 0 in [`PlayMode::Slice`]; each semitone above
    /// it steps one slice on.
    #[serde(default = "default_slice_base_note")]
    pub slice_base_note: u8,
    /// The filter envelope's own stages, or `None` to follow the amplitude
    /// envelope.
    ///
    /// `None` is what every project saved before the filter envelope existed
    /// means, and it is the migration: those patches drove `filter_env_amount`
    /// from the amp ADSR, so following it reproduces their filter motion
    /// exactly rather than approximately. A fresh sampler starts there too,
    /// and materializes its own stages the moment one is edited. Absence has
    /// to be representable for this to work at all -- a plain field with a
    /// serde default could not copy the amp stages, because a default cannot
    /// see its siblings.
    #[serde(default)]
    pub filter_env: Option<EnvTimes>,
    /// Portamento time in seconds, 0 to 2 (MOO-45). Heard only when the
    /// sampler plays one voice at a time (Voices 1, Pitched): which of
    /// several voices a slide would come from has no one answer, so the face
    /// disables it otherwise rather than inventing polyphonic portamento.
    ///
    /// The three glide fields default to what a song saved before them
    /// played: no glide, and every note restarting its envelopes.
    #[serde(default)]
    pub glide: f32,
    /// When a note glides: `Always` also slides into a release tail,
    /// `Legato` only between notes that overlap. ML-M1's control, on Adam's
    /// ruling (2026-09-23), so the two instruments read the same way.
    #[serde(default)]
    pub glide_mode: crate::mlm1::GlideMode,
    /// Whether an overlapping note restarts the envelopes and the sample
    /// (`Retrig`), or only moves the pitch of the note already sounding
    /// (`Legato`), keeping its envelopes and its place in the sample.
    #[serde(default)]
    pub env_trigger: crate::mlm1::EnvTrigger,
}

fn default_retune_live() -> bool {
    true
}

fn unity_stretch_ratio() -> f32 {
    1.0
}

fn default_stretch_grain() -> u16 {
    1024
}

fn default_stretch_bars() -> f32 {
    1.0
}

fn default_slice_base_note() -> u8 {
    DEFAULT_SLICE_BASE_NOTE
}

/// The trim a project saved before the field existed plays at. Those mixes
/// were balanced against a sampler running at unity, so they keep unity;
/// only a newly created sampler gets `default_output_gain()`.
fn legacy_output_gain() -> f32 {
    1.0
}

impl Default for SamplerParams {
    fn default() -> Self {
        Self {
            voice_mode: VoiceMode::OneShot,
            polyphony: 1,
            retrigger_mode: RetriggerMode::Restart,
            choke_group: 0,
            start: 0.0,
            end: 1.0,
            reverse: false,
            root_note: 60,
            tune_semitones: 0.0,
            tune_cents: 0.0,
            retune_live: true,
            loop_start: 0.0,
            loop_end: 1.0,
            loop_mode: LoopMode::Off,
            loop_crossfade_ms: 0.0,
            loop_quantize: LoopQuantize::Off,
            attack: 0.001,
            decay: 0.25,
            sustain: 1.0,
            release: 0.05,
            filter_cutoff: 1.0,
            filter_resonance: 0.0,
            filter_env_amount: 0.0,
            drive: 0.0,
            bit_reduction: 0.0,
            rate_reduction: 0.0,
            output_gain: default_output_gain(),
            stretch_enabled: false,
            stretch_mode: StretchMode::Music,
            stretch_ratio: 1.0,
            stretch_grain: 1024,
            stretch_sync: false,
            stretch_bars: 1.0,
            play_mode: PlayMode::Pitched,
            slice_base_note: DEFAULT_SLICE_BASE_NOTE,
            filter_env: None,
            glide: 0.0,
            glide_mode: crate::mlm1::GlideMode::default(),
            env_trigger: crate::mlm1::EnvTrigger::default(),
        }
    }
}

impl SamplerParams {
    /// The amplitude envelope's stages.
    pub fn amp_env(&self) -> EnvTimes {
        EnvTimes {
            attack: self.attack,
            decay: self.decay,
            sustain: self.sustain,
            release: self.release,
        }
    }

    /// The filter envelope's stages, resolved: its own when it has them, the
    /// amplitude envelope's when it does not. Every reader goes through here
    /// so "follows amp" is decided in one place.
    pub fn resolved_filter_env(&self) -> EnvTimes {
        self.filter_env.unwrap_or_else(|| self.amp_env())
    }

    /// Give the filter envelope its own stages, seeded from wherever it is
    /// reading now, so the first edit to one stage does not silently move the
    /// other three.
    pub fn filter_env_mut(&mut self) -> &mut EnvTimes {
        if self.filter_env.is_none() {
            self.filter_env = Some(self.resolved_filter_env());
        }
        self.filter_env.as_mut().expect("just materialized")
    }
}

/// Clamp helper used by both DSP (defensive) and UI (input validation).
pub fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// How many voices' time-stretch state a sampler needs: the size of its
/// stretch pool, and 0 when it does not stretch (MOO-7).
///
/// The pool follows Voices (Adam, 2026-09-23: "size the pool to Voices, with
/// a structural resize"), at **twice** Voices. A steal hands the stolen voice
/// to an idle slot above the count to fade out (MOO-110), and a stretching
/// voice fading without a reader would jump to unstretched playback for its
/// last 5 ms. Twice the count is enough for every held voice to be stolen at
/// once, which is what restriking a whole chord does. Capped at the voice
/// array. At the default single voice that is 2 readers, about 200 KB, where
/// every stretching sampler used to hold 16, about 1.6 MB.
///
/// Voices is a stepped parameter, so no modulation route can reach it
/// (`ModDestinationDescriptor::for_param` refuses stepped destinations), and
/// a MIDI-learned binding moves it on the control thread, where the session
/// reconciles the pool like any other edit. An automation lane is the one
/// path that changes it on the audio thread, where nothing can allocate, so
/// a channel with a lane on Voices gets the whole array.
pub fn stretch_pool_voices(params: &SamplerParams, lanes: &[Vec<AutomationLane>]) -> usize {
    if !params.stretch_enabled {
        return 0;
    }
    let all = usize::from(MAX_SAMPLER_VOICES);
    let lane_on_voices = lanes.iter().flatten().any(|lane| {
        lane.target.owner == ParamOwner::Source
            && lane.target.param == crate::generator::SAMPLER_PARAM_POLYPHONY
    });
    if lane_on_voices {
        return all;
    }
    (usize::from(params.polyphony.clamp(1, MAX_SAMPLER_VOICES)) * 2).min(all)
}

#[cfg(test)]
mod stretch_pool_tests {
    use super::*;
    use crate::{DeviceKind, EffectTarget, ModDestinationDescriptor, ParamAddr};

    fn voices(polyphony: u8) -> SamplerParams {
        SamplerParams {
            polyphony,
            stretch_enabled: true,
            ..SamplerParams::default()
        }
    }

    /// The pool follows Voices at twice the count, so a whole chord can be
    /// stolen and still fade out stretched, and never exceeds the voice
    /// array. A sampler that does not stretch holds none (MOO-7).
    #[test]
    fn the_pool_is_twice_voices_and_none_when_not_stretching() {
        assert_eq!(stretch_pool_voices(&voices(1), &[]), 2);
        assert_eq!(stretch_pool_voices(&voices(4), &[]), 8);
        assert_eq!(stretch_pool_voices(&voices(8), &[]), 16);
        assert_eq!(stretch_pool_voices(&voices(16), &[]), 16);
        let off = SamplerParams {
            stretch_enabled: false,
            ..voices(4)
        };
        assert_eq!(stretch_pool_voices(&off, &[]), 0);
    }

    /// A lane on Voices changes it on the audio thread, which cannot grow a
    /// pool, so the pool covers everything the lane can reach. A lane on
    /// anything else changes nothing.
    #[test]
    fn a_lane_on_voices_sizes_the_pool_to_every_voice() {
        let lane = |param| {
            AutomationLane::new(ParamAddr {
                scope: EffectTarget::Channel(0),
                owner: ParamOwner::Source,
                param,
            })
        };
        let on_voices = vec![vec![], vec![lane(crate::generator::SAMPLER_PARAM_POLYPHONY)]];
        assert_eq!(stretch_pool_voices(&voices(2), &on_voices), 16);
        let elsewhere = vec![vec![lane(crate::generator::SAMPLER_PARAM_START)]];
        assert_eq!(stretch_pool_voices(&voices(2), &elsewhere), 4);
    }

    /// The claim `stretch_pool_voices` rests on for routes: Voices is a
    /// stepped destination, and a stepped destination refuses modulation,
    /// so no route can move it on the audio thread. If Voices ever becomes
    /// modulatable, this fails and the pool has to cover routes too.
    #[test]
    fn no_modulation_route_can_reach_voices() {
        let descriptor = DeviceKind::Sampler
            .descriptor(crate::generator::SAMPLER_PARAM_POLYPHONY)
            .expect("the sampler has a Voices parameter");
        assert!(!ModDestinationDescriptor::for_param(descriptor).allowed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A patch saved before mono glide existed (MOO-45) loads with no glide
    /// and every note retriggering, which is how it played; one saved with
    /// them comes back with them.
    #[test]
    fn glide_fields_default_for_an_old_patch_and_round_trip() {
        let old = SamplerParams::default();
        let mut table = toml::Value::try_from(old).unwrap();
        let fields = table.as_table_mut().unwrap();
        for key in ["glide", "glide_mode", "env_trigger"] {
            assert!(fields.remove(key).is_some(), "{key} was not written");
        }
        let loaded: SamplerParams = table.try_into().unwrap();
        assert_eq!(loaded.glide, 0.0);
        assert_eq!(loaded.glide_mode, crate::mlm1::GlideMode::Legato);
        assert_eq!(loaded.env_trigger, crate::mlm1::EnvTrigger::Retrig);

        let gliding = SamplerParams {
            glide: 0.25,
            glide_mode: crate::mlm1::GlideMode::Always,
            env_trigger: crate::mlm1::EnvTrigger::Legato,
            ..SamplerParams::default()
        };
        let text = toml::to_string(&gliding).unwrap();
        let back: SamplerParams = toml::from_str(&text).unwrap();
        assert_eq!(back, gliding);
    }

    /// A patch saved before the filter envelope existed carries no field for
    /// it, and has to come back following whatever amplitude envelope it was
    /// actually using -- not the default one. This is the migration: an old
    /// project's filter motion is reproduced exactly, because the filter is
    /// still reading the same envelope it read before.
    #[test]
    fn a_patch_without_a_filter_envelope_follows_its_own_amp_envelope() {
        let manifest = r#"
voice_mode = "one_shot"
polyphony = 1
retrigger_mode = "restart"
choke_group = 0
start = 0.0
end = 1.0
reverse = false
root_note = 60
tune_semitones = 0.0
tune_cents = 0.0
loop_start = 0.0
loop_end = 1.0
loop_mode = "off"
attack = 0.3
decay = 1.5
sustain = 0.4
release = 2.0
filter_cutoff = 0.5
filter_resonance = 0.2
filter_env_amount = 0.75
drive = 0.0
bit_reduction = 0.0
rate_reduction = 0.0
"#;
        let params: SamplerParams = toml::from_str(manifest).unwrap();
        assert_eq!(params.filter_env, None, "absence has to survive the load");
        assert_eq!(
            params.resolved_filter_env(),
            EnvTimes {
                attack: 0.3,
                decay: 1.5,
                sustain: 0.4,
                release: 2.0,
            }
        );
        // And the trim from the same era still loads at unity.
        assert_eq!(params.output_gain, 1.0);
        // And its loops keep their hard seam (MOO-43): no fade means the
        // voice reads the plain wrap it always did, bit for bit.
        assert_eq!(params.loop_crossfade_ms, 0.0);
    }

    /// Once a patch has its own filter envelope, a round trip keeps it
    /// separate from the amplitude one rather than collapsing them.
    #[test]
    fn an_owned_filter_envelope_round_trips_separately() {
        let mut params = SamplerParams {
            attack: 0.3,
            decay: 1.5,
            sustain: 0.4,
            release: 2.0,
            ..SamplerParams::default()
        };
        params.filter_env_mut().decay = 0.01;
        params.filter_env_mut().sustain = 0.0;

        let text = toml::to_string(&params).unwrap();
        let loaded: SamplerParams = toml::from_str(&text).unwrap();
        assert_eq!(loaded, params);
        assert_eq!(loaded.amp_env().decay, 1.5);
        assert_eq!(loaded.resolved_filter_env().decay, 0.01);
        assert_eq!(loaded.resolved_filter_env().attack, 0.3, "seeded from amp");
    }
}

#[cfg(test)]
mod stretch_tests {
    use super::*;

    /// The snapping rule, spelled out from the examples it was specified
    /// with: whichever end of the power-of-two bracket the length is nearer,
    /// split at the arithmetic midpoint.
    #[test]
    fn a_loop_length_snaps_to_the_nearer_power_of_two_bars() {
        // The boundary cases the rule was described by.
        assert_eq!(snap_bars_to_power_of_two(1.49), 1.0);
        assert_eq!(snap_bars_to_power_of_two(1.5), 2.0);
        assert_eq!(snap_bars_to_power_of_two(2.99), 2.0);
        assert_eq!(snap_bars_to_power_of_two(3.0), 4.0);
        assert_eq!(snap_bars_to_power_of_two(5.99), 4.0);
        assert_eq!(snap_bars_to_power_of_two(6.0), 8.0);

        // Exact lengths stay put rather than drifting to a neighbour.
        for bars in [0.25f32, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0] {
            assert_eq!(snap_bars_to_power_of_two(bars), bars, "{bars} moved");
        }

        // A slightly long or short recording lands on the intended length,
        // which is the case the whole rule exists for.
        assert_eq!(snap_bars_to_power_of_two(2.02), 2.0);
        assert_eq!(snap_bars_to_power_of_two(3.97), 4.0);
    }

    /// It generalizes below a bar, so a half-bar chop is not dragged up to a
    /// whole one.
    #[test]
    fn the_rule_holds_below_a_single_bar() {
        assert_eq!(snap_bars_to_power_of_two(0.6), 0.5);
        assert_eq!(snap_bars_to_power_of_two(0.8), 1.0);
        assert_eq!(snap_bars_to_power_of_two(0.3), 0.25);
        assert_eq!(snap_bars_to_power_of_two(0.4), 0.5);
    }

    /// Nonsense in, something usable out. This runs off a measured sample
    /// length, and an empty or unloaded sample measures zero.
    #[test]
    fn a_meaningless_length_snaps_to_one_bar() {
        assert_eq!(snap_bars_to_power_of_two(0.0), 1.0);
        assert_eq!(snap_bars_to_power_of_two(-3.0), 1.0);
        assert_eq!(snap_bars_to_power_of_two(f32::NAN), 1.0);
        // Infinity falls in with the other nonsense rather than clamping to
        // the ceiling: a 64-bar guess from a broken measurement would be a
        // worse answer than one bar, not a better one.
        assert_eq!(snap_bars_to_power_of_two(f32::INFINITY), 1.0);
    }

    /// Four beats to the bar, matching the buffer device, and inversely
    /// proportional to tempo.
    #[test]
    fn a_bar_is_four_beats_at_the_project_tempo() {
        assert_eq!(frames_per_bar(48_000, 120.0), 96_000.0);
        assert_eq!(frames_per_bar(48_000, 60.0), 192_000.0);
        // A zero or negative tempo must not divide by zero.
        assert!(frames_per_bar(48_000, 0.0).is_finite());
    }
}

#[cfg(test)]
mod slice_tests {
    use super::*;

    /// The two operations #15 calls for on the whole map: lay slices out
    /// evenly, then throw them away. Nothing may survive the clear.
    #[test]
    fn dividing_evenly_then_clearing_returns_the_map_to_empty() {
        let mut map = SliceMap::new();
        map.divide_evenly(8, 0, 800);
        assert_eq!(map.len(), 8);
        let frames: Vec<u32> = map.markers().iter().map(|marker| marker.frame).collect();
        assert_eq!(frames, vec![0, 100, 200, 300, 400, 500, 600, 700]);
        // The last slice runs to the region end rather than to a ninth
        // marker: eight slices need eight boundaries, not nine.
        assert_eq!(map.span(7, 800.0), Some((700.0, 800.0)));
        map.clear();
        assert!(map.is_empty());
        assert_eq!(map.span(0, 800.0), None);
    }

    /// A persisted reference names a slice, not a position. Inserting a
    /// marker before it and deleting one after it must leave the same id
    /// pointing at the same audio, even though its ordinal position moved.
    #[test]
    fn slice_ids_survive_inserting_and_deleting_neighbours() {
        let mut map = SliceMap::new();
        let first = map.add(0).unwrap();
        let middle = map.add(100).unwrap();
        let last = map.add(200).unwrap();
        assert_eq!(map.index_of(middle), Some(1));

        map.add(50).unwrap();
        assert_eq!(map.index_of(middle), Some(2), "an insert before it shifts it");
        assert_eq!(map.get(2).map(|marker| marker.frame), Some(100));

        assert!(map.remove(first));
        assert_eq!(map.index_of(middle), Some(1));
        assert!(map.remove(last));
        assert_eq!(map.index_of(middle), Some(1));
        assert_eq!(map.get(1).map(|marker| marker.frame), Some(100));
        assert_eq!(map.index_of(last), None, "a deleted id resolves to nothing");
    }

    /// Moving a marker past its neighbour reorders the map, because ordinal
    /// position is what a note selects and that has to follow the frame.
    #[test]
    fn moving_a_marker_past_a_neighbour_reorders_the_map() {
        let mut map = SliceMap::new();
        let a = map.add(0).unwrap();
        let b = map.add(100).unwrap();
        assert!(map.move_to(a, 150));
        assert_eq!(map.index_of(b), Some(0));
        assert_eq!(map.index_of(a), Some(1));
        // A move onto an occupied frame is refused rather than collapsing
        // two boundaries into one zero-length slice.
        assert!(!map.move_to(a, 100));
        assert_eq!(map.get(1).map(|marker| marker.frame), Some(150));
    }

    /// Replace keeps what the hand placed or moved and drops what Divide
    /// laid down; a detected frame beside a kept marker is left out, because
    /// the hand already put one there (MOO-44).
    #[test]
    fn replacing_with_detected_markers_keeps_the_hand_placed_ones() {
        let mut map = SliceMap::new();
        map.divide_evenly(4, 0, 4_000);
        assert!(map.markers().iter().all(|marker| !marker.hand));
        let placed = map.add(2_500).unwrap();
        let moved = map.markers()[1].id;
        assert!(map.move_to(moved, 1_100));

        let added = map.replace_detected(&[0, 1_120, 1_900, 2_510, 3_300], 50);
        let frames: Vec<u32> = map.markers().iter().map(|marker| marker.frame).collect();
        assert_eq!(frames, [0, 1_100, 1_900, 2_500, 3_300]);
        assert_eq!(added, 3, "1_120 and 2_510 sit beside kept markers");
        assert!(map.index_of(placed).is_some() && map.index_of(moved).is_some());
        assert!(!map.markers()[0].hand, "a detected marker is not the hand's");
    }

    /// Merge keeps every marker, and adds only what isn't beside one.
    #[test]
    fn merging_detected_markers_keeps_every_existing_one() {
        let mut map = SliceMap::new();
        map.divide_evenly(2, 0, 2_000);
        let before: Vec<u64> = map.markers().iter().map(|marker| marker.id).collect();
        let added = map.merge_detected(&[10, 500, 1_020, 1_500], 50);
        assert_eq!(added, 2);
        for id in before {
            assert!(map.index_of(id).is_some(), "merge dropped marker {id}");
        }
        let frames: Vec<u32> = map.markers().iter().map(|marker| marker.frame).collect();
        assert_eq!(frames, [0, 500, 1_000, 1_500]);
    }

    /// Detection respects the map's cap: what doesn't fit is not added.
    #[test]
    fn detection_never_overfills_the_map() {
        let mut map = SliceMap::new();
        let frames: Vec<u32> = (0..(MAX_SLICES as u32 + 40)).map(|n| n * 100).collect();
        let added = map.merge_detected(&frames, 10);
        assert_eq!(added, MAX_SLICES);
        assert_eq!(map.len(), MAX_SLICES);
    }

    /// Deserialization is the one way into the map that skips its mutators,
    /// so it is the one way an unsorted or duplicated map could reach a
    /// voice. A hand-edited document is repaired on the way in, like every
    /// other out-of-range field in the format.
    #[test]
    fn a_hand_edited_map_is_sorted_and_deduplicated_on_load() {
        let map: SliceMap = toml::from_str(
            r#"
next_id = 3
markers = [
  { id = 1, frame = 900 },
  { id = 2, frame = 100 },
  { id = 3, frame = 900 },
]
"#,
        )
        .unwrap();
        assert_eq!(
            map.markers(),
            // Saved with no `hand`, so both load as placed by hand (MOO-44).
            &[
                SliceMarker {
                    id: 2,
                    frame: 100,
                    hand: true
                },
                SliceMarker {
                    id: 1,
                    frame: 900,
                    hand: true
                },
            ]
        );
        // And a fresh marker must not collide with an adopted id.
        let fresh = map.clone();
        let mut fresh = fresh;
        let id = fresh.add(500).unwrap();
        assert!(id > 3, "minting handed out an id already in use: {id}");
    }

    /// The map holds its own invariants: no duplicate frames, and never more
    /// than the addressable range.
    #[test]
    fn the_map_refuses_duplicates_and_stops_at_the_cap() {
        let mut map = SliceMap::new();
        assert!(map.add(10).is_some());
        assert!(map.add(10).is_none());
        map.divide_evenly(MAX_SLICES + 40, 0, 1_000_000);
        assert_eq!(map.len(), MAX_SLICES);
        assert!(map.add(999_999).is_none());
    }
}
