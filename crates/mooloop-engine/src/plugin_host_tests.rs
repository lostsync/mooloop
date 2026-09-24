//! A hosted CLAP effect in a chain (MOO-81,
//! `docs/plans/plugin-hosting/06-a-headless-clap-effect.md`), through the
//! engine: the in-repo test plugin (`mooloop-test-plugin`), loaded by path
//! the way a third-party `.clap` is, and run by `ClapProcessor`.
//!
//! Three claims, each the step's own:
//!
//! - **Offline and realtime agree.** The same song rendered by the export
//!   path and by the executor, at a live block size, with the processor
//!   swapped in the way playback swaps it in (`ReplaceEffect` down the
//!   command ring), is the same audio sample for sample.
//! - **A plugin's latency is compensated in an export.** `host_plugins`
//!   derives the plan from the processors' own latencies, so a song whose
//!   plugin adds 64 frames comes out as the song without it, 64 frames late,
//!   on every channel.
//! - **Nothing allocates on the callback.** Sixty-four channels, each with
//!   the test gain, through the executor, with a processor swapped mid-run,
//!   allocate and free nothing per block (the counting allocator, as in
//!   `soak_tests.rs`).
//!
//! Every instance is created on the test's own thread, which is its CLAP
//! main thread, and every processor runs on a thread spawned for it: the
//! test plugin asks the host which thread it is on, and a process call on
//! the main thread is a host bug it logs (which would also allocate on the
//! counted thread).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use mooloop_core::{
    EffectKind, EffectParams, EffectSlotState, EffectTarget, EngineCommand, NoteEvent,
    PluginFormat, PluginRef, PluginSlotId, PluginSlotState, PluginState, PluginStateChunk,
    Project, ProjectChannel,
};
use mooloop_dsp::AudioNode;
use mooloop_plugin_host::clap::{ClapInstance, MAX_FRAMES, STATE_TAG};
use mooloop_plugin_host::{AudioConfig, HostedInstance, Lifeline};
use mooloop_test_plugin as test_plugin;

use crate::executor::{Executor, ExecutorIo};
use crate::load::LoadMeters;
use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use crate::{
    ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RealtimeCommand, RenderScope,
    StructuralCommand, WavEncoding,
};

/// The test plugin's library, next to this test binary: cargo builds it
/// there because this crate names `mooloop-test-plugin` as a
/// dev-dependency (the same arrangement as the host crate's spike).
fn test_plugin_path() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary has a path");
    let deps = exe.parent().expect("the test binary is in a directory");
    let name = format!(
        "{}mooloop_test_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    for dir in [deps, deps.parent().unwrap_or(deps)] {
        let candidate = dir.join(&name);
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("{name} is not next to {}", exe.display());
}

fn gain_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: test_plugin::GAIN_ID.to_owned(),
        name: "Test Gain".to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    }
}

/// The test gain's own saved state: `gain_db`, and `latency_step` into
/// `LATENCY_STEPS`.
fn gain_state(gain_db: f64, latency_step: u32) -> PluginState {
    let mut data = test_plugin::STATE_MAGIC.to_vec();
    data.extend_from_slice(&gain_db.to_le_bytes());
    data.extend_from_slice(&latency_step.to_le_bytes());
    data.push(0);
    PluginState {
        chunks: vec![PluginStateChunk {
            tag: STATE_TAG.to_owned(),
            data,
        }],
    }
}

fn open_gain(gain_db: f64, latency_step: u32) -> ClapInstance {
    ClapInstance::open(
        &test_plugin_path(),
        &gain_ref(),
        &gain_state(gain_db, latency_step),
        AudioConfig {
            sample_rate: SAMPLE_RATE,
            max_frames: MAX_FRAMES,
        },
    )
    .expect("the test gain opens")
}

