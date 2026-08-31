//! Sampler device parameters. Pure data so the bridge can carry them.

pub const MAX_SAMPLER_VOICES: u8 = 16;
pub const MAX_CHOKE_GROUP: u8 = 16;

/// A fresh sampler's output trim: the generator output reference, as gain.
///
/// Loading or replacing a sample never touches this. The other generators
/// calibrate their own default patch to peak at
/// `gain::GENERATOR_OUTPUT_REFERENCE_DBFS`; the sampler cannot, because the
/// audio is whatever the user loaded. Spending that much headroom is the
/// closest honest equivalent -- a normalized, full-scale file then peaks
/// where a default DrumSynth hit peaks, at any pan position. It is
/// predictable headroom, not normalization: nothing measures, matches, or
/// rewrites the audio.
pub fn default_output_gain() -> f32 {
    crate::gain::reference_level_gain()
}

/// How the sampler treats the loop region once the play head reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    /// No looping. Play from `start` to `loop_end` (or sample end), then stop.
    #[default]
    Off,
    /// Loop forward: wrap from `loop_end` back to `loop_start`.
    Forward,
    /// Loop ping-pong: reverse direction at both loop points.
    Pingpong,
}

impl LoopMode {
    pub fn all() -> [LoopMode; 3] {
        [LoopMode::Off, LoopMode::Forward, LoopMode::Pingpong]
    }

    pub fn label(self) -> &'static str {
        match self {
            LoopMode::Off => "Off",
            LoopMode::Forward => "Fwd",
            LoopMode::Pingpong => "Pong",
        }
    }
}

/// How note-off events affect sample playback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceMode {
    /// Play the full region. For a looped voice, note-off exits the loop and
    /// lets the remaining sample tail play once.
    #[default]
    OneShot,
    /// Note-off enters the amplitude envelope's release stage.
    Gate,
}

/// How repeated notes of the same pitch use the voice pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetriggerMode {
    /// Replace the oldest active voice on the same pitch.
    #[default]
    Restart,
    /// Allow repeated pitches to overlap up to the polyphony limit.
    Layer,
}

impl LoopMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Forward,
            2 => Self::Pingpong,
            _ => Self::Off,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::Forward => 1,
            Self::Pingpong => 2,
        }
    }
}

impl VoiceMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Gate,
            _ => Self::OneShot,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::OneShot => 0,
            Self::Gate => 1,
        }
    }
}

impl RetriggerMode {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Layer,
            _ => Self::Restart,
        }
    }

    pub fn to_index(self) -> i32 {
        match self {
            Self::Restart => 0,
            Self::Layer => 1,
        }
    }
}

/// The four stage values an ADSR envelope runs on. Times in seconds, sustain
/// as a level in `[0, 1]`.
///
/// Named as one value because an envelope's shape is a thing a patch has,
/// not four unrelated numbers: the sampler now carries two of them, and
/// copying one into the other is what an old project's migration is. Kept
/// here rather than promoted to a shared type until a second device wants it.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EnvTimes {
    /// Attack time in seconds.
    pub attack: f32,
    /// Decay time in seconds.
    pub decay: f32,
    /// Sustain level in `[0, 1]`.
    pub sustain: f32,
    /// Release time in seconds.
    pub release: f32,
}

/// All sampler parameters, in the units the DSP and UI share. All points are
/// fractions of the sample length in `[0, 1]`; times are seconds.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SamplerParams {
    pub voice_mode: VoiceMode,
    /// Active voice limit in `1..=16`.
    pub polyphony: u8,
    pub retrigger_mode: RetriggerMode,
    /// `0` disables choking; matching non-zero groups choke each other.
    pub choke_group: u8,
    /// Play start point as a fraction of the sample length.
    pub start: f32,
    /// Play end point as a fraction of the sample length.
    pub end: f32,
    /// Play the selected region backwards.
    pub reverse: bool,
    /// Root MIDI note used for keyboard tracking.
    pub root_note: u8,
    /// Coarse tuning offset in semitones.
    pub tune_semitones: f32,
    /// Fine tuning offset in cents.
    pub tune_cents: f32,
    /// Loop start point as a fraction.
    pub loop_start: f32,
    /// Loop end point as a fraction.
    pub loop_end: f32,
    pub loop_mode: LoopMode,
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
    /// Bit-depth reduction amount in `[0, 1]`. `0` bypasses it.
    pub bit_reduction: f32,
    /// Sample-rate reduction amount in `[0, 1]`. `0` bypasses it.
    pub rate_reduction: f32,
    /// Patch-level output gain, linear, in `[0, MAX_LINEAR_GAIN]` (+12 dB).
    /// This is the sampler's own trim ahead of the channel's inserts, not the
    /// channel fader: a fresh sampler starts at `default_output_gain()`, so a
    /// full-scale commercial sample arrives level with the calibrated
    /// generators instead of well above them.
    #[serde(default = "legacy_output_gain")]
    pub output_gain: f32,
    /// The filter envelope's own stages, or `None` to follow the amplitude
    /// envelope.
    ///
    /// `None` is what every project saved before the filter envelope existed
    /// means, and it is the migration: those patches drove `filter_env_amount`
    /// from the amp ADSR, so following it reproduces their filter motion
    /// exactly rather than approximately. A fresh sampler starts there too,
    /// and materializes its own stages the moment one is edited. Absence has
    /// to be representable for this to work at all -- a plain field with a
    /// serde default could not copy the amp stages, because a default cannot
    /// see its siblings.
    #[serde(default)]
    pub filter_env: Option<EnvTimes>,
}

