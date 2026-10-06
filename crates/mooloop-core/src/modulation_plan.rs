//! The song's modulation as the engine runs it.
//!
//! [`SongModulation`] is the document's: modules and routes that name
//! channels, modules and devices by identity. The audio thread cannot look an
//! identity up, so this is the same set resolved once, off the audio thread,
//! against the seats of one song (`docs/plans/archive/song-modulation/02`):
//!
//! - every module gets a **list position**, which is the order the engine
//!   ticks them in. A Math module reads the module it names at that
//!   position: one listed before it this tick, one listed after it a tick
//!   late, the rule a rack's slots had;
//! - every input is the **seat** whose notes it hears;
//! - every route is **filed under the chain it lands on**, so a device asks
//!   the routes onto its own chain rather than walking every route in the
//!   song. Within a chain the routes keep the song's order, so their offsets
//!   sum in the order a channel's rack summed them.
//!
//! A route whose source the song does not have, or whose channel or chain is
//! not there, is left out: it would resolve to nothing, and the document still
//! holds it.
//!
//! The session derives one of these from the document and compares it with
//! the one it last sent, the way it reconciles the audio graph; the engine
//! builds one when it builds a project. `PartialEq` is that comparison.

use crate::mixer::{EffectTarget, MAX_BUSES};
use crate::mod_metadata::{ModDestinationDescriptor, ModSourceId, ModSourceRef};
use crate::modulation::{
    InputSource, ModPolarity, ModulatorParams, ParamAddr, SongModulation, MAX_GENERATOR_OUTLETS,
    PERFORMANCE_SOURCES,
};
use crate::{ChannelId, MAX_CHANNELS};

/// How many chains a route can land on: every channel, then every track.
pub const MODULATION_CHAINS: usize = MAX_CHANNELS + MAX_BUSES;

/// Where the routes onto `scope` are filed, or `None` for a seat past the
/// ends of the bank.
pub fn chain_index(scope: EffectTarget) -> Option<usize> {
    match scope {
        EffectTarget::Channel(seat) => {
            let seat = usize::from(seat);
            (seat < MAX_CHANNELS).then_some(seat)
        }
        EffectTarget::Bus(seat) => {
            let seat = usize::from(seat);
            (seat < MAX_BUSES).then_some(MAX_CHANNELS + seat)
        }
    }
}

/// One module as the engine runs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledModule {
    pub id: ModSourceId,
    pub params: ModulatorParams,
    /// What its random generator is seeded from ([`crate::SongModule::seed`]).
    pub seed: u32,
    /// What its input names, by identity. Kept beside the resolved seat so
    /// that a set compiled after a channel move can tell "the same input, at
    /// a new seat" from "a different input".
    pub input: InputSource,
    /// The seat of the channel whose notes it hears, for the four kinds that
    /// hear notes.
    pub gate: Option<u8>,
    /// The list position of the module a Math module reads.
    pub reads: Option<u16>,
}

/// A route's source, resolved to where the engine reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompiledSource {
    /// A module, by list position.
    Module(u16),
    /// A generator's published outlet, by the seat of its channel.
    Outlet { seat: u8, outlet: u8 },
    /// The keyboard's mod wheel or aftertouch on a channel, by its seat.
    Performance { seat: u8, source: u8 },
}

/// One route as the engine resolves it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledRoute {
    /// What the document names the source by: with `destination`, the key a
    /// narrow edit finds the route by.
    pub source: ModSourceRef,
    pub resolved: CompiledSource,
    pub destination: ParamAddr,
    pub depth: f32,
    pub polarity: ModPolarity,
}

/// The song's modulation resolved against one song's seats. See the module.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompiledModulation {
    pub modules: Vec<CompiledModule>,
    /// Grouped by the chain each lands on, in chain order; the song's order
    /// within a chain.
    pub routes: Vec<CompiledRoute>,
    /// `routes[chains[c]..chains[c + 1]]` land on chain `c`
    /// ([`chain_index`]). Empty for a set with no routes at all.
    chains: Vec<u32>,
}