/// A drum loop on `channels` drum-synth channels, four hits a bar, with the
/// test gain on each channel in `hosted`. Returns the song and the plugin
/// slot of each hosted channel, in order.
fn drum_loop(channels: usize, hosted: &[usize]) -> (Project, Vec<PluginSlotId>) {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths[0] = 16;
    for index in 0..channels {
        let mut channel = ProjectChannel::drum_synth(index, 1);
        channel.setup.channel.volume = 0.5 / (channels as f32).sqrt();
        for (step, pitch) in [(0u32, 36u8), (4, 38), (8, 36), (12, 42)] {
            channel.notes[0].push(NoteEvent::new(step + 1, step * 24, 12, pitch, 110));
        }
        project.channels.push(channel);
    }
    project.assign_channel_ids();
    let mut slots = Vec::new();
    for &index in hosted {
        let slot = project.add_plugin_slot(PluginSlotState::new(gain_ref()));
        let mut device = EffectSlotState::of_kind(EffectKind::Plugin);
        device.params = EffectParams::Plugin(slot);
        project.channels[index].setup.push_effect(device).expect("room in the chain");
        slots.push(slot);
    }
    (project, slots)
}

/// Render `frames` of `project` through a fresh state in `block`-sized
/// blocks, with `plugins` swapped in the way an export swaps them, on a
/// thread of its own.
fn render_hosted(
    project: &Project,
    plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>,
    frames: usize,
    block: usize,
) -> Vec<f32> {
    let mut state = RenderState::from_project(SAMPLE_RATE, project, &[]);
    state.host_plugins(project, plugins);
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                state.play();
                let mut out = Vec::with_capacity(frames * 2);
                let mut remaining = frames;
                while remaining > 0 {
                    let n = remaining.min(block);
                    state.process_once_block(n);
                    let master = state.master();
                    for frame in 0..n {
                        out.push(master.l[frame]);
                        out.push(master.r[frame]);
                    }
                    remaining -= n;
                }
                out
            })
            .join()
            .expect("the render thread did not panic")
    })
}

/// An executor over `render`, the way a driver owns one, and the rings to
/// feed it.
struct Live {
    executor: Executor,
    commands: rtrb::Producer<RealtimeCommand>,
    events: rtrb::Consumer<mooloop_core::EngineEvent>,
    reclaim: rtrb::Consumer<crate::StructuralReclaim>,
}

fn live(render: RenderState) -> Live {
    let (commands, cmd_rx) = rtrb::RingBuffer::new(256);
    let (evt_tx, events) = rtrb::RingBuffer::new(4096);
    let (reclaim_tx, reclaim) = rtrb::RingBuffer::new(256);
    Live {
        executor: Executor::new(
            ExecutorIo {
                cmd_rx,
                evt_tx,
                reclaim_tx,
            },
            Box::new(render),
            Arc::new(AtomicU64::new(0)),
            SAMPLE_RATE,
            LoadMeters::new(),
        ),
        commands,
        events,
        reclaim,
    }
}

/// What the session sends to swap a hosted processor into its device.
fn replace(target: EffectTarget, row: u8, slot: PluginSlotId, node: Box<dyn AudioNode + Send>) -> RealtimeCommand {
    let key = u64::from(slot.0);
    let align = mooloop_dsp::IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
    RealtimeCommand::Structural(StructuralCommand::ReplaceEffect {
        target,
        slot: row,
        expected_kind: EffectKind::Plugin,
        expected_resource_key: key,
        resource_key: key,
        node,
        align,
    })
}

fn read_wav(path: &std::path::Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .expect("the export is a WAV")
        .into_samples::<f32>()
        .map(|sample| sample.expect("a float sample"))
        .collect()
}

