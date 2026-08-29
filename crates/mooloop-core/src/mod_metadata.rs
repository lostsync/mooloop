//! Source and destination metadata for the modulator system.
//!
//! `docs/MODULATOR_SYSTEM_SPEC.md` extends the existing `ParamAddr` /
//! `ModRack` foundation with two declarations:
//!
//! - A **source** publishes what it produces — shape, update rate, latency,
//!   and trigger policy — so the engine and UI can treat an LFO, a macro,
//!   and a future device outlet identically.
//! - A **destination** declares whether modulation makes sense for a
//!   parameter and how depth is interpreted, so devices add modulation
//!   support through metadata rather than matrix special cases.
//!
//! This module is metadata only: nothing here runs on the realtime thread,
//! and no persisted format changes until a consumer needs one.

use crate::effect::{ParamCurve, ParamDescriptor};
use crate::modulation::{ModPolarity, ModRack, ModulatorParams};

/// The durable identity of one modulation source, stable within its owning
/// channel across rack reorders and runtime-slot reassignment. Runtime
/// addressing stays on the bounded local slot; this is the persisted and
/// route-level vocabulary (`MODULATOR_SYSTEM_SPEC.md`, "Sources and source
/// metadata").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ModSourceId(pub u32);

/// What family a source belongs to. Only `Lfo` has runtime behavior today;
/// the rest name the planned collection so metadata and UI can already be
/// written against them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModSourceKind {
    Lfo,
    /// Clocked patterns with probability and controlled variation.
    Step,
    /// Sample-and-hold variation beyond what an LFO wave expresses.
    Random,
    /// A user macro or another declared channel value.
    Macro,
    /// Velocity, gate, key track, pressure — note-derived values.
    NoteValue,
    /// A named control signal published by a generator or effect.
    DeviceOutlet,
}

impl ModSourceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lfo => "LFO",
            Self::Step => "Step",
            Self::Random => "Random",
            Self::Macro => "Macro",
            Self::NoteValue => "Note",
            Self::DeviceOutlet => "Outlet",
        }
    }
}

/// The range a source's output lives in. The realtime convention is that
/// every source computes `-1..1` and shape decides how consumers read it,
/// so this declares intent rather than a second numeric format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalShape {
    /// Full signed swing, `-1..1`.
    Bipolar,
    /// `0..1`; negative excursions are meaningless for this source.
    Unipolar,
    /// Two states; destinations must opt in to stepped modulation.
    Gate,
    /// Quantized levels; destinations state their own quantization rules.
    Stepped,
}

/// How often a source recomputes. `Subdivision32` is the engine's existing
/// control tick; the others gate reevaluation on coarser events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlRate {
    /// Every 32-frame control subdivision.
    Subdivision32,
    /// On note events only.
    NoteEvent,
    /// Once per audio block.
    PerBlock,
    /// Only when explicitly set.
    Manual,
}

/// Declared staleness of a source's published value, in audio blocks.
/// Generator and device outlets publish one block behind; local sources are
/// immediate. Declaring this is what keeps realtime and offline renders
/// identical once outlets exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ControlLatency {
    pub blocks: u8,
}

impl ControlLatency {
    pub const IMMEDIATE: Self = Self { blocks: 0 };
    /// The mandatory one-block publish rule for outlets.
    pub const OUTLET: Self = Self { blocks: 1 };
}

/// When a source resets or advances its internal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerPolicy {
    /// Runs from load onward.
    Free,
    /// Restart on note-on: an LFO that feels played.
    NoteReset,
    /// Advance one step/phase on note-on.
    NoteAdvance,
    /// Only explicit user action moves it.
    Manual,
}

/// Everything the engine and UI need to use a source consistently.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModSourceDescriptor {
    pub id: ModSourceId,
    pub kind: ModSourceKind,
    /// User-renamable where useful.
    pub name: String,
    pub signal: SignalShape,
    pub update: ControlRate,
    pub latency: ControlLatency,
    pub trigger: TriggerPolicy,
}

