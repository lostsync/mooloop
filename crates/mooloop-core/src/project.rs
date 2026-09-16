//! Canonical editable project state shared by UI, persistence, and rendering.

use std::path::PathBuf;

use crate::structure::{rescope_lanes, rescope_lanes_for_track, ChannelEdit, TrackEdit};
use crate::{
    default_buses, BusSetup, Channel, DeviceKind, Ds01Params, DrumMode, DrumSynthParams,
    EffectTarget,
    KickCharacter, AutomationLane, LoopRange, ModRack, MAX_CHANNELS, MonoSynthParams, MlM1Params, MlP8Params, NoteEvent, NoteId,
    PatternPlacement,
    PlaybackMode, PolySynthParams,
    SampleCommit, SamplerParams, SliceMap, SnareCharacter, DEFAULT_STEPS, STARTER_LOOP_BARS,
    TICKS_PER_BAR,
};

pub const MIN_SWING_PERCENT: u8 = 50;
pub const MAX_SWING_PERCENT: u8 = 75;
pub const DEFAULT_SWING_PERCENT: u8 = MIN_SWING_PERCENT;

const fn default_swing_percent() -> u8 {
    DEFAULT_SWING_PERCENT
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SampleReference {
    /// No sample assigned. The default for a freshly created channel.
    #[default]
    Empty,
    /// Kept for backward compatibility: projects saved before the sampler
    /// stopped auto-loading a kick may still reference it by id.
    Builtin { id: String },
    File { path: PathBuf, embedded: bool },
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SamplerState {
    pub params: SamplerParams,
    pub sample: SampleReference,
    /// Slice boundaries into the sample. Absent in a project written before
    /// slicing existed, which loads as an empty map -- and an empty map with
    /// the defaulted `Pitched` play mode is exactly the old behaviour.
    #[serde(default)]
    pub slices: SliceMap,
    /// The stretch render this sampler's published buffer was baked from, if
    /// it has one. Absent means the published buffer is the source.
    ///
    /// The rendered audio is deliberately not persisted: a commit is
    /// reproducible from this spec, so loading decodes the source as usual
    /// and re-renders.
    ///
    /// Boxed because most samplers have no commit and this type is embedded
    /// in every channel of every project, kit, and preset: out of line it is
    /// a pointer in the common case instead of six fields and a `Vec`. That
    /// is also what keeps `DocumentResult`'s variants within sight of each
    /// other -- growing this by 48 bytes was enough to trip
    /// `large_enum_variant` two crates away.
    #[serde(default)]
    pub commit: Option<Box<SampleCommit>>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DrumSynthState {
    pub params: DrumSynthParams,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ds01State {
    pub params: Ds01Params,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MonoSynthState {
    pub params: MonoSynthParams,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MlM1State {
    pub params: MlM1Params,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MlP8State {
    pub params: MlP8Params,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PolySynthState {
    pub params: PolySynthParams,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AuxInState {
    pub params: crate::AuxInParams,
}

/// Tagged now so later synth variants can join the v1 envelope additively.
// Every variant here is one synth's state block, and the ML-P8 is simply the
// newest and widest; the next synth will be wider still. Boxing whichever
// variant currently happens to be largest buys a size ratio at the cost of a
// pointer chase, and hands the same choice back on every synth added after.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "state", rename_all = "snake_case")]
pub enum ChannelSource {
    Sampler(SamplerState),
    DrumSynth(DrumSynthState),
    MonoSynth(MonoSynthState),
    PolySynth(PolySynthState),
    /// Tagged `ml1` on disk, not the `rename_all` default `ml_m1`. This is the
    /// `type` field every saved song and channel preset carries for an ML-M1
    /// channel, written before the device's name was corrected. See
    /// [`crate::DeviceKind::MlM1`], which is frozen for the same reason.
    #[serde(rename = "ml1")]
    MlM1(MlM1State),
    /// Tagged `mlp8`, matching [`crate::DeviceKind::MlP8`]. Named explicitly
    /// rather than taking the `rename_all` default `ml_p8`, because a
    /// serialized tag is an on-disk identifier and this device gets to choose
    /// its own before any project carries it.
    #[serde(rename = "mlp8")]
    MlP8(MlP8State),
    /// Tagged `ds01`, matching [`crate::DeviceKind::Ds01`], and chosen the
    /// same way and for the same reason.
    #[serde(rename = "ds01")]
    Ds01(Ds01State),
    /// Tagged `aux_in`, matching [`crate::DeviceKind::AuxIn`]. Old projects
    /// have no such channel and load unchanged; a fresh one subscribes to
    /// nothing and is silent rather than invalid.
    #[serde(rename = "aux_in")]
    AuxIn(AuxInState),
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
            Self::PolySynth(_) => DeviceKind::PolySynth,
            Self::MlM1(_) => DeviceKind::MlM1,
            Self::MlP8(_) => DeviceKind::MlP8,
            Self::Ds01(_) => DeviceKind::Ds01,
            Self::AuxIn(_) => DeviceKind::AuxIn,
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

    pub fn mlm1_state(&self) -> Option<&MlM1State> {
        match self {
            Self::MlM1(state) => Some(state),
            _ => None,
        }
    }

    pub fn mlm1_state_mut(&mut self) -> Option<&mut MlM1State> {
        match self {
            Self::MlM1(state) => Some(state),
            _ => None,
        }
    }

    pub fn mlp8_state(&self) -> Option<&MlP8State> {
        match self {
            Self::MlP8(state) => Some(state),
            _ => None,
        }
    }

    pub fn mlp8_state_mut(&mut self) -> Option<&mut MlP8State> {
        match self {
            Self::MlP8(state) => Some(state),
            _ => None,
        }
    }

    pub fn ds01_state(&self) -> Option<&Ds01State> {
        match self {
            Self::Ds01(state) => Some(state),
            _ => None,
        }
    }

    pub fn ds01_state_mut(&mut self) -> Option<&mut Ds01State> {
        match self {
            Self::Ds01(state) => Some(state),
            _ => None,
        }
    }

    pub fn aux_in_state(&self) -> Option<&AuxInState> {
        match self {
            Self::AuxIn(state) => Some(state),
            _ => None,
        }
    }

    pub fn aux_in_state_mut(&mut self) -> Option<&mut AuxInState> {
        match self {
            Self::AuxIn(state) => Some(state),
            _ => None,
        }
    }

    /// Follow a channel edit, for the one kind that names another channel.
    ///
    /// Returns whether anything moved. Every other source is unaffected: a
    /// sampler does not care which seat it is in.
    pub fn rescope_subscription(&mut self, edit: crate::structure::ChannelEdit) -> bool {
        match self {
            Self::AuxIn(state) => state.params.rescope(edit),
            _ => false,
        }
    }

    /// The audio edge this channel's generator is asking for.
    ///
    /// `None` for every kind but Aux In, and for an Aux In that has not been
    /// pointed at anything. This is what `compile_audio_graph` is given, one
    /// entry a channel.
    pub fn audio_subscription(&self) -> Option<crate::AudioSubscription> {
        match self {
            Self::AuxIn(state) => state.params.subscription(),
            _ => None,
        }
    }

    pub fn poly_synth_state(&self) -> Option<&PolySynthState> {
        match self {
            Self::PolySynth(state) => Some(state),
            _ => None,
        }
    }

    pub fn poly_synth_state_mut(&mut self) -> Option<&mut PolySynthState> {
        match self {
            Self::PolySynth(state) => Some(state),
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
    /// Per-channel modulator slots and their matrix routes. The default keeps
    /// projects written before modulation was added completely compatible.
    #[serde(default)]
    pub modulation: ModRack,
    /// Next device identity to mint for `effects`. Monotonic, so removing a
    /// device and adding another never hands the newcomer the departed
    /// device's routes. Defaults to zero and is raised past whatever the
    /// chain already holds by [`Self::assign_device_ids`], which is what
    /// makes a project written before this field loads correctly.
    #[serde(default)]
    pub next_device_id: u32,
}

impl ChannelSetup {
    pub fn sampler(name: impl Into<String>) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::Sampler),
            source: ChannelSource::default(),
            effects: Vec::new(),
            modulation: ModRack::default(),
            next_device_id: 0,
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
            modulation: ModRack::default(),
            next_device_id: 0,
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
            modulation: ModRack::default(),
            next_device_id: 0,
        }
    }

    pub fn mlm1(name: impl Into<String>) -> Self {
        Self::mlm1_with_params(name, MlM1Params::default())
    }

    pub fn mlm1_with_params(name: impl Into<String>, params: MlM1Params) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::MlM1),
            source: ChannelSource::MlM1(MlM1State { params }),
            effects: Vec::new(),
            modulation: ModRack::default(),
            next_device_id: 0,
        }
    }

    pub fn mlp8(name: impl Into<String>) -> Self {
        Self::mlp8_with_params(name, MlP8Params::default())
    }

    pub fn mlp8_with_params(name: impl Into<String>, params: MlP8Params) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::MlP8),
            source: ChannelSource::MlP8(MlP8State { params }),
            effects: Vec::new(),
            modulation: ModRack::default(),
            next_device_id: 0,
        }
    }

    pub fn ds01(name: impl Into<String>) -> Self {
        Self::ds01_with_params(name, Ds01Params::default())
    }

    pub fn ds01_with_params(name: impl Into<String>, params: Ds01Params) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::Ds01),
            source: ChannelSource::Ds01(Ds01State { params }),
            effects: Vec::new(),
            modulation: ModRack::default(),
            next_device_id: 0,
        }
    }

    pub fn aux_in(name: impl Into<String>) -> Self {
        Self::aux_in_with_params(name, crate::AuxInParams::default())
    }

    pub fn aux_in_with_params(name: impl Into<String>, params: crate::AuxInParams) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::AuxIn),
            source: ChannelSource::AuxIn(AuxInState { params }),
            effects: Vec::new(),
            modulation: ModRack::default(),
            next_device_id: 0,
        }
    }

    pub fn poly_synth(name: impl Into<String>) -> Self {
        Self::poly_synth_with_params(name, PolySynthParams::default())
    }

    pub fn poly_synth_with_params(name: impl Into<String>, params: PolySynthParams) -> Self {
        Self {
            channel: Channel::new(name, DeviceKind::PolySynth),
            source: ChannelSource::PolySynth(PolySynthState { params }),
            effects: Vec::new(),
            modulation: ModRack::default(),
            next_device_id: 0,
        }
    }

    pub fn kind(&self) -> DeviceKind {
        self.source.kind()
    }

    /// Points every channel-scoped modulation route in this setup at
    /// `channel`.
    ///
    /// A route names its destination channel absolutely, so a rack is only
    /// correct on the channel it was authored on. That is right for a project
    /// -- the scope is what will let one channel modulate another -- and wrong
    /// for a preset, a kit entry, or a pasted channel, none of which has any
    /// business claiming a channel number. Wherever a setup lands somewhere
    /// new is where the rewrite belongs. Bus-scoped routes are left alone: a
    /// bus exists independently of which channel loaded the setup.
    /// Append `effect` to the chain, minting it an identity.
    ///
    /// The way to add a device to a chain in code. Pushing onto `effects`
    /// directly produces a device with no identity, which no route and no
    /// lane can then name.
    pub fn push_effect(&mut self, effect: crate::EffectSlotState) -> Option<crate::DeviceId> {
        let at = self.effects.len();
        crate::insert_effect(&mut self.effects, &mut self.next_device_id, at, effect)?;
        Some(self.effects[at].id)
    }

    /// Give this chain's devices their identities and put the mint past them.
    /// See [`crate::assign_device_ids`] for why this is a no-op on a project
    /// that has never seen it rather than a migration.
    pub fn assign_device_ids(&mut self) {
        crate::assign_device_ids(&mut self.effects, &mut self.next_device_id);
    }

    pub fn rescope_modulation(&mut self, channel: u8) {
        for route in self.modulation.routes.iter_mut().flatten() {
            if matches!(route.destination.scope, EffectTarget::Channel(_)) {
                route.destination.scope = EffectTarget::Channel(channel);
            }
        }
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

    pub fn mlm1_state(&self) -> Option<&MlM1State> {
        self.source.mlm1_state()
    }

    pub fn mlm1_state_mut(&mut self) -> Option<&mut MlM1State> {
        self.source.mlm1_state_mut()
    }

    pub fn mlp8_state(&self) -> Option<&MlP8State> {
        self.source.mlp8_state()
    }

    pub fn mlp8_state_mut(&mut self) -> Option<&mut MlP8State> {
        self.source.mlp8_state_mut()
    }

    pub fn ds01_state(&self) -> Option<&Ds01State> {
        self.source.ds01_state()
    }

    pub fn ds01_state_mut(&mut self) -> Option<&mut Ds01State> {
        self.source.ds01_state_mut()
    }

    pub fn poly_synth_state(&self) -> Option<&PolySynthState> {
        self.source.poly_synth_state()
    }

    pub fn poly_synth_state_mut(&mut self) -> Option<&mut PolySynthState> {
        self.source.poly_synth_state_mut()
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectChannel {
    pub setup: ChannelSetup,
    /// Pattern-indexed note lanes. Notes beyond a pattern's logical length are retained.
    pub notes: Vec<Vec<NoteEvent>>,
    /// Pattern-indexed automation lanes, parallel to `notes`. Absent in songs
    /// written before clip automation existed, which load with none.
    #[serde(default)]
    pub automation: Vec<Vec<AutomationLane>>,
    pub next_note_id: NoteId,
}

impl ProjectChannel {
    pub fn sampler(index: usize, pattern_count: usize) -> Self {
        Self {
            setup: ChannelSetup::sampler(DeviceKind::Sampler.default_channel_name(index)),
            notes: vec![Vec::new(); pattern_count.max(1)],
            automation: vec![Vec::new(); pattern_count.max(1)],
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
                DeviceKind::DrumSynth.default_channel_name(index),
                params,
            ),
            notes: vec![Vec::new(); pattern_count.max(1)],
            automation: vec![Vec::new(); pattern_count.max(1)],
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
                DeviceKind::MonoSynth.default_channel_name(index),
                params,
            ),
            notes: vec![Vec::new(); pattern_count.max(1)],
            automation: vec![Vec::new(); pattern_count.max(1)],
            next_note_id: 1,
        }
    }

    pub fn mlm1(index: usize, pattern_count: usize) -> Self {
        Self::mlm1_with_params(index, pattern_count, MlM1Params::default())
    }

    pub fn mlm1_with_params(
        index: usize,
        pattern_count: usize,
        params: MlM1Params,
    ) -> Self {
        Self {
            setup: ChannelSetup::mlm1_with_params(DeviceKind::MlM1.default_channel_name(index), params),
            notes: vec![Vec::new(); pattern_count.max(1)],
            automation: vec![Vec::new(); pattern_count.max(1)],
            next_note_id: 1,
        }
    }

    pub fn mlp8(index: usize, pattern_count: usize) -> Self {
        Self::mlp8_with_params(index, pattern_count, MlP8Params::default())
    }

    pub fn mlp8_with_params(index: usize, pattern_count: usize, params: MlP8Params) -> Self {
        Self {
            setup: ChannelSetup::mlp8_with_params(DeviceKind::MlP8.default_channel_name(index), params),
            notes: vec![Vec::new(); pattern_count.max(1)],
            automation: vec![Vec::new(); pattern_count.max(1)],
            next_note_id: 1,
        }
    }

    pub fn ds01(index: usize, pattern_count: usize) -> Self {
        Self::ds01_with_params(index, pattern_count, Ds01Params::default())
    }

    pub fn ds01_with_params(index: usize, pattern_count: usize, params: Ds01Params) -> Self {
        Self {
            setup: ChannelSetup::ds01_with_params(DeviceKind::Ds01.default_channel_name(index), params),
            notes: vec![Vec::new(); pattern_count.max(1)],
            automation: vec![Vec::new(); pattern_count.max(1)],
            next_note_id: 1,
        }
    }

    pub fn aux_in(index: usize, pattern_count: usize) -> Self {
        Self::aux_in_with_params(index, pattern_count, crate::AuxInParams::default())
    }

    pub fn aux_in_with_params(
        index: usize,
        pattern_count: usize,
        params: crate::AuxInParams,
    ) -> Self {
        Self {
            setup: ChannelSetup::aux_in_with_params(DeviceKind::AuxIn.default_channel_name(index), params),
            notes: vec![Vec::new(); pattern_count.max(1)],
            automation: vec![Vec::new(); pattern_count.max(1)],
            next_note_id: 1,
        }
    }

    pub fn poly_synth(index: usize, pattern_count: usize) -> Self {
        Self::poly_synth_with_params(index, pattern_count, PolySynthParams::default())
    }

    pub fn poly_synth_with_params(
        index: usize,
        pattern_count: usize,
        params: PolySynthParams,
    ) -> Self {
        Self {
            setup: ChannelSetup::poly_synth_with_params(
                DeviceKind::PolySynth.default_channel_name(index),
                params,
            ),
            notes: vec![Vec::new(); pattern_count.max(1)],
            automation: vec![Vec::new(); pattern_count.max(1)],
            next_note_id: 1,
        }
    }

    /// Point everything in this channel that names its own channel index at
    /// `channel`: its routes, and every lane in every pattern. What a
    /// pasted or loaded channel needs before it can live at a new index.
    pub fn rescope(&mut self, channel: u8) {
        self.setup.rescope_modulation(channel);
        for lanes in &mut self.automation {
            for lane in lanes.iter_mut() {
                if matches!(lane.target.scope, EffectTarget::Channel(_)) {
                    lane.target.scope = EffectTarget::Channel(channel);
                }
            }
        }
    }

    /// Pad `automation` out to match `notes`. A song saved before clip
    /// automation has none at all, and one saved before a pattern was added
    /// has fewer; both must end up addressable by pattern index.
    pub fn normalize_automation(&mut self) {
        self.automation.resize_with(self.notes.len(), Vec::new);
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

/// The top of the retired `Offset` descriptor's range, in beats. Spelled here
/// because the descriptor it came from no longer exists to be asked, and a
/// migration has to keep meaning what the old table meant.
const OFFSET_BEATS_FULL_SCALE: f32 = 16.0;

fn history_beats(bars: u8) -> f32 {
    f32::from(bars) * crate::time::BEATS_PER_BAR as f32
}

/// One stored lane value, from normalized `Offset` to normalized `Position`.
fn offset_value_as_position(value: f32, history_beats: f32) -> f32 {
    let beats = value.clamp(0.0, 1.0) * OFFSET_BEATS_FULL_SCALE;
    (1.0 - beats / history_beats).clamp(0.0, 1.0)
}

/// What a pattern is called and what colour it was given.
///
/// Both are optional in the only sense that matters to a document: an empty
/// name is a pattern shown by its number -- which is why `rename_pattern`
/// accepts a blank where `rename_channel` refuses one, the number being right
/// beside it -- and no colour is a pattern drawn in the theme's own.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PatternMeta {
    #[serde(default)]
    pub name: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::color::deserialize_lenient"
    )]
    pub color: Option<crate::color::ProjectColor>,
}

