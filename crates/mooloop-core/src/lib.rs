//! Core data model and message types shared between the engine and the UI.
//!
//! No dependency on audio or UI code so this can be linked freely from both the
//! realtime and GUI threads.

pub mod bridge;
pub mod channel;
pub mod pattern;
pub mod playlist;
pub mod project;
pub mod sampler;
pub mod synth;
pub mod time;

pub use bridge::{EngineCommand, EngineEvent};
pub use channel::{Channel, DeviceKind, MAX_CHANNELS, MAX_PATTERNS};
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
    ProjectChannel, SampleReference, SamplerState,
};
pub use sampler::{
    clamp01, LoopMode, RetriggerMode, SamplerParams, VoiceMode, MAX_CHOKE_GROUP, MAX_SAMPLER_VOICES,
};
pub use synth::{DrumMode, DrumSynthParams, MonoSynthParams, OscParams, OscWave, MAX_DRUM_VOICES};
pub use time::{ticks_per_sample, Ppq, Samples, Ticks};
