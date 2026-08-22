//! Canonical editable project state shared by UI, persistence, and rendering.

use std::path::PathBuf;

use crate::{
    default_buses, BusSetup, Channel, DeviceKind, DrumMode, DrumSynthParams, KickCharacter,
    MonoSynthParams, NoteEvent, NoteId, PatternPlacement, PlaybackMode, SamplerParams,
    SnareCharacter, DEFAULT_STEPS,
};

pub const MIN_SWING_PERCENT: u8 = 50;
pub const MAX_SWING_PERCENT: u8 = 75;
pub const DEFAULT_SWING_PERCENT: u8 = MIN_SWING_PERCENT;

const fn default_swing_percent() -> u8 {
    DEFAULT_SWING_PERCENT
}

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

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrumSynthState {
    pub params: DrumSynthParams,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MonoSynthState {
    pub params: MonoSynthParams,
}

/// Tagged now so later synth variants can join the v1 envelope additively.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "state", rename_all = "snake_case")]
pub enum ChannelSource {
    Sampler(SamplerState),
    DrumSynth(DrumSynthState),
    MonoSynth(MonoSynthState),
}

impl Default for ChannelSource {
    fn default() -> Self {
        Self::Sampler(SamplerState::default())
    }
}

impl ChannelSource {
    pub fn kind(&self) -> DeviceKind {
        match self {
            Self::Sampler(_) => DeviceKind::Sampler,
            Self::DrumSynth(_) => DeviceKind::DrumSynth,
            Self::MonoSynth(_) => DeviceKind::MonoSynth,
        }
    }

    pub fn sampler_state(&self) -> Option<&SamplerState> {
        match self {
            Self::Sampler(state) => Some(state),
            _ => None,
        }
    }

    pub fn sampler_state_mut(&mut self) -> Option<&mut SamplerState> {
        match self {
            Self::Sampler(state) => Some(state),
            _ => None,
        }
    }

    pub fn drum_synth_state(&self) -> Option<&DrumSynthState> {
        match self {
            Self::DrumSynth(state) => Some(state),
            _ => None,
        }
    }

    pub fn drum_synth_state_mut(&mut self) -> Option<&mut DrumSynthState> {
        match self {
            Self::DrumSynth(state) => Some(state),
            _ => None,
        }
    }

    pub fn mono_synth_state(&self) -> Option<&MonoSynthState> {
        match self {
            Self::MonoSynth(state) => Some(state),
            _ => None,
        }
    }

    pub fn mono_synth_state_mut(&mut self) -> Option<&mut MonoSynthState> {
        match self {
            Self::MonoSynth(state) => Some(state),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ChannelSetup {
    pub channel: Channel,
    pub source: ChannelSource,
    /// Effect chain slots in order. Defaulted on load so songs written
    /// before effects existed stay readable (same pattern as
    /// `MonoSynthParams.lfo`).
    #[serde(default)]
    pub effects: Vec<crate::EffectSlotState>,
}

impl ChannelSetup {
    pub fn sampler(name: impl Into<String>) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::Sampler),
            source: ChannelSource::default(),
            effects: Vec::new(),
        }
    }

    pub fn drum_synth(name: impl Into<String>) -> Self {
        Self::drum_synth_with_params(name, DrumSynthParams::default())
    }

    pub fn drum_synth_with_params(name: impl Into<String>, params: DrumSynthParams) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::DrumSynth),
            source: ChannelSource::DrumSynth(DrumSynthState { params }),
            effects: Vec::new(),
        }
    }

    pub fn mono_synth(name: impl Into<String>) -> Self {
        Self::mono_synth_with_params(name, MonoSynthParams::default())
    }

    pub fn mono_synth_with_params(name: impl Into<String>, params: MonoSynthParams) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::MonoSynth),
            source: ChannelSource::MonoSynth(MonoSynthState { params }),
            effects: Vec::new(),
        }
    }

    pub fn kind(&self) -> DeviceKind {
        self.source.kind()
    }

    pub fn sampler_state(&self) -> Option<&SamplerState> {
        self.source.sampler_state()
    }

    pub fn sampler_state_mut(&mut self) -> Option<&mut SamplerState> {
        self.source.sampler_state_mut()
    }

    pub fn drum_synth_state(&self) -> Option<&DrumSynthState> {
        self.source.drum_synth_state()
    }

    pub fn drum_synth_state_mut(&mut self) -> Option<&mut DrumSynthState> {
        self.source.drum_synth_state_mut()
    }

    pub fn mono_synth_state(&self) -> Option<&MonoSynthState> {
        self.source.mono_synth_state()
    }

    pub fn mono_synth_state_mut(&mut self) -> Option<&mut MonoSynthState> {
        self.source.mono_synth_state_mut()
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

    pub fn drum_synth(index: usize, pattern_count: usize) -> Self {
        Self::drum_synth_with_params(index, pattern_count, DrumSynthParams::default())
    }

    pub fn drum_synth_with_params(
        index: usize,
        pattern_count: usize,
        params: DrumSynthParams,
    ) -> Self {
        Self {
            setup: ChannelSetup::drum_synth_with_params(
                format!("Drum Synth {}", index + 1),
                params,
            ),
            notes: vec![Vec::new(); pattern_count.max(1)],
            next_note_id: 1,
        }
    }

    pub fn mono_synth(index: usize, pattern_count: usize) -> Self {
        Self::mono_synth_with_params(index, pattern_count, MonoSynthParams::default())
    }

    pub fn mono_synth_with_params(
        index: usize,
        pattern_count: usize,
        params: MonoSynthParams,
    ) -> Self {
        Self {
            setup: ChannelSetup::mono_synth_with_params(
                format!("Mono Synth {}", index + 1),
                params,
            ),
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
    #[serde(default = "default_swing_percent")]
    pub swing_percent: u8,
    pub ppq: u16,
    pub beats_per_bar: u8,
    pub playback_mode: PlaybackMode,
    pub current_pattern: u16,
    pub selected_channel: u8,
    pub channels: Vec<ProjectChannel>,
    /// Mixer buses, master first. Defaulted on load so songs written before
    /// the mixer existed get the full bank with everything on the master.
    #[serde(default = "default_buses")]
    pub buses: Vec<BusSetup>,
    pub pattern_lengths: Vec<u16>,
    pub playlist: Vec<PatternPlacement>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            bpm: 120,
            swing_percent: DEFAULT_SWING_PERCENT,
            ppq: 96,
            beats_per_bar: 4,
            playback_mode: PlaybackMode::Pattern,
            current_pattern: 0,
            selected_channel: 0,
            channels: vec![ProjectChannel::sampler(0, 1)],
            buses: default_buses(),
            pattern_lengths: vec![DEFAULT_STEPS],
            playlist: Vec::new(),
        }
    }
}