impl PatternMeta {
    /// Whether this entry says nothing at all, and so is worth neither
    /// writing nor keeping.
    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.color.is_none()
    }
}

/// The form of a pattern-metadata list worth writing to disk: the same
/// entries, with the trailing ones that say nothing dropped.
///
/// The session holds one entry per pattern so nothing has to bounds-check an
/// index. A document does not need them, and a song where nobody has named or
/// coloured anything should write the same bytes it wrote before the field
/// existed -- otherwise merely opening and saving a song rewrites it, which is
/// the thing `PROJECT_FORMAT.md`'s defaulted-field rule is there to prevent.
///
/// Only the *trailing* empties go: an unnamed pattern 1 in front of a named
/// pattern 2 has to keep its place in the list.
pub fn trim_pattern_meta(meta: &[PatternMeta]) -> Vec<PatternMeta> {
    let keep = meta
        .iter()
        .rposition(|entry| !entry.is_empty())
        .map_or(0, |last| last + 1);
    meta[..keep].to_vec()
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Project {
    pub bpm: u16,
    #[serde(default = "default_swing_percent")]
    pub swing_percent: u8,
    pub ppq: u16,
    /// Time signature numerator, persisted and validated. **The engine does
    /// not read this.** Bar arithmetic everywhere uses
    /// [`time::BEATS_PER_BAR`]; this field exists so the format does not have
    /// to change when a signature can actually be honoured, and
    /// `integrity.rs` holds the two to the same value meanwhile.
    ///
    /// [`time::BEATS_PER_BAR`]: crate::time::BEATS_PER_BAR
    pub beats_per_bar: u8,
    pub playback_mode: PlaybackMode,
    pub current_pattern: u16,
    pub selected_channel: u8,
    pub channels: Vec<ProjectChannel>,
    /// Mixer buses, master first. Defaulted on load so songs written before
    /// the mixer existed get the master and nothing else -- a bank is the
    /// tracks somebody made, and `default_buses` stopped returning seventeen
    /// of them when the mixer became a list rather than a fixed bank.
    #[serde(default = "default_buses")]
    pub buses: Vec<BusSetup>,
    pub pattern_lengths: Vec<u16>,
    /// Pattern-indexed names and colours, parallel to `pattern_lengths`.
    ///
    /// Defaulted, and shorter or longer than the bank is not an error -- the
    /// loader fits it to `pattern_lengths`, which is the field that decides
    /// how many patterns a song has. A pattern with no entry is a pattern
    /// nobody has named or coloured, which is what every song written before
    /// this field existed is.
    ///
    /// **Patterns could be renamed for months and the name was never saved**:
    /// `Session::pattern_names` existed, `rename_pattern` wrote to it, and
    /// nothing carried it into the document, so reopening a song blanked every
    /// one. Colour arrives with the name rather than beside it for that
    /// reason.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pattern_meta: Vec<PatternMeta>,
    pub playlist: Vec<PatternPlacement>,
    /// The repeating section of the arrangement. Defaulted on load, so a song
    /// written before looping existed opens with the loop off and its points
    /// at the origin rather than failing to decode.
    #[serde(default)]
    pub loop_range: LoopRange,
    /// Control-surface bindings: which knob on a desk moves what here.
    ///
    /// In the project because its targets are: a [`crate::ParamAddr`] names a
    /// device on a channel of *this* song, so a map stored beside the
    /// application would be pointing at another song's channels the moment
    /// one was opened. A surface template that outlives a song -- a desk's
    /// transport row, say -- is a separate document that stamps bindings into
    /// a project, and `docs/CONTROL_SURFACES.md` records it as not built.
    ///
    /// Defaulted and skipped when empty, so a song nobody has mapped is
    /// byte-identical to one written before the field existed.
    #[serde(default, skip_serializing_if = "is_empty_control_map")]
    pub control_map: crate::control::ControlMap,
}

fn is_empty_control_map(map: &crate::control::ControlMap) -> bool {
    map.bindings.is_empty()
}

/// Whether a bank of `count` tracks can move the one at `from` to `to`.
///
/// The one rule [`Project::move_track`] applies and every surface that offers
/// a move greys itself by, so a menu row cannot be enabled for a move the
/// model would refuse: both seats exist, they differ, and neither is the
/// master's.
pub fn track_move_allowed(count: usize, from: usize, to: usize) -> bool {
    let master = crate::MASTER_BUS as usize;
    from < count && to < count && from != to && from != master && to != master
}

impl Project {
    /// Roughly what one copy of this project occupies, counting the heap it
    /// owns as well as its own bytes.
    ///
    /// An estimate, deliberately: it walks the collections that actually
    /// scale with a song -- channels, their pattern-indexed note and
    /// automation lanes, the effect chains, the playlist -- and does not
    /// chase the last few bytes of a `String` allocator header. The caller
    /// is the undo history deciding how many snapshots it can afford, and a
    /// budget wants the shape of the number rather than its exact value.
    ///
    /// `Vec::capacity` rather than `len`, because a vector that has grown and
    /// been drained is still holding the memory.
    pub fn heap_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.channels.capacity() * std::mem::size_of::<ProjectChannel>()
            + self.channels.iter().map(ProjectChannel::heap_bytes).sum::<usize>()
            + self.buses.capacity() * std::mem::size_of::<BusSetup>()
            + self.buses.iter().map(BusSetup::heap_bytes).sum::<usize>()
            + self.pattern_lengths.capacity() * std::mem::size_of::<u16>()
            + self.pattern_meta.capacity() * std::mem::size_of::<PatternMeta>()
            + self.pattern_meta.iter().map(|meta| meta.name.capacity()).sum::<usize>()
            + self.playlist.capacity() * std::mem::size_of::<PatternPlacement>()
    }
}