impl CompiledModulation {
    /// Resolve `song` against the seats `seat_of` gives each channel.
    pub fn compile(song: &SongModulation, seat_of: impl Fn(ChannelId) -> Option<u8>) -> Self {
        let position_of = |id: ModSourceId| {
            song.modules
                .iter()
                .position(|module| module.id == id)
                .and_then(|position| u16::try_from(position).ok())
        };
        let modules = song
            .modules
            .iter()
            .filter_map(|module| {
                // A list position is a u16; a song with more modules than
                // that runs the first 65,535.
                position_of(module.id)?;
                let math = matches!(module.params, ModulatorParams::Math(_));
                Some(CompiledModule {
                    id: module.id,
                    params: module.params,
                    seed: module.seed,
                    input: module.input,
                    gate: module
                        .input
                        .channel()
                        .filter(|_| !math)
                        .and_then(&seat_of),
                    reads: module
                        .input
                        .module()
                        .filter(|_| math)
                        .and_then(position_of),
                })
            })
            .collect::<Vec<_>>();

        let mut filed: Vec<(usize, CompiledRoute)> = song
            .routes
            .iter()
            .filter_map(|route| {
                let chain = chain_index(route.destination.scope)?;
                let resolved = match route.source {
                    ModSourceRef::Id(id) => {
                        CompiledSource::Module(position_of(id).filter(|&at| usize::from(at) < modules.len())?)
                    }
                    ModSourceRef::GeneratorOutlet { channel, outlet } => {
                        let outlet = u8::try_from(outlet)
                            .ok()
                            .filter(|&outlet| usize::from(outlet) < MAX_GENERATOR_OUTLETS)?;
                        CompiledSource::Outlet {
                            seat: seat_of(channel)?,
                            outlet,
                        }
                    }
                    ModSourceRef::Performance { channel, source } => {
                        let source = u8::try_from(source)
                            .ok()
                            .filter(|&source| usize::from(source) < PERFORMANCE_SOURCES)?;
                        CompiledSource::Performance {
                            seat: seat_of(channel)?,
                            source,
                        }
                    }
                    ModSourceRef::LocalSlot(_) => return None,
                };
                Some((
                    chain,
                    CompiledRoute {
                        source: route.source,
                        resolved,
                        destination: route.destination,
                        depth: route.depth,
                        polarity: route.polarity,
                    },
                ))
            })
            .collect();
        if filed.is_empty() {
            return Self {
                modules,
                ..Self::default()
            };
        }
        // Stable, so a chain's routes keep the song's order.
        filed.sort_by_key(|(chain, _)| *chain);
        let mut chains = vec![0u32; MODULATION_CHAINS + 1];
        for (chain, _) in &filed {
            chains[chain + 1] += 1;
        }
        for chain in 0..MODULATION_CHAINS {
            chains[chain + 1] += chains[chain];
        }
        Self {
            modules,
            routes: filed.into_iter().map(|(_, route)| route).collect(),
            chains,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty() && self.routes.is_empty()
    }

    /// The routes landing on `scope`, in the song's order.
    pub fn chain_routes(&self, scope: EffectTarget) -> &[CompiledRoute] {
        let Some(chain) = chain_index(scope) else {
            return &[];
        };
        match (self.chains.get(chain), self.chains.get(chain + 1)) {
            (Some(&start), Some(&end)) => &self.routes[start as usize..end as usize],
            _ => &[],
        }
    }

    /// The list position of module `id`.
    pub fn position_of(&self, id: ModSourceId) -> Option<usize> {
        self.modules.iter().position(|module| module.id == id)
    }

    /// The route from `source` onto `destination`.
    pub fn route_mut(
        &mut self,
        source: ModSourceRef,
        destination: ParamAddr,
    ) -> Option<&mut CompiledRoute> {
        let chain = chain_index(destination.scope)?;
        let (start, end) = (*self.chains.get(chain)?, *self.chains.get(chain + 1)?);
        self.routes[start as usize..end as usize]
            .iter_mut()
            .find(|route| route.source == source && route.destination == destination)
    }

    /// Whether `other` differs from this set only in module parameters and
    /// in route depths and polarities -- what a narrow edit can carry -- and
    /// not in which modules there are, in what order, hearing what, or in
    /// which routes there are.
    pub fn same_shape(&self, other: &Self) -> bool {
        let module = |module: &CompiledModule| {
            (
                module.id,
                module.params.kind(),
                module.seed,
                module.input,
                module.gate,
                module.reads,
            )
        };
        let route = |route: &CompiledRoute| (route.source, route.resolved, route.destination);
        self.chains == other.chains
            && self.modules.len() == other.modules.len()
            && self.routes.len() == other.routes.len()
            && self
                .modules
                .iter()
                .zip(&other.modules)
                .all(|(a, b)| module(a) == module(b))
            && self
                .routes
                .iter()
                .zip(&other.routes)
                .all(|(a, b)| route(a) == route(b))
    }

    /// How far a source's wire output can travel from zero, which a unipolar
    /// route lifts by so the base is the floor: an LFO's depth, and the full
    /// span for everything else (`ModRack::offset_for`'s rule).
    pub fn wire_span(&self, source: CompiledSource) -> f32 {
        match source {
            CompiledSource::Module(at) => match self.modules.get(usize::from(at)).map(|module| module.params) {
                Some(ModulatorParams::Lfo(lfo)) => lfo.depth.clamp(0.0, 1.0),
                _ => 1.0,
            },
            CompiledSource::Outlet { .. } | CompiledSource::Performance { .. } => 1.0,
        }
    }

    /// Total signed offset on `destination`, as a fraction of its range,
    /// given each source's current output (`level`): the sum the engine's
    /// control pass makes, for the knobs to draw. A destination whose policy
    /// refuses modulation takes nothing, each depth is clamped into the
    /// declared limit, and a unipolar route is lifted onto its source's span.
    pub fn offset_for(
        &self,
        destination: ParamAddr,
        policy: &ModDestinationDescriptor,
        level: impl Fn(CompiledSource) -> f32,
    ) -> f32 {
        if !policy.allowed {
            return 0.0;
        }
        let mut total = 0.0;
        for route in self.chain_routes(destination.scope) {
            if route.destination != destination {
                continue;
            }
            let output = level(route.resolved);
            let shaped = match route.polarity {
                ModPolarity::Bipolar => output,
                ModPolarity::Unipolar => (output + self.wire_span(route.resolved)) * 0.5,
            };
            total += shaped * policy.clamp_depth(route.depth);
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulation::{ModLfoParams, ModMathParams, ModRoute, SongModule};
    use crate::DeviceId;

    fn module(id: u32, params: ModulatorParams, input: InputSource) -> SongModule {
        SongModule {
            id: ModSourceId(id),
            name: String::new(),
            seed: id,
            input,
            rack: None,
            params,
        }
    }

    fn route(source: ModSourceRef, scope: EffectTarget, param: u32) -> ModRoute {
        ModRoute {
            source,
            source_slot: crate::modulation::UNRESOLVED_SLOT,
            destination: ParamAddr::effect(scope, DeviceId(0), param),
            depth: 0.5,
            polarity: ModPolarity::Bipolar,
        }
    }

    /// Routes are filed under the chain they land on, a track's included,
    /// and keep the song's order within it; inputs become seats and a Math
    /// module's operand a list position.
    #[test]
    fn a_set_is_filed_by_chain_with_inputs_as_seats() {
        let lfo = ModulatorParams::Lfo(ModLfoParams::default());
        let song = SongModulation {
            modules: vec![
                module(7, lfo, InputSource::ChannelNotes(ChannelId(30))),
                module(
                    3,
                    ModulatorParams::Math(ModMathParams::default()),
                    InputSource::Module(ModSourceId(7)),
                ),
            ],
            routes: vec![
                route(ModSourceRef::Id(ModSourceId(3)), EffectTarget::Bus(2), 1),
                route(ModSourceRef::Id(ModSourceId(7)), EffectTarget::Channel(1), 4),
                route(ModSourceRef::Id(ModSourceId(7)), EffectTarget::Bus(2), 0),
                // No such module: left out.
                route(ModSourceRef::Id(ModSourceId(99)), EffectTarget::Channel(1), 5),
                route(
                    ModSourceRef::GeneratorOutlet {
                        channel: ChannelId(30),
                        outlet: 2,
                    },
                    EffectTarget::Channel(1),
                    6,
                ),
            ],
            next_source_id: 8,
        };
        let seat_of = |id: ChannelId| (id == ChannelId(30)).then_some(1);
        let plan = CompiledModulation::compile(&song, seat_of);
        assert_eq!(plan.modules[0].gate, Some(1));
        assert_eq!(plan.modules[1].reads, Some(0));
        assert_eq!(plan.modules[1].gate, None);
        let params = |scope| -> Vec<u32> {
            plan.chain_routes(scope)
                .iter()
                .map(|route| route.destination.param)
                .collect()
        };
        assert_eq!(params(EffectTarget::Channel(1)), [4, 6]);
        assert_eq!(params(EffectTarget::Bus(2)), [1, 0]);
        assert!(params(EffectTarget::Channel(0)).is_empty());
        assert_eq!(
            plan.chain_routes(EffectTarget::Channel(1))[1].resolved,
            CompiledSource::Outlet { seat: 1, outlet: 2 }
        );

        // A depth is a narrow edit; a route more is not.
        let mut deeper = song.clone();
        deeper.routes[1].depth = 0.9;
        assert!(plan.same_shape(&CompiledModulation::compile(&deeper, seat_of)));
        let mut wider = song;
        wider.routes.push(route(ModSourceRef::Id(ModSourceId(3)), EffectTarget::Channel(0), 2));
        assert!(!plan.same_shape(&CompiledModulation::compile(&wider, seat_of)));
    }
}
