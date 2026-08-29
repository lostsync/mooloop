//! Parameter addressing and the modulator rack.
//!
//! `docs/MODULATION_PLAN.md` is the approved design; this implements it.
//! Two ideas carry the whole thing:
//!
//! - A parameter is named by a [`ParamAddr`], not by a bespoke command per
//!   device kind. One address type is what makes an automation lane, a mod
//!   matrix row, and a knob all talk about the same thing.
//! - The engine owns a **base** value and the sum of **modulation offsets**,
//!   and emits the resolved sum. Devices store only resolved values, so no
//!   effect needs any change to support modulation.

use crate::effect::{ParamCurve, ParamDescriptor};
use crate::gain::MAX_LINEAR_GAIN;
use crate::mod_metadata::ModDestinationDescriptor;
use crate::EffectTarget;

/// Which device inside a channel or bus owns the parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamOwner {
    /// The channel's generator. Buses have none.
    Source,
    Effect {
        slot: u8,
    },
    Modulator {
        slot: u8,
    },
    /// Volume, pan, mute — the strip itself rather than a device on it.
    Strip,
}

/// A parameter, anywhere in the project.
///
/// `scope` carries the channel or bus from the day this type exists, so
/// enabling cross-channel modulation later is a routing change rather than a
/// retyping of every engine command (`MODULATION_PLAN.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ParamAddr {
    pub scope: EffectTarget,
    pub owner: ParamOwner,
    /// The owning kind's stable descriptor id. Never renumbered, because
    /// modulation and automation persist it.
    pub param: u32,
}

impl ParamAddr {
    pub const fn effect(scope: EffectTarget, slot: u8, param: u32) -> Self {
        Self {
            scope,
            owner: ParamOwner::Effect { slot },
            param,
        }
    }

    pub const fn strip(scope: EffectTarget, param: u32) -> Self {
        Self {
            scope,
            owner: ParamOwner::Strip,
            param,
        }
    }
}

/// The strip's own parameters. The strip is addressed like any device, so its
/// controls need stable descriptor ids too -- that is what lets a source
/// target a fader without the mixer growing a modulation special case
/// (`MODULATOR_SYSTEM_SPEC.md`, "Destinations and destination metadata").
pub const STRIP_PARAM_VOLUME: u32 = 0;
pub const STRIP_PARAM_PAN: u32 = 1;

pub static STRIP_DESCRIPTORS: [ParamDescriptor; 2] = [
    ParamDescriptor {
        id: STRIP_PARAM_VOLUME,
        name: "Volume",
        // Linear rather than the fader's display taper: modulation depth is a
        // fraction of the normalized range, and the taper belongs to the
        // control surface, not to the destination's numeric truth.
        unit: "x",
        min: 0.0,
        max: MAX_LINEAR_GAIN,
        curve: ParamCurve::Linear,
        default: 0.8,
    },
    ParamDescriptor {
        id: STRIP_PARAM_PAN,
        name: "Pan",
        unit: "",
        min: -1.0,
        max: 1.0,
        curve: ParamCurve::Linear,
        default: 0.0,
    },
];

pub fn strip_descriptor(id: u32) -> Option<&'static ParamDescriptor> {
    STRIP_DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id == id)
}

/// Shape of a free-running LFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModLfoWaveform {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    /// Stepped random, held between transitions. Sample-and-hold as a wave
    /// rather than a separate modulator kind.
    Random,
}

/// A modulator's own parameters. Modulators are addressable like any other
/// device, so these have descriptor ids too.
pub const LFO_PARAM_RATE_HZ: u32 = 0;
pub const LFO_PARAM_DEPTH: u32 = 1;
pub const LFO_PARAM_WAVEFORM: u32 = 2;
pub const LFO_PARAM_PHASE: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModLfoParams {
    pub rate_hz: f32,
    /// Output scale, `0..1`. Per-destination depth is separate and lives in
    /// the matrix row; this is the modulator's own level.
    pub depth: f32,
    pub waveform: ModLfoWaveform,
    /// Starting phase in `0..1`, applied on reset.
    pub phase: f32,
    /// Restart the phase on note-on. What makes an LFO feel played rather
    /// than merely running.
    pub retrigger: bool,
}

impl Default for ModLfoParams {
    fn default() -> Self {
        Self {
            rate_hz: 1.0,
            depth: 1.0,
            waveform: ModLfoWaveform::Sine,
            phase: 0.0,
            retrigger: false,
        }
    }
}

