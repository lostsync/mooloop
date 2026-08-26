//! Core data model and message types shared between the engine and the UI.
//!
//! No dependency on audio or UI code so this can be linked freely from both the
//! realtime and GUI threads.

pub mod bridge;
pub mod buffer;
pub mod channel;
pub mod effect;
pub mod mixer;
pub mod pattern;
pub mod playlist;
pub mod project;
pub mod sampler;
pub mod synth;
pub mod time;

pub use bridge::{EngineCommand, EngineEvent};
pub use buffer::{BufferDuration, BufferEvent};
pub use channel::{
    Channel, DeviceKind, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL, MAX_LINEAR_GAIN, MAX_PATTERNS,
};
pub use effect::{
    BitcrushParams, BufferParams, CompressorParams, DelayMode, DelayParams, DriveCurve,
    DriveParams, EffectKind, EffectParams, EffectSlotState, EqBand, EqBandKind, EqParams,
    EqPassFilter, EqQProfile, EqSlope, FilterMode, FilterParams, GateParams, LimiterParams,
    ModulationMode, ModulationParams, ParamCurve, ParamDescriptor, PlateParams, ReverbMaterial,
    ReverbParams, ReverbShape, BITCRUSH_PARAM_BITS, BITCRUSH_PARAM_DOWNSAMPLE, BITCRUSH_PARAM_MIX,
    COMP_PARAM_ATTACK_MS, COMP_PARAM_KNEE_DB, COMP_PARAM_MAKEUP_DB, COMP_PARAM_RATIO,
    COMP_PARAM_RELEASE_MS, COMP_PARAM_THRESHOLD_DB, DELAY_MAX_TIME_MS, DELAY_PARAM_CROSS,
    DELAY_PARAM_FEEDBACK, DELAY_PARAM_MIX, DELAY_PARAM_MODE, DELAY_PARAM_TIME_MS, DELAY_PARAM_TONE,
    DRIVE_PARAM_CURVE, DRIVE_PARAM_DRIVE, DRIVE_PARAM_MIX, DRIVE_PARAM_OUTPUT, DRIVE_PARAM_TONE,
    EQ_MAX_BANDS, EQ_PARAM_CHARACTER, EQ_PARAM_ENABLED, EQ_PARAM_FREQUENCY_HZ, EQ_PARAM_GAIN_DB,
    EQ_PARAM_Q, EQ_PARAM_TARGET, FILTER_PARAM_CUTOFF_HZ, FILTER_PARAM_MODE, FILTER_PARAM_RESONANCE,
    GATE_PARAM_ATTACK_MS, GATE_PARAM_HOLD_MS, GATE_PARAM_RANGE_DB, GATE_PARAM_RELEASE_MS,
    GATE_PARAM_THRESHOLD_DB, LIMITER_PARAM_CEILING_DB, LIMITER_PARAM_GAIN_DB,
    LIMITER_PARAM_RELEASE_MS, MODULATION_PARAM_COLOR, MODULATION_PARAM_DEPTH,
    MODULATION_PARAM_FEEDBACK, MODULATION_PARAM_MODE, MODULATION_PARAM_RATE_HZ,
    MODULATION_PARAM_SPREAD, MODULATION_PARAM_STAGES, MODULATION_PARAM_TONE, PLATE_PARAM_DAMPING,
    PLATE_PARAM_DECAY_S, PLATE_PARAM_SIZE, PLATE_PARAM_WIDTH, REVERB_PARAM_CAPTURE_X,
    REVERB_PARAM_CAPTURE_Y, REVERB_PARAM_DECAY_S, REVERB_PARAM_DEPTH_M, REVERB_PARAM_HEIGHT_M,
    REVERB_PARAM_MATERIAL, REVERB_PARAM_SHAPE, REVERB_PARAM_WIDTH_M,
};
pub use mixer::{
    compile_bus_graph, compile_render_order, default_buses, default_render_order, is_legal_route,
    sanitize_route, would_create_cycle, BusSetup, CompiledBusGraph, EffectTarget, MixerBus,
    RenderOrder, INSERT_BUSES, MASTER_BUS, MAX_BUSES,
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
    ChannelPreset, ChannelSetup, ChannelSource, DrumSynthState, Kit, MonoSynthState,
    PolySynthState, Project, ProjectChannel, SampleReference, SamplerState, DEFAULT_SWING_PERCENT,
    MAX_SWING_PERCENT, MIN_SWING_PERCENT,
};
pub use sampler::{
    clamp01, LoopMode, RetriggerMode, SamplerParams, VoiceMode, MAX_CHOKE_GROUP, MAX_SAMPLER_VOICES,
};
pub use synth::{
    DrumMode, DrumSynthParams, HatCharacter, KickCharacter, LfoParams, LfoWave, MonoSynthParams,
    OscParams, OscWave, PolySynthParams, SnareCharacter, MAX_DRUM_VOICES, MAX_POLY_VOICES,
};
pub use time::{ticks_per_sample, Ppq, Samples, Ticks};
