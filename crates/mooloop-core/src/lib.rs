//! Core data model and message types shared between the engine and the UI.
//!
//! No dependency on audio or UI code so this can be linked freely from both the
//! realtime and GUI threads.

pub mod bridge;
pub mod channel;
pub mod effect;
pub mod pattern;
pub mod playlist;
pub mod project;
pub mod sampler;
pub mod synth;
pub mod time;

pub use bridge::{EngineCommand, EngineEvent};
pub use channel::{Channel, DeviceKind, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL, MAX_PATTERNS};
pub use effect::{
    BitcrushParams, DriveCurve, DriveParams, EffectKind, EffectParams, EffectSlotState, FilterMode,
    FilterParams, ParamCurve, ParamDescriptor, BITCRUSH_PARAM_BITS, BITCRUSH_PARAM_DOWNSAMPLE,
    BITCRUSH_PARAM_MIX, DRIVE_PARAM_CURVE, DRIVE_PARAM_DRIVE, DRIVE_PARAM_MIX, DRIVE_PARAM_OUTPUT,
    DRIVE_PARAM_TONE, FILTER_PARAM_CUTOFF_HZ, FILTER_PARAM_MODE, FILTER_PARAM_RESONANCE,
};
pub use pattern::{
    ChannelPattern, NoteEvent, NoteId, Pattern, Step, DEFAULT_NOTE_DURATION_TICKS, DEFAULT_STEPS,
    MAX_NOTES_PER_CHANNEL_PATTERN, MAX_PATTERN_STEPS, TICKS_PER_64TH, TICKS_PER_STEP,
};
pub use playlist::{
    PatternPlacement, PlaybackMode, MAX_PLAYLIST_BARS, MAX_PLAYLIST_PLACEMENTS, MAX_PLAYLIST_TICKS,
    STEPS_PER_BAR, TICKS_PER_BAR,
};
pub use project::{
    ChannelPreset, ChannelSetup, ChannelSource, DrumSynthState, Kit, MonoSynthState, Project,
    ProjectChannel, SampleReference, SamplerState, DEFAULT_SWING_PERCENT, MAX_SWING_PERCENT,
    MIN_SWING_PERCENT,
};
pub use sampler::{
    clamp01, LoopMode, RetriggerMode, SamplerParams, VoiceMode, MAX_CHOKE_GROUP, MAX_SAMPLER_VOICES,
};
pub use synth::{
    DrumMode, DrumSynthParams, HatCharacter, KickCharacter, LfoParams, LfoWave, MonoSynthParams,
    OscParams, OscWave, SnareCharacter, MAX_DRUM_VOICES,
};
pub use time::{ticks_per_sample, Ppq, Samples, Ticks};