/// One modulator slot's configuration.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ModulatorParams {
    Lfo(ModLfoParams),
}

impl ModulatorParams {
    pub fn kind(self) -> ModulatorKind {
        match self {
            Self::Lfo(_) => ModulatorKind::Lfo,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModulatorKind {
    Lfo,
}

impl ModulatorKind {
    pub const ALL: [ModulatorKind; 1] = [ModulatorKind::Lfo];

    pub fn label(self) -> &'static str {
        match self {
            Self::Lfo => "LFO",
        }
    }

    pub fn default_params(self) -> ModulatorParams {
        match self {
            Self::Lfo => ModulatorParams::Lfo(ModLfoParams::default()),
        }
    }
}

/// How a source's `-1..1` output is applied to a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPolarity {
    /// The full signed swing, centred on the base value.
    #[default]
    Bipolar,
    /// Only the positive half, so the base value is the floor.
    Unipolar,
}

/// One matrix row: a modulator slot driving one parameter.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModRoute {
    pub source_slot: u8,
    pub destination: ParamAddr,
    /// Signed, `-1..1`, as a fraction of the destination's full range. The
    /// drag depth of the assignment gesture.
    pub depth: f32,
    pub polarity: ModPolarity,
}

/// Fixed rack size. Four slots per channel, matching the rack UI and keeping
/// a channel a self-contained instrument (`MODULATION_PLAN.md`).
pub const MAX_MODULATORS_PER_CHANNEL: usize = 4;
/// Ceiling on matrix rows per channel. Bounded so evaluation is a fixed cost
/// and the whole rack stays `Copy`.
pub const MAX_MOD_ROUTES_PER_CHANNEL: usize = 16;

/// One channel's complete modulation state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModRack {
    pub slots: [Option<ModulatorParams>; MAX_MODULATORS_PER_CHANNEL],
    pub routes: [Option<ModRoute>; MAX_MOD_ROUTES_PER_CHANNEL],
}

/// The persisted form stays sparse: TOML has no `null`, so serializing the
/// fixed realtime arrays directly would either fail or write sixteen empty
/// rows. Slot numbers make an absent entry unambiguous and leave room for the
/// rack capacity to grow without changing a saved route's meaning.
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedModRack {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    slots: Vec<SavedModulatorSlot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    routes: Vec<ModRoute>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedModulatorSlot {
    slot: u8,
    params: ModulatorParams,
}

impl serde::Serialize for ModRack {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let slots = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, params)| {
                params.map(|params| SavedModulatorSlot {
                    slot: slot as u8,
                    params,
                })
            })
            .collect();
        let routes = self.routes.iter().flatten().copied().collect();
        SavedModRack { slots, routes }.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for ModRack {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let saved = SavedModRack::deserialize(deserializer)?;
        let mut rack = Self::default();
        for saved_slot in saved.slots {
            if let Some(slot) = rack.slots.get_mut(saved_slot.slot as usize) {
                *slot = Some(saved_slot.params);
            }
        }
        for route in saved.routes {
            let _ = rack.add_route(route);
        }
        Ok(rack)
    }
}

impl Default for ModRack {
    fn default() -> Self {
        Self {
            slots: [None; MAX_MODULATORS_PER_CHANNEL],
            routes: [None; MAX_MOD_ROUTES_PER_CHANNEL],
        }
    }
}

impl ModRack {
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Add a route, returning its index. Returns `None` when the matrix is
    /// full rather than silently dropping the assignment.
    pub fn add_route(&mut self, route: ModRoute) -> Option<usize> {
        // An existing row for the same pair is retuned rather than doubled:
        // dragging depth on an already-assigned knob must not stack a second
        // route on top of the first.
        if let Some(index) = self.routes.iter().position(|existing| {
            existing.is_some_and(|existing| {
                existing.source_slot == route.source_slot
                    && existing.destination == route.destination
            })
        }) {
            self.routes[index] = Some(route);
            return Some(index);
        }
        let index = self.routes.iter().position(Option::is_none)?;
        self.routes[index] = Some(route);
        Some(index)
    }

    pub fn remove_route(&mut self, source_slot: u8, destination: ParamAddr) {
        for route in self.routes.iter_mut() {
            if route.is_some_and(|route| {
                route.source_slot == source_slot && route.destination == destination
            }) {
                *route = None;
            }
        }
    }

