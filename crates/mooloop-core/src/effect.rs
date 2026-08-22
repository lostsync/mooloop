//! Effect types: the per-slot state persisted in a project and the parameter
//! sets the DSP nodes consume. Effects are chainable units that run after a
//! channel's generator; see `docs/EFFECTS_PLAN.md`.

/// Effect kind. With only one kind shipped, `EffectSlotState::params` stays
/// concrete (`FilterParams`); widen it into a tagged `EffectParams` enum when
/// a second kind lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Filter,
}

/// Filter mode: low-pass attenuates above the cutoff, high-pass below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterMode {
    #[default]
    LowPass,
    HighPass,
}

/// Parameters for the filter effect (`FilterEffect` in `mooloop-dsp`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FilterParams {
    /// Cutoff frequency in Hz, clamped by the DSP to [20, sample_rate * 0.45].
    pub cutoff_hz: f32,
    /// Resonance in `[0, 1]`, approaching self-oscillation at the top.
    pub resonance: f32,
    pub mode: FilterMode,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            cutoff_hz: 8_000.0,
            resonance: 0.0,
            mode: FilterMode::default(),
        }
    }
}

/// Persisted state of one slot in a channel's effect chain.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EffectSlotState {
    pub kind: EffectKind,
    pub params: FilterParams,
    pub bypassed: bool,
}

impl EffectSlotState {
    pub fn filter(params: FilterParams) -> Self {
        Self {
            kind: EffectKind::Filter,
            params,
            bypassed: false,
        }
    }
}
