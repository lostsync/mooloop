//! A channel source the engine did not build (MOO-84,
//! `docs/plans/plugin-hosting/09-a-boxed-channel-source.md`).
//!
//! The plan's three tests, with a fake instrument in place of a plugin (a
//! cosine voice per note id, so a note's first frame is audible rather than
//! a sine's zero):
//!
//! - it plays a pattern the same offline as through the executor at 64 and
//!   at 512 frames, with every note starting and ending on its own frame;
//! - switching it for a native source and back, and swapping its processor
//!   in and out, allocate and free nothing in the callback, and everything
//!   displaced comes back on the reclaim ring;
//! - saving and loading it with its state is the project crate's
//!   (`a_song_whose_source_is_a_plugin_round_trips`), and the session's rack
//!   hosting it is `plugin_rack.rs`'s.
//!
//! Plus the one thing a hosted source adds to the carry plan: a strip is
//! carried across an install only onto the same plugin slot.

use std::collections::BTreeMap;

use mooloop_core::{
    ChannelSource, DeviceKind, EngineCommand, NoteEvent, PluginFormat, PluginRef, PluginSlotId,
    PluginSlotState, Project, ProjectChannel, TICKS_PER_STEP,
};
use mooloop_dsp::{
    AudioNode, Event, EventList, HostedNode, HostedSource, ProcessContext, StereoBus,
};

use crate::plugin_host_tests::{live, play_with, render_hosted, worst_difference};
use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use crate::{RealtimeCommand, StructuralCommand, StructuralReclaim};

/// One held note of the fake instrument.
#[derive(Clone, Copy)]
struct Voice {
    id: u64,
    phase: f32,
    step: f32,
}

/// The step's `FakeSourceNode`: a cosine per note id, eight at most, each
/// starting at its note-on's frame and stopping dead at its note-off's, so
/// both edges are visible to the frame.
pub(crate) struct FakeInstrument {
    voices: [Option<Voice>; 8],
    sample_rate: f32,
}

impl FakeInstrument {
    fn new() -> Self {
        Self {
            voices: [None; 8],
            sample_rate: SAMPLE_RATE as f32,
        }
    }

    fn render(&mut self, bus: &mut StereoBus, from: usize, to: usize) {
        for frame in from..to {
            let mut sum = 0.0;
            for voice in self.voices.iter_mut().flatten() {
                sum += 0.25 * voice.phase.cos();
                voice.phase = (voice.phase + voice.step) % std::f32::consts::TAU;
            }
            bus.l[frame] += sum;
            bus.r[frame] += sum;
        }
    }

    fn apply(&mut self, event: &Event) {
        match *event {
            Event::NoteOn { id, note, .. } => {
                let hz = 440.0 * 2f32.powf((f32::from(note) - 69.0) / 12.0);
                if let Some(free) = self.voices.iter_mut().find(|voice| voice.is_none()) {
                    *free = Some(Voice {
                        id,
                        phase: 0.0,
                        step: std::f32::consts::TAU * hz / self.sample_rate,
                    });
                }
            }
            Event::NoteOff { id, .. } => {
                for voice in &mut self.voices {
                    if voice.is_some_and(|held| held.id == id) {
                        *voice = None;
                    }
                }
            }
            Event::Choke => self.voices = [None; 8],
            _ => {}
        }
    }
}

impl AudioNode for FakeInstrument {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let mut at = 0;
        for event in events_in.iter() {
            let offset = (event.offset as usize).min(ctx.frames);
            self.render(bus, at, offset);
            at = offset;
            self.apply(&event.event);
        }
        self.render(bus, at, ctx.frames);
    }
}

pub(crate) fn fake() -> HostedNode {
    Box::new(FakeInstrument::new())
}

/// A fake already holding `note` (id `u64::MAX`, which no song note has): a
/// processor that arrives sounding, the case a swap has to fade in.
pub(crate) fn fake_holding(note: u8) -> HostedNode {
    let mut instrument = FakeInstrument::new();
    instrument.apply(&Event::NoteOn {
        id: u64::MAX,
        note,
        velocity: 100,
    });
    Box::new(instrument)
}

pub(crate) fn fake_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: "org.mooloop.fake-instrument".to_owned(),
        name: "Fake Instrument".to_owned(),
        vendor: String::new(),
        version: String::new(),
    }
}

const FRAMES_PER_STEP: usize = 6_000;
const STARTS: [u32; 4] = [0, 4, 8, 13];
const LENGTH_TICKS: u32 = 12;

