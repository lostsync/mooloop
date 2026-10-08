//! The song's modulation as the audio thread runs it
//! (`docs/plans/archive/song-modulation/02-the-engine-runs-one-set.md`).
//!
//! One set for the whole song, not a rack per channel: the patch ticks once
//! per control tick, in its compiled order, before anything renders, and every chain
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
use mooloop_core::modulation::{MAX_GENERATOR_OUTLETS, PERFORMANCE_SOURCES};
use crate::note_patch::NotePass;
use mooloop_dsp::{
    EventList, ModuleSpec, ModulatorSet, NoteGateEvents, SongInputs, SpecInlet, TransportTick,
    CONTROL_RATE_FRAMES, MAX_CONTROL_TICKS_PER_BLOCK,
};

/// Each seat's generator outlets and keyboard, as routes read them.
pub(crate) type ChannelInputs<'a> = (
    &'a [[f32; MAX_GENERATOR_OUTLETS]; MAX_CHANNELS],
    &'a [[f32; PERFORMANCE_SOURCES]; MAX_CHANNELS],
);

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
    /// `tag_table[tick * tags + tag]`: each tag's value at each control
    /// tick, for a route from a tag (song patch step 06).
    tag_table: Vec<f32>,
    /// The note wires (song patch step 07).
    notes: NotePass,
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
        let set = ModulatorSet::new(
            plan.modules.iter().map(|module| ModuleSpec {
                params: module.params,
                seed: module.seed,
                inlets: module.inlets.map(|inlet| {
                    inlet.map(|inlet| SpecInlet {
                        node: inlet.node,
                        delayed: inlet.delayed,
                    })
                }),
            }),
            plan.tags.iter().map(|tag| tag.source),
            plan.order.iter().copied(),
        )
        .with_knobs(plan.knobs.iter().copied());
        let table = vec![0.0; plan.modules.len() * MAX_CONTROL_TICKS_PER_BLOCK];
        let tag_table = vec![0.0; plan.tags.len() * MAX_CONTROL_TICKS_PER_BLOCK];
        let notes = NotePass::new(&plan);
        Self {
            plan,
            set,
            table,
            tag_table,
            notes,
        }
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

    /// Take every module's and tag's running state from `previous` that
    /// names the same node, wherever it sat there. Audio thread: compares
    /// and copies, allocating nothing. A search per node, which on a song of
    /// a few hundred boxes is a few tens of thousands of comparisons, once
    /// per change of shape.
    pub(crate) fn carry_from(&mut self, previous: &SongModulator) {
        for at in 0..self.plan.modules.len() {
            let module = self.plan.modules[at];
            let Some(from) = previous.plan.position_of(module.id) else {
                continue;
            };
            // The same wires, by where they come from: a node that moved in
            // the order is not a different input.
            let wired = |inlets: &[Option<mooloop_core::CompiledInlet>; mooloop_core::MAX_INLETS]| {
                inlets.map(|inlet| inlet.map(|inlet| inlet.from))
            };
            let input_changed = wired(&previous.plan.modules[from].inlets) != wired(&module.inlets);
            self.set.carry(at, &previous.set, from, input_changed);
        }
        for at in 0..self.plan.tags.len() {
            if let Some(from) = previous.plan.tag_position_of(self.plan.tags[at].id) {
                self.set.carry_tag(at, &previous.set, from);
            }
        }
        self.notes.carry_from(&previous.notes);
    }

    /// Run the note wires over the first `live` channels' complete event
    /// lists ([`NotePass::process`]).
    pub(crate) fn pass_notes(&mut self, events: &mut [Box<EventList>], live: usize, panicked: bool) {
        self.notes.process(events, live, panicked);
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
    /// subdivision can be shorter. Returns how many ticks were run. The
    /// tests' form: the render loop passes the transport and outlets too.
    #[cfg(test)]
    pub(crate) fn tick_block(
        &mut self,
        sample_rate: u32,
        bpm: f64,
        frames: usize,
        gates: &[[NoteGateEvents; MAX_CHANNELS]],
        song_beats: &[Option<f64>],
    ) -> usize {
        self.tick_block_with(sample_rate, bpm, frames, gates, song_beats, &[], None)
    }

    /// [`Self::tick_block`] with the rest of what the song sends the patch's
    /// tags: where the transport is at each tick, and each seat's generator
    /// outlets and keyboard. A tick past the end of `transport` reads it
    /// stopped or playing as `song_beats` says, at the start of a beat.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tick_block_with(
        &mut self,
        sample_rate: u32,
        bpm: f64,
        frames: usize,
        gates: &[[NoteGateEvents; MAX_CHANNELS]],
        song_beats: &[Option<f64>],
        transport: &[TransportTick],
        sources: Option<ChannelInputs>,
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
                let at = transport.get(tick).copied().unwrap_or(TransportTick {
                    playing: beats.is_some(),
                    ..TransportTick::default()
                });
                let inputs = match sources {
                    Some((outlets, performance)) => SongInputs {
                        gates,
                        transport: at,
                        outlets,
                        performance,
                    },
                    None => SongInputs::quiet(gates, at),
                };
                self.set.tick_with(sample_rate, span, bpm, beats, &inputs);
            }
            self.table[tick * modules..(tick + 1) * modules].copy_from_slice(self.set.outputs());
            let tags = self.plan.tags.len();
            for (slot, (_, value)) in self.tag_table[tick * tags..(tick + 1) * tags]
                .iter_mut()
                .zip(self.set.tag_activity())
            {
                *slot = value;
            }
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

    /// Tag `at`'s value at control tick `tick` of this block.
    #[inline]
    pub(crate) fn tag_output(&self, tick: usize, at: u16) -> f32 {
        let tags = self.plan.tags.len();
        let at = usize::from(at);
        if at >= tags {
            return 0.0;
        }
        self.tag_table.get(tick * tags + at).copied().unwrap_or(0.0)
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

    /// Each tag's NoteOn count and value, in tag order. A notes tag's
    /// count is the notes the note pass ran through it, and its value how
    /// many a notes-out tag has refused.
    pub(crate) fn tag_activity(&self) -> impl Iterator<Item = (u32, f32)> + '_ {
        self.set
            .tag_activity()
            .zip(self.notes.tag_counts())
            .zip(&self.plan.tags)
            .map(|(((notes, value), (passed, refused)), tag)| match tag.source {
                Some(_) => (notes, value),
                None => (passed, refused as f32),
            })
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
    use mooloop_core::{Jack, ModLfoParams, ModRandomParams, SongModulation, SongModule};

    fn module(id: u32, params: ModulatorParams) -> SongModule {
        SongModule {
            id: ModSourceId(id),
            name: String::new(),
            seed: id,
            at: Default::default(),
            open: false,
            rack: None,
            params,
            text: String::new(),
        }
    }

    fn compiled(modules: Vec<SongModule>) -> CompiledModulation {
        CompiledModulation::compile(
            &SongModulation {
                modules,
                routes: Vec::new(),
                next_source_id: 100,
                ..SongModulation::default()
            },
            |_| Some(0),
        )
    }

    /// One control tick (32 frames) with `gates` on every channel, returning
    /// every module's output.
    fn tick(set: &mut SongModulator, gates: [NoteGateEvents; MAX_CHANNELS]) -> Vec<f32> {
        let table = vec![gates; MAX_CONTROL_TICKS_PER_BLOCK];
        set.tick_block(48_000, 120.0, CONTROL_RATE_FRAMES, &table, &[None; MAX_CONTROL_TICKS_PER_BLOCK]);
        set.outputs().to_vec()
    }

    fn patch(modules: Vec<SongModule>) -> SongModulation {
        let next_source_id = modules.iter().map(|module| module.id.0 + 1).max().unwrap_or(0);
        SongModulation {
            modules,
            next_source_id,
            ..SongModulation::default()
        }
    }

    /// An LFO wired into another LFO's `rate` moves it two octaves for a
    /// wire at +1: a 1 Hz saw runs at 4 Hz.
    #[test]
    fn an_lfo_into_an_lfos_rate_bends_it_by_octaves() {
        let square = ModulatorParams::Lfo(ModLfoParams {
            waveform: mooloop_core::ModLfoWaveform::Square,
            rate_hz: 0.001,
            ..ModLfoParams::default()
        });
        let saw = ModulatorParams::Lfo(ModLfoParams {
            waveform: mooloop_core::ModLfoWaveform::Saw,
            rate_hz: 1.0,
            ..ModLfoParams::default()
        });
        let mut song = patch(vec![module(1, saw), module(2, square), module(3, saw)]);
        song.connect(Jack::new(ModSourceId(2), 0), Jack::new(ModSourceId(1), 0))
            .expect("out into rate");
        let mut set = SongModulator::new(CompiledModulation::compile(&song, |_| None));
        assert_eq!(set.plan().order, [1, 0, 2], "the square runs before the saw it bends");
        let quiet = [NoteGateEvents::default(); MAX_CHANNELS];
        let first = tick(&mut set, quiet);
        assert_eq!((first[0], first[1], first[2]), (-1.0, 1.0, -1.0));
        let second = tick(&mut set, quiet);
        let step = 2.0 * CONTROL_RATE_FRAMES as f32 / 48_000.0;
        assert!((second[2] - (-1.0 + step)).abs() < 1e-5, "the unwired saw at 1 Hz: {}", second[2]);
        assert!((second[0] - (-1.0 + 4.0 * step)).abs() < 1e-5, "the bent saw at 4 Hz: {}", second[0]);
    }

    /// A gate tag into a Step's `advance` moves it once per NoteOn, held
    /// notes and all, and a reset wire takes it back to the first step.
    #[test]
    fn a_gate_tag_advances_a_step_once_per_note() {
        let mut steps = [0.0; mooloop_core::MOD_STEP_MAX_STEPS];
        steps[..3].copy_from_slice(&[1.0, -1.0, 0.5]);
        let step = ModulatorParams::Step(mooloop_core::ModStepParams {
            steps,
            length: 3,
            trigger: mooloop_core::ModStepTrigger::NoteAdvance,
            ..mooloop_core::ModStepParams::default()
        });
        let mut song = patch(vec![module(1, step)]);
        let kick = mooloop_core::ChannelId(5);
        assert!(song.set_input(ModSourceId(1), mooloop_core::InputSource::ChannelNotes(kick)));
        let mut set = SongModulator::new(CompiledModulation::compile(&song, |_| Some(3)));
        let quiet = [NoteGateEvents::default(); MAX_CHANNELS];
        let mut note = quiet;
        note[3].note_ons = 1;
        let mut other = quiet;
        other[2].note_ons = 1;
        assert_eq!(tick(&mut set, quiet)[0], 1.0);
        assert_eq!(tick(&mut set, note)[0], -1.0, "a note advances");
        assert_eq!(tick(&mut set, quiet)[0], -1.0, "a held note does not again");
        assert_eq!(tick(&mut set, other)[0], -1.0, "another seat's note does not");
        assert_eq!(tick(&mut set, note)[0], 0.5, "an overlapping note does");
        let mut both = note;
        both[3].note_ons = 2;
        assert_eq!(tick(&mut set, both)[0], 1.0, "two notes in one tick advance once");
    }

    /// Two boxes in a loop run: the loop is broken at the first in the
    /// list, which reads the other's previous tick.
    #[test]
    fn a_two_box_loop_runs_a_tick_late() {
        let add = ModulatorParams::Math(mooloop_core::ModMathParams {
            op: mooloop_core::ModMathOp::Add,
            operand: 0.25,
            ..mooloop_core::ModMathParams::default()
        });
        let mut song = patch(vec![module(1, add), module(2, add)]);
        song.connect(Jack::new(ModSourceId(1), 0), Jack::new(ModSourceId(2), 0)).unwrap();
        song.connect(Jack::new(ModSourceId(2), 0), Jack::new(ModSourceId(1), 0)).unwrap();
        let plan = CompiledModulation::compile(&song, |_| None);
        assert_eq!(plan.order, [0, 1]);
        assert_eq!(plan.modules[0].inlets[0].map(|inlet| inlet.delayed), Some(true));
        let mut set = SongModulator::new(plan);
        let quiet = [NoteGateEvents::default(); MAX_CHANNELS];
        assert_eq!(tick(&mut set, quiet), [0.25, 0.5]);
        assert_eq!(tick(&mut set, quiet), [0.75, 1.0]);
        assert_eq!(tick(&mut set, quiet), [1.0, 1.0], "both clamp at the edge");
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
