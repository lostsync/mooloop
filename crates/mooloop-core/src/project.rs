//! Canonical editable project state shared by UI, persistence, and rendering.

use std::path::PathBuf;

use crate::structure::{
    mint_channel_id, rescope_lanes, rescope_lanes_for_track, ChannelEdit, TrackEdit,
};
use crate::{
    default_buses, BusSetup, Channel, ChannelId, DeviceKind, Ds01Params, DrumMode,
    DrumSynthParams, EffectTarget,
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
    /// How the Record page records (`audio-recording/05`). Omitted when it
    /// holds the default, so a sampler saved before it is byte-identical.
    #[serde(default, skip_serializing_if = "crate::SamplerRecord::is_default")]
    pub record: crate::SamplerRecord,
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
    /// This channel's durable identity, minted from [`Project::next_channel_id`].
    ///
    /// Defaulted and skipped when unassigned, so a song written before
    /// channels had identities is byte-identical to one saved now with none;
    /// [`Project::assign_channel_ids`] gives such a song id = position on the
    /// way in, which is what everything that said `channel = 3` already meant.
    #[serde(default, skip_serializing_if = "crate::effect::channel_id_is_unassigned")]
    pub id: ChannelId,
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
            id: ChannelId::UNASSIGNED,
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
            id: ChannelId::UNASSIGNED,
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
            id: ChannelId::UNASSIGNED,
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
            id: ChannelId::UNASSIGNED,
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
            id: ChannelId::UNASSIGNED,
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
            id: ChannelId::UNASSIGNED,
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
            id: ChannelId::UNASSIGNED,
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
            id: ChannelId::UNASSIGNED,
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
    /// Which curve this song's saved strip-volume values -- lane points,
    /// binding ranges, route depths, all normalized -- were written against.
    /// [`STRIP_VOLUME_TAPER_LINEAR`] is what a song written before the field
    /// existed decodes to, and [`Self::migrate_linear_strip_volume`] converts
    /// one to [`STRIP_VOLUME_TAPER`] on load. The descriptor id did not move,
    /// so without this marker a converted song and an old one are the same
    /// bytes, and the pass could not be idempotent (MOO-131).
    #[serde(default)]
    pub strip_volume_taper: u8,
    /// The channel the interface is editing, as an identity rather than a
    /// seat.
    ///
    /// **This is why the identity exists**, in its smallest form. As a
    /// position it had to be renumbered by every structural edit, and two of
    /// the three did it wrong: `insert_channel` never touched it at all, so
    /// adding a channel above the selected one silently moved the selection
    /// to its neighbour, and `remove_channel` *clamped* rather than followed,
    /// which is only accidentally right when the selection is at the end.
    /// As an identity there is nothing to renumber and nothing to get wrong.
    ///
    /// Serialized as a bare number and read back as one, so a song written
    /// before this decodes its old index as an id -- which names the same
    /// channel, because a bank with no identities takes its positions.
    /// Resolve it through [`Self::selected_index`] rather than by hand.
    pub selected_channel: ChannelId,
    pub channels: Vec<ProjectChannel>,
    /// The mint [`ProjectChannel::id`] comes from, with the same defaulting
    /// as `ChannelSetup::next_device_id`: a song written before channels had
    /// identities decodes with a zero here, and
    /// [`Self::assign_channel_ids`] raises it past whatever it hands out.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub next_channel_id: u32,
    /// Mixer buses, master first. Defaulted on load so songs written before
    /// the mixer existed get the master and nothing else -- a bank is the
    /// tracks somebody made, and `default_buses` stopped returning seventeen
    /// of them when the mixer became a list rather than a fixed bank.
    #[serde(default = "default_buses")]
    pub buses: Vec<BusSetup>,
    /// The mint [`BusSetup::id`] comes from, defaulted and raised exactly as
    /// [`Self::next_channel_id`] is -- see [`Self::assign_track_ids`].
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub next_track_id: u32,
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
    /// The song's hosted plugins, by slot: what each one is, its parameters
    /// as it last reported them, and its saved state
    /// (`docs/plans/plugin-hosting/02-the-neutral-contract.md`).
    ///
    /// A device on a chain names its slot through `EffectParams::Plugin`.
    /// Keyed per project rather than per chain so that moving a device to
    /// another channel renumbers nothing.
    ///
    /// Defaulted and skipped when empty, so a song with no plugins is
    /// byte-identical to one written before the field existed. The format
    /// stays at version 1: an older reader ignores this table, and refuses
    /// the song at the first `EffectParams` tag it does not know
    /// (`PROJECT_FORMAT.md`, "Hosted plugins").
    #[serde(
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty",
        with = "crate::plugin::slot_table"
    )]
    pub plugins: crate::plugin::PluginSlots,
    /// The mint [`crate::PluginSlotId`] comes from. Never lowered, so a slot
    /// id is never reused: an address left holding a removed plugin's slot
    /// names nothing rather than whatever took its number.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub next_plugin_slot: u32,
}

/// [`Project::strip_volume_taper`] for a song whose strip volume was
/// `ParamCurve::Linear` over `0..MAX_LINEAR_GAIN`: every song written before
/// 2026-09-23.
pub const STRIP_VOLUME_TAPER_LINEAR: u8 = 0;

/// [`Project::strip_volume_taper`] for the strip volume's present curve,
/// `ParamCurve::Fader` over `0..FADER_MAX_GAIN`.
pub const STRIP_VOLUME_TAPER: u8 = 1;

/// A normalized strip-volume value written under the Linear curve, as fader
/// travel. Anything above the fader's +6 dB ceiling lands at full throw,
/// which is the most the descriptor can now reach.
fn linear_volume_as_fader(value: f32) -> f32 {
    crate::gain::fader_gain_to_position(value.clamp(0.0, 1.0) * crate::gain::MAX_LINEAR_GAIN)
}