impl ProjectChannel {
    /// The heap this channel owns, beyond its own `size_of`.
    ///
    /// The pattern-indexed banks are the part that grows: a song with
    /// twenty-four patterns carries twenty-four note vectors and twenty-four
    /// automation vectors on every channel, whether or not they hold
    /// anything, and each automation lane owns its own points.
    pub fn heap_bytes(&self) -> usize {
        self.setup.heap_bytes()
            + self.notes.capacity() * std::mem::size_of::<Vec<NoteEvent>>()
            + self
                .notes
                .iter()
                .map(|pattern| pattern.capacity() * std::mem::size_of::<NoteEvent>())
                .sum::<usize>()
            + self.automation.capacity() * std::mem::size_of::<Vec<AutomationLane>>()
            + self
                .automation
                .iter()
                .map(|pattern| {
                    pattern.capacity() * std::mem::size_of::<AutomationLane>()
                        + pattern.iter().map(AutomationLane::heap_bytes).sum::<usize>()
                })
                .sum::<usize>()
    }
}

impl ChannelSetup {
    /// The heap this channel's devices own.
    ///
    /// `EffectSlotState` allocates nothing -- every effect's parameters are a
    /// fixed-size variant and a container names its children by count rather
    /// than by owning them -- so the chain costs its capacity and no walk.
    pub fn heap_bytes(&self) -> usize {
        self.channel.name.capacity()
            + self.effects.capacity() * std::mem::size_of::<crate::EffectSlotState>()
            + self.source.heap_bytes()
    }
}

