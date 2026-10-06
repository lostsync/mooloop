//! The song's modulation as the audio thread runs it
//! (`docs/plans/song-modulation/02-the-engine-runs-one-set.md`).
//!
//! One set for the whole song, not a rack per channel: the modules tick once
//! per control tick, in list order, before anything renders, and every chain
//! -- a channel's source, strip and inserts, a track's inserts and fader --
//! reads the routes filed under it ([`CompiledModulation::chain_routes`]).
//!
//! The set is built off the audio thread at the size of the song's set and
//! is never resized on it. A change of shape -- a module or a route added,
//! removed or reordered, an input repointed, a channel or a track moved --
//! arrives as a whole new set on the structural ring
//! ([`crate::StructuralCommand::SetModulation`]); the audio thread carries
//! each surviving module's running state into it by identity
//! ([`SongModulator::carry_from`]) and hands the old one back to be dropped
//! off-thread. So there is no ceiling for an edit to pass: every edit that
//! would need more room arrives with it. What a narrow edit can change -- a
//! module's params, a route's depth or polarity -- is changed in place.

use mooloop_core::{
    CompiledModulation, CompiledSource, ModRoute, ModSourceId, ModulatorParams, Project,
    MAX_CHANNELS,
};
use mooloop_dsp::{
    ModuleSpec, ModulatorSet, NoteGateEvents, CONTROL_RATE_FRAMES, MAX_CONTROL_TICKS_PER_BLOCK,
};

/// The song's modulation set, resolved, with its running modules and every
/// module's output at every control tick of the current block.
pub struct SongModulator {
    plan: CompiledModulation,
    set: ModulatorSet,
    /// `table[tick * modules + module]`: each module's output at each
    /// control tick of this block, captured before the module advanced.
    /// Tick-major, because a destination reads every route's source at one
    /// tick before moving to the next.
    table: Vec<f32>,
}

impl std::fmt::Debug for SongModulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SongModulator")
            .field("modules", &self.plan.modules.len())
            .field("routes", &self.plan.routes.len())
            .finish()
    }
}

impl Default for SongModulator {
    fn default() -> Self {
        Self::build(CompiledModulation::default())
    }
}

impl SongModulator {
    /// Every module fresh. Allocates: call it off the audio thread.
    pub fn new(plan: CompiledModulation) -> Box<Self> {
        Box::new(Self::build(plan))
    }

    fn build(plan: CompiledModulation) -> Self {
        let set = ModulatorSet::new(plan.modules.iter().map(|module| ModuleSpec {
            params: module.params,
            seed: module.seed,
            gate: module.gate,
            reads: module.reads,
        }));
        let table = vec![0.0; plan.modules.len() * MAX_CONTROL_TICKS_PER_BLOCK];
        Self { plan, set, table }
    }

    /// `project`'s modulation, resolved against its own seats.
    pub fn compile(project: &Project) -> CompiledModulation {
        CompiledModulation::compile(&project.modulation, |id| {
            project
                .channels
                .iter()
                .position(|channel| channel.id == id)
                .and_then(|seat| u8::try_from(seat).ok())
        })
    }

    /// [`Self::new`] for `project`'s modulation.
    pub fn of_project(project: &Project) -> Box<Self> {
        Self::new(Self::compile(project))
    }

    pub fn plan(&self) -> &CompiledModulation {
        &self.plan
    }

    /// Take every module's running state from `previous` that names the same
    /// module, wherever it sat there. Audio thread: compares and copies,
    /// allocating nothing. A search per module, which on a song of a few
    /// hundred modules is a few tens of thousands of comparisons, once per
    /// change of shape.
    pub(crate) fn carry_from(&mut self, previous: &SongModulator) {
        for at in 0..self.plan.modules.len() {
            let module = self.plan.modules[at];
            let Some(from) = previous.plan.position_of(module.id) else {
                continue;
            };
            let input_changed = previous.plan.modules[from].input != module.input;
            self.set.carry(at, &previous.set, from, input_changed);
        }
    }

    /// Retune module `id` in place. Returns whether the set holds it.
    pub(crate) fn retune(&mut self, id: ModSourceId, params: ModulatorParams) -> bool {
        let Some(at) = self.plan.position_of(id) else {
            return false;
        };
        // A Math module's operand and an envelope's gate are its input, not
        // its params; the song keeps both out of the params it sends.
        let module = &mut self.plan.modules[at];
        if module.params.kind() != params.kind() {
            // A kind change changes what the module is; it arrives as a set.
            return false;
        }
        module.params = params;
        self.set.retune(at, params);
        true
    }