impl ModSourceDescriptor {
    /// Descriptor for a local-slot LFO. Bipolar `-1..1`, computed on the
    /// 32-frame tick, immediate, and free-running unless the params opt into
    /// note retriggering.
    pub fn local_lfo(id: ModSourceId, name: impl Into<String>, retrigger: bool) -> Self {
        Self {
            id,
            kind: ModSourceKind::Lfo,
            name: name.into(),
            signal: SignalShape::Bipolar,
            update: ControlRate::Subdivision32,
            latency: ControlLatency::IMMEDIATE,
            trigger: if retrigger {
                TriggerPolicy::NoteReset
            } else {
                TriggerPolicy::Free
            },
        }
    }
}

/// How a route's depth is interpreted at the destination. The first form is
/// exactly the current `ModRoute` behavior; a musical mapping must still
/// resolve through the parameter descriptor and emit ordinary
/// `ParamValue` events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModInterpretation {
    /// Depth is a fraction of the destination's entire normalized range.
    NormalizedRange,
}

/// Optional per-destination smoothing of the summed modulation offset.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Smoothing {
    /// One-pole time constant in milliseconds.
    pub time_ms: f32,
}

/// A destination's sidecar declaration: whether the parameter accepts
/// modulation and how a route behaves when it does. Authored per device kind
/// or strip; the descriptor's range/mapping stays the single source of
/// numeric truth.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModDestinationDescriptor {
    /// The existing stable descriptor id, not a new address.
    pub param: u32,
    pub allowed: bool,
    pub interpretation: ModInterpretation,
    pub default_polarity: ModPolarity,
    /// Bounds on signed depth, as a fraction of the destination's range.
    pub depth_limit: (f32, f32),
    pub smoothing: Option<Smoothing>,
}

impl ModDestinationDescriptor {
    /// The default policy for a described parameter. Continuous targets
    /// accept `NormalizedRange` modulation at full signed depth; stepped
    /// targets — mode selectors, booleans, source pickers — opt in
    /// explicitly instead, so an LFO cannot flap a toggle or switch an
    /// algorithm by accident.
    pub fn for_param(descriptor: &ParamDescriptor) -> Self {
        Self {
            param: descriptor.id,
            allowed: !matches!(descriptor.curve, ParamCurve::Stepped(_)),
            interpretation: ModInterpretation::NormalizedRange,
            default_polarity: ModPolarity::Bipolar,
            depth_limit: (-1.0, 1.0),
            smoothing: None,
        }
    }

    /// A destination that accepts full-range modulation. For parameters that
    /// have no authored declaration yet, and for tests that are exercising
    /// route arithmetic rather than policy.
    pub const fn unrestricted(param: u32) -> Self {
        Self {
            param,
            allowed: true,
            interpretation: ModInterpretation::NormalizedRange,
            default_polarity: ModPolarity::Bipolar,
            depth_limit: (-1.0, 1.0),
            smoothing: None,
        }
    }

    /// Depth clamped into this destination's declared limit.
    pub fn clamp_depth(&self, depth: f32) -> f32 {
        depth.clamp(self.depth_limit.0, self.depth_limit.1)
    }
}

/// A route-level reference to a source. `LocalSlot` adapts to today's
/// persisted `source_slot: u8`; `Id` is the durable form future sources
/// persist instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModSourceRef {
    LocalSlot(u8),
    Id(ModSourceId),
}

impl ModSourceRef {
    /// Resolve to the bounded runtime slot, off the audio thread. Local
    /// slots are their own locator; durable ids look the source table up
    /// and fail closed when the source is gone. The table carries each
    /// source's runtime slot explicitly, because a sparse legacy rack must
    /// not compact its slot numbering.
    pub fn to_local_slot(self, sources: &[(u8, ModSourceDescriptor)]) -> Option<u8> {
        match self {
            Self::LocalSlot(slot) => Some(slot),
            Self::Id(id) => sources
                .iter()
                .find(|(_, source)| source.id == id)
                .map(|(slot, _)| *slot),
        }
    }
}