fn is_empty_control_map(map: &crate::control::ControlMap) -> bool {
    map.bindings.is_empty()
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
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
            + self
                .plugins
                .values()
                .map(|slot| std::mem::size_of::<crate::PluginSlotState>() + slot.heap_bytes())
                .sum::<usize>()
    }

    /// Add `slot` to the song under a fresh [`crate::PluginSlotId`] and
    /// return the id. Ids come from [`Self::next_plugin_slot`] and are never
    /// reused. The mint skips any id already in the table, so a song whose
    /// counter was lost or hand-edited below its table cannot hand out a
    /// slot that is taken.
    pub fn add_plugin_slot(&mut self, slot: crate::PluginSlotState) -> crate::PluginSlotId {
        crate::plugin::mint_plugin_slot(&mut self.plugins, &mut self.next_plugin_slot, slot)
    }
}

impl ProjectChannel {
    /// This channel wearing `id`. For the constructors, which build a channel
    /// before it has joined a song and so cannot know one.
    pub fn with_id(mut self, id: ChannelId) -> Self {
        self.id = id;
        self
    }

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
            strip_volume_taper: STRIP_VOLUME_TAPER,
            selected_channel: ChannelId(0),
            channels: vec![ProjectChannel::sampler(0, 1).with_id(ChannelId(0))],
            next_channel_id: 1,
            buses: default_buses(),
            next_track_id: 1,
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
            plugins: crate::plugin::PluginSlots::new(),
            next_plugin_slot: 0,
        }
    }
}

impl Project {
    /// Duplicate pattern `index` immediately after itself: its length, its
    /// name and colour, and every channel's notes and automation for it.
    /// Playlist placements keep pointing at the same *content*, so every
    /// index past `index` shifts up one to follow the insertion, and the copy
    /// becomes the current pattern. Returns `false`, changing nothing, when
    /// the bank is full or `index` is not a pattern.
    ///
    /// One place for every list that runs parallel to the bank, because the
    /// lists are only right while they move together. The interface did this
    /// inline and left `pattern_meta` out, so cloning a pattern gave the copy
    /// the *next* pattern's name, and every name after it slid one pattern
    /// along.
    pub fn clone_pattern(&mut self, index: usize) -> bool {
        if self.pattern_lengths.len() >= crate::MAX_PATTERNS || index >= self.pattern_lengths.len()
        {
            return false;
        }
        let length = self.pattern_lengths[index];
        self.pattern_lengths.insert(index + 1, length);
        for channel in &mut self.channels {
            if let Some(notes) = channel.notes.get(index).cloned() {
                channel.notes.insert(index + 1, notes);
            }
            if let Some(automation) = channel.automation.get(index).cloned() {
                channel.automation.insert(index + 1, automation);
            }
        }
        // A list that stops short of `index` holds nothing for it or for
        // anything after it: every entry past its end is the empty default
        // (`trim_pattern_meta`), so there is nothing to copy or to shift.
        if let Some(meta) = self.pattern_meta.get(index).cloned() {
            self.pattern_meta.insert(index + 1, meta);
        }
        for placement in &mut self.playlist {
            if usize::from(placement.pattern) > index {
                placement.pattern += 1;
            }
        }
        self.current_pattern = (index + 1) as u16;
        true
    }

    /// Remove pattern `index`, with its name and colour and every channel's
    /// notes and automation for it. Its playlist placements go; later ones
    /// shift down one to keep pointing at the same content. Returns `false`,
    /// changing nothing, when `index` is not a pattern or is the only one.
    ///
    /// The other half of [`Self::clone_pattern`], and it had the same hole:
    /// the name of the pattern removed passed to the one after it.
    pub fn remove_pattern(&mut self, index: usize) -> bool {
        if self.pattern_lengths.len() <= 1 || index >= self.pattern_lengths.len() {
            return false;
        }
        self.pattern_lengths.remove(index);
        for channel in &mut self.channels {
            if index < channel.notes.len() {
                channel.notes.remove(index);
            }
            if index < channel.automation.len() {
                channel.automation.remove(index);
            }
        }
        if index < self.pattern_meta.len() {
            self.pattern_meta.remove(index);
        }
        self.playlist
            .retain(|placement| usize::from(placement.pattern) != index);
        for placement in &mut self.playlist {
            if usize::from(placement.pattern) > index {
                placement.pattern -= 1;
            }
        }
        self.current_pattern = index.min(self.pattern_lengths.len() - 1) as u16;
        true
    }

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

    /// Give every channel its identity, and put the mint past them all.
    ///
    /// [`crate::assign_device_ids`] for channels, and a no-op on a song that
    /// already has them rather than a migration. A song written before
    /// channels had identities holds none, so each takes its **position** --
    /// which is what every saved address that said `channel = 3` already
    /// meant, so an older song loads pointing where it pointed and
    /// `FORMAT_VERSION` does not move.
    ///
    /// A part-assigned bank is a hand-edited file rather than anything this
    /// code can produce; its unassigned channels are minted fresh instead of
    /// taking positions that could collide with an id already in use.
    ///
    /// Call it after decode and before anything resolves a channel address.
    pub fn assign_channel_ids(&mut self) {
        if !self.channels.iter().any(|channel| channel.id.is_assigned()) {
            for (index, channel) in self.channels.iter_mut().enumerate() {
                channel.id = ChannelId(index as u32);
            }
        }
        let highest = self
            .channels
            .iter()
            .filter(|channel| channel.id.is_assigned())
            .map(|channel| channel.id.0)
            .max();
        if let Some(highest) = highest {
            self.next_channel_id = self.next_channel_id.max(highest.saturating_add(1));
        }
        for index in 0..self.channels.len() {
            if !self.channels[index].id.is_assigned() {
                self.channels[index].id = mint_channel_id(&mut self.next_channel_id);
            }
        }
        // Every channel now has an identity, which is the precondition for
        // anything *naming* a channel to take one. Done here rather than at
        // the caller so that the two cannot come apart: a load that assigned
        // ids and forgot to identify the references would leave the
        // references positional and silently so.
        self.identify_channel_references();
    }