/// **Offline and realtime agree, and the plugin is heard.** The same song
/// -- a drum loop with the test gain at -6 dB on one of its two channels --
/// is exported by `OfflineRenderer::render_with_plugins` and played through
/// the executor with the processor swapped in over the command ring, as
/// playback does it. They agree to the sample; the export is deterministic;
/// and it differs from the song with the plugin missing.
#[test]
fn the_test_plugin_renders_the_same_offline_as_through_the_executor() {
    let (project, slots) = drum_loop(2, &[0]);
    let slot = slots[0];
    let dir = tempfile::tempdir().expect("a scratch directory");
    let export = |name: &str, plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>| {
        let path = dir.path().join(name);
        let spec = ExportSpec {
            path: path.clone(),
            scope: RenderScope::Pattern { index: 0 },
            tail_seconds: 0.0,
            format: ExportFormat::Wav(WavEncoding::Float32),
        };
        let summary = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    OfflineRenderer::render_with_plugins(
                        &project,
                        &[],
                        SAMPLE_RATE,
                        &spec,
                        &ExportProgress::new(),
                        plugins,
                    )
                })
                .join()
                .expect("the export thread did not panic")
        })
        .expect("the export succeeds");
        (read_wav(&path), summary)
    };

    let mut first = open_gain(-6.0, 0);
    let first_life = Lifeline::new();
    let (wet, summary) = export(
        "wet.wav",
        BTreeMap::from([(slot, first.build_processor(first_life.tie()).expect("a processor"))]),
    );
    let mut second = open_gain(-6.0, 0);
    let second_life = Lifeline::new();
    let (again, _) = export(
        "again.wav",
        BTreeMap::from([(slot, second.build_processor(second_life.tie()).expect("a processor"))]),
    );
    let (dry, _) = export("dry.wav", BTreeMap::new());
    assert!(first_life.is_alone() && second_life.is_alone(), "the export dropped its processors");
    assert_eq!(wet, again, "the export is deterministic");
    assert_ne!(wet, dry, "the plugin is heard");
    let peak = |samples: &[f32]| samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak(&dry) > 1.0e-3, "the loop is not silent");
    assert!(peak(&wet) < peak(&dry), "-6 dB on one channel makes the mix quieter");
    assert!(!first.failed() && !second.failed());

    // The same song through the executor, as a driver runs it: the install
    // builds the placeholder, as it does live, and the processor arrives
    // down the command ring before the first block. At the export's own
    // block size and at a 64-frame callback, it is the export to the sample
    // (a note's frame stopped depending on block boundaries with MOO-211).
    let frames = summary.base_frames as usize;
    let mut third = open_gain(-6.0, 0);
    let third_life = Lifeline::new();
    for block in [512, 64] {
        let played = play(&project, slot, third.build_processor(third_life.tie()).expect("a processor"), frames, block);
        assert!(third_life.is_alone());
        assert_eq!(played.len(), wet.len());
        assert_eq!(
            worst_difference(&played, &wet),
            0.0,
            "the executor at {block} frames and the export disagree"
        );
    }
    assert_eq!(third.misbehaviour(), 0, "no call on the wrong thread");
}

fn worst_difference(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).fold(0.0f32, |worst, (x, y)| worst.max((x - y).abs()))
}

/// Play `frames` of `project` through the executor in `block`-sized
/// callbacks on a thread of its own, with `node` swapped into plugin
/// `slot` (first on channel 0) down the command ring before the first
/// block, as the session's rack swaps a processor in.
fn play(
    project: &Project,
    slot: PluginSlotId,
    node: Box<dyn AudioNode + Send>,
    frames: usize,
    block: usize,
) -> Vec<f32> {
    let mut live = live(RenderState::from_project(SAMPLE_RATE, project, &[]));
    assert!(live.commands.push(replace(EffectTarget::Channel(0), 0, slot, node)).is_ok());
    assert!(live.commands.push(RealtimeCommand::Engine(EngineCommand::Play)).is_ok());
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                crate::executor::prepare_audio_thread();
                let silence = vec![0.0f32; block];
                let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
                let mut out = Vec::with_capacity(frames * 2);
                let mut done = 0;
                while done < frames {
                    let n = block.min(frames - done);
                    live.executor.process_with_input(
                        std::iter::empty(),
                        &silence[..n],
                        &silence[..n],
                        &mut l[..n],
                        &mut r[..n],
                    );
                    for frame in 0..n {
                        out.push(l[frame]);
                        out.push(r[frame]);
                    }
                    while live.events.pop().is_ok() {}
                    while let Ok(reclaimed) = live.reclaim.pop() {
                        drop(reclaimed);
                    }
                    done += n;
                }
                drop(live);
                out
            })
            .join()
            .expect("the audio thread did not panic")
    })
}