/// Channel 0 plays the fake through a plugin slot: four notes, the last on
/// an odd step. Channel 1 is a quiet drum synth, so the mix is not the
/// plugin alone. At 120 bpm a step is 6,000 frames.
fn song() -> (Project, PluginSlotId) {
    let mut project = Project {
        bpm: 120,
        ..Project::default()
    };
    project.pattern_lengths[0] = 16;
    let slot = project.add_plugin_slot(PluginSlotState::new(fake_ref()));
    let mut hosted = ProjectChannel::drum_synth(0, 1);
    hosted.setup.source = ChannelSource::Plugin(slot);
    hosted.setup.channel.kind = DeviceKind::Plugin;
    hosted.setup.channel.volume = 1.0;
    for (id, step) in STARTS.iter().enumerate() {
        hosted.notes[0].push(NoteEvent::new(
            id as u32 + 1,
            step * TICKS_PER_STEP,
            LENGTH_TICKS,
            57 + 5 * id as u8,
            100,
        ));
    }
    let drums = ProjectChannel::drum_synth(1, 1);
    project.channels = vec![hosted, drums];
    project.channels[1].setup.channel.muted = true;
    project.assign_channel_ids();
    (project, slot)
}

fn swap_in(slot: PluginSlotId, node: HostedNode) -> RealtimeCommand {
    RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor {
        channel: 0,
        slot,
        node: Some(node),
    })
}

/// The frames where the left channel goes from silent to sounding, and
/// from sounding to silent.
fn edges(interleaved: &[f32]) -> (Vec<usize>, Vec<usize>) {
    let (mut on, mut off) = (Vec::new(), Vec::new());
    let mut was = false;
    for (frame, pair) in interleaved.chunks(2).enumerate() {
        let now = pair[0] != 0.0;
        if now && !was {
            on.push(frame);
        }
        if was && !now {
            off.push(frame);
        }
        was = now;
    }
    (on, off)
}

/// **Offline and realtime agree, to the frame.** The export's path
/// (`host_plugins` swapping the processor into the silent source the load
/// built) and the executor at 512 and at 64 frames (the rack's
/// `HostSourceProcessor` down the command ring ahead of Play) produce the
/// same samples, and each note starts and stops exactly a whole number of
/// steps and its own length after the first. With no processor the channel
/// is silent: the missing-instrument placeholder.
#[test]
fn a_hosted_source_plays_a_pattern_the_same_offline_and_through_the_executor() {
    let (project, slot) = song();
    let frames = 16 * FRAMES_PER_STEP;
    let offline = render_hosted(&project, BTreeMap::from([(slot, fake())]), frames, 512);

    let (on, off) = edges(&offline);
    assert_eq!(on.len(), STARTS.len(), "every note is heard, once: {on:?}");
    assert_eq!(off.len(), STARTS.len(), "every note ends: {off:?}");
    let first = on[0];
    for (index, step) in STARTS.iter().enumerate() {
        assert_eq!(on[index] - first, *step as usize * FRAMES_PER_STEP, "note {index} starts off its frame");
        assert_eq!(
            off[index] - on[index],
            LENGTH_TICKS as usize * FRAMES_PER_STEP / TICKS_PER_STEP as usize,
            "note {index} is not its own length"
        );
    }

    for block in [512, 64] {
        let played = play_with(&project, vec![swap_in(slot, fake())], frames, block);
        assert_eq!(played.len(), offline.len());
        assert_eq!(
            worst_difference(&played, &offline),
            0.0,
            "the executor at {block} frames and the export disagree"
        );
    }

    let missing = render_hosted(&project, BTreeMap::new(), frames, 512);
    assert!(missing.iter().all(|&s| s == 0.0), "a missing instrument is silent");
}

/// What `EngineHandle` sends for a native source change.
fn source_change(kind: DeviceKind) -> RealtimeCommand {
    let bank = crate::render::empty_channel_audio_bank();
    crate::realtime_command(EngineCommand::SetChannelSource { channel: 0, source: kind }, &bank, SAMPLE_RATE)
        .expect("an addressable channel")
}

