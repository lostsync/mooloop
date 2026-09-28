//! A hosted processor is stopped on its audio thread before it leaves the
//! engine (MOO-311). CLAP marks `stop_processing` `[audio-thread]`, and a
//! strict plugin -- Odin2, built on `clap-helpers` with its misbehaviour
//! handler set to terminate -- aborts the process when a host calls it
//! anywhere else. The test plugin's strict mode does the same
//! (`mooloop-test-plugin`'s crate documentation).
//!
//! Each test runs a processor on an audio thread, takes it out by one of the
//! paths a processor leaves by -- removed, pulled back for a restart, a
//! sample-rate change or a quit, an instrument's processor pulled out, the
//! instrument itself swapped for a native one, the song closed, an export
//! finished, the engine closed or reconnected -- and then does on the test's
//! thread, its CLAP main thread, what the control thread does: drops what
//! came back and drops the instance. An instance's `deactivate` stops a
//! processor still started, on the main thread, and strict mode aborts the
//! test binary when it does. So each test passes by finishing.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mooloop_core::{DeviceKind, EffectTarget, EngineCommand, PluginSlotId, Project};
use mooloop_dsp::effects::PluginPlaceholder;
use mooloop_plugin_host::clap::ClapInstance;
use mooloop_plugin_host::{HostedInstance, Lifeline};

use crate::plugin_host_tests::{drum_loop, live, open_gain, replace, Live};
use crate::plugin_instrument_tests::{open_sine, song};
use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use crate::{
    CommandSink, EngineHandle, ExportFormat, ExportProgress, ExportSpec, InputState,
    OfflineRenderer, PreparedProject, RealtimeCommand, RenderScope, StructuralCommand,
    StructuralReclaim, WavEncoding,
};

const FX: EffectTarget = EffectTarget::Channel(0);
const BLOCK: usize = 256;
/// Callbacks after each step's commands: long enough for a slot or a source
/// to fade out and hand back what left it.
const BLOCKS_PER_STEP: usize = 64;

fn play() -> RealtimeCommand {
    RealtimeCommand::Engine(EngineCommand::Play)
}

/// Run `live` on one thread of its own, as a driver's callback does: each
/// step's commands, then [`BLOCKS_PER_STEP`] callbacks. Returns everything
/// the reclaim ring handed back, for the test's thread to drop, as
/// `EngineHandle::poll` does.
fn run(mut live: Live, steps: Vec<Vec<RealtimeCommand>>) -> Vec<StructuralReclaim> {
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                crate::executor::prepare_audio_thread();
                let silence = [0.0f32; BLOCK];
                let (mut l, mut r) = ([0.0f32; BLOCK], [0.0f32; BLOCK]);
                let mut back = Vec::new();
                for step in steps {
                    for command in step {
                        assert!(live.commands.push(command).is_ok(), "room in the ring");
                    }
                    for _ in 0..BLOCKS_PER_STEP {
                        live.executor.process_with_input(
                            std::iter::empty(),
                            &silence,
                            &silence,
                            &mut l,
                            &mut r,
                        );
                        while live.events.pop().is_ok() {}
                        while let Ok(reclaimed) = live.reclaim.pop() {
                            back.push(reclaimed);
                        }
                    }
                }
                back
            })
            .join()
            .expect("the audio thread did not panic")
    })
}

/// What the control thread does once a processor has left: drop what came
/// back, then the instance. The processor must have come back through the
/// ring, not stayed in the engine.
fn come_home(back: Vec<StructuralReclaim>, life: &Lifeline, instance: ClapInstance) {
    assert!(!life.is_alone(), "the processor came back through the reclaim ring");
    drop(back);
    assert!(life.is_alone(), "the processor was dropped with what came back");
    assert_eq!(instance.misbehaviour(), 0, "no call on the wrong thread");
    // Deactivates: a processor left started would be stopped here, on the
    // main thread, and strict mode would abort.
    drop(instance);
}

/// A gain on channel 0 of a one-channel loop, and its processor.
fn gain_song() -> (Project, PluginSlotId, ClapInstance, Lifeline, mooloop_dsp::HostedNode) {
    let (project, slots) = drum_loop(1, &[0]);
    let mut instance = open_gain(-6.0, 0);
    let life = Lifeline::new();
    let node = instance.build_processor(life.tie()).expect("a processor");
    (project, slots[0], instance, life, node)
}

/// The sine as channel 0's source, and its processor.
fn sine_song() -> (Project, PluginSlotId, ClapInstance, Lifeline, mooloop_dsp::HostedNode) {
    let (project, slot) = song();
    let mut instance = open_sine();
    let life = Lifeline::new();
    let node = instance.build_processor(life.tie()).expect("a processor");
    (project, slot, instance, life, node)
}

fn into_source(slot: PluginSlotId, node: Option<mooloop_dsp::HostedNode>) -> RealtimeCommand {
    RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor {
        channel: 0,
        slot,
        node,
    })
}

/// **Removed.** The device is deleted from its chain.
#[test]
fn a_removed_plugin_is_stopped_on_the_audio_thread() {
    let (project, slot, instance, life, node) = gain_song();
    let back = run(
        live(RenderState::from_project(SAMPLE_RATE, &project, &[])),
        vec![
            vec![replace(FX, 0, slot, node), play()],
            vec![RealtimeCommand::Structural(StructuralCommand::RemoveEffect {
                target: FX,
                slot: 0,
            })],
        ],
    );
    come_home(back, &life, instance);
}

