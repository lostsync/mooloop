//! Core data model and message types shared between the engine and the UI.
//!
//! No dependency on audio or UI code so this can be linked freely from both the
//! realtime and GUI threads.

pub mod automation;
pub mod bridge;
pub mod buffer;
pub mod channel;
pub mod ds01;
pub mod effect;
pub mod gain;
pub mod generator;
pub mod log;
pub mod midi;
pub mod modulation;
pub mod mixer;
pub mod mod_metadata;
pub mod mlm1;
pub mod mlm1_factory;
pub mod mlp8;
pub mod pattern;
pub mod playlist;
pub mod project;
pub mod sampler;
pub mod structure;
pub mod synth;
pub mod time;

pub use automation::{
    AutomationLane, AutomationPoint, PointId, MAX_AUTOMATION_LANES_PER_CHANNEL,
    MAX_AUTOMATION_POINTS_PER_LANE,
};
pub use bridge::{EngineCommand, EngineEvent};
pub use ds01::{
    body_mode_ratio, Ds01EnvParams, Ds01NoiseColor, Ds01Params, Ds01PitchEnvParams,
    Ds01Retrigger, DS01_BODY_HARMONIC, DS01_BODY_INHARMONIC, DS01_BODY_MODES,
    DS01_BURST_MAX_S, DS01_MAX_PARTIALS, DS01_MAX_REPEATS, DS01_VOICES,
};
pub use buffer::{BufferDuration, BufferEvent};
pub use midi::{cc_bucket, MidiKind, MidiMessage, RelativeEncoding};
pub use mlm1::{EnvTrigger, FilterModel, GlideMode, MlM1Params, NotePriority};
pub use mlp8::{
    MlP8FilterMode, MlP8LfoParams, MlP8LfoRetrigger, MlP8LfoWave, MlP8ModDest,
    MlP8ModSource, MlP8Params, MlP8Route, MlP8Routes, SubOctave, SubSource, SubWave,
    SyncSource, MLP8_MAX_ROUTES, MLP8_VOICES,
};
pub use mlm1_factory::FactoryPatch;
pub use modulation::{
    step_value_index, strip_descriptor, ModEnvelopeParams, ModLfoParams, ModLfoWaveform,
    ModMathOp, ModMathParams, ModPolarity, ModRack, ModRandomParams, ModRandomTrigger, ModRoute,
    ModStepParams, ModStepTrigger, ModTimeDivision, ModulatorKind, ModulatorParams, ParamAddr,
    ParamOwner,
    ENVELOPE_DESCRIPTORS, ENV_PARAM_AMOUNT, ENV_PARAM_ATTACK_DIVISION, ENV_PARAM_ATTACK_S,
    ENV_PARAM_ATTACK_SYNC, ENV_PARAM_DECAY_DIVISION, ENV_PARAM_DECAY_S, ENV_PARAM_DECAY_SYNC,
    ENV_PARAM_RELEASE_DIVISION, ENV_PARAM_RELEASE_S, ENV_PARAM_RELEASE_SYNC, ENV_PARAM_SUSTAIN,
    LFO_DESCRIPTORS, LFO_PARAM_DEPTH, LFO_PARAM_FADE_IN_DIVISION, LFO_PARAM_FADE_IN_S,
    LFO_PARAM_FADE_IN_SYNC, LFO_PARAM_PHASE, LFO_PARAM_PULSE_WIDTH, LFO_PARAM_RATE_DIVISION,
    LFO_PARAM_RATE_HZ, LFO_PARAM_RETRIGGER, LFO_PARAM_SMOOTHING_S, LFO_PARAM_TEMPO_SYNC,
    LFO_PARAM_WAVEFORM, MATH_DESCRIPTORS, MATH_PARAM_CLAMP_HIGH, MATH_PARAM_CLAMP_LOW,
    MATH_PARAM_INPUT_SLOT, MATH_PARAM_OP, MATH_PARAM_OPERAND, MAX_MODULATORS_PER_CHANNEL,
    MAX_MOD_ROUTES_PER_CHANNEL, MOD_STEP_MAX_STEPS, RANDOM_DESCRIPTORS, RANDOM_PARAM_BIPOLAR,
    RANDOM_PARAM_DRUNK, RANDOM_PARAM_PROBABILITY, RANDOM_PARAM_QUANTIZE,
    RANDOM_PARAM_RATE_DIVISION, RANDOM_PARAM_RATE_HZ, RANDOM_PARAM_TEMPO_SYNC,
    RANDOM_PARAM_TRIGGER, RANDOM_PARAM_WALK, STEP_DESCRIPTORS, STEP_PARAM_DIVISION,
    STEP_PARAM_GLIDE, STEP_PARAM_LENGTH, STEP_PARAM_TRIGGER, STEP_PARAM_VALUE_BASE,
    STRIP_DESCRIPTORS, STRIP_PARAM_PAN, STRIP_PARAM_VOLUME,
};
pub use mod_metadata::{
    local_slot_sources, ControlLatency, ControlRate, ModDestinationDescriptor, ModInterpretation,
    ModSourceDescriptor, ModSourceId, ModSourceKind, ModSourceRef, SignalShape, Smoothing,
    TriggerPolicy,
};
pub use generator::{
    GeneratorParams, OSC_OFFSET_CENTS, OSC_OFFSET_LEVEL, OSC_OFFSET_PULSE_WIDTH,
    OSC_OFFSET_SEMITONES, OSC_OFFSET_WAVE, SAMPLER_PARAM_ATTACK, SAMPLER_PARAM_BIT_REDUCTION,
    SAMPLER_PARAM_DECAY, SAMPLER_PARAM_DRIVE, SAMPLER_PARAM_END, SAMPLER_PARAM_FILTER_CUTOFF,
    SAMPLER_PARAM_FILTER_ENV_AMOUNT, SAMPLER_PARAM_FILTER_RESONANCE, SAMPLER_PARAM_LOOP_END,
    SAMPLER_PARAM_LOOP_MODE, SAMPLER_PARAM_LOOP_START, SAMPLER_PARAM_POLYPHONY,
    SAMPLER_PARAM_RATE_REDUCTION, SAMPLER_PARAM_RELEASE, SAMPLER_PARAM_RETRIGGER_MODE,
    SAMPLER_PARAM_REVERSE, SAMPLER_PARAM_ROOT_NOTE, SAMPLER_PARAM_START, SAMPLER_PARAM_SUSTAIN,
    SAMPLER_PARAM_TUNE_CENTS, SAMPLER_PARAM_TUNE_SEMITONES, SAMPLER_PARAM_VOICE_MODE,
    SYNTH_PARAM_ATTACK, SYNTH_PARAM_DECAY, SYNTH_PARAM_DRIVE, SYNTH_PARAM_FILTER_CUTOFF,
    SYNTH_PARAM_FILTER_ENV_AMOUNT, SYNTH_PARAM_FILTER_RESONANCE, SYNTH_PARAM_GLIDE,
    SYNTH_PARAM_LFO_RATE_HZ, SYNTH_PARAM_LFO_TO_AMP, SYNTH_PARAM_LFO_TO_FILTER,
    SYNTH_PARAM_LFO_TO_PITCH, SYNTH_PARAM_LFO_TO_PULSE_WIDTH, SYNTH_PARAM_LFO_WAVE,
    SYNTH_PARAM_POLYPHONY, SYNTH_PARAM_RELEASE, SYNTH_PARAM_SPREAD, SYNTH_PARAM_SUSTAIN,
    synth_osc_param,
};
pub use channel::{Channel, DeviceKind, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL, MAX_PATTERNS};
pub use gain::{
    db_to_linear, format_db, linear_to_db, reference_level_gain, MAX_DB, MAX_LINEAR_GAIN, METER_HOT_DB,
    METER_WARNING_DB, MIN_DB, REFERENCE_PEAK_DBFS,
};
pub use effect::{
    BitcrushParams, BitcrushStyle, BufferParams, CompressorParams, DelayMode, DelayParams,
    DelayTimeDivision, DriveCurve, DriveParams, EffectKind, EffectParams, EffectSlotState, EqBand,
    EqBandKind, EqParams, EqPassFilter, EqQProfile, EqSlope, FilterMode, FilterParams, FilterSlope,
    GateParams, LimiterParams, ModulationMode, ModulationParams, ParamCurve, ParamDescriptor,
    PlateParams, ReverbParams, BITCRUSH_PARAM_BITS, BITCRUSH_PARAM_DOWNSAMPLE, BITCRUSH_PARAM_MIX,
    BITCRUSH_PARAM_STYLE,
    BUFFER_PARAM_CROSSFADE_MS, BUFFER_PARAM_OFFSET_BEATS,
    COMP_PARAM_ATTACK_MS, COMP_PARAM_KNEE_DB, COMP_PARAM_MAKEUP_DB, COMP_PARAM_RATIO,
    COMP_PARAM_RELEASE_MS, COMP_PARAM_THRESHOLD_DB, DELAY_MAX_TIME_MS, DELAY_PARAM_CROSS,
    DELAY_PARAM_FEEDBACK, DELAY_PARAM_MIX, DELAY_PARAM_MODE, DELAY_PARAM_TIME_MS, DELAY_PARAM_TONE,
    DRIVE_PARAM_CURVE, DRIVE_PARAM_DRIVE, DRIVE_PARAM_MIX, DRIVE_PARAM_OUTPUT, DRIVE_PARAM_TONE,
    EQ_MAX_BANDS, EQ_PARAM_CHARACTER, EQ_PARAM_ENABLED, EQ_PARAM_FREQUENCY_HZ, EQ_PARAM_GAIN_DB,
    EQ_PARAM_Q, EQ_PARAM_TARGET, FILTER_PARAM_CUTOFF_HZ, FILTER_PARAM_DRIVE, FILTER_PARAM_MODE,
    FILTER_PARAM_RESONANCE, FILTER_PARAM_SLOPE,
    GATE_PARAM_ATTACK_MS, GATE_PARAM_HOLD_MS, GATE_PARAM_RANGE_DB, GATE_PARAM_RELEASE_MS,
    GATE_PARAM_THRESHOLD_DB, LIMITER_PARAM_CEILING_DB, LIMITER_PARAM_GAIN_DB,
    LIMITER_PARAM_RELEASE_MS, MODULATION_PARAM_COLOR, MODULATION_PARAM_DEPTH,
    MODULATION_PARAM_FEEDBACK, MODULATION_PARAM_MODE, MODULATION_PARAM_RATE_HZ,
    MODULATION_PARAM_SPREAD, MODULATION_PARAM_STAGES, MODULATION_PARAM_TONE, PLATE_PARAM_DAMPING,
    PLATE_PARAM_DECAY_S, PLATE_PARAM_PREDELAY_MS, PLATE_PARAM_SIZE, PLATE_PARAM_WIDTH, REVERB_PARAM_DAMPING,
    REVERB_PARAM_DECAY_S, REVERB_PARAM_DIFFUSION, REVERB_PARAM_LOW_CUT_HZ,
    REVERB_PARAM_MODULATION,
    REVERB_PARAM_PREDELAY_MS, REVERB_PARAM_SIZE, REVERB_PARAM_WIDTH,
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
    ChannelPreset, ChannelSetup, ChannelSource, Ds01State, DrumSynthState, Kit, MonoSynthState,
    MlM1State, MlP8State, PolySynthState, Project, ProjectChannel, SampleReference, SamplerState,
    DEFAULT_SWING_PERCENT, MAX_SWING_PERCENT, MIN_SWING_PERCENT,
};
pub use sampler::{
    clamp01, frames_per_bar, snap_bars_to_power_of_two, EnvTimes, LoopMode, PlayMode,
    RetriggerMode, SampleCommit, SamplerParams, SliceMap, SliceMarker, StretchMode, VoiceMode,
    DEFAULT_SLICE_BASE_NOTE, MAX_CHOKE_GROUP, MAX_SAMPLER_VOICES, MAX_SLICES, MAX_STRETCH_BARS,
    MAX_STRETCH_GRAIN, MAX_STRETCH_RATIO, MIN_STRETCH_BARS, MIN_STRETCH_GRAIN, MIN_STRETCH_RATIO,
};
pub use structure::{
    insert_effect, move_effect, remove_effect, rescope_lanes, retarget_lanes, ChannelEdit,
    SlotRemap,
};
pub use synth::{
    DrumMode, DrumSynthParams, HatCharacter, KickCharacter, LfoParams, LfoWave, MonoSynthParams,
    OscParams, OscWave, PolySynthParams, SnareCharacter, MAX_DRUM_VOICES, MAX_POLY_VOICES,
};
pub use time::{ticks_per_sample, Ppq, Samples, Ticks};