/// **Nothing is built or freed in the callback.** While the fake is
/// sounding: switch the channel to the sampler, back to a hosted source
/// arriving with its processor, pull the processor out, offer one for
/// another slot, and put one back. Each block allocates and frees nothing,
/// and every box that leaves comes back on the reclaim ring for the control
/// thread to drop.
#[test]
fn switching_and_swapping_a_hosted_source_allocates_nothing() {
    let (project, slot) = song();
    let mut live = live(RenderState::from_project(SAMPLE_RATE, &project, &[]));
    assert!(live.commands.push(swap_in(slot, fake())).is_ok());
    assert!(live.commands.push(RealtimeCommand::Engine(EngineCommand::Play)).is_ok());
    let block = |live: &mut crate::plugin_host_tests::Live| {
        let (mut l, mut r) = ([0.0f32; 256], [0.0f32; 256]);
        let silence = [0.0f32; 256];
        live.executor
            .process_with_input(std::iter::empty(), &silence, &silence, &mut l, &mut r);
    };
    crate::executor::prepare_audio_thread();
    block(&mut live);
    block(&mut live);
    assert!(!live.executor.render().channel_source(0).is_at_rest(), "the fake is playing");

    let wrong = PluginSlotId(slot.0 + 1);
    let steps: Vec<(&str, RealtimeCommand)> = vec![
        ("to the sampler", source_change(DeviceKind::Sampler)),
        (
            "back to a hosted source with its processor",
            RealtimeCommand::Structural(StructuralCommand::InstallSource {
                channel: 0,
                node: Box::new(HostedSource::with_processor(slot, fake())),
            }),
        ),
        (
            "pulling the processor out",
            RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor {
                channel: 0,
                slot,
                node: None,
            }),
        ),
        ("offering another slot's processor", {
            let RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor { node, .. }) =
                swap_in(slot, fake())
            else {
                unreachable!()
            };
            RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor {
                channel: 0,
                slot: wrong,
                node,
            })
        }),
        ("putting a processor back", swap_in(slot, fake())),
    ];
    // Pulling a sounding processor out waits for its fade (MOO-230), so each
    // step renders until what it displaced comes back, or for longer than
    // the longest wait (100 ms is under 19 blocks of 256) if nothing does.
    let mut reclaimed = Vec::new();
    for (what, command) in steps {
        assert!(live.commands.push(command).is_ok());
        let mut back = "nothing".to_owned();
        for _ in 0..24 {
            let (allocations, frees) = (crate::COUNTING.allocations(), crate::COUNTING.frees());
            block(&mut live);
            let counted = (
                crate::COUNTING.allocations() - allocations,
                crate::COUNTING.frees() - frees,
            );
            assert_eq!(counted, (0, 0), "{what} allocated or freed in the callback");
            match live.reclaim.pop() {
                Ok(StructuralReclaim::Source(node)) => back = format!("source {:?}", node.kind()),
                Ok(StructuralReclaim::HostedProcessor(_)) => back = "processor".to_owned(),
                Ok(_) => back = "something else".to_owned(),
                Err(_) => continue,
            }
            break;
        }
        reclaimed.push(back);
    }
    assert_eq!(
        reclaimed,
        ["source Plugin", "source Sampler", "processor", "processor", "nothing"]
    );
    let source = live.executor.render().channel_source(0);
    assert_eq!(source.kind(), DeviceKind::Plugin);
    assert!(!source.is_at_rest(), "the processor put back is playing");
}

/// **A hosted source is carried only onto its own slot.** An install whose
/// plan says the channel did not change carries the live strip, and with it
/// the processor that is sounding. But the plan compares projects, and a
/// channel can be moved to another plugin between two installs without one
/// (an `InstallSource` for another slot); carrying that strip would put the
/// second plugin in the first one's place, so the fresh, silent source stays
/// and the rack swaps the right processor in.
#[test]
fn a_hosted_source_is_carried_only_onto_its_own_slot() {
    let (project, slot) = song();
    let plan = crate::carry_plan(&project, &project);

    let mut playing = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    drop(playing.apply_structural(StructuralCommand::HostSourceProcessor {
        channel: 0,
        slot,
        node: Some(fake()),
    }));
    let mut incoming = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    incoming.carry_strips_from(&mut playing, &plan);
    let carried = incoming.channel_source(0);
    assert_eq!(carried.generator_params(), mooloop_core::GeneratorParams::Plugin(slot));
    assert!(!carried.is_at_rest(), "the sounding processor came across");

    let mut moved_on = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    let other = PluginSlotId(slot.0 + 1);
    drop(moved_on.apply_structural(StructuralCommand::InstallSource {
        channel: 0,
        node: Box::new(HostedSource::with_processor(other, fake())),
    }));
    let mut incoming = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    incoming.carry_strips_from(&mut moved_on, &plan);
    let kept = incoming.channel_source(0);
    assert_eq!(kept.generator_params(), mooloop_core::GeneratorParams::Plugin(slot));
    assert!(kept.is_at_rest(), "another slot's processor was carried into this one");
}