/// Legacy decode: the current `ModRack` LFO slots described as sources, so
/// existing routes keep their behavior when the collection grows. Each entry
/// carries its original runtime slot — the rack is sparse, so the slot is
/// not the position in this list. Empty slots stay out of the collection.
pub fn local_slot_sources(rack: &ModRack) -> Vec<(u8, ModSourceDescriptor)> {
    rack.slots
        .iter()
        .enumerate()
        .filter_map(|(slot, params)| {
            let retrigger = match params {
                Some(ModulatorParams::Lfo(lfo)) => lfo.retrigger,
                None => return None,
            };
            Some((
                slot as u8,
                ModSourceDescriptor::local_lfo(
                    ModSourceId(slot as u32),
                    format!("LFO {}", slot + 1),
                    retrigger,
                ),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::EffectKind;

    fn descriptor(kind: EffectKind, id: u32) -> ParamDescriptor {
        *kind.descriptor(id).unwrap()
    }

    /// Continuous parameters accept full-range normalized modulation;
    /// stepped ones — the EQ's band selector and its on toggle — do not,
    /// because a stepped destination must opt in with its own rules.
    #[test]
    fn default_destination_policy_follows_the_curve() {
        let freq = descriptor(EffectKind::Eq, crate::effect::EQ_PARAM_FREQUENCY_HZ);
        let on = descriptor(EffectKind::Eq, crate::effect::EQ_PARAM_ENABLED);
        let band = descriptor(EffectKind::Eq, crate::effect::EQ_PARAM_TARGET);

        let freq_dest = ModDestinationDescriptor::for_param(&freq);
        assert!(freq_dest.allowed);
        assert_eq!(freq_dest.interpretation, ModInterpretation::NormalizedRange);
        assert_eq!(freq_dest.default_polarity, ModPolarity::Bipolar);
        assert_eq!(freq_dest.depth_limit, (-1.0, 1.0));

        for stepped in [on, band] {
            assert!(!ModDestinationDescriptor::for_param(&stepped).allowed);
        }
    }

    #[test]
    fn depth_is_clamped_to_the_declared_limit() {
        let freq = descriptor(EffectKind::Eq, crate::effect::EQ_PARAM_FREQUENCY_HZ);
        let dest = ModDestinationDescriptor {
            depth_limit: (-0.5, 0.25),
            ..ModDestinationDescriptor::for_param(&freq)
        };
        assert_eq!(dest.clamp_depth(-2.0), -0.5);
        assert_eq!(dest.clamp_depth(0.1), 0.1);
        assert_eq!(dest.clamp_depth(3.0), 0.25);
    }

    /// Local LFO slots decode as declared sources with the trigger policy
    /// their params imply, and empty slots stay out of the collection.
    #[test]
    fn legacy_lfo_slots_decode_as_sources() {
        let mut rack = ModRack::default();
        rack.slots[1] = Some(ModulatorParams::Lfo(crate::modulation::ModLfoParams {
            retrigger: true,
            ..crate::modulation::ModLfoParams::default()
        }));

        let sources = local_slot_sources(&rack);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].0, 1);
        let source = &sources[0].1;
        assert_eq!(source.id, ModSourceId(1));
        assert_eq!(source.kind, ModSourceKind::Lfo);
        assert_eq!(source.signal, SignalShape::Bipolar);
        assert_eq!(source.update, ControlRate::Subdivision32);
        assert_eq!(source.latency, ControlLatency::IMMEDIATE);
        assert_eq!(source.trigger, TriggerPolicy::NoteReset);
        assert_eq!(source.name, "LFO 2");
    }

    /// Durable ids resolve through the descriptor table; local slots pass
    /// through unchanged; an id with no source fails closed.
    #[test]
    fn source_refs_resolve_to_runtime_slots() {
        let mut rack = ModRack::default();
        rack.slots[2] = Some(ModulatorParams::Lfo(
            crate::modulation::ModLfoParams::default(),
        ));
        let sources = local_slot_sources(&rack);

        assert_eq!(ModSourceRef::LocalSlot(1).to_local_slot(&sources), Some(1));
        assert_eq!(
            ModSourceRef::Id(ModSourceId(2)).to_local_slot(&sources),
            Some(2)
        );
        assert_eq!(
            ModSourceRef::Id(ModSourceId(0)).to_local_slot(&sources),
            None
        );
    }
    #[test]
    fn source_descriptors_round_trip_through_toml() {
        let source = ModSourceDescriptor::local_lfo(ModSourceId(0), "LFO 1", false);
        let text = toml::to_string(&source).unwrap();
        assert!(text.contains("bipolar"));
        assert_eq!(
            toml::from_str::<ModSourceDescriptor>(&text).unwrap(),
            source
        );
    }
}
