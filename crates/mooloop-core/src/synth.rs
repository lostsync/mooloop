//! Synth device parameters. Pure data so the bridge can carry them, mirroring
//! how `SamplerParams` lives here rather than in the DSP crate.

/// Upper bound on simultaneous drum synth voices.
pub const MAX_DRUM_VOICES: u8 = 8;

/// Which drum sound a `DrumSynth` channel produces. One mode per channel,
/// matching the one-sound-per-channel groovebox model the sampler uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrumMode {
    #[default]
    Kick,
    Snare,
    Hat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KickCharacter {
    Sub,
    Punch,
    Deep,
    #[default]
    Kit,
    Dnb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnareCharacter {
    #[default]
    Pop,
    Snap,
    Power,
    Clap,
    Rim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HatCharacter {
    Soft,
    #[default]
    Tight,
    Metal,
    Sizzle,
    Trash,
}

impl DrumMode {
    pub fn all() -> [DrumMode; 3] {
        [DrumMode::Kick, DrumMode::Snare, DrumMode::Hat]
    }

    pub fn label(self) -> &'static str {
        match self {
            DrumMode::Kick => "Kick",
            DrumMode::Snare => "Snare",
            DrumMode::Hat => "Hat",
        }
    }
}

/// All drum synth parameters, in the units the DSP and UI share. Knobs that
/// belong to other modes are inert but retained, so switching modes never
/// loses an authored setting.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DrumSynthParams {
    pub mode: DrumMode,
    pub kick_character: KickCharacter,
    pub snare_character: SnareCharacter,
    pub hat_character: HatCharacter,
    /// `0` disables choking; matching non-zero groups choke each other.
    pub choke_group: u8,
    /// Master decay of the amplitude envelope (seconds).
    pub decay: f32,
    /// Keyboard tracking: semitone offset from MIDI note 60 applied to the
    /// tonal parts (kick/snare body, hat squares).
    pub tune_semitones: f32,
    /// Soft saturation in `[0, 1]`. `0` bypasses it.
    pub drive: f32,
    /// Short transient emphasis in `[0, 1]`.
    pub punch: f32,

    // Kick
    /// Sweep start frequency (Hz) at the moment of impact.
    pub kick_start_hz: f32,
    /// Sweep end frequency (Hz) the body settles into.
    pub kick_end_hz: f32,
    /// Pitch sweep time (seconds) from start to end frequency.
    pub kick_sweep: f32,
    /// Beater click level in `[0, 1]`.
    pub kick_click: f32,

    // Snare
    /// Body oscillator frequency (Hz).
    pub snare_tone_hz: f32,
    /// Secondary body tone frequency (Hz).
    pub snare_tone2_hz: f32,
    /// Secondary tone level in `[0, 1]`.
    pub snare_tone2_mix: f32,
    /// Noise vs body mix in `[0, 1]`. `0` is pure tone, `1` is pure noise.
    pub snare_noise_mix: f32,
    /// Noise burst decay (seconds), independent of the body decay.
    pub snare_noise_decay: f32,
    /// Noise brightness in `[0, 1]`.
    pub snare_noise_color: f32,

    // Hat
    /// High-pass cutoff (Hz) shaping the noise.
    pub hat_hp_hz: f32,
    /// Blend of detuned metallic squares vs white noise in `[0, 1]`.
    pub hat_metallic: f32,
}

impl Default for DrumSynthParams {
    fn default() -> Self {
        Self {
            mode: DrumMode::Kick,
            kick_character: KickCharacter::Kit,
            snare_character: SnareCharacter::Pop,
            hat_character: HatCharacter::Tight,
            choke_group: 0,
            decay: 0.24,
            tune_semitones: 0.0,
            drive: 0.0,
            punch: 0.35,
            kick_start_hz: 160.0,
            kick_end_hz: 48.0,
            kick_sweep: 0.045,
            kick_click: 0.5,
            snare_tone_hz: 180.0,
            snare_tone2_hz: 330.0,
            snare_tone2_mix: 0.2,
            snare_noise_mix: 0.6,
            snare_noise_decay: 0.11,
            snare_noise_color: 0.65,
            hat_hp_hz: 7500.0,
            hat_metallic: 0.5,
        }
    }
}

