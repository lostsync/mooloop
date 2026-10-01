//! A hosted plugin's knobs and its own GUI follow each other while its slot
//! is not processed (MOO-498): asleep in silence, bypassed, on a muted
//! channel, and as a muted or sleeping instrument.
//!
//! Parameter changes cross between the host and a CLAP plugin inside
//! `process`; a slot that is not processed is flushed instead (CLAP's
//! `params.flush`, on the audio thread), every block it has a knob edit
//! waiting or the plugin has called `request_flush`. Each case drives both
//! directions through the executor with the test plugins:
//!
//! - **face to plugin**: a knob edit pushed before a block is the plugin's
//!   value once that block has run;
//! - **plugin to face**: the plugin's GUI turns the knob between two blocks
//!   ([`test_plugin::PROBE_GUI_EDIT`], which moves the value and calls
//!   `request_flush`), and once the next block has run the change is on the
//!   ring the control thread drains (`drain_param_events`).
//!
//! Every block is counted on the audio thread: no allocation, no free, no
//! lock. The plugins' thread checks count a call on the wrong thread.

use std::sync::mpsc;

use mooloop_core::{EffectTarget, EngineCommand, PluginSlotId, Project};
use mooloop_plugin_host::clap::ClapInstance;
use mooloop_plugin_host::{HostedInstance, Lifeline, PluginParamEvent};
use mooloop_test_plugin as test_plugin;

use crate::plugin_host_tests::{drum_loop, live, open_gain, replace};
use crate::plugin_instrument_tests::{open_sine, song};
use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use crate::{RealtimeCommand, StructuralCommand};

const BLOCK: usize = 256;

/// Blocks run before the first edit, so a slot that sleeps in silence is
/// asleep and a muted channel has finished fading out.
const SETTLE: usize = 64;

/// Where the plugin under test sits.
#[derive(Clone, Copy)]
enum Seat {
    /// The test gain, the first effect on channel 0.
    Effect,
    /// The test sine, channel 0's source.
    Source,
}

impl Seat {
    /// The knob both sides turn: the gain's `gain`, the sine's `level`.
    fn param(self) -> u32 {
        match self {
            Self::Effect => test_plugin::PARAM_GAIN,
            Self::Source => test_plugin::PARAM_LEVEL,
        }
    }

    /// What the face sends for a knob turned to `value`.
    fn edit(self, value: f32) -> RealtimeCommand {
        RealtimeCommand::Engine(match self {
            Self::Effect => EngineCommand::SetEffectParam {
                target: EffectTarget::Channel(0),
                slot: 0,
                id: self.param(),
                value,
            },
            Self::Source => EngineCommand::SetChannelGeneratorParam {
                channel: 0,
                id: self.param(),
                value,
            },
        })
    }

    /// What swaps `node` into its seat, as the rack does.
    fn host(self, slot: PluginSlotId, node: Box<dyn mooloop_dsp::AudioNode + Send>) -> RealtimeCommand {
        match self {
            Self::Effect => replace(EffectTarget::Channel(0), 0, slot, node),
            Self::Source => RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor {
                channel: 0,
                slot,
                node: Some(node),
            }),
        }
    }
}

/// A drum loop with the test gain on channel 0, `notes` or none.
fn gain_song(notes: bool) -> (Project, PluginSlotId) {
    let (mut project, slots) = drum_loop(1, &[0]);
    if !notes {
        project.channels[0].notes[0].clear();
    }
    (project, slots[0])
}

/// The sine instrument's song, `notes` or none.
fn sine_song(notes: bool) -> (Project, PluginSlotId) {
    let (mut project, slot) = song();
    if !notes {
        project.channels[0].notes[0].clear();
    }
    (project, slot)
}

/// The values a drain finds for `id`, in order.
fn drained_values(instance: &mut ClapInstance, id: u32) -> Vec<f64> {
    let mut values = Vec::new();
    instance.drain_param_events(&mut |event| {
        if let PluginParamEvent::Value { id: at, value } = event {
            if at == id {
                values.push(value);
            }
        }
    });
    values
}