impl Project {
    /// Creates a concise, deterministic four-piece drum kit ready for sequencing.
    pub fn starter_kit(seed: u64) -> Self {
        let mut random = StarterRandom::new(seed);
        let mut kick = DrumSynthParams::preset(DrumMode::Kick);
        kick.kick_character = KickCharacter::Punch;
        kick.decay = random.range(0.18, 0.32);
        kick.punch = random.range(0.45, 0.7);
        kick.kick_start_hz = random.range(145.0, 185.0);
        kick.kick_end_hz = random.range(42.0, 58.0);
        kick.kick_sweep = random.range(0.035, 0.065);
        kick.kick_click = random.range(0.35, 0.6);
        kick.drive = random.range(0.02, 0.14);

        let mut snare = DrumSynthParams::preset(DrumMode::Snare);
        snare.snare_character = SnareCharacter::Snap;
        snare.decay = random.range(0.10, 0.18);
        snare.punch = random.range(0.45, 0.72);
        snare.snare_tone_hz = random.range(150.0, 220.0);
        snare.snare_tone2_hz = random.range(280.0, 520.0);
        snare.snare_tone2_mix = random.range(0.14, 0.34);
        snare.snare_noise_mix = random.range(0.52, 0.76);
        snare.snare_noise_decay = random.range(0.07, 0.14);
        snare.snare_noise_color = random.range(0.54, 0.82);
        snare.drive = random.range(0.0, 0.1);

        let mut closed_hat = DrumSynthParams::preset(DrumMode::Hat);
        closed_hat.choke_group = 1;
        closed_hat.decay = random.range(0.025, 0.07);
        closed_hat.hat_hp_hz = random.range(6_500.0, 10_500.0);
        closed_hat.hat_metallic = random.range(0.35, 0.7);

        let mut open_hat = DrumSynthParams::preset(DrumMode::Hat);
        open_hat.choke_group = 1;
        open_hat.decay = random.range(0.25, 0.52);
        open_hat.hat_hp_hz = random.range(6_000.0, 9_500.0);
        open_hat.hat_metallic = random.range(0.3, 0.68);

        Self {
            channels: [
                ("Kick", kick),
                ("Snare", snare),
                ("Closed Hat", closed_hat),
                ("Open Hat", open_hat),
            ]
            .into_iter()
            .map(|(name, params)| ProjectChannel {
                setup: ChannelSetup::drum_synth_with_params(name, params),
                notes: vec![Vec::new()],
                next_note_id: 1,
            })
            .collect(),
            ..Self::default()
        }
    }
}