impl ChannelSource {
    /// The heap this generator owns. Only the sampler has any: a sample
    /// reference is a path, and a slice map is a list of markers.
    pub fn heap_bytes(&self) -> usize {
        match self {
            Self::Sampler(state) => state.sample.heap_bytes() + state.slices.heap_bytes(),
            _ => 0,
        }
    }
}

impl SampleReference {
    pub fn heap_bytes(&self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Builtin { id } => id.capacity(),
            Self::File { path, .. } => path.as_os_str().len(),
        }
    }
}

impl BusSetup {
    pub fn heap_bytes(&self) -> usize {
        self.bus.name.capacity()
            + self.effects.capacity() * std::mem::size_of::<crate::EffectSlotState>()
    }
}

impl Default for Project {
    fn default() -> Self {
        Self {
            bpm: 120,
            swing_percent: DEFAULT_SWING_PERCENT,
            ppq: 96,
            beats_per_bar: crate::time::BEATS_PER_BAR as u8,
            playback_mode: PlaybackMode::Pattern,
            current_pattern: 0,
            selected_channel: 0,
            channels: vec![ProjectChannel::sampler(0, 1)],
            buses: default_buses(),
            pattern_lengths: vec![DEFAULT_STEPS],
            // Empty rather than one blank entry per pattern: an entry that
            // says nothing is not worth storing, and a song nobody has named
            // or coloured must be byte-identical to one written before this
            // field existed. `Session` fits the list to the bank on the way
            // in and `trim_pattern_meta` trims it on the way out.
            pattern_meta: Vec::new(),
            playlist: Vec::new(),
            loop_range: LoopRange::default(),
            control_map: crate::control::ControlMap::default(),
        }
    }
}

impl Project {
    /// Give every device in every chain -- channels and buses -- its
    /// identity, and put each chain's mint past it.
    ///
    /// The one place a loaded song gets this done. Called after decode and
    /// before anything resolves an address, because until it has run a chain
    /// written by an older version holds `DeviceId::UNASSIGNED` in every row
    /// and every route in the song resolves to nothing.
    pub fn assign_device_ids(&mut self) {
        for channel in &mut self.channels {
            channel.setup.assign_device_ids();
        }
        for bus in &mut self.buses {
            bus.assign_device_ids();
        }
    }

    /// Point every lane and route that still names the Buffer's retired
    /// `Offset` at `Position` instead, in the new coordinate.
    ///
    /// `Offset` was beats behind a moving writer, on `0..16` linear;
    /// `Position` is normalized over the ring and counts from the *old* end,
    /// so the conversion is `pos = 1 - beats / history_beats` and it inverts
    /// the direction. A lane's stored value is normalized against its
    /// descriptor, so a stored `v` meant `v * 16` beats.
    ///
    /// **It needs the device, not just the id**, because `history_beats`
    /// depends on that buffer's own `bars` -- which is why this is a pass over
    /// the project rather than a `From` on a lane. A route's `depth` is a
    /// signed fraction of the destination's range and converts the same way,
    /// negated because the axis turned round.
    ///
    /// Run after [`Self::assign_device_ids`] and before anything resolves an
    /// address: until ids exist, a chain written by an older version cannot be
    /// looked up at all. Idempotent -- a lane already on `Position` does not
    /// match, and id 0 is spent, so nothing can come to mean `Offset` again.
    pub fn migrate_retired_buffer_offset(&mut self) {
        let bars_for = |scope: crate::EffectTarget, device: crate::DeviceId| -> Option<u8> {
            let effects = match scope {
                crate::EffectTarget::Channel(channel) => {
                    &self.channels.get(channel as usize)?.setup.effects
                }
                crate::EffectTarget::Bus(bus) => &self.buses.get(bus as usize)?.effects,
            };
            effects
                .iter()
                .find(|slot| slot.id == device)
                .and_then(|slot| slot.params.buffer())
                .map(|params| params.bars.max(1))
        };

        // Collected first because the closure above borrows `self`, and the
        // edits below need it mutably. A song's lanes are a few hundred at
        // the outside and this runs once per open.
        let mut lane_conversions: Vec<(usize, usize, usize, f32)> = Vec::new();
        for (channel_index, channel) in self.channels.iter().enumerate() {
            for (pattern, lanes) in channel.automation.iter().enumerate() {
                for (lane_index, lane) in lanes.iter().enumerate() {
                    if lane.target.param != crate::BUFFER_PARAM_OFFSET_BEATS {
                        continue;
                    }
                    let crate::ParamOwner::Effect { device } = lane.target.owner else {
                        continue;
                    };
                    let Some(bars) = bars_for(lane.target.scope, device) else {
                        continue;
                    };
                    lane_conversions.push((
                        channel_index,
                        pattern,
                        lane_index,
                        history_beats(bars),
                    ));
                }
            }
        }
        let mut route_conversions: Vec<(usize, usize, f32)> = Vec::new();
        for (channel_index, channel) in self.channels.iter().enumerate() {
            for (route_index, route) in channel.setup.modulation.routes.iter().enumerate() {
                let Some(route) = route else { continue };
                if route.destination.param != crate::BUFFER_PARAM_OFFSET_BEATS {
                    continue;
                }
                let crate::ParamOwner::Effect { device } = route.destination.owner else {
                    continue;
                };
                let Some(bars) = bars_for(route.destination.scope, device) else {
                    continue;
                };
                route_conversions.push((channel_index, route_index, history_beats(bars)));
            }
        }

        for (channel_index, pattern, lane_index, history) in lane_conversions {
            let lane = &mut self.channels[channel_index].automation[pattern][lane_index];
            lane.target.param = crate::BUFFER_PARAM_POSITION;
            let points: Vec<crate::AutomationPoint> = lane
                .points()
                .iter()
                .map(|point| crate::AutomationPoint {
                    value: offset_value_as_position(point.value, history),
                    ..*point
                })
                .collect();
            lane.reset_points(points);
        }
        for (channel_index, route_index, history) in route_conversions {
            let Some(route) =
                &mut self.channels[channel_index].setup.modulation.routes[route_index]
            else {
                continue;
            };
            route.destination.param = crate::BUFFER_PARAM_POSITION;
            // Negated: more offset was further back, and more position is
            // further forward.
            route.depth = (-route.depth * OFFSET_BEATS_FULL_SCALE / history).clamp(-1.0, 1.0);
        }
    }