impl DrumSynthParams {
    /// A musically sane starting point for each mode. Mode-specific knobs are
    /// tuned for the selected mode; the rest stay at their defaults.
    pub fn preset(mode: DrumMode) -> Self {
        let mut params = Self {
            mode,
            ..Self::default()
        };
        match mode {
            DrumMode::Kick => {}
            DrumMode::Snare => {
                params.decay = 0.14;
                params.punch = 0.45;
            }
            DrumMode::Hat => {
                params.decay = 0.045;
                params.punch = 0.15;
            }
        }
        params
    }
}

/// Oscillator waveform for the mono synth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OscWave {
    Sine,
    Triangle,
    #[default]
    Saw,
    Pulse,
}

impl OscWave {
    pub fn all() -> [OscWave; 4] {
        [
            OscWave::Sine,
            OscWave::Triangle,
            OscWave::Saw,
            OscWave::Pulse,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            OscWave::Sine => "Sine",
            OscWave::Triangle => "Tri",
            OscWave::Saw => "Saw",
            OscWave::Pulse => "Pulse",
        }
    }
}


impl OscWave {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Triangle,
            2 => Self::Saw,
            3 => Self::Pulse,
            _ => Self::Sine,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Sine => 0,
            Self::Triangle => 1,
            Self::Saw => 2,
            Self::Pulse => 3,
        }
    }
}

impl LfoWave {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Triangle,
            2 => Self::Saw,
            3 => Self::Square,
            4 => Self::Random,
            _ => Self::Sine,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Sine => 0,
            Self::Triangle => 1,
            Self::Saw => 2,
            Self::Square => 3,
            Self::Random => 4,
        }
    }
}

/// One mono synth oscillator's contribution.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OscParams {
    pub wave: OscWave,
    /// Coarse pitch offset in semitones.
    pub semitones: f32,
    /// Fine pitch offset in cents.
    pub cents: f32,
    /// Oscillator mix level in `[0, 1]`.
    pub level: f32,
    /// Pulse width in `[0.05, 0.95]`; only used by the `Pulse` wave.
    pub pulse_width: f32,
}

impl Default for OscParams {
    fn default() -> Self {
        Self {
            wave: OscWave::Saw,
            semitones: 0.0,
            cents: 0.0,
            level: 0.0,
            pulse_width: 0.5,
        }
    }
}

/// Mono synth LFO waveform. Bipolar shapes start at zero so a retriggered
/// LFO never steps the modulation at the note boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LfoWave {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    /// Sample and hold: one random value per cycle.
    Random,
}

impl LfoWave {
    pub fn all() -> [LfoWave; 5] {
        [
            LfoWave::Sine,
            LfoWave::Triangle,
            LfoWave::Saw,
            LfoWave::Square,
            LfoWave::Random,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            LfoWave::Sine => "Sine",
            LfoWave::Triangle => "Tri",
            LfoWave::Saw => "Saw",
            LfoWave::Square => "Square",
            LfoWave::Random => "S&H",
        }
    }
}

/// The mono synth's single LFO. One shared shape with a depth per
/// destination, so no destination selector is needed and several targets can
/// move together.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LfoParams {
    pub wave: LfoWave,
    /// Rate in Hz, `[0.01, 20]`.
    pub rate_hz: f32,
    /// Restart the LFO phase on every note-on. Off is free running.
    pub retrigger: bool,
    /// Pitch depth in semitones, bipolar, `[-24, 24]`.
    pub to_pitch: f32,
    /// Filter cutoff depth in octaves, bipolar, `[-4, 4]`.
    pub to_filter: f32,
    /// Pulse width depth, bipolar, `[-0.45, 0.45]`; added to every
    /// oscillator's own width.
    pub to_pulse_width: f32,
    /// Tremolo depth in `[0, 1]`. Modulates downward from unity gain.
    pub to_amp: f32,
}