/// **Pulled back.** The rack swaps the placeholder in for a restart, a
/// sample-rate change, a song close and a quit (`Session::pull_back`).
#[test]
fn a_plugin_pulled_back_is_stopped_on_the_audio_thread() {
    let (project, slot, instance, life, node) = gain_song();
    let placeholder = Box::new(PluginPlaceholder::with_latency(slot, 0));
    let back = run(
        live(RenderState::from_project(SAMPLE_RATE, &project, &[])),
        vec![
            vec![replace(FX, 0, slot, node), play()],
            vec![replace(FX, 0, slot, placeholder)],
        ],
    );
    come_home(back, &life, instance);
}

/// **An instrument's processor pulled out** of its hosted source, as the
/// rack does for a restart (MOO-230).
#[test]
fn an_instruments_processor_pulled_out_is_stopped_on_the_audio_thread() {
    let (project, slot, instance, life, node) = sine_song();
    let back = run(
        live(RenderState::from_project(SAMPLE_RATE, &project, &[])),
        vec![vec![into_source(slot, Some(node)), play()], vec![into_source(slot, None)]],
    );
    come_home(back, &life, instance);
}

/// **The instrument swapped out**: the channel changed to a native source,
/// the hosted one leaving with its processor still in it.
#[test]
fn an_instrument_swapped_for_a_native_source_is_stopped_on_the_audio_thread() {
    let (project, slot, instance, life, node) = sine_song();
    let bank = crate::render::empty_channel_audio_bank();
    let sampler = crate::realtime_command(
        EngineCommand::SetChannelSource {
            channel: 0,
            source: DeviceKind::Sampler,
        },
        &bank,
        SAMPLE_RATE,
    )
    .expect("an addressable channel");
    let back = run(
        live(RenderState::from_project(SAMPLE_RATE, &project, &[])),
        vec![vec![into_source(slot, Some(node)), play()], vec![sampler]],
    );
    come_home(back, &life, instance);
}

/// **The song closed**: another project installed, which carries nothing,
/// so the whole renderer the plugin ran in goes back.
#[test]
fn a_closed_songs_plugins_are_stopped_on_the_audio_thread() {
    let (project, slot, instance, life, node) = gain_song();
    let (sine_project, sine_slot, sine_instance, sine_life, sine_node) = sine_song();
    let close = |generation| {
        RealtimeCommand::InstallProject(PreparedProject {
            generation,
            render: Box::new(RenderState::from_project(SAMPLE_RATE, &Project::default(), &[])),
            keep_transport: false,
            carry: crate::CarryPlan::default(),
        })
    };
    let back = run(
        live(RenderState::from_project(SAMPLE_RATE, &project, &[])),
        vec![vec![replace(FX, 0, slot, node), play()], vec![close(1)]],
    );
    come_home(back, &life, instance);
    let back = run(
        live(RenderState::from_project(SAMPLE_RATE, &sine_project, &[])),
        vec![vec![into_source(sine_slot, Some(sine_node)), play()], vec![close(1)]],
    );
    come_home(back, &sine_life, sine_instance);
}

/// **An export**: its processors run on the render thread, which is their
/// audio thread, and are stopped there.
#[test]
fn an_exports_plugins_are_stopped_on_its_render_thread() {
    let (project, slot, instance, life, node) = gain_song();
    let dir = tempfile::tempdir().expect("a scratch directory");
    let spec = ExportSpec {
        path: dir.path().join("export.wav"),
        scope: RenderScope::Pattern { index: 0 },
        tail_seconds: 0.0,
        format: ExportFormat::Wav(WavEncoding::Float32),
    };
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                OfflineRenderer::render_with_plugins(
                    &project,
                    &[],
                    SAMPLE_RATE,
                    &spec,
                    &ExportProgress::new(),
                    BTreeMap::from([(slot, node)]),
                )
            })
            .join()
            .expect("the export thread did not panic")
    })
    .expect("the export succeeds");
    assert!(life.is_alone(), "the export dropped its processor");
    assert_eq!(instance.misbehaviour(), 0, "no call on the wrong thread");
    drop(instance);
}

/// An engine with no device, playing the gain's loop with the gain's
/// processor swapped in, for long enough that it has run.
fn engine_playing(project: &Project, slot: PluginSlotId, node: mooloop_dsp::HostedNode) -> EngineHandle {
    let mut handle = EngineHandle::without_device("MOO-311");
    assert!(handle.install_project(Arc::new(project.clone()), Vec::new(), InputState::default(), false));
    let RealtimeCommand::Structural(command) = replace(FX, 0, slot, node) else {
        unreachable!("a replacement is structural");
    };
    assert!(handle.send_structural(command));
    assert!(handle.send(EngineCommand::Play));
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        while handle.poll().is_some() {}
        std::thread::sleep(Duration::from_millis(10));
    }
    handle
}

/// **The engine closed** with a plugin still in it: a quit whose bounded
/// wait for the rack ran out, or any other drop of the handle.
#[test]
fn an_engine_closed_with_a_plugin_in_it_stops_it_on_the_audio_thread() {
    let (project, slot, instance, life, node) = gain_song();
    let handle = engine_playing(&project, slot, node);
    assert!(!life.is_alone(), "the processor is in the engine");
    drop(handle);
    assert!(life.is_alone(), "the engine dropped its processor");
    assert_eq!(instance.misbehaviour(), 0, "no call on the wrong thread");
    drop(instance);
}

/// **The engine reconnected**, which is how a new sample rate arrives
/// (MOO-118): the old engine goes with its processors in it.
#[test]
fn a_reconnect_stops_the_old_engines_plugins_on_its_audio_thread() {
    let (project, slot, instance, life, node) = gain_song();
    let mut handle = engine_playing(&project, slot, node);
    assert!(!life.is_alone(), "the processor is in the engine");
    handle.reconnect(crate::AudioConfig::default());
    assert!(life.is_alone(), "the old engine dropped its processor");
    assert_eq!(instance.misbehaviour(), 0, "no call on the wrong thread");
    drop(instance);
    drop(handle);
}