    /// Give every track an identity, and put the mint past them all.
    ///
    /// [`Self::assign_channel_ids`] for the track list, by the same rules: a
    /// bank with no identities takes its positions, the mint is raised past
    /// the highest id held, and any track still unassigned after that -- a
    /// hand-edited file, or a master `sanitize_bank` had to restore -- is
    /// minted fresh. Nothing *names* a track by id yet, so unlike the channel
    /// pass there are no references to identify afterwards.
    pub fn assign_track_ids(&mut self) {
        if !self.buses.iter().any(|track| track.id.is_assigned()) {
            for (index, track) in self.buses.iter_mut().enumerate() {
                track.id = crate::TrackId(index as u32);
            }
        }
        let highest = self
            .buses
            .iter()
            .filter(|track| track.id.is_assigned())
            .map(|track| track.id.0)
            .max();
        if let Some(highest) = highest {
            self.next_track_id = self.next_track_id.max(highest.saturating_add(1));
        }
        for track in &mut self.buses {
            if !track.id.is_assigned() {
                track.id = crate::mint_track_id(&mut self.next_track_id);
            }
        }
    }

    /// Where the track with `id` sits, or `None` when no track has it.
    pub fn track_index(&self, id: crate::TrackId) -> Option<usize> {
        if !id.is_assigned() {
            return None;
        }
        self.buses.iter().position(|track| track.id == id)
    }

    /// Give an identity to every field that names another channel by seat and
    /// does not have one yet.
    ///
    /// Two fields reach here -- an Aux In's subscription and an envelope
    /// gate's input -- and both keep their seat as well, for reasons of their
    /// own written where they are declared. This is the *adopt* half; the
    /// seat is recomputed from the identity by
    /// [`Self::reseat_channel_references`].
    ///
    /// Idempotent, and deliberately harmless to call too often: a reference
    /// that already has an identity is left alone. Calling it too *rarely* is
    /// the failure this design guards against, which is why
    /// [`Self::assign_channel_ids`] ends with it and every structural edit
    /// runs it again.
    pub fn identify_channel_references(&mut self) {
        let ids: Vec<ChannelId> = self.channels.iter().map(|channel| channel.id).collect();
        let id_at = |seat: u8| ids.get(usize::from(seat)).copied();
        for channel in &mut self.channels {
            if let Some(state) = channel.setup.source.aux_in_state_mut() {
                state.params.identify(id_at);
            }
            channel.setup.modulation.identify_gates(id_at);
        }
    }

    /// Point every identified cross-channel reference at the seat its channel
    /// now occupies. Returns whether anything moved.
    ///
    /// The *reseat* half of [`Self::identify_channel_references`], and the
    /// reason the two exist as a pair rather than as one function: adoption
    /// reads the seat and writes the identity, reseating reads the identity
    /// and writes the seat, and running them in that order after a structural
    /// edit is correct whichever of the two a given reference needed.
    pub fn reseat_channel_references(&mut self) -> bool {
        let seats: Vec<(ChannelId, u8)> = self
            .channels
            .iter()
            .enumerate()
            .filter_map(|(index, channel)| Some((channel.id, u8::try_from(index).ok()?)))
            .collect();
        let seat_of = |id: ChannelId| {
            if !id.is_assigned() {
                return None;
            }
            seats
                .iter()
                .find(|(held, _)| *held == id)
                .map(|(_, seat)| *seat)
        };
        let mut changed = false;
        for channel in &mut self.channels {
            if let Some(state) = channel.setup.source.aux_in_state_mut() {
                changed |= state.params.reseat(seat_of);
            }
            changed |= channel.setup.modulation.reseat_gates(seat_of);
        }
        changed
    }

    /// Where the channel wearing `id` currently sits.
    ///
    /// **The only id-to-position conversion anything should use.** A second
    /// one written by hand is the shape `AGENTS.md` calls this codebase's
    /// characteristic fault, and the shape that made Buffer's face operate
    /// the wrong parameters while every test stayed green.
    pub fn channel_index(&self, id: ChannelId) -> Option<usize> {
        if !id.is_assigned() {
            return None;
        }
        self.channels.iter().position(|channel| channel.id == id)
    }

    /// Where [`Self::selected_channel`] currently sits.
    ///
    /// Falls back to the first channel when the selection names one this song
    /// does not have -- a hand-edited file, or a document saved by a version
    /// that let the selection go stale. Every reader goes through here, so
    /// there is one answer to "which seat is selected" rather than a `as
    /// usize` at each call site that has to remember the fallback.
    pub fn selected_index(&self) -> usize {
        self.channel_index(self.selected_channel).unwrap_or(0)
    }

    /// The durable name of an effect chain's seat, or `None` when that seat
    /// is empty. [`Self::chain_target`]'s inverse.
    pub fn chain_key(&self, target: crate::EffectTarget) -> Option<crate::ChainKey> {
        match target {
            crate::EffectTarget::Channel(channel) => self
                .channels
                .get(usize::from(channel))
                .map(|channel| crate::ChainKey::Channel(channel.id)),
            crate::EffectTarget::Bus(bus) => Some(crate::ChainKey::Bus(bus)),
        }
    }

    /// The addressable form of a durable chain name, or `None` when this song
    /// has no such channel.
    ///
    /// The session has the same pair over its own channel list
    /// (`Session::chain_key` / `Session::chain_target`); both are one line
    /// over [`Self::channel_index`] rather than a second search.
    pub fn chain_target(&self, key: crate::ChainKey) -> Option<crate::EffectTarget> {
        match key {
            crate::ChainKey::Channel(id) => u8::try_from(self.channel_index(id)?)
                .ok()
                .map(crate::EffectTarget::Channel),
            crate::ChainKey::Bus(bus) => Some(crate::EffectTarget::Bus(bus)),
        }
    }

    /// A control binding's target as something addressable, or `None` when it
    /// names a channel this song does not have.
    pub fn param_addr(&self, key: crate::ParamKey) -> Option<crate::ParamAddr> {
        key.resolve(|scope| self.chain_target(scope))
    }

    /// An address as a control binding would store it.
    pub fn param_key(&self, address: crate::ParamAddr) -> Option<crate::ParamKey> {
        crate::ParamKey::of(address, |scope| self.chain_key(scope))
    }