/// The trim a project saved before the field existed plays at. Those mixes
/// were balanced against a sampler running at unity, so they keep unity;
/// only a newly created sampler gets `default_output_gain()`.
fn legacy_output_gain() -> f32 {
    1.0
}

impl Default for SamplerParams {
    fn default() -> Self {
        Self {
            voice_mode: VoiceMode::OneShot,
            polyphony: 1,
            retrigger_mode: RetriggerMode::Restart,
            choke_group: 0,
            start: 0.0,
            end: 1.0,
            reverse: false,
            root_note: 60,
            tune_semitones: 0.0,
            tune_cents: 0.0,
            loop_start: 0.0,
            loop_end: 1.0,
            loop_mode: LoopMode::Off,
            attack: 0.001,
            decay: 0.25,
            sustain: 1.0,
            release: 0.05,
            filter_cutoff: 1.0,
            filter_resonance: 0.0,
            filter_env_amount: 0.0,
            drive: 0.0,
            bit_reduction: 0.0,
            rate_reduction: 0.0,
            output_gain: default_output_gain(),
            filter_env: None,
        }
    }
}

impl SamplerParams {
    /// The amplitude envelope's stages.
    pub fn amp_env(&self) -> EnvTimes {
        EnvTimes {
            attack: self.attack,
            decay: self.decay,
            sustain: self.sustain,
            release: self.release,
        }
    }

    /// The filter envelope's stages, resolved: its own when it has them, the
    /// amplitude envelope's when it does not. Every reader goes through here
    /// so "follows amp" is decided in one place.
    pub fn resolved_filter_env(&self) -> EnvTimes {
        self.filter_env.unwrap_or_else(|| self.amp_env())
    }

    /// Give the filter envelope its own stages, seeded from wherever it is
    /// reading now, so the first edit to one stage does not silently move the
    /// other three.
    pub fn filter_env_mut(&mut self) -> &mut EnvTimes {
        if self.filter_env.is_none() {
            self.filter_env = Some(self.resolved_filter_env());
        }
        self.filter_env.as_mut().expect("just materialized")
    }
}

/// Clamp helper used by both DSP (defensive) and UI (input validation).
pub fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A patch saved before the filter envelope existed carries no field for
    /// it, and has to come back following whatever amplitude envelope it was
    /// actually using -- not the default one. This is the migration: an old
    /// project's filter motion is reproduced exactly, because the filter is
    /// still reading the same envelope it read before.
    #[test]
    fn a_patch_without_a_filter_envelope_follows_its_own_amp_envelope() {
        let manifest = r#"
voice_mode = "one_shot"
polyphony = 1
retrigger_mode = "restart"
choke_group = 0
start = 0.0
end = 1.0
reverse = false
root_note = 60
tune_semitones = 0.0
tune_cents = 0.0
loop_start = 0.0
loop_end = 1.0
loop_mode = "off"
attack = 0.3
decay = 1.5
sustain = 0.4
release = 2.0
filter_cutoff = 0.5
filter_resonance = 0.2
filter_env_amount = 0.75
drive = 0.0
bit_reduction = 0.0
rate_reduction = 0.0
"#;
        let params: SamplerParams = toml::from_str(manifest).unwrap();
        assert_eq!(params.filter_env, None, "absence has to survive the load");
        assert_eq!(
            params.resolved_filter_env(),
            EnvTimes {
                attack: 0.3,
                decay: 1.5,
                sustain: 0.4,
                release: 2.0,
            }
        );
        // And the trim from the same era still loads at unity.
        assert_eq!(params.output_gain, 1.0);
    }

    /// Once a patch has its own filter envelope, a round trip keeps it
    /// separate from the amplitude one rather than collapsing them.
    #[test]
    fn an_owned_filter_envelope_round_trips_separately() {
        let mut params = SamplerParams {
            attack: 0.3,
            decay: 1.5,
            sustain: 0.4,
            release: 2.0,
            ..SamplerParams::default()
        };
        params.filter_env_mut().decay = 0.01;
        params.filter_env_mut().sustain = 0.0;

        let text = toml::to_string(&params).unwrap();
        let loaded: SamplerParams = toml::from_str(&text).unwrap();
        assert_eq!(loaded, params);
        assert_eq!(loaded.amp_env().decay, 1.5);
        assert_eq!(loaded.resolved_filter_env().decay, 0.01);
        assert_eq!(loaded.resolved_filter_env().attack, 0.3, "seeded from amp");
    }
}
