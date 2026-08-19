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
pub struct DrumSynthParams {
    pub mode: DrumMode,
    /// `0` disables choking; matching non-zero groups choke each other.
    pub choke_group: u8,
    /// Master decay of the amplitude envelope (seconds).
    pub decay: f32,
    /// Keyboard tracking: semitone offset from MIDI note 60 applied to the
    /// tonal parts (kick/snare body, hat squares).
    pub tune_semitones: f32,
    /// Soft saturation in `[0, 1]`. `0` bypasses it.
    pub drive: f32,

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
    /// Noise vs body mix in `[0, 1]`. `0` is pure tone, `1` is pure noise.
    pub snare_noise_mix: f32,
    /// Noise burst decay (seconds), independent of the body decay.
    pub snare_noise_decay: f32,

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
            choke_group: 0,
            decay: 0.35,
            tune_semitones: 0.0,
            drive: 0.0,
            kick_start_hz: 160.0,
            kick_end_hz: 48.0,
            kick_sweep: 0.045,
            kick_click: 0.5,
            snare_tone_hz: 180.0,
            snare_noise_mix: 0.6,
            snare_noise_decay: 0.15,
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
                params.decay = 0.18;
            }
            DrumMode::Hat => {
                params.decay = 0.05;
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
}

impl Default for MonoSynthParams {
    fn default() -> Self {
        Self {
            osc: [
                OscParams {
                    level: 0.8,
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
        }
    }
}