    /// Delete the channel at `index`, closing the gap. Every route and lane
    /// in the song that named a later channel is renumbered to follow it,
    /// and anything that named the deleted channel is dropped with it: a
    /// lane left pointing at index 3 would otherwise start automating
    /// whichever channel moved into that seat. `None` leaves the song
    /// untouched when the index does not exist or it is the last channel.
    pub fn remove_channel(&mut self, index: usize) -> Option<ProjectChannel> {
        if self.channels.len() <= 1 || index >= self.channels.len() {
            return None;
        }
        let removed = self.channels.remove(index);
        self.rescope_after(ChannelEdit::Removed(index as u8));
        self.selected_channel =
            (self.selected_channel as usize).min(self.channels.len() - 1) as u8;
        Some(removed)
    }

    /// Insert `channel` at `index` (clamped to the end), opening a gap. The
    /// newcomer's own references are pointed at its new index and every
    /// later channel's follow it up by one. Refused when the song is full.
    pub fn insert_channel(&mut self, index: usize, mut channel: ProjectChannel) -> Option<usize> {
        if self.channels.len() >= MAX_CHANNELS {
            return None;
        }
        let index = index.min(self.channels.len());
        let edit = ChannelEdit::Inserted(index as u8);
        self.rescope_after(edit);
        channel.rescope(index as u8);
        // The newcomer is not in the list yet, so `rescope_after` did not
        // reach it -- and unlike its own addresses, a subscription names
        // somebody else and has to follow the same shift everyone else did.
        channel.setup.source.rescope_subscription(edit);
        self.channels.insert(index, channel);
        Some(index)
    }

    /// Move the channel at `from` to `to`, carrying everything that named
    /// it.
    ///
    /// The third channel edit, and the one that could not be composed from
    /// the other two: `remove_channel` drops the departing channel's own
    /// routes and lanes on purpose, so a reorder built from a removal and an
    /// insertion would put the channel back with its own automation missing.
    ///
    /// `None` when either index is out of range or they are the same, which
    /// is the drag that landed where it started.
    pub fn move_channel(&mut self, from: usize, to: usize) -> Option<ChannelEdit> {
        let count = self.channels.len();
        if from >= count || to >= count || from == to {
            return None;
        }
        let channel = self.channels.remove(from);
        self.channels.insert(to, channel);
        let edit = ChannelEdit::Moved {
            from: from as u8,
            to: to as u8,
        };
        self.rescope_after(edit);
        // The selection is one more thing that named a channel. Nothing can
        // be dropped by a move, so this never has to clamp.
        self.selected_channel = edit.channel(self.selected_channel).unwrap_or(self.selected_channel);
        Some(edit)
    }

    /// Add a track, returning where it landed. Refused when the bank is full.
    ///
    /// Appends rather than inserting, because appending renumbers nothing. A
    /// user arranges the order afterwards with [`Self::move_track`].
    pub fn add_track(&mut self) -> Option<usize> {
        if self.buses.len() >= crate::MAX_BUSES {
            return None;
        }
        let index = self.buses.len();
        self.buses.push(crate::BusSetup::new(index));
        Some(index)
    }

    /// Make sure the bank has at least `count` tracks, adding plain ones to
    /// reach it. Returns how many there are now.
    ///
    /// Useful beyond tests: a hand-edited or future-format file can name a
    /// track it did not save, and materialising it is a kinder repair than
    /// dropping the routing that named it.
    pub fn ensure_tracks(&mut self, count: usize) -> usize {
        while self.buses.len() < count.min(crate::MAX_BUSES) {
            self.add_track();
        }
        self.buses.len()
    }

    /// Remove the track at `index`, closing the gap.
    ///
    /// Everything that named a later track is renumbered to follow it, and
    /// anything that named *this* one is dealt with rather than left dangling:
    /// a channel routed here falls back to the master, a track feeding here
    /// falls back to the master, and a lane or route scoped to its chain is
    /// dropped with the chain it drove.
    ///
    /// The master cannot be removed -- it is the sink every route reaches.
    pub fn remove_track(&mut self, index: usize) -> Option<crate::BusSetup> {
        if index == crate::MASTER_BUS as usize || index >= self.buses.len() {
            return None;
        }
        let removed = self.buses.remove(index);
        let edit = TrackEdit::Removed(index as u8);
        self.rescope_tracks_after(edit);
        Some(removed)
    }

    /// Move the track at `from` to `to`, carrying everything that named it.
    ///
    /// [`Self::move_channel`]'s twin, with one more refusal: **neither index
    /// may be the master.** The master is first by being bus 0 --
    /// `MixerBus::new` names it from its index and `is_legal_route` refuses it
    /// as a source by index -- so moving a track *into* seat 0 would displace
    /// it just as surely as moving the master out. Seat 0 is refused rather
    /// than clamped; clamping a drop is the interface's job.
    ///
    /// `None` when either index is out of range, is the master, or they are
    /// the same.
    pub fn move_track(&mut self, from: usize, to: usize) -> Option<TrackEdit> {
        if !track_move_allowed(self.buses.len(), from, to) {
            return None;
        }
        let track = self.buses.remove(from);
        self.buses.insert(to, track);
        let edit = TrackEdit::Moved {
            from: from as u8,
            to: to as u8,
        };
        self.rescope_tracks_after(edit);
        Some(edit)
    }

    /// Re-scope every track-addressed thing in the song after a track edit.
    ///
    /// Four kinds of address name a track: a channel's destination, a track's
    /// own destination, a track's **sends**, and anything scoped to a track's
    /// effect chain -- which is automation lanes and modulation routes, in any
    /// channel, because a track's chain can be automated from any channel's
    /// clip. A control binding on a track's strip or chain is a fifth, and
    /// follows too.
    fn rescope_tracks_after(&mut self, edit: TrackEdit) {
        for setup in &mut self.buses {
            setup.bus.output = edit.destination(setup.bus.output);
            // A send whose target went is **dropped**, where an output that
            // lost its target falls back to the master. `TrackEdit::track`
            // rather than `destination` is that difference: a producer with
            // nowhere to go must still be heard, and a send with nowhere to go
            // is simply not a send. Silently re-pointing it at the master
            // would put a wet path into the mix at full level.
            setup.sends.retain_mut(|send| match edit.track(send.target) {
                Some(target) => {
                    send.target = target;
                    true
                }
                None => false,
            });
        }
        for channel in &mut self.channels {
            channel.setup.channel.bus = edit.destination(channel.setup.channel.bus);
            channel.setup.modulation.rescope_tracks(edit);
            for lanes in &mut channel.automation {
                rescope_lanes_for_track(lanes, edit);
            }
        }
        self.control_map.rescope_tracks(edit);
        // A track that fed the removed one, or the removed one itself, may
        // have left the graph in a shape that no longer sorts.
        //
        // The repairs are dropped here and not logged, which is deliberate
        // rather than the omission it looks like: a removal only ever
        // *removes* edges and a move only *relabels* them, so neither can
        // introduce a cycle, and the rescope above has already re-pointed or
        // dropped everything that named a track. Anything this finds is a bug
        // in `TrackEdit`, and the place that reports a repaired bank to the
        // user is the load path, where the bank came from a file rather than
        // from this program.
        self.buses = crate::sanitize_bank(&self.buses).buses;
    }