/// Play `project` with `instance`'s processor in `seat`, transport running,
/// and check both directions while the seat is not processed.
fn knobs_and_gui_follow_each_other(project: &Project, slot: PluginSlotId, seat: Seat, mut instance: ClapInstance) {
    let lifeline = Lifeline::new();
    let node = instance.build_processor(lifeline.tie()).expect("a processor");
    let mut live = live(RenderState::from_project(SAMPLE_RATE, project, &[]));
    assert!(live.commands.push(seat.host(slot, node)).is_ok());
    assert!(live.commands.push(RealtimeCommand::Engine(EngineCommand::Play)).is_ok());

    // The control thread hands the audio thread the commands for each block
    // and waits for it, so it can look at the plugin between two blocks.
    let (to_audio, blocks) = mpsc::channel::<Vec<RealtimeCommand>>();
    let (ran, done) = mpsc::channel::<()>();
    std::thread::scope(|scope| {
        let audio = scope.spawn(move || {
            crate::executor::prepare_audio_thread();
            let silence = vec![0.0f32; BLOCK];
            let (mut l, mut r) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
            let mut index = 0;
            while let Ok(commands) = blocks.recv() {
                for command in commands {
                    assert!(live.commands.push(command).is_ok(), "the command ring has room");
                }
                let locks = mooloop_core::lock_check::locks_taken();
                let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
                live.executor
                    .process_with_input(std::iter::empty(), &silence, &silence, &mut l, &mut r);
                let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
                assert_eq!(after, before, "block {index} allocated or freed");
                let locked = mooloop_core::lock_check::locks_taken() - locks;
                assert_eq!(locked, 0, "block {index} took a lock {locked} times");
                while live.events.pop().is_ok() {}
                while let Ok(reclaimed) = live.reclaim.pop() {
                    drop(reclaimed);
                }
                index += 1;
                if ran.send(()).is_err() {
                    break;
                }
            }
            drop(live);
        });
        let block = |commands: Vec<RealtimeCommand>| {
            to_audio.send(commands).expect("the audio thread is running");
            done.recv().expect("the audio thread ran the block");
        };
        for _ in 0..SETTLE {
            block(Vec::new());
        }
        let param = seat.param();
        drained_values(&mut instance, param);

        // Face to plugin, twice, so a second edit is not the first one late.
        for value in [-12.0f32, -7.0] {
            block(vec![seat.edit(value)]);
            assert_eq!(
                instance.param_value(param),
                Some(f64::from(value)),
                "a knob edit did not reach the plugin within a block"
            );
        }

        // Plugin to face, twice: the GUI turns the knob between blocks.
        for _ in 0..2 {
            let moved = instance
                .param_value(test_plugin::PROBE_GUI_EDIT)
                .expect("the GUI edit probe answers");
            block(Vec::new());
            assert_eq!(
                drained_values(&mut instance, param),
                vec![moved],
                "the GUI's change did not reach the drain within a block"
            );
        }

        // A face edit after the GUI's lands over it.
        block(vec![seat.edit(-3.0)]);
        assert_eq!(instance.param_value(param), Some(-3.0), "the face's edit after the GUI's was lost");
        block(Vec::new());
        assert!(
            drained_values(&mut instance, param).is_empty(),
            "a flush with nothing to report reported something"
        );

        drop(to_audio);
        audio.join().expect("the audio thread did not panic");
    });
    assert!(lifeline.is_alone(), "the executor dropped its processor");
    assert_eq!(instance.misbehaviour(), 0, "no call on the wrong thread");
}

#[test]
fn a_plugin_effect_asleep_in_silence_follows_its_gui() {
    let (project, slot) = gain_song(false);
    knobs_and_gui_follow_each_other(&project, slot, Seat::Effect, open_gain(0.0, 0));
}

#[test]
fn a_bypassed_plugin_effect_follows_its_gui() {
    let (mut project, slot) = gain_song(true);
    project.channels[0].setup.effects[0].bypassed = true;
    knobs_and_gui_follow_each_other(&project, slot, Seat::Effect, open_gain(0.0, 0));
}

#[test]
fn a_plugin_effect_on_a_muted_channel_follows_its_gui() {
    let (mut project, slot) = gain_song(true);
    project.channels[0].setup.channel.muted = true;
    knobs_and_gui_follow_each_other(&project, slot, Seat::Effect, open_gain(0.0, 0));
}

#[test]
fn a_muted_plugin_instrument_follows_its_gui() {
    let (mut project, slot) = sine_song(true);
    project.channels[0].setup.channel.muted = true;
    knobs_and_gui_follow_each_other(&project, slot, Seat::Source, open_sine());
}

#[test]
fn a_plugin_instrument_asleep_in_silence_follows_its_gui() {
    let (project, slot) = sine_song(false);
    knobs_and_gui_follow_each_other(&project, slot, Seat::Source, open_sine());
}