    /// Retune the route from `route.source` onto `route.destination`.
    /// Returns whether the set holds it: one it does not is a change of
    /// shape, and arrives as a set.
    pub(crate) fn set_route(&mut self, route: &ModRoute) -> bool {
        let Some(held) = self.plan.route_mut(route.source, route.destination) else {
            return false;
        };
        held.depth = route.depth;
        held.polarity = route.polarity;
        true
    }

    /// Tick every module for each control subdivision of a `frames`-long
    /// block and capture each output before it advances. The final
    /// subdivision can be shorter. Returns how many ticks were run.
    pub(crate) fn tick_block(
        &mut self,
        sample_rate: u32,
        bpm: f64,
        frames: usize,
        gates: &[[NoteGateEvents; MAX_CHANNELS]],
        song_beats: &[Option<f64>],
    ) -> usize {
        let modules = self.set.len();
        let mut tick = 0;
        for offset in (0..frames).step_by(CONTROL_RATE_FRAMES) {
            if tick >= MAX_CONTROL_TICKS_PER_BLOCK {
                break;
            }
            let span = (frames - offset).min(CONTROL_RATE_FRAMES);
            let beats = song_beats.get(tick).copied().flatten();
            if let Some(gates) = gates.get(tick) {
                self.set.tick(sample_rate, span, bpm, beats, gates);
            }
            self.table[tick * modules..(tick + 1) * modules].copy_from_slice(self.set.outputs());
            tick += 1;
        }
        tick
    }

    /// Module `at`'s output at control tick `tick` of this block.
    #[inline]
    pub(crate) fn output(&self, tick: usize, at: u16) -> f32 {
        let modules = self.set.len();
        let at = usize::from(at);
        if at >= modules {
            return 0.0;
        }
        self.table.get(tick * modules + at).copied().unwrap_or(0.0)
    }

    /// Set module `at`'s output at control tick `tick`, as if it had ticked
    /// there.
    #[cfg(test)]
    pub(crate) fn set_output(&mut self, tick: usize, at: usize, value: f32) {
        let modules = self.set.len();
        if at < modules {
            self.table[tick * modules + at] = value;
        }
    }

    /// Every module's output as of the last tick run, in list order.
    pub(crate) fn outputs(&self) -> &[f32] {
        self.set.outputs()
    }

    /// How far a source's wire output can travel from zero; see
    /// [`CompiledModulation::wire_span`].
    #[inline]
    pub(crate) fn wire_span(&self, source: CompiledSource) -> f32 {
        self.plan.wire_span(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{
        InputSource, ModLfoParams, ModRandomParams, SongModulation, SongModule,
    };

    fn module(id: u32, params: ModulatorParams) -> SongModule {
        SongModule {
            id: ModSourceId(id),
            name: String::new(),
            seed: id,
            input: InputSource::None,
            rack: None,
            params,
        }
    }

    fn compiled(modules: Vec<SongModule>) -> CompiledModulation {
        CompiledModulation::compile(
            &SongModulation {
                modules,
                routes: Vec::new(),
                next_source_id: 100,
            },
            |_| Some(0),
        )
    }

    /// A set replaced by one with a module more keeps every module that
    /// survived running from where it was, by identity, not by position.
    #[test]
    fn a_replacement_carries_each_module_by_identity() {
        let lfo = ModulatorParams::Lfo(ModLfoParams {
            rate_hz: 3.0,
            ..ModLfoParams::default()
        });
        let gates = vec![[NoteGateEvents::default(); MAX_CHANNELS]; MAX_CONTROL_TICKS_PER_BLOCK];
        let beats = [None; MAX_CONTROL_TICKS_PER_BLOCK];
        let mut before = SongModulator::new(compiled(vec![module(1, lfo)]));
        let mut continued = SongModulator::new(compiled(vec![module(1, lfo)]));
        for _ in 0..10 {
            before.tick_block(48_000, 120.0, 512, &gates, &beats);
            continued.tick_block(48_000, 120.0, 512, &gates, &beats);
        }
        continued.tick_block(48_000, 120.0, 512, &gates, &beats);

        let random = ModulatorParams::Random(ModRandomParams::default());
        let mut after = SongModulator::new(compiled(vec![module(2, random), module(1, lfo)]));
        after.carry_from(&before);
        after.tick_block(48_000, 120.0, 512, &gates, &beats);
        assert_eq!(after.outputs()[1], continued.outputs()[0]);
        assert!(after.retune(ModSourceId(1), lfo));
        assert!(!after.retune(ModSourceId(9), lfo));
        assert!(!after.set_route(&ModRoute::from_module(
            ModSourceId(1),
            mooloop_core::ParamAddr::strip(mooloop_core::EffectTarget::Bus(1), 0),
            0.5,
            mooloop_core::ModPolarity::Bipolar,
        )));
    }
}