    /// The audio edges this project's channels compile to, and the order
    /// that satisfies them.
    ///
    /// Derived rather than tracked, for the reason `Session::latency_plan`
    /// gives: the answer is a property of every channel at once, so a flag
    /// each edit had to remember to set is a list that grows silently. An
    /// offline render compiles its own through here, which is what keeps an
    /// export and a live take rendering in the same order.
    pub fn audio_graph(&self) -> crate::CompiledAudioGraph {
        let count = self.channels.len().min(MAX_CHANNELS);
        let mut subscriptions = [None; MAX_CHANNELS];
        let mut published: [&'static [crate::OutletDescriptor]; MAX_CHANNELS] = [&[]; MAX_CHANNELS];
        for (index, channel) in self.channels.iter().take(count).enumerate() {
            subscriptions[index] = channel.setup.source.audio_subscription();
            published[index] = crate::PublishesOutlets::outlets(&channel.setup.source.kind());
        }
        crate::compile_audio_graph(&subscriptions[..count], &published[..count])
    }

    fn rescope_after(&mut self, edit: ChannelEdit) {
        for channel in &mut self.channels {
            // An Aux In's subscription is one more channel-scoped address and
            // moves through this pass rather than growing a repair path of
            // its own.
            channel.setup.source.rescope_subscription(edit);
            channel.setup.modulation.rescope_channels(edit);
            for lanes in &mut channel.automation {
                rescope_lanes(lanes, edit);
            }
        }
        self.control_map.rescope_channels(edit);
    }

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

        let mut project = Self {
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
                automation: vec![Vec::new()],
                next_note_id: 1,
            })
            .collect(),
            buses: starter_tracks(),
            // The first two bars, marked and switched off. The strip above
            // the bar numbers is where a loop is made and nothing says so, so
            // a new song arrives with one already drawn on it -- close enough
            // to grab an end of, and inert until the toggle is pressed.
            loop_range: LoopRange::marked(0, STARTER_LOOP_BARS * TICKS_PER_BAR)
                .unwrap_or_default(),
            ..Self::default()
        };
        // Four drum channels onto one track, which is the grouping: it is the
        // `bus` field several channels share and needs no other concept.
        for channel in &mut project.channels {
            channel.setup.channel.bus = DRUM_TRACK;
        }
        project
    }
}

/// The track a starter kit's drums are grouped onto.
const DRUM_TRACK: u8 = 1;

/// The track the starter kit's second voice would land on.
const BASS_TRACK: u8 = 2;

/// The track the starter kit's two others send to, which is what makes it a
/// return. Nothing about the track itself says so.
const REVERB_TRACK: u8 = 3;