    /// Total signed offset applied to `destination`, as a fraction of its
    /// range, given each slot's current output and the destination's declared
    /// policy.
    ///
    /// The policy is the gate, not a suggestion: a destination that refuses
    /// modulation contributes nothing however many routes name it, and each
    /// route's depth is clamped into the declared limit before it sums. An
    /// illegal route is therefore inert rather than deleted -- the spec keeps
    /// it as inspectable authored work.
    pub fn offset_for(
        &self,
        destination: ParamAddr,
        outputs: &[f32; MAX_MODULATORS_PER_CHANNEL],
        policy: &ModDestinationDescriptor,
    ) -> f32 {
        if !policy.allowed {
            return 0.0;
        }
        let mut total = 0.0;
        for route in self.routes.iter().flatten() {
            if route.destination != destination {
                continue;
            }
            let Some(output) = outputs.get(route.source_slot as usize) else {
                continue;
            };
            let shaped = match route.polarity {
                ModPolarity::Bipolar => *output,
                // Half the swing, lifted, so the base value is the floor
                // rather than the midpoint.
                ModPolarity::Unipolar => (*output + 1.0) * 0.5,
            };
            total += shaped * policy.clamp_depth(route.depth);
        }
        total
    }

    /// Whether a control signal will actually resolve `destination` this
    /// block. This must agree with [`ModRack::offset_for`]: the engine uses it
    /// to decide whether to suppress a knob's base write, and a route parked
    /// on a destination that refuses modulation must not hold that knob
    /// hostage.
    pub fn modulates(&self, destination: ParamAddr, policy: &ModDestinationDescriptor) -> bool {
        policy.allowed && self.destinations().any(|address| address == destination)
    }

