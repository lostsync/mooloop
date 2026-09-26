//! Core data model and message types shared between the engine and the UI.
//!
//! No dependency on audio or UI code so this can be linked freely from both the
//! realtime and GUI threads.

pub mod automation;
pub mod aux_in;
pub mod bridge;
pub mod buffer;
pub mod channel;
pub mod color;
pub mod control;
pub mod ds01;
pub mod ds01_factory;
pub mod effect;
pub mod effect_factory;
pub mod file_names;
pub mod gain;
pub mod generator;
pub mod log;
pub mod input;
pub mod midi;
pub mod modulation;
pub mod mixer;
pub mod mod_metadata;
pub mod mlm1;
pub mod mlm1_factory;
pub mod mlp8_factory;
pub mod mlp8;
pub mod outlet;
pub mod pattern;
pub mod playlist;
pub mod plugin;
pub mod project;
pub mod sampler;
pub mod strip;
pub mod structure;
pub mod synth;
pub mod time;

#[cfg(test)]
mod stepped_round_trip_tests;

#[cfg(test)]
mod param_id_freeze_tests;

pub use automation::{
    AutomationLane, AutomationPoint, LanePool, PointId, MAX_AUTOMATION_LANES_PER_CHANNEL,
    MAX_AUTOMATION_POINTS_PER_LANE,
};
pub use aux_in::AuxInParams;
pub use bridge::{EngineCommand, EngineEvent, MusicalEdge};
pub use ds01::{
    body_mode_ratio, matrix_param, Ds01Character, Ds01EnvParams, Ds01ModSource, Ds01NoiseColor,
    Ds01Params, Ds01PitchEnvParams, Ds01Retrigger, Ds01Route, DS01_BITS_TRANSPARENT,
    DS01_BODY_HARMONIC, DS01_BODY_INHARMONIC, DS01_BODY_MODES, DS01_BURST_MAX_S,
    DS01_DESTINATIONS, DS01_MATRIX_ROWS, DS01_MAX_PARTIALS, DS01_MAX_REPEATS, DS01_VOICES,
};
pub use buffer::{BufferDuration, BufferEvent};
pub use control::{
    ClaimedNotes, ControlBinding, ControlLearn, ControlMap, ControlMapState, ControlMode,
    ControlOutcome, ControlSource, ControlTarget, ControlValue, PickupState, Takeover,
    TransportControl,
};
pub use input::{
    audio_input_is_off, audio_input_taps, audio_source_rows, AudioInputPicker, AudioInputSource,
    AudioSourceRow, AudioTap, RecordFace, SamplerRecord, MAX_RECORD_BARS,
};
pub use midi::{
    cc_bucket, ChannelMidiInput, MidiChannelFilter, MidiInputRoute, MidiInputSource, MidiKind,
    MidiMessage, MidiPortFilter, MidiPortId, MidiPortInfo, MidiPortMatch, MidiRouteSource,
    RelativeEncoding, MIDI_CHANNEL_FILTER_ROWS, SYSTEM_CHANNEL,
};
pub use mlm1::{EnvTrigger, FilterModel, GlideMode, MlM1Params, NotePriority};
pub use mlp8::{
    route_descriptor as mlp8_route_descriptor, MlP8Chorus, MlP8FilterMode, MlP8LfoParams,
    MlP8LfoRetrigger, MlP8LfoWave, MlP8ModDest, MlP8ModSource, MlP8Params, MlP8Route, MlP8Routes,
    MlP8Unison, SubOctave, SubSource, SubWave, SyncSource, MLP8_MAX_ROUTES, MLP8_MOD_DESTS,
    MLP8_ROUTE_PARAM_AMOUNT, MLP8_VOICES,
};
pub use effect_factory::{EffectFactoryPatch, EffectFactoryRun};
pub use plugin::{
    mint_plugin_slot, PluginFormat, PluginParamInfo, PluginRef, PluginSlotId, PluginSlotState, PluginSlots,
    PluginState, PluginStateChunk, PluginStateText,
};
pub use mlm1_factory::FactoryPatch;
pub use modulation::{
    step_value_index, strip_descriptor, ModEnvelopeParams, ModLfoParams, ModLfoWaveform,
    ModMathOp, ModMathParams, ModPolarity, ModRack, ModRandomParams, ModRandomTrigger, ModRoute,
    ModStepParams, ModStepTrigger, ModTimeDivision, ModulatorKind, ModulatorParams, ParamAddr,
    ParamKey, ParamOwner,
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
pub use strip::{
    strip_band_of, strip_band_param, strip_band_shelf, StripBand, StripParams, STRIP_BAND_BASE,
    STRIP_BAND_POSITIONS,
    STRIP_BAND_FREQ, STRIP_BAND_GAIN, STRIP_BAND_KIND, STRIP_BAND_Q, STRIP_BAND_STRIDE,
    STRIP_COMP_ATTACK_MS, STRIP_COMP_IN, STRIP_COMP_KNEE_DB,
    STRIP_COMP_MAKEUP_DB, STRIP_COMP_MIX, STRIP_COMP_RATIO, STRIP_COMP_RELEASE_MS,
    STRIP_COMP_THRESHOLD_DB, STRIP_DRIVE_DB, STRIP_EQ_BANDS, STRIP_EQ_IN, STRIP_FIRST,
    STRIP_PRE_IN, STRIP_VOICING,
};
pub use mod_metadata::{
    local_slot_sources, ControlLatency, ControlRate, ModDestinationDescriptor, ModInterpretation,
    ModSourceDescriptor, ModSourceId, ModSourceKind, ModSourceRef, SignalShape, Smoothing,
    TriggerPolicy,
};
pub use outlet::{
    audio_tap_index, AudioSubscription, OutletDescriptor, OutletDomain, OutletTap,
    PublishesOutlets, MAX_DEVICE_AUDIO_TAPS,
};
pub use generator::{
    ENV_MAX_SECONDS, ENV_MIN_SECONDS,
    DRUM_PARAM_DECAY, DRUM_PARAM_DRIVE, DRUM_PARAM_HAT_CHARACTER, DRUM_PARAM_HAT_HP_HZ,
    DRUM_PARAM_HAT_METALLIC, DRUM_PARAM_KICK_CHARACTER, DRUM_PARAM_KICK_CLICK,
    DRUM_PARAM_KICK_END_HZ, DRUM_PARAM_KICK_START_HZ, DRUM_PARAM_KICK_SWEEP, DRUM_PARAM_MODE,
    DRUM_PARAM_PUNCH, DRUM_PARAM_SNARE_CHARACTER, DRUM_PARAM_SNARE_NOISE_COLOR,
    DRUM_PARAM_SNARE_NOISE_DECAY, DRUM_PARAM_SNARE_NOISE_MIX, DRUM_PARAM_SNARE_TONE2_HZ,
    DRUM_PARAM_SNARE_TONE2_MIX, DRUM_PARAM_SNARE_TONE_HZ, DRUM_PARAM_TUNE_SEMITONES,
    DRUM_TUNE_RANGE,
    GeneratorParams, OSC_CENT_RANGE, OSC_OFFSET_CENTS, OSC_OFFSET_LEVEL, OSC_OFFSET_PULSE_WIDTH,
    OSC_OFFSET_SEMITONES, OSC_OFFSET_WAVE, OSC_SEMITONE_RANGE,
    SAMPLER_PARAM_ATTACK, SAMPLER_PARAM_BIT_REDUCTION,
    SAMPLER_PARAM_DECAY, SAMPLER_PARAM_DRIVE, SAMPLER_PARAM_END, SAMPLER_PARAM_FILTER_ATTACK,
    SAMPLER_PARAM_FILTER_CUTOFF, SAMPLER_PARAM_FILTER_DECAY, SAMPLER_PARAM_FILTER_RELEASE,
    SAMPLER_PARAM_FILTER_ENV_AMOUNT, SAMPLER_PARAM_FILTER_RESONANCE, SAMPLER_PARAM_LOOP_END,
    SAMPLER_PARAM_LOOP_MODE, SAMPLER_PARAM_LOOP_START, SAMPLER_PARAM_POLYPHONY,
    SAMPLER_PARAM_RATE_REDUCTION, SAMPLER_PARAM_RELEASE, SAMPLER_PARAM_RETRIGGER_MODE,
    SAMPLER_PARAM_REVERSE, SAMPLER_PARAM_ROOT_NOTE, SAMPLER_PARAM_START, SAMPLER_PARAM_SUSTAIN,
    SAMPLER_PARAM_TUNE_CENTS, SAMPLER_PARAM_TUNE_SEMITONES, SAMPLER_PARAM_VOICE_MODE,
    SAMPLER_TUNE_CENT_CLAMP, SAMPLER_TUNE_SEMITONE_CLAMP,
    SYNTH_PARAM_ATTACK, SYNTH_PARAM_DECAY, SYNTH_PARAM_DRIVE, SYNTH_PARAM_FILTER_ATTACK,
    SYNTH_PARAM_FILTER_CUTOFF, SYNTH_PARAM_FILTER_DECAY, SYNTH_PARAM_FILTER_RELEASE,
    SYNTH_PARAM_FILTER_ENV_AMOUNT, SYNTH_PARAM_FILTER_RESONANCE, SYNTH_PARAM_GLIDE,
    SYNTH_PARAM_LFO_RATE_HZ, SYNTH_PARAM_LFO_TO_AMP, SYNTH_PARAM_LFO_TO_FILTER,
    SYNTH_PARAM_LFO_TO_PITCH, SYNTH_PARAM_LFO_TO_PULSE_WIDTH, SYNTH_PARAM_LFO_WAVE,
    SYNTH_PARAM_POLYPHONY, SYNTH_PARAM_RELEASE, SYNTH_PARAM_SPREAD, SYNTH_PARAM_SUSTAIN,
    synth_osc_param,
};
pub use channel::{
    Channel, DeviceKind, DEFAULT_CHANNEL_VOLUME, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL,
    MAX_PATTERNS,
};
pub use color::ProjectColor;
pub use gain::{
    db_to_linear, db_to_linear_unfloored, format_db, linear_to_db, linear_to_db_unfloored,
    reference_level_gain, DETECTOR_MIN_LEVEL, MAX_DB, MAX_LINEAR_GAIN, METER_HOT_DB,
    METER_WARNING_DB, MIN_DB, REFERENCE_PEAK_DBFS,
};
pub use effect::{
    BitcrushParams, BitcrushStyle, BufferParams, CompressorParams, DelayMode, DelayParams,
    ContainerFlow, ContainerParams, ChannelId, DeviceId, TrackId, track_id_is_unassigned, DriveCurve, DriveParams, EffectKind, EffectParams,
    EffectRun, EffectSlotState, eq_effective_q, EqBand,
    EqBandKind, EqParams, EqPassFilter, EqQProfile, EqSlope, FilterMode, FilterParams, FilterSlope,
    GateParams, LimiterParams, ModulationMode, ModulationParams, ParamCurve, ParamDescriptor,
    PlateParams, PreampParams, PreampVoicing, ReverbParams, BITCRUSH_PARAM_BITS, BITCRUSH_PARAM_DOWNSAMPLE, BITCRUSH_PARAM_MIX,
    BITCRUSH_PARAM_STYLE,
    bus_comp_master_id, BusCompParams, BUS_COMP_PARAM_COUNT, BUS_COMP_PARAM_GRIP_ATTACK,
    BUS_COMP_PARAM_GRIP_RATIO, BUS_COMP_PARAM_GRIP_RELEASE, BUS_COMP_PARAM_MAKEUP_DB,
    BUS_COMP_PARAM_MIX, BUS_COMP_PARAM_PUNCH_ATTACK, BUS_COMP_PARAM_PUNCH_RATIO,
    BUS_COMP_PARAM_PUNCH_RELEASE, BUS_COMP_PARAM_THRESHOLD_DB, BUS_COMP_PARAM_TUBE_TIME,
    BUS_COMP_PARAM_VOICING,
    BUFFER_PARAM_CROSSFADE_MS, BUFFER_PARAM_FREEZE, BUFFER_PARAM_JUMP, BUFFER_PARAM_LENGTH,
    BUFFER_PARAM_LOOP, BUFFER_PARAM_OFFSET_BEATS, BUFFER_PARAM_POSITION,
    BUFFER_PARAM_QUANTIZE, BUFFER_PARAM_QUANT_START, BUFFER_PARAM_JUMP_BACK,
    BUFFER_PARAM_REVERSE, BUFFER_PARAM_STUTTER, BUFFER_PARAM_STUTTER_LENGTH,
    BUFFER_PARAM_POSITION_SPAN, BUFFER_SPAN_FULL, BUFFER_PARAM_RATE,
    CONTAINER_PARAM_LEVEL, CONTAINER_PARAM_MIX, CONTAINER_PARAM_MUTE, CONTAINER_PARAM_SOLO,
    MOD_TIME_DIVISION_TOP,
    COMP_PARAM_ATTACK_MS, COMP_PARAM_KNEE_DB, COMP_PARAM_MAKEUP_DB, COMP_PARAM_MIX, COMP_PARAM_RATIO,
    COMP_PARAM_RELEASE_MS, COMP_PARAM_THRESHOLD_DB, DELAY_MAX_TIME_MS, DELAY_PARAM_CROSS,
    DELAY_PARAM_FEEDBACK, DELAY_PARAM_MIX, DELAY_PARAM_MODE, DELAY_PARAM_TIME_MS, DELAY_PARAM_TONE,
    MODULATION_MAX_RATE_HZ, MODULATION_MIN_RATE_HZ,
    DRIVE_PARAM_CURVE, DRIVE_PARAM_DRIVE, DRIVE_PARAM_MIX, DRIVE_PARAM_OUTPUT, DRIVE_PARAM_TONE,
    eq_band_of, eq_band_param, eq_pass_of, eq_pass_param, eq_plot_frequency,
    eq_plot_position, EqFaceControl, EQ_BAND_BASE, EQ_PLOT_MAX_HZ, EQ_PLOT_MIN_HZ,
    EQ_BAND_FIELDS, EQ_BAND_FREQ, EQ_BAND_GAIN, EQ_BAND_KIND, EQ_BAND_ON, EQ_BAND_Q,
    EQ_BAND_Q_PROFILE, EQ_BAND_STRIDE, EQ_FIRST, EQ_HIGH_PASS, EQ_LOW_PASS, EQ_MAX_BANDS,
    EQ_PASS_BASE, EQ_PASS_COUNT, EQ_PASS_FIELDS, EQ_PASS_FREQ, EQ_PASS_ON, EQ_PASS_Q,
    EQ_PASS_SLOPE, EQ_PASS_STRIDE, EQ_FACE_CONTROLS, EQ_SLOPE_COUNT,
    EQ_DEFAULT_BAND_HZ, EQ_DEFAULT_BAND_KIND, EQ_DEFAULT_Q,
    PASS_STAGE_DB_PER_OCTAVE,
    PREAMP_PARAM_DRIVE_DB, PREAMP_PARAM_MIX, PREAMP_PARAM_OUTPUT_DB, PREAMP_PARAM_VOICING,
    FILTER_PARAM_CUTOFF_HZ, FILTER_PARAM_DRIVE, FILTER_PARAM_MODE,
    FILTER_PARAM_RESONANCE, FILTER_PARAM_SLOPE,
    GATE_PARAM_ATTACK_MS, GATE_PARAM_HOLD_MS, GATE_PARAM_RANGE_DB, GATE_PARAM_RELEASE_MS,
    GATE_PARAM_THRESHOLD_DB, LIMITER_PARAM_CEILING_DB, LIMITER_PARAM_GAIN_DB,
    LIMITER_PARAM_RELEASE_MS, MODULATION_PARAM_COLOR, MODULATION_PARAM_DEPTH,
    MODULATION_PARAM_FEEDBACK, MODULATION_PARAM_MODE, MODULATION_PARAM_RATE_HZ,
    MODULATION_PARAM_SPREAD, MODULATION_PARAM_STAGES, MODULATION_PARAM_TONE, MODULATION_PARAM_WIDTH,
    PLATE_PARAM_DAMPING,
    PLATE_PARAM_DECAY_S, PLATE_PARAM_PREDELAY_MS, PLATE_PARAM_SIZE, PLATE_PARAM_WIDTH, REVERB_PARAM_DAMPING,
    REVERB_PARAM_DECAY_S, REVERB_PARAM_DIFFUSION, REVERB_PARAM_LOW_CUT_HZ,
    REVERB_PARAM_MODULATION,
    REVERB_PARAM_PREDELAY_MS, REVERB_PARAM_SIZE, REVERB_PARAM_WIDTH,
};
pub use mixer::{
    branch_alignment, branch_alignment_with, chain_latency, chain_latency_with,
    container_rings_with, layer_branches, run_latency, run_latency_with, ContainerRing, clamp_bus, compensable_send_edges, compile_audio_graph,
    compile_bus_graph, compile_latency,
    compile_render_order, default_buses, default_render_order, is_legal_route, is_legal_send,
    sanitize_bank, sanitize_route, send_edges, sends_are_compensable,
    would_create_cycle, AudioEdge, AudioOrder, AuxSend, BankRepair, BusSetup, CompiledAudioGraph,
    CompiledBusGraph,
    ChainKey, CompiledLatency, EdgeRefusal, EffectTarget, MixerBus, RenderOrder, SanitizedBank,
    SendEdge, SendTap,
    INSERT_BUSES, MASTER_BUS, MAX_BUSES,
};
pub use pattern::{
    ChannelPattern, NoteEvent, NoteId, Pattern, Step, DEFAULT_NOTE_DURATION_TICKS, DEFAULT_STEPS,
    MAX_NOTES_PER_CHANNEL_PATTERN, MAX_PATTERN_STEPS, STEPS_PER_BEAT, TICKS_PER_64TH,
    TICKS_PER_STEP,
};
pub use playlist::{
    LoopRange, PatternPlacement, PlaybackMode, MAX_PLAYLIST_BARS, MAX_PLAYLIST_PLACEMENTS,
    MAX_PLAYLIST_TICKS, STARTER_LOOP_BARS, STEPS_PER_BAR, TICKS_PER_BAR,
};
pub use project::{
    AuxInState, ChannelPreset, ChannelSetup, ChannelSource, Ds01State, DrumSynthState, Kit,
    MonoSynthState,
    MlM1State, MlP8State, PatternMeta, PolySynthState, Project, ProjectChannel, SampleReference,
    SamplerState, trim_pattern_meta, track_move_allowed,
    DEFAULT_SWING_PERCENT, MAX_SWING_PERCENT, MIN_SWING_PERCENT, STRIP_VOLUME_TAPER,
    STRIP_VOLUME_TAPER_LINEAR,
};
pub use sampler::{
    clamp01, frames_per_bar, snap_bars_to_power_of_two, zone_for_note, EnvTimes, KeyRange,
    LoopMode, PlayMode, RetriggerMode, SampleCommit, SampleZone, SamplerParams, VelocityRange,
    ZoneChoice, SliceMap, SliceMarker, StretchMode, VoiceMode,
    DEFAULT_SLICE_BASE_NOTE, MAX_CHOKE_GROUP, MAX_SAMPLER_VOICES, MAX_SLICES, MAX_STRETCH_BARS,
    MAX_STRETCH_GRAIN, MAX_STRETCH_RATIO, MIN_STRETCH_BARS, MIN_STRETCH_GRAIN, MIN_STRETCH_RATIO,
};
pub use structure::{
    assign_device_ids, depth_at, device_slot, drop_lanes_for_device, insert_effect,
    mint_channel_id, mint_track_id,
    append_into_container, can_insert_into_container, can_move_into_container, can_wrap,
    insert_into_container, insert_run,
    insert_run_beside, mint_device_id, move_sequence, parent_of, run_of, span_of, span_problem,
    unwrap_container, wrap_in_container,
    MAX_CONTAINER_DEPTH, move_effect, move_effect_into_container, remove_effect,
    replace_run, rescope_lanes, rescope_lanes_for_track, slot_of, ChannelEdit, ListEdit, TrackEdit,
};
pub use synth::{
    DrumMode, DrumSynthParams, HatCharacter, KickCharacter, LfoParams, LfoWave, MonoSynthParams,
    OscParams, OscWave, PolySynthParams, SnareCharacter, MAX_DRUM_VOICES, MAX_POLY_VOICES,
};
pub use time::{
    ticks_per_sample, BbtDuration, BbtPosition, Ppq, Samples, Ticks, BEATS_PER_BAR,
};