impl Default for LfoParams {
    fn default() -> Self {
        Self {
            wave: LfoWave::Sine,
            rate_hz: 5.0,
            retrigger: false,
            to_pitch: 0.0,
            to_filter: 0.0,
            to_pulse_width: 0.0,
            to_amp: 0.0,
        }
    }
}

/// All mono synth parameters, in the units the DSP and UI share. Three
/// oscillators into one low-pass filter with a shared ADSR, mono voice with
/// glide. Note duration and note-off semantics are honored (gate), per the
/// project's event-model-first rule.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MonoSynthParams {
    pub osc: [OscParams; 3],
    /// Portamento time (seconds). `0` is instant pitch changes.
    pub glide: f32,
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
    /// One LFO into pitch, filter, pulse width, and amplitude. Defaulted on
    /// load so manifests written before the LFO existed stay readable.
    #[serde(default)]
    pub lfo: LfoParams,
}

impl Default for MonoSynthParams {
    fn default() -> Self {
        Self {
            osc: [
                OscParams {
                    // Wide open: the default patch IS the reference patch,
                    // calibrated against `gain::REFERENCE_PEAK_DBFS` with the
                    // oscillator's knob at its 0 dB top.
                    level: 1.0,
                    ..OscParams::default()
                },
                OscParams {
                    semitones: 12.0,
                    cents: 4.0,
                    level: 0.0,
                    ..OscParams::default()
                },
                OscParams {
                    semitones: -12.0,
                    cents: -4.0,
                    level: 0.0,
                    ..OscParams::default()
                },
            ],
            glide: 0.0,
            attack: 0.005,
            decay: 0.2,
            sustain: 0.7,
            release: 0.15,
            filter_cutoff: 1.0,
            filter_resonance: 0.0,
            filter_env_amount: 0.0,
            drive: 0.0,
            lfo: LfoParams::default(),
        }
    }
}

/// Upper bound on simultaneous poly synth voices.
pub const MAX_POLY_VOICES: u8 = 16;

/// All poly synth parameters, in the units the DSP and UI share. A pool of
/// independent voices, each with three oscillators, a shared ADSR, a
/// filter, and a stereo spread. The LFO is global and runs into the same
/// destinations as on the mono synth.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PolySynthParams {
    pub osc: [OscParams; 3],
    /// Portamento time (seconds). `0` is instant pitch changes.
    pub glide: f32,
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
    /// One LFO into pitch, filter, pulse width, and amplitude.
    pub lfo: LfoParams,
    /// Active voice count in `[1, MAX_POLY_VOICES]`.
    pub polyphony: u8,
    /// Stereo spread in `[0, 1]`. `0` is mono; `1` pans voices hard across the
    /// field with the centre voice(s) staying centred.
    pub spread: f32,
}

impl Default for PolySynthParams {
    fn default() -> Self {
        Self {
            osc: [
                OscParams {
                    // Wide open: the default patch IS the reference patch,
                    // calibrated against `gain::REFERENCE_PEAK_DBFS` with the
                    // oscillator's knob at its 0 dB top.
                    level: 1.0,
                    ..OscParams::default()
                },
                OscParams {
                    semitones: 12.0,
                    cents: 4.0,
                    level: 0.0,
                    ..OscParams::default()
                },
                OscParams {
                    semitones: -12.0,
                    cents: -4.0,
                    level: 0.0,
                    ..OscParams::default()
                },
            ],
            glide: 0.0,
            attack: 0.005,
            decay: 0.2,
            sustain: 0.7,
            release: 0.15,
            filter_cutoff: 1.0,
            filter_resonance: 0.0,
            filter_env_amount: 0.0,
            drive: 0.0,
            lfo: LfoParams::default(),
            polyphony: 8,
            spread: 0.0,
        }
    }
}