    /// Convert a song written under the strip volume's old Linear curve to
    /// the fader taper, so it plays at the gains it was saved at (MOO-131).
    ///
    /// The channel and track volumes themselves are linear gain and need
    /// nothing. What moves is everything *normalized* against the volume
    /// descriptor: a lane point `v` meant `v * MAX_LINEAR_GAIN` and becomes
    /// that gain's fader travel; a binding's `min` and `max` convert the same
    /// way, so a learned 0..1 range stays 0..1 and now puts unity at CC 96. A
    /// route's `depth` was a signed fraction of the linear range; it becomes
    /// the travel between the destination's own fader position and the gain
    /// the old depth reached from there, which keeps a route's peak
    /// excursion. Values above +6 dB are clamped to it -- the fader cannot go
    /// higher, which is the point.
    ///
    /// Idempotent through [`Self::strip_volume_taper`], which it sets.
    pub fn migrate_linear_strip_volume(&mut self) {
        if self.strip_volume_taper != STRIP_VOLUME_TAPER_LINEAR {
            return;
        }
        self.strip_volume_taper = STRIP_VOLUME_TAPER;
        let is_volume = |owner: crate::ParamOwner, param: u32| {
            owner == crate::ParamOwner::Strip && param == crate::STRIP_PARAM_VOLUME
        };

        for channel in &mut self.channels {
            for lanes in &mut channel.automation {
                for lane in lanes.iter_mut() {
                    if !is_volume(lane.target.owner, lane.target.param) {
                        continue;
                    }
                    let points: Vec<crate::AutomationPoint> = lane
                        .points()
                        .iter()
                        .map(|point| crate::AutomationPoint {
                            value: linear_volume_as_fader(point.value),
                            ..*point
                        })
                        .collect();
                    lane.reset_points(points);
                }
            }
        }

        let volume_at = |project: &Self, scope: crate::EffectTarget| -> Option<f32> {
            match scope {
                crate::EffectTarget::Channel(channel) => project
                    .channels
                    .get(usize::from(channel))
                    .map(|channel| channel.setup.channel.volume),
                crate::EffectTarget::Bus(bus) => {
                    project.buses.get(usize::from(bus)).map(|bus| bus.bus.volume)
                }
            }
        };
        let mut depths: Vec<(usize, usize, f32)> = Vec::new();
        for (channel_index, channel) in self.channels.iter().enumerate() {
            for (route_index, route) in channel.setup.modulation.routes.iter().enumerate() {
                let Some(route) = route else { continue };
                if !is_volume(route.destination.owner, route.destination.param) {
                    continue;
                }
                let base = volume_at(self, route.destination.scope)
                    .unwrap_or(crate::DEFAULT_CHANNEL_VOLUME)
                    .clamp(0.0, crate::gain::MAX_LINEAR_GAIN);
                let reached = (base + route.depth * crate::gain::MAX_LINEAR_GAIN)
                    .clamp(0.0, crate::gain::MAX_LINEAR_GAIN);
                let depth = crate::gain::fader_gain_to_position(reached)
                    - crate::gain::fader_gain_to_position(base);
                depths.push((channel_index, route_index, depth.clamp(-1.0, 1.0)));
            }
        }
        for (channel_index, route_index, depth) in depths {
            if let Some(route) = &mut self.channels[channel_index].setup.modulation.routes[route_index]
            {
                route.depth = depth;
            }
        }

        for binding in &mut self.control_map.bindings {
            let crate::ControlTarget::Param(key) = binding.target else {
                continue;
            };
            if !is_volume(key.owner, key.param) {
                continue;
            }
            binding.min = linear_volume_as_fader(binding.min);
            binding.max = linear_volume_as_fader(binding.max);
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
        // The selection names a channel rather than a seat, so removing any
        // *other* channel leaves it exactly where it was. Only losing the
        // selected channel itself needs an answer, and the answer is whoever
        // closed the gap -- or the last channel, when it was the end.
        if removed.id == self.selected_channel {
            let seat = index.min(self.channels.len() - 1);
            self.selected_channel = self.channels[seat].id;
        }
        Some(removed)
    }

    /// Insert `channel` at `index` (clamped to the end), opening a gap. The
    /// newcomer's own references are pointed at its new index and every
    /// later channel's follow it up by one. Refused when the song is full.
    ///
    /// **The newcomer is always minted a fresh identity**, whatever it
    /// arrived wearing. This is the paste path, and what it is handed is a
    /// clipboard copy of a channel that is very probably still in the song:
    /// keeping the id would put two channels wearing one id into the same
    /// bank, which is the one state the identity has to make impossible. A
    /// paste makes another channel, not another view of the same one.
    pub fn insert_channel(&mut self, index: usize, mut channel: ProjectChannel) -> Option<usize> {
        if self.channels.len() >= MAX_CHANNELS {
            return None;
        }
        channel.id = mint_channel_id(&mut self.next_channel_id);
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
        // The selection is *not* one more thing to renumber any more: it
        // names the channel, and a move does not change which channel that
        // is. This is what the identity bought.
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
        let mut track = crate::BusSetup::new(index);
        track.id = crate::mint_track_id(&mut self.next_track_id);
        self.buses.push(track);
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
        // The control map is deliberately *not* walked here. A binding names
        // its channel by `ChannelId`, so a channel edit cannot move one;
        // `rescope_tracks_after` still calls its track twin, because a track
        // is still a seat.
        //
        // The two cross-channel references above *were* followed positionally
        // by the walk, and that is what keeps a reference with no identity yet
        // correct. It is a no-op for one that has an identity, because the
        // reseat below overwrites its seat anyway. Adopt first, so a reference
        // the walk has just moved to the right seat takes the identity of the
        // channel actually sitting there.
        self.identify_channel_references();
        self.reseat_channel_references();
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
                id: ChannelId::UNASSIGNED,
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
        // Built as a literal rather than through `insert_channel`, so the
        // four arrive unminted; this is the same pass a loaded song gets.
        project.assign_channel_ids();
        // And the tracks, for the same reason: `starter_tracks` builds all
        // but the master by hand.
        project.assign_track_ids();
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
        collapsed: false,
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

    fn named_patterns(names: &[&str]) -> Project {
        let mut project = Project {
            channels: vec![ProjectChannel::sampler(0, names.len()).with_id(ChannelId(0))],
            pattern_lengths: vec![DEFAULT_STEPS; names.len()],
            ..Project::default()
        };
        project.pattern_meta = names
            .iter()
            .map(|name| PatternMeta {
                name: (*name).into(),
                ..PatternMeta::default()
            })
            .collect();
        for (index, _) in names.iter().enumerate() {
            project.channels[0].notes[index].push(NoteEvent::new(index as u32 + 1, 0, 24, 60, 100));
            project.playlist.push(crate::PatternPlacement::new(index as u8, index as u32 * 384));
        }
        project
    }

    fn names(project: &Project) -> Vec<&str> {
        project.pattern_meta.iter().map(|meta| meta.name.as_str()).collect()
    }

    /// A pattern's name and colour travel with its notes. Shaped against the
    /// interface's inline clone, which moved every other parallel list and
    /// not this one: cloning A in `[A, B, C]` named the copy B, and C's name
    /// landed on B.
    #[test]
    fn cloning_a_pattern_carries_its_name_and_shifts_the_rest() {
        let mut project = named_patterns(&["A", "B", "C"]);
        assert!(project.clone_pattern(0));
        assert_eq!(names(&project), ["A", "A", "B", "C"]);
        assert_eq!(project.pattern_lengths.len(), 4);
        // Every list agrees about which pattern is which: the notes of the
        // pattern called B are B's.
        assert_eq!(project.channels[0].notes[2][0].id, 2);
        assert_eq!(project.channels[0].notes[1][0].id, 1, "the copy holds A's notes");
        let placed: Vec<u8> = project.playlist.iter().map(|placement| placement.pattern).collect();
        assert_eq!(placed, [0, 2, 3], "placements follow their content");
        assert_eq!(project.current_pattern, 1, "the copy is selected");
    }

    #[test]
    fn removing_a_pattern_takes_its_name_with_it() {
        let mut project = named_patterns(&["A", "B", "C"]);
        assert!(project.remove_pattern(1));
        assert_eq!(names(&project), ["A", "C"]);
        assert_eq!(project.channels[0].notes[1][0].id, 3, "C's notes are under C's name");
        let placed: Vec<u8> = project.playlist.iter().map(|placement| placement.pattern).collect();
        assert_eq!(placed, [0, 1]);
    }

    /// The stored list stops at the last pattern anybody named, so an edit
    /// past its end has nothing of its own to move -- and must not invent
    /// entries or panic reaching for one.
    #[test]
    fn a_short_name_list_is_left_alone_past_its_end() {
        let mut project = named_patterns(&["A", "B", "C"]);
        project.pattern_meta.truncate(1);
        assert!(project.clone_pattern(2));
        assert_eq!(names(&project), ["A"]);
        assert!(project.remove_pattern(3));
        assert!(project.remove_pattern(1));
        assert_eq!(names(&project), ["A"]);
        assert!(project.clone_pattern(0));
        assert_eq!(names(&project), ["A", "A"]);
    }

    #[test]
    fn a_pattern_edit_that_cannot_happen_changes_nothing() {
        let mut project = named_patterns(&["A"]);
        let before = project.clone();
        assert!(!project.remove_pattern(0), "the last pattern stays");
        assert!(!project.clone_pattern(1), "there is no pattern 1");
        assert_eq!(project, before);
    }

    /// **The selection names a channel, so an edit to a different channel
    /// leaves it alone.** Both halves of this failed before the id existed,
    /// and they failed in opposite directions: `insert_channel` never
    /// rescoped the selection at all, so adding a channel above it moved the
    /// selection down one; `remove_channel` *clamped* instead of following,
    /// which is only accidentally right when the selection is at the end of
    /// the bank. Verified failing on the tree before the change, which is the
    /// only way to know a test of this shape is doing anything.
    #[test]
    fn the_selection_survives_an_edit_to_another_channel() {
        let mut project = Project::default();
        for index in 1..4 {
            project.insert_channel(index, ProjectChannel::mlm1(index, 1));
        }
        for index in 0..4usize {
            project.channels[index].setup.channel.name = format!("ch{index}");
        }
        let selected = |project: &Project| {
            project.channels[project.selected_index()].setup.channel.name.clone()
        };

        project.selected_channel = project.channels[2].id;
        assert_eq!(selected(&project), "ch2");

        project.insert_channel(0, ProjectChannel::sampler(9, 1)).expect("room");
        assert_eq!(selected(&project), "ch2", "an insert above the selection");

        project.remove_channel(0).expect("the newcomer exists");
        assert_eq!(selected(&project), "ch2", "a removal above the selection");

        project.move_channel(3, 0).expect("a real move");
        assert_eq!(selected(&project), "ch2", "a move that passed the selection");

        // And the selected channel moving is the same answer, which is the
        // case a position got right for the wrong reason.
        let selected_index = project.selected_index();
        project.move_channel(selected_index, 0).expect("a real move");
        assert_eq!(selected(&project), "ch2", "the selected channel itself moved");
    }

    /// Losing the selected channel is the one case that needs an answer, and
    /// the answer is whoever closed the gap -- or the last channel, when the
    /// selection was at the end.
    #[test]
    fn deleting_the_selected_channel_selects_its_seat() {
        let mut project = Project::default();
        for index in 1..4 {
            project.insert_channel(index, ProjectChannel::mlm1(index, 1));
        }
        for index in 0..4usize {
            project.channels[index].setup.channel.name = format!("ch{index}");
        }
        let selected = |project: &Project| {
            project.channels[project.selected_index()].setup.channel.name.clone()
        };

        project.selected_channel = project.channels[1].id;
        project.remove_channel(1).expect("channel 1 exists");
        assert_eq!(selected(&project), "ch2", "whoever closed the gap");

        project.selected_channel = project.channels[2].id;
        project.remove_channel(2).expect("the last channel");
        assert_eq!(selected(&project), "ch2", "the new last channel");
    }

    /// A selection naming a channel the song does not have resolves to the
    /// first one rather than panicking or addressing a stranger. `selected_index`
    /// is the only place that decides this.
    #[test]
    fn a_selection_that_names_nothing_resolves_to_the_first_channel() {
        let mut project = Project::default();
        project.insert_channel(1, ProjectChannel::mlm1(1, 1));

        project.selected_channel = ChannelId(9_999);
        assert_eq!(project.selected_index(), 0);

        project.selected_channel = ChannelId::UNASSIGNED;
        assert_eq!(project.selected_index(), 0);
    }

    /// **A removed channel's id is never handed to its successor.** The whole
    /// point of an identity over a position: an address left holding the id of
    /// a deleted channel has to resolve to nothing, not to whichever channel
    /// closed the gap.
    #[test]
    fn a_deleted_channels_identity_is_not_reused() {
        let mut project = Project::default();
        for _ in 0..2 {
            project.insert_channel(project.channels.len(), ProjectChannel::sampler(0, 1));
        }
        let doomed = project.channels[1].id;
        assert!(doomed.is_assigned());

        project.remove_channel(1).expect("channel 1 exists");
        assert_eq!(project.channel_index(doomed), None);

        project.insert_channel(project.channels.len(), ProjectChannel::sampler(0, 1));
        let fresh = project.channels.last().expect("the bank is not empty").id;
        assert_ne!(fresh, doomed);
        assert_eq!(project.channel_index(fresh), Some(project.channels.len() - 1));
    }

    /// **Pasting one channel twice makes three channels and three
    /// identities.** The clipboard hands `insert_channel` a copy of a channel
    /// that is still in the song, so keeping the copy's id would put two
    /// channels wearing one id into the bank -- the one state this must make
    /// impossible.
    #[test]
    fn pasting_a_channel_twice_makes_three_identities() {
        let mut project = Project::default();
        let clipboard = project.channels[0].clone();
        project.insert_channel(1, clipboard.clone()).expect("room for a paste");
        project.insert_channel(1, clipboard).expect("room for another");

        let ids: Vec<_> = project.channels.iter().map(|channel| channel.id).collect();
        assert_eq!(ids.len(), 3);
        let unique: std::collections::HashSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), 3, "three channels wearing {ids:?}");
        for (index, id) in ids.iter().enumerate() {
            assert_eq!(project.channel_index(*id), Some(index));
        }
    }

