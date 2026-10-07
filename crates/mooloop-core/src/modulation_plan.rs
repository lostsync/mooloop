//! The song's modulation as the engine runs it.
//!
//! [`SongModulation`] is the document's: modules and routes that name
//! channels, modules and devices by identity. The audio thread cannot look an
//! identity up, so this is the same set resolved once, off the audio thread,
//! against the seats of one song (`docs/plans/archive/song-modulation/02`):
//!
//! - every module gets a **list position**, which is where its output is
//!   read by routes and meters, and every tag a node index after the
//!   modules;
//! - the patch's wires become each box's **inlets**, and the boxes and tags
//!   a **tick order**: topological over the wires, ties in the file's order,
//!   tags first (`docs/plans/song-patch/02-the-engine-runs-a-graph.md`). A
//!   wire that would close a loop, and a wire saved [`crate::Wire::late`],
//!   reads its outlet as of the previous tick;
//! - every gate tag is the **seat** whose notes it hears;
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
    ModPolarity, ModulatorParams, ParamAddr, SongModulation, MAX_GENERATOR_OUTLETS,
    PERFORMANCE_SOURCES,
};
use crate::patch::{InletSource, JackSort, TagKind};
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

/// The most inlets a box has. Inlet storage is a fixed array so a module
/// stays `Copy` on the audio thread; the jack table decides how many of
/// them a kind uses.
pub const MAX_INLETS: usize = 9;

/// The wire into one inlet, as the engine reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledInlet {
    /// The box or tag it comes from, by identity: what tells "the same
    /// wire, from a node that moved in the order" from "a different wire".
    pub from: ModSourceId,
    /// That node's index: a module's list position, or a tag's index past
    /// the last module.
    pub node: u16,
    /// Read the outlet as of the previous control tick.
    pub delayed: bool,
}

/// One module as the engine runs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledModule {
    pub id: ModSourceId,
    pub params: ModulatorParams,
    /// What its random generator is seeded from ([`crate::SongModule::seed`]).
    pub seed: u32,
    /// The wire into each inlet, by port ([`crate::ModulatorKind::ports`]).
    pub inlets: [Option<CompiledInlet>; MAX_INLETS],
}

/// One tag as the engine runs it, `None` for a tag that sends nothing (an
/// empty slot, a notes-out tag, a channel the song no longer has).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledTag {
    pub id: ModSourceId,
    pub source: Option<TagSource>,
}

/// What a tag reads, resolved to seats (song patch step 06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagSource {
    /// A seat's notes: a gate tag, or a notes-in tag. A notes-in tag's
    /// wires go to note inlets, which no box runs before step 07; it is
    /// heard so the canvas can show its notes.
    Gate(u8),
    /// A seat's generator outlet, by its place in the published outlets.
    Outlet { seat: u8, outlet: u8 },
    /// A seat's mod wheel or aftertouch.
    Performance { seat: u8, source: u8 },
    Beat,
    Bar,
    PatternPosition,
    Pattern,
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

/// A route from a box onto a knob of another box's face (song patch step
/// 05), as the set runs it: before the box ticks, the knob is offset from
/// its setting by the source's output, as a route offsets a device's knob.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompiledKnob {
    /// The box whose knob it is, by list position.
    pub module: u16,
    pub param: u32,
    /// The box that moves it, by list position. Its output is read as the
    /// tick has it when the knob's box runs: this tick's when the order ran
    /// it first, else the tick before's.
    pub source: u16,
    /// Clamped into the knob's declared limit.
    pub depth: f32,
    pub polarity: ModPolarity,
}

/// The song's modulation resolved against one song's seats. See the module.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompiledModulation {
    pub modules: Vec<CompiledModule>,
    pub tags: Vec<CompiledTag>,
    /// Every node index (modules, then tags) in the order a control tick
    /// runs them.
    pub order: Vec<u16>,
    /// Grouped by the chain each lands on, in chain order; the song's order
    /// within a chain.
    pub routes: Vec<CompiledRoute>,
    /// `routes[chains[c]..chains[c + 1]]` land on chain `c`
    /// ([`chain_index`]). Empty for a set with no routes at all.
    chains: Vec<u32>,
    /// Every route onto a box's knob that the box takes, grouped by the box
    /// it moves and then by knob, the song's order within a knob. Knobs that refuse
    /// modulation (a stepped setting) and routes from anything but a box
    /// are left out.
    pub knobs: Vec<CompiledKnob>,
}

