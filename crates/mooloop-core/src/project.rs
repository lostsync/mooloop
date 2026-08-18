//! Canonical editable project state shared by UI, persistence, and rendering.

use std::path::PathBuf;

use crate::{
    Channel, DeviceKind, NoteEvent, NoteId, PatternPlacement, PlaybackMode, SamplerParams,
    DEFAULT_STEPS,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SampleReference {
    Builtin { id: String },
    File { path: PathBuf, embedded: bool },
}

impl Default for SampleReference {
    fn default() -> Self {
        Self::Builtin {
            id: "default_kick".into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SamplerState {
    pub params: SamplerParams,
    pub sample: SampleReference,
}

/// Tagged now so later synth variants can join the v1 envelope additively.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "state", rename_all = "snake_case")]
pub enum ChannelSource {
    Sampler(SamplerState),
}

impl Default for ChannelSource {
    fn default() -> Self {
        Self::Sampler(SamplerState::default())
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChannelSetup {
    pub channel: Channel,
    pub source: ChannelSource,
}

impl ChannelSetup {
    pub fn sampler(name: impl Into<String>) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::Sampler),
            source: ChannelSource::default(),
        }
    }

    pub fn sampler_state(&self) -> &SamplerState {
        match &self.source {
            ChannelSource::Sampler(state) => state,
        }
    }

    pub fn sampler_state_mut(&mut self) -> &mut SamplerState {
        match &mut self.source {
            ChannelSource::Sampler(state) => state,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectChannel {
    pub setup: ChannelSetup,
    /// Pattern-indexed note lanes. Notes beyond a pattern's logical length are retained.
    pub notes: Vec<Vec<NoteEvent>>,
    pub next_note_id: NoteId,
}

impl ProjectChannel {
    pub fn sampler(index: usize, pattern_count: usize) -> Self {
        Self {
            setup: ChannelSetup::sampler(format!("Sampler {}", index + 1)),
            notes: vec![Vec::new(); pattern_count.max(1)],
            next_note_id: 1,
        }
    }

    pub fn recompute_next_note_id(&mut self) {
        self.next_note_id = self
            .notes
            .iter()
            .flatten()
            .map(|note| note.id)
            .max()
            .unwrap_or(0)
            .wrapping_add(1)
            .max(1);
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Project {
    pub bpm: u16,
    pub ppq: u16,
    pub beats_per_bar: u8,
    pub playback_mode: PlaybackMode,
    pub current_pattern: u16,
    pub selected_channel: u8,
    pub channels: Vec<ProjectChannel>,
    pub pattern_lengths: Vec<u16>,
    pub playlist: Vec<PatternPlacement>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            bpm: 120,
            ppq: 96,
            beats_per_bar: 4,
            playback_mode: PlaybackMode::Pattern,
            current_pattern: 0,
            selected_channel: 0,
            channels: vec![ProjectChannel::sampler(0, 1)],
            pattern_lengths: vec![DEFAULT_STEPS],
            playlist: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Kit {
    pub channels: Vec<ChannelSetup>,
}

pub type ChannelPreset = ChannelSetup;