    /// **A bank with no identities takes its positions**, which is what every
    /// saved address in such a song already meant by `channel = 3`. That is
    /// what makes this a defaulted field rather than a format migration.
    #[test]
    fn a_bank_with_no_identities_takes_its_positions() {
        let mut project = Project {
            channels: vec![ProjectChannel::sampler(0, 1); 4],
            next_channel_id: 0,
            ..Project::default()
        };
        assert!(project.channels.iter().all(|channel| !channel.id.is_assigned()));

        project.assign_channel_ids();

        for (index, channel) in project.channels.iter().enumerate() {
            assert_eq!(channel.id, ChannelId(index as u32));
        }
        // And the mint is past them, so the next paste cannot collide.
        assert_eq!(project.next_channel_id, 4);

        // Idempotent: running it again on a bank that has them changes
        // nothing, which is why the loader can call it unconditionally.
        let before = project.clone();
        project.assign_channel_ids();
        assert_eq!(project, before);
    }

    /// A hand-edited bank where only some channels have ids: the ones that do
    /// keep them, and the stragglers are minted past the highest rather than
    /// taking positions that are already in use.
    #[test]
    fn a_part_assigned_bank_mints_its_stragglers() {
        let mut project = Project {
            channels: vec![ProjectChannel::sampler(0, 1); 3],
            next_channel_id: 0,
            ..Project::default()
        };
        project.channels[1].id = ChannelId(7);

        project.assign_channel_ids();

        assert_eq!(project.channels[1].id, ChannelId(7));
        assert_eq!(project.channels[0].id, ChannelId(8));
        assert_eq!(project.channels[2].id, ChannelId(9));
        assert_eq!(project.next_channel_id, 10);
    }