/// **An export compensates a plugin's latency.** The test gain at 0 dB and
/// 64 frames of latency on one channel of two: the whole mix comes out as
/// the song without the plugin, 64 frames late -- the dry channel held back
/// to meet the plugin's, as the session's plan holds it back live.
#[test]
fn an_export_compensates_the_plugins_latency() {
    let latency = test_plugin::LATENCY_STEPS[1] as usize;
    let frames = SAMPLE_RATE as usize;
    let (plain, _) = drum_loop(2, &[]);
    let (hosted, slots) = drum_loop(2, &[0]);
    let mut instance = open_gain(0.0, 1);
    let lifeline = Lifeline::new();
    let node = instance.build_processor(lifeline.tie()).expect("a processor");
    assert_eq!(node.latency_frames(), latency as u32);
    assert_eq!(instance.latency_frames(), latency as u32);

    let without = render_hosted(&plain, BTreeMap::new(), frames, 512);
    let with = render_hosted(&hosted, BTreeMap::from([(slots[0], node)]), frames, 512);
    let stereo_latency = latency * 2;
    assert!(with[..stereo_latency].iter().all(|&s| s == 0.0), "the first 64 frames are the delay");
    // Not bit-exact: moving everything 64 frames moves where the block
    // boundaries fall against the notes, which rounds the last bit of a
    // few samples differently (7e-9 measured).
    let worst = worst_difference(&with[stereo_latency..], &without);
    assert!(worst < 1.0e-6, "the mix is the plain song, 64 frames late: {worst}");
}

/// **Sixty-four channels of the test plugin allocate nothing on the
/// callback**, at a small block and a large one, with one processor swapped
/// for a new one partway through -- which is what a restart does.
#[test]
fn sixty_four_hosted_channels_allocate_nothing_on_the_callback() {
    const CHANNELS: usize = 64;
    for block in [64usize, 1_024] {
        let hosted: Vec<usize> = (0..CHANNELS).collect();
        let (project, slots) = drum_loop(CHANNELS, &hosted);
        let mut instances: Vec<(ClapInstance, Lifeline)> =
            (0..=CHANNELS).map(|_| (open_gain(-3.0, 0), Lifeline::new())).collect();
        let mut plugins = BTreeMap::new();
        for (slot, (instance, lifeline)) in slots.iter().zip(instances.iter_mut()) {
            plugins.insert(*slot, instance.build_processor(lifeline.tie()).expect("a processor"));
        }
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        assert_eq!(render.host_plugins(&project, plugins), CHANNELS);
        let (spare, spare_life) = instances.last_mut().expect("the spare");
        let swap = spare.build_processor(spare_life.tie()).expect("a processor");

        let mut live = live(render);
        let blocks = (2 * SAMPLE_RATE as usize).div_ceil(block).max(32);
        let loudest = std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    crate::executor::prepare_audio_thread();
                    let silence = vec![0.0f32; block];
                    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
                    let mut swap = Some(swap);
                    let mut loudest = 0.0f32;
                    for index in 0..blocks {
                        if index == 0 {
                            assert!(live.commands.push(RealtimeCommand::Engine(EngineCommand::Play)).is_ok());
                        }
                        if index == blocks / 2 {
                            let node = swap.take().expect("one swap");
                            assert!(live
                                .commands
                                .push(replace(EffectTarget::Channel(0), 0, slots[0], node))
                                .is_ok());
                        }
                        let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
                        live.executor.process_with_input(std::iter::empty(), &silence, &silence, &mut l, &mut r);
                        let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
                        assert_eq!(
                            after, before,
                            "block {index} at {block} frames allocated {} and freed {} times",
                            after.0 - before.0,
                            after.1 - before.1
                        );
                        assert!(l.iter().chain(&r).all(|s| s.is_finite()));
                        loudest = l.iter().chain(&r).fold(loudest, |m, s| m.max(s.abs()));
                        // Control-thread work, outside the counted window.
                        while live.events.pop().is_ok() {}
                        while let Ok(reclaimed) = live.reclaim.pop() {
                            drop(reclaimed);
                        }
                    }
                    drop(live);
                    loudest
                })
                .join()
                .expect("the audio thread did not panic")
        });
        assert!(loudest > 1.0e-3, "the run at {block} frames was silent: {loudest}");
        for (instance, lifeline) in &instances {
            assert!(lifeline.is_alone(), "every processor came back");
            assert!(!instance.failed());
            assert_eq!(instance.misbehaviour(), 0);
        }
    }
}