struct StarterRandom(u64);

impl StarterRandom {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        let unit = (self.next_u64() >> 40) as f32 / ((1_u64 << 24) - 1) as f32;
        min + (max - min) * unit
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Kit {
    pub channels: Vec<ChannelSetup>,
}

pub type ChannelPreset = ChannelSetup;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_kit_is_deterministic_and_musically_shaped() {
        let first = Project::starter_kit(42);
        let repeated = Project::starter_kit(42);
        let varied = Project::starter_kit(43);

        assert_eq!(first, repeated);
        assert_ne!(first, varied);
        assert_eq!(
            first
                .channels
                .iter()
                .map(|channel| channel.setup.channel.name.as_str())
                .collect::<Vec<_>>(),
            ["Kick", "Snare", "Closed Hat", "Open Hat"]
        );
        assert!(first
            .channels
            .iter()
            .all(|channel| channel.setup.kind() == DeviceKind::DrumSynth));
        assert!(first
            .channels
            .iter()
            .all(|channel| channel.notes == vec![Vec::new()]));

        let kick = first.channels[0].setup.drum_synth_state().unwrap().params;
        let snare = first.channels[1].setup.drum_synth_state().unwrap().params;
        let closed_hat = first.channels[2].setup.drum_synth_state().unwrap().params;
        let open_hat = first.channels[3].setup.drum_synth_state().unwrap().params;
        assert_eq!(kick.mode, DrumMode::Kick);
        assert_eq!(snare.mode, DrumMode::Snare);
        assert_eq!(closed_hat.mode, DrumMode::Hat);
        assert_eq!(open_hat.mode, DrumMode::Hat);
        assert_eq!(kick.kick_character, KickCharacter::Punch);
        assert_eq!(snare.snare_character, SnareCharacter::Snap);
        assert_eq!(closed_hat.choke_group, 1);
        assert_eq!(open_hat.choke_group, 1);
        assert!(closed_hat.decay < open_hat.decay);
    }

    #[test]
    fn source_accessors_are_optional_and_report_their_kind() {
        let sampler = ChannelSetup::sampler("Sample");
        let drum = ChannelSetup::drum_synth("Drum");
        let mono = ChannelSetup::mono_synth("Mono");

        assert_eq!(sampler.kind(), DeviceKind::Sampler);
        assert!(sampler.sampler_state().is_some());
        assert!(sampler.drum_synth_state().is_none());
        assert_eq!(drum.kind(), DeviceKind::DrumSynth);
        assert!(drum.drum_synth_state().is_some());
        assert!(drum.mono_synth_state().is_none());
        assert_eq!(mono.kind(), DeviceKind::MonoSynth);
        assert!(mono.mono_synth_state().is_some());
        assert!(mono.sampler_state().is_none());
    }
}