    /// [`Self::a_bank_with_no_identities_takes_its_positions`] for tracks.
    #[test]
    fn a_track_bank_with_no_identities_takes_its_positions() {
        let mut project = Project {
            buses: (0..3).map(crate::BusSetup::new).collect(),
            next_track_id: 0,
            ..Project::default()
        };
        assert!(project.buses.iter().all(|track| !track.id.is_assigned()));

        project.assign_track_ids();

        for (index, track) in project.buses.iter().enumerate() {
            assert_eq!(track.id, crate::TrackId(index as u32));
        }
        assert_eq!(project.next_track_id, 3);
        let before = project.clone();
        project.assign_track_ids();
        assert_eq!(project, before, "not idempotent");
    }

    /// A removed track's id is not handed to the next one, and a move keeps
    /// every id with its track -- the property the engine's carry leans on.
    #[test]
    fn a_track_id_is_never_reused_and_follows_a_move() {
        let mut project = Project::default();
        assert_eq!(project.buses[0].id, crate::TrackId(0), "the master is minted");
        let first = project.add_track().expect("room");
        let second = project.add_track().expect("room");
        let (a, b) = (project.buses[first].id, project.buses[second].id);
        assert_ne!(a, b);

        project.remove_track(second).expect("not the master");
        let third = project.add_track().expect("room");
        assert_ne!(project.buses[third].id, b, "a removed track's id came back");

        project.move_track(first, third).expect("a real move");
        assert_eq!(project.track_index(a), Some(third));
        assert_eq!(project.track_index(crate::TrackId::UNASSIGNED), None);
    }

    /// The starter song builds its tracks by hand, and still arrives with
    /// every one of them identified.
    #[test]
    fn the_starter_kit_identifies_its_tracks() {
        let project = Project::starter_kit(1);
        let ids: std::collections::HashSet<_> =
            project.buses.iter().map(|track| track.id).collect();
        assert_eq!(ids.len(), project.buses.len(), "{:?}", project.buses);
        assert!(ids.iter().all(|id| id.is_assigned()));
        assert!(ids.iter().all(|id| id.0 < project.next_track_id));
    }