impl CompiledModulation {
    /// Resolve `song` against the seats `seat_of` gives each channel.
    pub fn compile(song: &SongModulation, seat_of: impl Fn(ChannelId) -> Option<u8>) -> Self {
        // A node index is a u16; a song with more boxes and tags than that
        // runs the first 65,535 of them.
        let module_count = song.modules.len().min(usize::from(u16::MAX));
        let tag_count = song.tags.len().min(usize::from(u16::MAX) - module_count);
        let mut nodes: std::collections::HashMap<ModSourceId, u16> =
            std::collections::HashMap::with_capacity(module_count + tag_count);
        for (index, module) in song.modules.iter().take(module_count).enumerate() {
            nodes.entry(module.id).or_insert(index as u16);
        }
        for (index, tag) in song.tags.iter().take(tag_count).enumerate() {
            nodes.entry(tag.id).or_insert((module_count + index) as u16);
        }
        let position_of = |id: ModSourceId| nodes.get(&id).copied().filter(|&at| usize::from(at) < module_count);

        let mut modules: Vec<CompiledModule> = song
            .modules
            .iter()
            .take(module_count)
            .map(|module| CompiledModule {
                id: module.id,
                params: module.params,
                seed: module.seed,
                inlets: [None; MAX_INLETS],
            })
            .collect();
        let tags: Vec<CompiledTag> = song
            .tags
            .iter()
            .take(tag_count)
            .map(|tag| CompiledTag {
                id: tag.id,
                source: match tag.kind {
                    TagKind::Inlet { bind: Some(source) } => match source {
                        InletSource::Gate(channel) => seat_of(channel).map(TagSource::Gate),
                        InletSource::Outlet { channel, outlet } => seat_of(channel)
                            .zip(u8::try_from(outlet).ok())
                            .filter(|&(_, outlet)| usize::from(outlet) < crate::modulation::MAX_GENERATOR_OUTLETS)
                            .map(|(seat, outlet)| TagSource::Outlet { seat, outlet }),
                        InletSource::Performance { channel, source } => seat_of(channel)
                            .zip(u8::try_from(source).ok())
                            .filter(|&(_, source)| usize::from(source) < crate::modulation::PERFORMANCE_SOURCES)
                            .map(|(seat, source)| TagSource::Performance { seat, source }),
                        InletSource::Beat => Some(TagSource::Beat),
                        InletSource::Bar => Some(TagSource::Bar),
                        InletSource::PatternPosition => Some(TagSource::PatternPosition),
                        InletSource::Pattern => Some(TagSource::Pattern),
                    },
                    TagKind::NotesIn {
                        channel: Some(channel),
                        ..
                    } => seat_of(channel).map(TagSource::Gate),
                    _ => None,
                },
            })
            .collect();

        // Each control wire into a module's inlet. A wire the song should
        // not hold (integrity drops it on load) is left out here too.
        for wire in &song.wires {
            let Some(to) = position_of(wire.to.node) else { continue };
            let Some(&from) = nodes.get(&wire.from.node) else { continue };
            let module = &mut modules[usize::from(to)];
            let port = usize::from(wire.to.port);
            let fits = song.check_wire(wire.from, wire.to).is_ok()
                && port < MAX_INLETS
                && song.ports_of(wire.from.node).and_then(|ports| ports.outlet(wire.from.port)).map(|outlet| outlet.sort)
                    == Some(JackSort::Control);
            if !fits || module.inlets[port].is_some() {
                continue;
            }
            module.inlets[port] = Some(CompiledInlet {
                from: wire.from.node,
                node: from,
                delayed: wire.late,
            });
        }
        let order = tick_order(&mut modules, tags.len());
        let mut knobs: Vec<CompiledKnob> = song
            .routes
            .iter()
            .filter_map(|route| {
                let module = position_of(route.destination.module()?)?;
                let ModSourceRef::Id(source) = route.source else {
                    return None;
                };
                let source = position_of(source)?;
                let descriptor = modules[usize::from(module)]
                    .params
                    .kind()
                    .descriptor(route.destination.param)?;
                let policy = ModDestinationDescriptor::for_param(descriptor);
                policy.allowed.then(|| CompiledKnob {
                    module,
                    param: route.destination.param,
                    source,
                    depth: policy.clamp_depth(route.depth),
                    polarity: route.polarity,
                })
            })
            .collect();
        knobs.sort_by_key(|knob| (knob.module, knob.param));
        let mut filed: Vec<(usize, CompiledRoute)> = song
            .routes
            .iter()
            .filter_map(|route| {
                if route.destination.module().is_some() {
                    return None;
                }
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
                tags,
                order,
                knobs,
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
            tags,
            order,
            routes: filed.into_iter().map(|(_, route)| route).collect(),
            chains,
            knobs,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty() && self.routes.is_empty()
    }

    /// The routes onto box `module`'s knobs, by its list position.
    pub fn knobs_of(&self, module: u16) -> &[CompiledKnob] {
        let start = self.knobs.partition_point(|knob| knob.module < module);
        let end = self.knobs.partition_point(|knob| knob.module <= module);
        &self.knobs[start..end]
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

    /// The index among the tags of tag `id`.
    pub fn tag_position_of(&self, id: ModSourceId) -> Option<usize> {
        self.tags.iter().position(|tag| tag.id == id)
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
    /// not in which modules and tags there are, in what order, wired how, or
    /// in which routes there are. A route onto a box's knob is part of the
    /// set's shape, depth and all: a change to one arrives as a set.
    pub fn same_shape(&self, other: &Self) -> bool {
        let module = |module: &CompiledModule| {
            (module.id, module.params.kind(), module.seed, module.inlets)
        };
        let route = |route: &CompiledRoute| (route.source, route.resolved, route.destination);
        self.chains == other.chains
            && self.knobs == other.knobs
            && self.tags == other.tags
            && self.order == other.order
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
        let shape = |source: CompiledSource, polarity: ModPolarity, output: f32| match polarity {
            ModPolarity::Bipolar => output,
            ModPolarity::Unipolar => (output + self.wire_span(source)) * 0.5,
        };
        if let Some(id) = destination.module() {
            let Some(at) = self.position_of(id).and_then(|at| u16::try_from(at).ok()) else {
                return 0.0;
            };
            return self
                .knobs_of(at)
                .iter()
                .filter(|knob| knob.param == destination.param)
                .map(|knob| {
                    let source = CompiledSource::Module(knob.source);
                    shape(source, knob.polarity, level(source)) * knob.depth
                })
                .sum();
        }
        let mut total = 0.0;
        for route in self.chain_routes(destination.scope) {
            if route.destination != destination {
                continue;
            }
            let shaped = shape(route.resolved, route.polarity, level(route.resolved));
            total += shaped * policy.clamp_depth(route.depth);
        }
        total
    }
}

/// The order a control tick runs `modules` (node indices `0..modules.len()`)
/// and `tags` tags (the indices after them) in: Kahn's topological order
/// over the inlets that are not delayed, taking the tags first and then the
/// modules in list order whenever more than one node is ready, so a song
/// whose every wire runs forward in its list keeps its list order. A loop is
/// broken at the first module in list order still waiting: its inlets from
/// nodes not yet run are marked delayed, and read the previous tick.
fn tick_order(modules: &mut [CompiledModule], tags: usize) -> Vec<u16> {
    let module_count = modules.len();
    let count = module_count + tags;
    // Ranks: tags first, then modules in list order.
    let rank = |node: usize| {
        if node >= module_count {
            node - module_count
        } else {
            tags + node
        }
    };
    let mut waiting = vec![0u32; count];
    let mut feeds: Vec<Vec<u16>> = vec![Vec::new(); count];
    for (to, module) in modules.iter().enumerate() {
        for inlet in module.inlets.iter().flatten().filter(|inlet| !inlet.delayed) {
            waiting[to] += 1;
            feeds[usize::from(inlet.node)].push(to as u16);
        }
    }
    let mut ready: std::collections::BinaryHeap<std::cmp::Reverse<(usize, u16)>> = (0..count)
        .filter(|&node| waiting[node] == 0)
        .map(|node| std::cmp::Reverse((rank(node), node as u16)))
        .collect();
    let mut placed = vec![false; count];
    let mut order = Vec::with_capacity(count);
    while order.len() < count {
        let node = match ready.pop() {
            Some(std::cmp::Reverse((_, node))) => usize::from(node),
            None => {
                // A loop: every node left waits on another. Break it at the
                // first module in list order.
                let Some(node) = (0..module_count).find(|&node| !placed[node]) else {
                    break;
                };
                for inlet in modules[node].inlets.iter_mut().flatten() {
                    if !inlet.delayed && !placed[usize::from(inlet.node)] {
                        inlet.delayed = true;
                        let feeder = &mut feeds[usize::from(inlet.node)];
                        if let Some(at) = feeder.iter().position(|&to| usize::from(to) == node) {
                            feeder.swap_remove(at);
                        }
                    }
                }
                waiting[node] = 0;
                node
            }
        };
        if placed[node] {
            continue;
        }
        placed[node] = true;
        order.push(node as u16);
        for &to in &feeds[node] {
            let to = usize::from(to);
            waiting[to] = waiting[to].saturating_sub(1);
            if waiting[to] == 0 && !placed[to] {
                ready.push(std::cmp::Reverse((rank(to), to as u16)));
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulation::{ModLfoParams, ModMathParams, ModRoute, SongModule};
    use crate::patch::Jack;
    use crate::InputSource;
    use crate::DeviceId;

    fn module(id: u32, params: ModulatorParams) -> SongModule {
        SongModule {
            id: ModSourceId(id),
            name: String::new(),
            seed: id,
            at: crate::patch::CanvasPoint::default(),
            open: false,
            rack: None,
            params,
            text: String::new(),
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
        let mut song = SongModulation {
            modules: vec![
                module(7, lfo),
                module(3, ModulatorParams::Math(ModMathParams::default())),
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
            ..SongModulation::default()
        };
        song.set_input(ModSourceId(7), InputSource::ChannelNotes(ChannelId(30)));
        song.set_input(ModSourceId(3), InputSource::Module(ModSourceId(7)));
        let seat_of = |id: ChannelId| (id == ChannelId(30)).then_some(1);
        let plan = CompiledModulation::compile(&song, seat_of);
        // The gate tag is node 2, after the two modules, and hears seat 1.
        assert_eq!(plan.tags[0].source, Some(TagSource::Gate(1)));
        let lfo_retrigger = plan.modules[0].inlets[1].expect("the LFO's retrigger is wired");
        assert_eq!((lfo_retrigger.node, lfo_retrigger.delayed), (2, false));
        let math_in = plan.modules[1].inlets[0].expect("the Math box's in is wired");
        assert_eq!((math_in.from, math_in.node, math_in.delayed), (ModSourceId(7), 0, false));
        assert_eq!(plan.order, [2, 0, 1], "the tag, then the list");
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

    fn lfo_song(count: u32) -> SongModulation {
        let lfo = ModulatorParams::Lfo(ModLfoParams::default());
        SongModulation {
            modules: (0..count).map(|id| module(id, lfo)).collect(),
            next_source_id: count,
            ..SongModulation::default()
        }
    }

    fn wire(song: &mut SongModulation, from: u32, to: u32, port: u8) {
        song.connect(Jack::new(ModSourceId(from), 0), Jack::new(ModSourceId(to), port))
            .expect("a control wire");
    }

    /// A box runs after the boxes that feed it, whatever the list says, and
    /// boxes nothing orders keep the list's order.
    #[test]
    fn the_order_follows_the_wires_and_ties_keep_the_list() {
        let mut song = lfo_song(4);
        // 3 -> 0 -> 2; 1 is free.
        wire(&mut song, 3, 0, 0);
        wire(&mut song, 0, 2, 0);
        let plan = CompiledModulation::compile(&song, |_| None);
        assert_eq!(plan.order, [1, 3, 0, 2]);
        assert!(plan.modules.iter().flat_map(|module| module.inlets).flatten().all(|inlet| !inlet.delayed));
    }

    /// A loop runs: it is broken at the first box in the list still
    /// waiting, whose wire from the loop reads the previous tick.
    #[test]
    fn a_loop_is_broken_at_its_first_box_and_reads_a_tick_late() {
        let mut song = lfo_song(3);
        // 1 -> 2 -> 1, and 0 -> 1.
        wire(&mut song, 1, 2, 0);
        wire(&mut song, 2, 1, 0);
        wire(&mut song, 0, 1, 1);
        let plan = CompiledModulation::compile(&song, |_| None);
        assert_eq!(plan.order, [0, 1, 2]);
        let into_one = plan.modules[1].inlets;
        assert_eq!(into_one[0].map(|inlet| inlet.delayed), Some(true), "2 -> 1 closes the loop");
        assert_eq!(into_one[1].map(|inlet| inlet.delayed), Some(false), "0 -> 1 does not");
        assert_eq!(plan.modules[2].inlets[0].map(|inlet| inlet.delayed), Some(false));
    }

    /// A wire saved late (an input that read a module listed after its own)
    /// reads the previous tick and does not order its boxes.
    #[test]
    fn a_late_wire_keeps_the_list_order() {
        let mut song = lfo_song(2);
        song.modules[0].params = ModulatorParams::Math(ModMathParams::default());
        assert!(song.set_input(ModSourceId(0), crate::InputSource::Module(ModSourceId(1))));
        assert!(song.wires[0].late, "Math reads a module listed after it");
        let plan = CompiledModulation::compile(&song, |_| None);
        assert_eq!(plan.order, [0, 1]);
        assert_eq!(plan.modules[0].inlets[0].map(|inlet| inlet.delayed), Some(true));
    }
    /// A route onto a box's knob (song patch step 05) is filed with the
    /// knobs, not under any chain; one onto a stepped knob, or from
    /// anything but a box, is left out. Its depth is part of the set's
    /// shape, and its live offset reads like any route's.
    #[test]
    fn a_route_onto_a_box_knob_is_filed_with_the_knobs() {
        use crate::modulation::{LFO_PARAM_DEPTH, LFO_PARAM_RATE_HZ, LFO_PARAM_WAVEFORM};
        let mut song = lfo_song(2);
        let onto = |param| ParamAddr::modulator(ModSourceId(1), param);
        let knob_route = |source, param| ModRoute {
            source,
            source_slot: crate::modulation::UNRESOLVED_SLOT,
            destination: onto(param),
            depth: 0.5,
            polarity: ModPolarity::Bipolar,
        };
        song.routes = vec![
            knob_route(ModSourceRef::Id(ModSourceId(0)), LFO_PARAM_RATE_HZ),
            knob_route(ModSourceRef::Id(ModSourceId(0)), LFO_PARAM_WAVEFORM),
            knob_route(
                ModSourceRef::Performance {
                    channel: ChannelId(1),
                    source: 0,
                },
                LFO_PARAM_DEPTH,
            ),
        ];
        let plan = CompiledModulation::compile(&song, |_| Some(0));
        assert!(plan.routes.is_empty(), "no chain holds a knob's route");
        assert_eq!(
            plan.knobs,
            [CompiledKnob {
                module: 1,
                param: LFO_PARAM_RATE_HZ,
                source: 0,
                depth: 0.5,
                polarity: ModPolarity::Bipolar,
            }]
        );
        assert_eq!(plan.knobs_of(1).len(), 1);
        assert!(plan.knobs_of(0).is_empty());
        let policy = ModDestinationDescriptor::unrestricted(LFO_PARAM_RATE_HZ);
        let offset = plan.offset_for(onto(LFO_PARAM_RATE_HZ), &policy, |_| 0.4);
        assert!((offset - 0.2).abs() < 1e-6, "{offset}");

        let mut deeper = song.clone();
        deeper.routes[0].depth = 0.9;
        assert!(!plan.same_shape(&CompiledModulation::compile(&deeper, |_| Some(0))));
    }

    /// A box's knob is on no channel or track, so moving or removing one
    /// leaves it where it is.
    #[test]
    fn a_box_knob_is_moved_by_no_seat_edit() {
        use crate::structure::{ChannelEdit, TrackEdit};
        let knob = ParamAddr::modulator(ModSourceId(4), 0);
        assert_eq!(ChannelEdit::Removed(0).address(knob), Some(knob));
        assert_eq!(TrackEdit::Removed(0).address(knob), Some(knob));
        assert_eq!(chain_index(knob.scope), None);
    }
}