    /// Every destination this rack drives, for the UI's "which knobs are
    /// modulated" pass. Includes routes whose destination currently refuses
    /// modulation, because the inspector still has to show them.
    pub fn destinations(&self) -> impl Iterator<Item = ParamAddr> + '_ {
        self.routes.iter().flatten().map(|route| route.destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(param: u32) -> ParamAddr {
        ParamAddr::effect(EffectTarget::Channel(0), 0, param)
    }

    fn open(param: u32) -> ModDestinationDescriptor {
        ModDestinationDescriptor::unrestricted(param)
    }

    /// Re-assigning the same source to the same destination retunes the
    /// existing row. Otherwise dragging depth on an assigned knob would
    /// stack routes until the matrix filled up.
    #[test]
    fn reassigning_a_pair_retunes_rather_than_stacking() {
        let mut rack = ModRack::default();
        let route = ModRoute {
            source_slot: 0,
            destination: addr(7),
            depth: 0.25,
            polarity: ModPolarity::Bipolar,
        };
        assert_eq!(rack.add_route(route), Some(0));
        assert_eq!(
            rack.add_route(ModRoute {
                depth: 0.75,
                ..route
            }),
            Some(0)
        );
        assert_eq!(rack.routes.iter().flatten().count(), 1);
        assert_eq!(rack.routes[0].unwrap().depth, 0.75);

        // A different source to the same destination is a separate row.
        assert_eq!(
            rack.add_route(ModRoute {
                source_slot: 1,
                ..route
            }),
            Some(1)
        );
        assert_eq!(rack.routes.iter().flatten().count(), 2);
    }

    #[test]
    fn a_full_matrix_refuses_rather_than_dropping_silently() {
        let mut rack = ModRack::default();
        for param in 0..MAX_MOD_ROUTES_PER_CHANNEL as u32 {
            assert!(rack
                .add_route(ModRoute {
                    source_slot: 0,
                    destination: addr(param),
                    depth: 1.0,
                    polarity: ModPolarity::Bipolar,
                })
                .is_some());
        }
        assert_eq!(
            rack.add_route(ModRoute {
                source_slot: 0,
                destination: addr(999),
                depth: 1.0,
                polarity: ModPolarity::Bipolar,
            }),
            None
        );
    }

    /// Offsets from several sources sum, and polarity decides whether the
    /// base value sits at the centre of the swing or at its floor.
    #[test]
    fn offsets_sum_and_polarity_shapes_the_swing() {
        let mut rack = ModRack::default();
        rack.add_route(ModRoute {
            source_slot: 0,
            destination: addr(1),
            depth: 0.5,
            polarity: ModPolarity::Bipolar,
        });
        rack.add_route(ModRoute {
            source_slot: 1,
            destination: addr(1),
            depth: 1.0,
            polarity: ModPolarity::Unipolar,
        });
        let mut outputs = [0.0; MAX_MODULATORS_PER_CHANNEL];

        // Both sources at full negative: bipolar swings down, unipolar rests
        // on the base value.
        outputs[0] = -1.0;
        outputs[1] = -1.0;
        assert_eq!(rack.offset_for(addr(1), &outputs, &open(1)), -0.5);

        // Both at full positive.
        outputs[0] = 1.0;
        outputs[1] = 1.0;
        assert_eq!(rack.offset_for(addr(1), &outputs, &open(1)), 1.5);

        // An unrelated destination is untouched.
        assert_eq!(rack.offset_for(addr(2), &outputs, &open(2)), 0.0);
    }

    /// The destination's declaration is the gate. A route parked on a
    /// parameter that refuses modulation resolves to nothing and does not
    /// count as modulating it -- otherwise it would hold that knob hostage,
    /// suppressing the base write while contributing no movement. A narrowed
    /// depth limit clamps the route rather than trusting the stored depth.
    #[test]
    fn the_destination_policy_gates_and_clamps_its_routes() {
        let mut rack = ModRack::default();
        rack.add_route(ModRoute {
            source_slot: 0,
            destination: addr(1),
            depth: 1.0,
            polarity: ModPolarity::Bipolar,
        });
        let mut outputs = [0.0; MAX_MODULATORS_PER_CHANNEL];
        outputs[0] = 1.0;

        let refused = ModDestinationDescriptor {
            allowed: false,
            ..open(1)
        };
        assert_eq!(rack.offset_for(addr(1), &outputs, &refused), 0.0);
        assert!(!rack.modulates(addr(1), &refused));
        // The route is still authored work: the inspector must be able to
        // show it, so it stays in the rack's destination list.
        assert!(rack.destinations().any(|address| address == addr(1)));

        let narrowed = ModDestinationDescriptor {
            depth_limit: (-0.2, 0.2),
            ..open(1)
        };
        assert_eq!(rack.offset_for(addr(1), &outputs, &narrowed), 0.2);
        assert!(rack.modulates(addr(1), &narrowed));
    }

    /// The strip's own controls are described destinations like any device's,
    /// which is what lets a source reach a fader without the mixer growing a
    /// modulation special case.
    #[test]
    fn strip_parameters_are_ordinary_modulation_destinations() {
        let volume = strip_descriptor(STRIP_PARAM_VOLUME).unwrap();
        let pan = strip_descriptor(STRIP_PARAM_PAN).unwrap();
        assert!(ModDestinationDescriptor::for_param(volume).allowed);
        assert!(ModDestinationDescriptor::for_param(pan).allowed);
        assert_eq!(volume.from_normalized(0.0), 0.0);
        assert_eq!(volume.from_normalized(1.0), MAX_LINEAR_GAIN);
        // Centre pan sits at the middle of the normalized range, so a bipolar
        // route swings evenly to both sides of it.
        assert_eq!(pan.to_normalized(0.0), 0.5);
        assert_eq!(
            ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_PAN).owner,
            ParamOwner::Strip
        );
    }

    #[test]
    fn removing_a_route_leaves_its_neighbours() {
        let mut rack = ModRack::default();
        for slot in 0..2u8 {
            rack.add_route(ModRoute {
                source_slot: slot,
                destination: addr(1),
                depth: 1.0,
                polarity: ModPolarity::Bipolar,
            });
        }
        rack.remove_route(0, addr(1));
        let remaining: Vec<_> = rack.routes.iter().flatten().collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].source_slot, 1);
    }

    #[test]
    fn sparse_rack_round_trips_through_toml() {
        let mut rack = ModRack::default();
        rack.slots[2] = Some(ModulatorParams::Lfo(ModLfoParams {
            rate_hz: 3.5,
            ..ModLfoParams::default()
        }));
        rack.add_route(ModRoute {
            source_slot: 2,
            destination: addr(4),
            depth: -0.75,
            polarity: ModPolarity::Unipolar,
        });

        let text = toml::to_string(&rack).unwrap();
        assert!(text.contains("slot = 2"));
        assert!(!text.contains("null"));
        assert_eq!(toml::from_str::<ModRack>(&text).unwrap(), rack);
    }
}