    /// The id survives a move, and `channel_index` is what says where it went.
    /// An unassigned id addresses nothing rather than the first channel.
    #[test]
    fn channel_index_follows_a_move_and_refuses_an_unassigned_id() {
        let mut project = Project::default();
        for _ in 0..2 {
            project.insert_channel(project.channels.len(), ProjectChannel::sampler(0, 1));
        }
        let travelling = project.channels[0].id;

        project.move_channel(0, 2).expect("a real move");

        assert_eq!(project.channel_index(travelling), Some(2));
        assert_eq!(project.channel_index(ChannelId::UNASSIGNED), None);
        assert_eq!(project.channel_index(ChannelId(9_999)), None);
    }

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

    /// A song written under the strip volume's old Linear curve plays at the
    /// gains it was saved at (MOO-131). A lane point at a quarter of the old
    /// 0..+12 dB range was unity and is now three-quarter travel; a learned
    /// 0..1 binding stays 0..1; a route keeps its peak excursion. And a song
    /// already on the fader taper is left alone.
    #[test]
    fn a_linear_volume_song_migrates_to_the_fader_taper() {
        use crate::modulation::{ModPolarity, ModRoute};
        use crate::{
            AutomationLane, AutomationPoint, ControlBinding, ControlSource, ControlTarget,
            EffectTarget, MidiChannelFilter, MidiPortFilter, ParamAddr, ParamKey,
        };

        let mut project = Project::default();
        project.assign_channel_ids();
        project.strip_volume_taper = STRIP_VOLUME_TAPER_LINEAR;
        project.channels[0].setup.channel.volume = 1.0;
        let address = ParamAddr::strip(EffectTarget::Channel(0), crate::STRIP_PARAM_VOLUME);

        project.channels[0].normalize_automation();
        let mut lane = AutomationLane::new(address);
        lane.reserve_points();
        // Silence, unity, +6 dB, and +12 dB under the old curve.
        lane.reset_points([
            AutomationPoint::new(1, 0, 0.0),
            AutomationPoint::new(2, 24, 0.25),
            AutomationPoint::new(3, 48, 0.5),
            AutomationPoint::new(4, 72, 1.0),
        ]);
        project.channels[0].automation[0].push(lane);
        project.channels[0].setup.modulation.routes[0] =
            Some(ModRoute::to_slot(0, address, 0.25, ModPolarity::Bipolar));
        project.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Param(ParamKey::strip(
                crate::ChainKey::Channel(project.channels[0].id),
                crate::STRIP_PARAM_VOLUME,
            )),
        ));

        project.migrate_linear_strip_volume();
        assert_eq!(project.strip_volume_taper, STRIP_VOLUME_TAPER);

        let volume = crate::strip_descriptor(crate::STRIP_PARAM_VOLUME).unwrap();
        let gains: Vec<f32> = project.channels[0].automation[0][0]
            .points()
            .iter()
            .map(|point| volume.from_normalized(point.value))
            .collect();
        assert_eq!(gains[0], 0.0);
        assert!((gains[1] - 1.0).abs() < 1e-3, "unity became {}", gains[1]);
        assert!(
            (gains[2] - crate::gain::FADER_MAX_GAIN).abs() < 1e-3,
            "+6 dB became {}",
            gains[2]
        );
        assert!(
            (gains[3] - crate::gain::FADER_MAX_GAIN).abs() < 1e-3,
            "+12 dB clamps to the fader's top, not {}",
            gains[3]
        );

        // The route reached unity + 1.0 (a quarter of 4.0), about +6 dB,
        // which is the whole quarter of travel above unity.
        let route = project.channels[0].setup.modulation.routes[0].unwrap();
        assert!((route.depth - 0.25).abs() < 1e-3, "depth became {}", route.depth);

        let binding = &project.control_map.bindings[0];
        assert_eq!((binding.min, binding.max), (0.0, 1.0));

        let before = project.clone();
        project.migrate_linear_strip_volume();
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
        // Pushed straight onto the list rather than inserted, so nothing has
        // minted them an identity yet. A load does this; a binding cannot be
        // learned onto a channel that has none.
        project.assign_channel_ids();
        let fader = |project: &Project, channel: u8| {
            crate::ParamKey::strip(
                crate::ChainKey::Channel(project.channels[channel as usize].id),
                crate::STRIP_PARAM_VOLUME,
            )
        };
        for index in 0..4u8 {
            let learned = fader(&project, index);
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
            project.control_map.bind(fader_on(index, learned));
        }
        // Each binding still moves the channel it was learned on, by name.
        //
        // **Nothing in this closure runs a rescope**, and that is what it is
        // here to show: a binding names a `ChannelId`, so a channel edit is
        // not an event it can observe and there is nothing to renumber. The
        // one learned on a removed channel stops *resolving* -- it is not
        // dropped, because the id is what an undo brings the channel back
        // under, and an inert row saying "Unavailable parameter" is a better
        // answer than a mapping silently thrown away.
        let bindings_follow = |project: &Project, gone: &[u8]| {
            let mut live = Vec::new();
            for binding in &project.control_map.bindings {
                let controller = controller_of(binding);
                let crate::ChainKey::Channel(id) = bound_key(binding) else {
                    panic!("a channel binding became {:?}", binding.target);
                };
                let Some(seat) = project.channel_index(id) else {
                    assert!(
                        gone.contains(&controller),
                        "the fader learned on ch{controller} stopped resolving"
                    );
                    continue;
                };
                assert_eq!(
                    project.channels[seat].setup.channel.name,
                    format!("ch{controller}"),
                    "the fader learned on ch{controller} now moves seat {seat}"
                );
                live.push(controller);
            }
            live.sort_unstable();
            let expected: Vec<u8> = (0..4).filter(|index| !gone.contains(index)).collect();
            assert_eq!(live, expected);
            assert_eq!(
                project.control_map.bindings.len(),
                4,
                "a channel edit drops no binding"
            );
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

    /// The two fields that name a channel *other than the one they are stored
    /// in* take an identity on the way in, and the identity is what decides
    /// where they point afterwards.
    ///
    /// Checked against the unfixed tree first, as
    /// `channel-identity/06` asks: on it the second half fails, because a
    /// reference whose channel was deleted was parked on a marker and had
    /// nothing left to say which channel it had named.
    #[test]
    fn a_cross_channel_reference_is_identified_on_the_way_in_and_survives_a_delete() {
        let mut project = Project::default();
        for index in 1..4 {
            project.channels.push(ProjectChannel::mlm1(index, 1));
        }
        // Channel 0 reads channel 2's outlet, and its envelope gates off
        // channel 3 -- the two shapes this pass exists for, written as an
        // older file wrote them: seats, and no identity anywhere.
        project.channels[0].setup.source = ChannelSource::AuxIn(Default::default());
        project.channels[0]
            .setup
            .source
            .aux_in_state_mut()
            .expect("aux in")
            .params
            .source_channel = 2;
        let gate = crate::ModEnvelopeParams {
            input_channel: 3,
            ..Default::default()
        };
        project.channels[0]
            .setup
            .modulation
            .install(0, crate::ModulatorParams::Envelope(gate));
        // A second envelope, parked. `u8::MAX` is the marker for "the channel
        // this named is gone" and is *also* an ordinary `ChannelId`, so it
        // must not be reread as one.
        let parked = crate::ModEnvelopeParams {
            input_channel: u8::MAX,
            ..Default::default()
        };
        project.channels[1]
            .setup
            .modulation
            .install(0, crate::ModulatorParams::Envelope(parked));

        project.assign_channel_ids();
        let id_of = |project: &Project, seat: usize| project.channels[seat].id;
        let subscription = |project: &Project| {
            project.channels[0]
                .setup
                .source
                .aux_in_state()
                .expect("aux in")
                .params
        };
        let gate_of = |project: &Project, seat: usize| {
            match project.channels[seat].setup.modulation.params(0) {
                Some(crate::ModulatorParams::Envelope(envelope)) => envelope,
                other => panic!("slot 0 of channel {seat} is {other:?}"),
            }
        };
        assert_eq!(subscription(&project).source_id, id_of(&project, 2));
        assert_eq!(gate_of(&project, 0).input_channel_id, id_of(&project, 3));
        assert!(
            !gate_of(&project, 1).input_channel_id.is_assigned(),
            "a parked gate names no channel, and 255 is not its identity"
        );

        // A move: the seats follow, the identities do not budge.
        let source = id_of(&project, 2);
        let gated = id_of(&project, 3);
        assert_eq!(
            project.move_channel(3, 1),
            Some(ChannelEdit::Moved { from: 3, to: 1 })
        );
        assert_eq!(project.channel_index(source), Some(3));
        assert_eq!(subscription(&project).source_channel, 3);
        assert_eq!(subscription(&project).source_id, source);
        assert_eq!(gate_of(&project, 0).input_channel, 1);
        assert_eq!(gate_of(&project, 0).input_channel_id, gated);

        // And a delete. Both park, as they always did -- but they still say
        // which channel they named, which is the half that is new.
        let removed = project.remove_channel(3).expect("the source is there");
        assert_eq!(removed.id, source);
        assert_eq!(subscription(&project).source_channel, crate::aux_in::DEPARTED_SOURCE);
        assert_eq!(
            subscription(&project).source_id,
            source,
            "a departed subscription still names the channel it lost"
        );

        // Put that same channel back -- ids intact, which is what restoring a
        // document does -- and the edge is live again without anybody having
        // repaired it.
        project.channels.insert(1, removed);
        assert!(project.reseat_channel_references());
        assert_eq!(subscription(&project).source_channel, 1);
        assert_eq!(gate_of(&project, 0).input_channel_id, gated);
    }

    fn fader_on(controller: u8, key: crate::ParamKey) -> crate::ControlBinding {
        crate::ControlBinding::new(
            crate::ControlSource::Cc {
                port: Default::default(),
                channel: Default::default(),
                controller,
            },
            crate::ControlTarget::Param(key),
        )
    }

    fn controller_of(binding: &crate::ControlBinding) -> u8 {
        match binding.source {
            crate::ControlSource::Cc { controller, .. } => controller,
            ref other => panic!("not a test fader: {other:?}"),
        }
    }

    /// What a binding names, before anything resolves it. A `ChainKey` rather
    /// than an `EffectTarget`, which is the whole difference the tests below
    /// are about: a channel arm is an identity and has no seat in it to read.
    fn bound_key(binding: &crate::ControlBinding) -> crate::ChainKey {
        match binding.target {
            crate::ControlTarget::Param(key) => key.scope,
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
        project.assign_channel_ids();
        let track_fader = |track: u8| {
            crate::ParamKey::strip(crate::ChainKey::Bus(track), crate::STRIP_PARAM_VOLUME)
        };
        project.control_map.bind(fader_on(3, track_fader(3)));
        project.control_map.bind(fader_on(1, track_fader(1)));
        project.control_map.bind(fader_on(
            100,
            crate::ParamKey::strip(
                crate::ChainKey::Channel(project.channels[0].id),
                crate::STRIP_PARAM_VOLUME,
            ),
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
            match (controller_of(binding), bound_key(binding)) {
                (100, scope) => assert_eq!(
                    scope,
                    crate::ChainKey::Channel(project.channels[0].id),
                    "a channel binding is untouched by a track edit"
                ),
                (controller, crate::ChainKey::Bus(seat)) => {
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
            if let crate::ChainKey::Bus(seat) = bound_key(binding) {
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
                ..Default::default()
            },
        ));
        // Written as an older file wrote it -- a seat and no identity -- and
        // given one on the way in, which is what a load does.
        project.assign_channel_ids();
        assert_eq!(
            project.channels[1]
                .setup
                .source
                .aux_in_state()
                .expect("aux in")
                .params
                .source_id,
            project.channels[0].id
        );
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
        // A song with no source writes no identity either, so it stays
        // byte-identical to one written before the field existed.
        assert!(!toml::to_string(&crate::AuxInParams::default())
            .unwrap()
            .contains("source_id"));
    }
}