/// The tracks a new song opens with.
///
/// Adam's sketch, and the reason he wanted channel grouping at all: *"a drum
/// kit, grouped, sent to mixer track 1, and then a monosynth or something on
/// mixer 2, and maybe one track set up as a reverb send -- a reasonable,
/// modest default that sort of also demonstrates what can be done just by
/// already having had it done to it."*
///
/// A blank project teaches nothing; this one shows a group, a bus and a send
/// by having already done them.
///
/// **Reverb is not a fourth kind of track.** It is an ordinary track with a
/// Reverb device on it that two other tracks send to, which is what makes it
/// an effects return -- `docs/TERMINOLOGY.md`. Nothing here creates a "send"
/// or a "return"; two tracks route to a third and the third is thereby one.
///
/// The send is post-fader, so pulling Drums down takes its reverb with it,
/// and the device is fully wet, because the dry path is already in the mix
/// through each track's own output. Turning the wet/dry knob down on it would
/// be the mistake the arrangement exists to avoid.
fn starter_tracks() -> Vec<crate::BusSetup> {
    let mut tracks = default_buses();
    for name in ["Drums", "Bass", "Reverb"] {
        let mut track = crate::BusSetup::new(tracks.len());
        track.bus.name = name.into();
        tracks.push(track);
    }
    tracks[REVERB_TRACK as usize].push_effect(crate::EffectSlotState {
        id: crate::DeviceId::default(),
        params: crate::EffectParams::Reverb(crate::ReverbParams::default()),
        bypassed: false,
        // Fully wet: the dry signal reaches the master by each track's own
        // output, so a return that passed any of it through would double it.
        wet_dry: 1.0,
        input_trim: 1.0,
        output_trim: 1.0,
    });
    for track in [DRUM_TRACK, BASS_TRACK] {
        tracks[track as usize]
            .sends
            .push(crate::AuxSend::new(REVERB_TRACK));
    }
    tracks
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

    /// Both halves of the retirement, in the units a musician would check.
    ///
    /// `mooloop-project` proves the on-disk half; this proves the arithmetic,
    /// and it covers the **route**, which the file test cannot reach without
    /// two substitutions on one key. A route's depth is a signed fraction of
    /// the destination's range, so it converts by the ratio of the two ranges
    /// *and turns round*: more offset was further back, more position is
    /// further forward.
    #[test]
    fn the_retired_offset_migrates_to_position_in_both_a_lane_and_a_route() {
        use crate::modulation::{ModPolarity, ModRoute};
        use crate::{AutomationLane, AutomationPoint, EffectTarget, ParamAddr, ParamOwner};

        let mut project = Project::default();
        let mut slot = crate::EffectSlotState::of_kind(crate::EffectKind::Buffer);
        slot.params = crate::EffectParams::Buffer(crate::BufferParams {
            bars: 8,
            ..Default::default()
        });
        project.channels[0].setup.push_effect(slot);
        project.channels[0].setup.assign_device_ids();
        let device = project.channels[0].setup.effects[0].id;
        let address = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Effect { device },
            param: crate::BUFFER_PARAM_OFFSET_BEATS,
        };

        project.channels[0].normalize_automation();
        let mut lane = AutomationLane::new(address);
        lane.reserve_points();
        // Live, one beat back, and four beats back: 0, 1/16 and 4/16 of the
        // old 0..16 beat range.
        lane.reset_points([
            AutomationPoint::new(1, 0, 0.0),
            AutomationPoint::new(2, 24, 0.0625),
            AutomationPoint::new(3, 48, 0.25),
        ]);
        project.channels[0].automation[0].push(lane);
        // Written straight into the row rather than through `add_route`,
        // which stamps a durable source identity out of an installed module.
        // The migration does not look at a route's source, and giving this
        // one a module would be setting up the half that is not under test.
        project.channels[0].setup.modulation.routes[0] =
            Some(ModRoute::to_slot(0, address, 0.5, ModPolarity::Bipolar));

        project.migrate_retired_buffer_offset();

        let lane = &project.channels[0].automation[0][0];
        assert_eq!(lane.target.param, crate::BUFFER_PARAM_POSITION);
        let values: Vec<f32> = lane.points().iter().map(|point| point.value).collect();
        // An eight-bar ring is 32 beats. Live is 1; one beat back is 31/32;
        // four beats back is 28/32.
        assert!(
            (values[0] - 1.0).abs() < 1e-5
                && (values[1] - 31.0 / 32.0).abs() < 1e-5
                && (values[2] - 28.0 / 32.0).abs() < 1e-5,
            "the lane converted to {values:?}"
        );
        assert!(
            values[0] > values[1] && values[1] > values[2],
            "the axis has to turn round: more offset was further back, and \
             more position is further forward"
        );

        let route = project.channels[0].setup.modulation.routes[0]
            .expect("the route is still there");
        assert_eq!(route.destination.param, crate::BUFFER_PARAM_POSITION);
        // Half of sixteen beats is eight, and eight of thirty-two is a
        // quarter -- pointing the other way.
        assert!(
            (route.depth + 0.25).abs() < 1e-5,
            "the route depth converted to {}",
            route.depth
        );

        // Idempotent: id 0 is spent, so a second pass finds nothing.
        let before = project.clone();
        project.migrate_retired_buffer_offset();
        assert_eq!(project, before, "running it twice must change nothing");
    }

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

        let poly = ChannelSetup::poly_synth("Poly");
        assert_eq!(poly.kind(), DeviceKind::PolySynth);
        assert!(poly.poly_synth_state().is_some());
        assert!(poly.sampler_state().is_none());
    }

    /// Every channel-scoped address in the song has to follow its channel
    /// through a deletion and an insertion, and the deleted channel's own
    /// have to go with it -- otherwise a lane on channel 3 starts driving
    /// whichever channel moves into seat 3.
    #[test]
    fn channel_edits_renumber_every_address_that_named_a_channel() {
        let mut project = Project::default();
        for index in 1..4 {
            project.channels.push(ProjectChannel::mlm1(index, 1));
        }
        let strip = |channel: u8| crate::ParamAddr::strip(
            crate::EffectTarget::Channel(channel),
            crate::STRIP_PARAM_VOLUME,
        );
        let bus = crate::ParamAddr::strip(crate::EffectTarget::Bus(2), crate::STRIP_PARAM_PAN);
        for index in 0..4u8 {
            let channel = &mut project.channels[index as usize];
            channel.setup.modulation.install(0, crate::ModulatorParams::Lfo(Default::default()));
            channel
                .setup
                .modulation
                .add_route(crate::ModRoute::to_slot(0, strip(index), 0.5, Default::default()))
                .unwrap();
            channel.automation[0].push(AutomationLane::new(strip(index)));
            channel.automation[0].push(AutomationLane::new(bus));
            // A desk fader per channel, told apart by its controller number,
            // which is the channel it was learned on.
            channel.setup.channel.name = format!("ch{index}");
            project.control_map.bind(fader_on(index, strip(index)));
        }
        // Each binding still moves the channel it was learned on, by name, and
        // the one learned on a removed channel is gone.
        let bindings_follow = |project: &Project, gone: &[u8]| {
            let mut learned = Vec::new();
            for binding in &project.control_map.bindings {
                let (controller, crate::EffectTarget::Channel(seat)) =
                    (controller_of(binding), bound_scope(binding))
                else {
                    panic!("a channel binding became {:?}", binding.target);
                };
                assert_eq!(
                    project.channels[seat as usize].setup.channel.name,
                    format!("ch{controller}"),
                    "the fader learned on ch{controller} now moves seat {seat}"
                );
                learned.push(controller);
            }
            learned.sort_unstable();
            let expected: Vec<u8> = (0..4).filter(|index| !gone.contains(index)).collect();
            assert_eq!(learned, expected);
        };

        let removed = project.remove_channel(1).expect("channel 1 exists");
        assert_eq!(project.channels.len(), 3);
        assert_eq!(removed.setup.modulation.routes[0].unwrap().destination, strip(1));
        for index in 0..3u8 {
            let channel = &project.channels[index as usize];
            assert_eq!(
                channel.setup.modulation.routes[0].unwrap().destination,
                strip(index),
                "route on channel {index}"
            );
            assert_eq!(channel.automation[0][0].target, strip(index), "lane on channel {index}");
            assert_eq!(channel.automation[0][1].target, bus, "bus lane on channel {index}");
        }
        bindings_follow(&project, &[1]);

        // Putting it back at the front renumbers everyone again, and the
        // newcomer's addresses point at its new seat rather than its old one.
        assert_eq!(project.insert_channel(0, removed), Some(0));
        for index in 0..4u8 {
            let channel = &project.channels[index as usize];
            assert_eq!(
                channel.setup.modulation.routes[0].unwrap().destination,
                strip(index),
                "route on channel {index} after insert"
            );
            assert_eq!(channel.automation[0][0].target, strip(index));
        }
        bindings_follow(&project, &[1]);
        assert!(project.remove_channel(9).is_none());

        // And a move, which is the edit neither of the two above can
        // express. Channel 0 also subscribes to channel 3's outlet, so the
        // one address that names *another* channel rides along too.
        project.channels[0].setup.source = ChannelSource::AuxIn(Default::default());
        project.channels[0]
            .setup
            .source
            .aux_in_state_mut()
            .expect("aux in")
            .params
            .source_channel = 3;

        let moved = project.channels[3].setup.channel.name.clone();
        assert_eq!(
            project.move_channel(3, 1),
            Some(ChannelEdit::Moved { from: 3, to: 1 })
        );
        assert_eq!(project.channels[1].setup.channel.name, moved);
        for index in 0..4u8 {
            let channel = &project.channels[index as usize];
            assert_eq!(
                channel.setup.modulation.routes[0].unwrap().destination,
                strip(index),
                "route on channel {index} after move"
            );
            assert_eq!(channel.automation[0][0].target, strip(index));
            assert_eq!(channel.automation[0][1].target, bus);
        }
        bindings_follow(&project, &[1]);
        // The subscription followed the channel it named, which is now in
        // seat 1 -- not seat 3, where a stranger is sitting.
        assert_eq!(
            project.channels[0]
                .setup
                .source
                .aux_in_state()
                .expect("aux in")
                .params
                .source_channel,
            1
        );

        // A move that lands where it started, or names a seat that is not
        // there, is not an edit.
        assert!(project.move_channel(2, 2).is_none());
        assert!(project.move_channel(0, 9).is_none());
        assert!(project.move_channel(9, 0).is_none());
    }

    fn fader_on(controller: u8, address: crate::ParamAddr) -> crate::ControlBinding {
        crate::ControlBinding::new(
            crate::ControlSource::Cc {
                port: Default::default(),
                channel: Default::default(),
                controller,
            },
            crate::ControlTarget::Param(address),
        )
    }

    fn controller_of(binding: &crate::ControlBinding) -> u8 {
        match binding.source {
            crate::ControlSource::Cc { controller, .. } => controller,
            ref other => panic!("not a test fader: {other:?}"),
        }
    }

    fn bound_scope(binding: &crate::ControlBinding) -> crate::EffectTarget {
        match binding.target {
            crate::ControlTarget::Param(address) => address.scope,
            other => panic!("not a parameter binding: {other:?}"),
        }
    }

    /// Every address that names a track has to follow it through a move, and
    /// fall back or go through a removal -- the track list's twin of the test
    /// above, and the first test `remove_track` has had.
    ///
    /// Addresses are checked by the *name* of the track they reach, because a
    /// name is carried by the track and a seat is not.
    #[test]
    fn track_edits_renumber_every_address_that_named_a_track() {
        let mut project = Project::default();
        project.channels.push(ProjectChannel::mlm1(1, 1));
        project.buses.truncate(1);
        assert_eq!(project.ensure_tracks(5), 5);
        let names: Vec<String> = project.buses.iter().map(|setup| setup.bus.name.clone()).collect();
        assert_eq!(names[3], "Bus 3");
        let name_of = |project: &Project, seat: u8| project.buses[seat as usize].bus.name.clone();
        let volume = |track: u8| {
            crate::ParamAddr::strip(crate::EffectTarget::Bus(track), crate::STRIP_PARAM_VOLUME)
        };

        // Track 3 is named by every kind of address there is.
        project.channels[0].setup.channel.bus = 3;
        project.channels[1].setup.channel.bus = 1;
        project.buses[4].bus.output = 3;
        project.buses[2].sends.push(crate::AuxSend::new(3));
        project.buses[4].sends.push(crate::AuxSend::new(2));
        project.channels[0].automation[0].push(AutomationLane::new(volume(3)));
        project.channels[1].automation[0].push(AutomationLane::new(volume(1)));
        let rack = &mut project.channels[0].setup.modulation;
        rack.install(0, crate::ModulatorParams::Lfo(Default::default()));
        rack.add_route(crate::ModRoute::to_slot(0, volume(3), 0.5, Default::default()))
            .unwrap();
        project.control_map.bind(fader_on(3, volume(3)));
        project.control_map.bind(fader_on(1, volume(1)));
        project.control_map.bind(fader_on(
            100,
            crate::ParamAddr::strip(crate::EffectTarget::Channel(0), crate::STRIP_PARAM_VOLUME),
        ));

        let edges = |project: &Project| {
            let mut edges: Vec<(String, String)> = Vec::new();
            for setup in project.buses.iter().skip(1) {
                let from = setup.bus.name.clone();
                edges.push((from.clone(), name_of(project, setup.bus.output)));
                for send in &setup.sends {
                    edges.push((from.clone(), name_of(project, send.target)));
                }
            }
            edges.sort();
            edges
        };
        let before = edges(&project);
        assert!(crate::compile_bus_graph(&project.buses).is_some());

        assert_eq!(project.move_track(3, 1), Some(TrackEdit::Moved { from: 3, to: 1 }));
        let seats: Vec<String> = project.buses.iter().map(|setup| setup.bus.name.clone()).collect();
        assert_eq!(seats, ["Master", "Bus 3", "Bus 1", "Bus 2", "Bus 4"]);
        let bus_3 = |seat: u8| name_of(&project, seat) == "Bus 3";

        assert!(bus_3(project.channels[0].setup.channel.bus), "a channel routed to it");
        assert_eq!(name_of(&project, project.channels[1].setup.channel.bus), "Bus 1");
        assert!(bus_3(project.buses[4].bus.output), "a track outputting to it");
        assert!(bus_3(project.buses[3].sends[0].target), "a send to it");
        assert_eq!(name_of(&project, project.buses[4].sends[0].target), "Bus 2");
        let scope_of = |address: crate::ParamAddr| match address.scope {
            crate::EffectTarget::Bus(seat) => seat,
            other => panic!("a track address became {other:?}"),
        };
        assert!(bus_3(scope_of(project.channels[0].automation[0][0].target)), "a lane on it");
        assert_eq!(
            name_of(&project, scope_of(project.channels[1].automation[0][0].target)),
            "Bus 1"
        );
        assert!(
            bus_3(scope_of(project.channels[0].setup.modulation.routes[0].unwrap().destination)),
            "a route on it"
        );
        for binding in &project.control_map.bindings {
            match (controller_of(binding), bound_scope(binding)) {
                (100, scope) => assert_eq!(scope, crate::EffectTarget::Channel(0)),
                (controller, crate::EffectTarget::Bus(seat)) => {
                    assert_eq!(name_of(&project, seat), format!("Bus {controller}"), "a binding")
                }
                (_, other) => panic!("a track binding became {other:?}"),
            }
        }

        // A move relabels edges and neither adds nor removes one, so the graph
        // still sorts and there is nothing to repair.
        assert_eq!(edges(&project), before);
        assert!(crate::compile_bus_graph(&project.buses).is_some());
        assert!(crate::sanitize_bank(&project.buses).repairs.is_empty());

        // Now remove it, from its new seat. Everything that named it falls
        // back or goes, as `remove_track` documents, and everything that
        // named a later track follows it down.
        let removed = project.remove_track(1).expect("track 1 exists");
        assert_eq!(removed.bus.name, "Bus 3");
        assert_eq!(project.channels[0].setup.channel.bus, crate::MASTER_BUS, "falls back");
        assert_eq!(name_of(&project, project.channels[1].setup.channel.bus), "Bus 1");
        assert_eq!(project.buses[3].bus.output, crate::MASTER_BUS, "falls back");
        assert!(project.buses[2].sends.is_empty(), "a send to it is dropped");
        assert_eq!(name_of(&project, project.buses[3].sends[0].target), "Bus 2");
        assert_eq!(project.channels[0].automation[0].len(), 0, "its lane goes");
        assert_eq!(
            name_of(&project, scope_of(project.channels[1].automation[0][0].target)),
            "Bus 1"
        );
        assert!(project.channels[0].setup.modulation.routes[0].is_none(), "its route goes");
        let mut learned: Vec<u8> = project.control_map.bindings.iter().map(controller_of).collect();
        learned.sort_unstable();
        assert_eq!(learned, [1, 100], "its binding goes");
        for binding in &project.control_map.bindings {
            if let crate::EffectTarget::Bus(seat) = bound_scope(binding) {
                assert_eq!(name_of(&project, seat), "Bus 1");
            }
        }
        assert!(crate::sanitize_bank(&project.buses).repairs.is_empty());
        assert!(project.remove_track(0).is_none(), "the master stays");
        assert!(project.remove_track(9).is_none());
    }

    /// The master is first by being bus 0, so a move may neither take it out
    /// of seat 0 nor put anything else in. A refused move is not an edit: the
    /// project is exactly as it was.
    #[test]
    fn a_track_move_that_would_displace_the_master_is_refused() {
        let mut project = Project::default();
        project.buses.truncate(1);
        project.ensure_tracks(4);
        project.buses[2].sends.push(crate::AuxSend::new(3));
        let before = project.clone();
        for (from, to) in [(0, 2), (2, 0), (2, 2), (9, 1), (1, 9), (0, 0)] {
            assert_eq!(project.move_track(from, to), None, "move {from} -> {to}");
            assert_eq!(project, before, "move {from} -> {to} changed the song");
        }
        // The last seat is a legal landing.
        assert!(project.move_track(1, 3).is_some());
    }

    #[test]
    fn project_round_trip_keeps_channel_modulation() {
        let mut project = Project::default();
        let rack = &mut project.channels[0].setup.modulation;
        rack.install(0, crate::ModulatorParams::Lfo(crate::ModLfoParams {
            rate_hz: 2.5,
            ..crate::ModLfoParams::default()
        }));
        assert!(rack
            .add_route(crate::ModRoute::to_slot(
                0,
                crate::ParamAddr::effect(
                    crate::EffectTarget::Channel(0),
                    crate::DeviceId(0),
                    crate::FILTER_PARAM_CUTOFF_HZ,
                ),
                0.3,
                crate::ModPolarity::Bipolar,
            ))
            .is_some());

        let text = toml::to_string(&project).unwrap();
        assert_eq!(toml::from_str::<Project>(&text).unwrap(), project);
    }

    /// An Aux In's subscription is the one generator parameter that names
    /// another channel, so it is the one whose round trip is worth asserting
    /// on its own: an edge that survived a save as "no source" would be a
    /// silent channel with no error anywhere.
    ///
    /// The other half is the format's defaulted-field rule: a manifest
    /// written before Aux In existed has no `aux_in` channel at all, and one
    /// whose `state.params` is empty loads as an Aux In subscribed to
    /// nothing — silent rather than invalid.
    #[test]
    fn an_aux_in_subscription_survives_a_round_trip_and_defaults_when_absent() {
        let mut project = Project::default();
        project.channels.push(ProjectChannel::aux_in_with_params(
            1,
            1,
            crate::AuxInParams {
                source_channel: 0,
                source_outlet: crate::mlp8::OUTLET_OSC3,
                level: 0.5,
            },
        ));
        let text = toml::to_string(&project).unwrap();
        let reloaded: Project = toml::from_str(&text).unwrap();
        assert_eq!(reloaded, project);
        assert_eq!(
            reloaded.channels[1].setup.source.audio_subscription(),
            Some(crate::AudioSubscription::new(0, crate::mlp8::OUTLET_OSC3))
        );

        let bare: ChannelSource =
            toml::from_str("type = \"aux_in\"\n[state.params]\n").expect("an empty Aux In loads");
        assert_eq!(bare.kind(), DeviceKind::AuxIn);
        assert!(bare.audio_subscription().is_none());
    }
}
