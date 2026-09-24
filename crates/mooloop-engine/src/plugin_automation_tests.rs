//! A hosted plugin's parameters driven by lanes and routes (MOO-82,
//! `docs/plans/plugin-hosting/07-parameters-and-state.md`), through the
//! engine, with the in-repo test plugin run by `ClapProcessor`.
//!
//! - **A lane lands on its tick.** The test gain under a lane: every 32-frame
//!   control tick of the export is the dry song times the gain the lane
//!   gives at that tick's first frame, and playback through the executor at
//!   64 and 512 frames is the export.
//! - **A route is an offset over the plugin's own value** (CLAP's parameter
//!   modulation), constant over each tick, and zeroed when the route goes.
//! - **What the plugin changes itself comes back on its ring**, never as a
//!   command, and a full ring is a number.
//!
//! Every instance is made on the test's own thread (its CLAP main thread)
//! and every processor runs on a thread spawned for it, as in
//! `plugin_host_tests.rs`, whose helpers these share.

use std::collections::BTreeMap;

use mooloop_core::{
    AutomationLane, AutomationPoint, EffectTarget, ModLfoParams, ModPolarity, ModRoute,
    ModulatorParams, ParamAddr, PluginSlotId, Project,
};
use mooloop_dsp::{AudioNode, Event, EventList, ProcessContext, StereoBus, TimedEvent};
use mooloop_plugin_host::{HostedInstance, Lifeline, PluginParamEvent};
use mooloop_test_plugin as test_plugin;

use crate::live_check::play_through_executor;
use crate::plugin_host_tests::{drum_loop, open_gain, read_wav, worst_difference};
use crate::render_test_support::SAMPLE_RATE;
use crate::{ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

/// The control tick, in frames: `mooloop_dsp::CONTROL_RATE_FRAMES`.
const TICK: usize = mooloop_dsp::CONTROL_RATE_FRAMES;

/// Export `project` with `plugins`, float WAV, no tail, and read it back.
fn export(project: &Project, plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>) -> Vec<f32> {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let path = dir.path().join("export.wav");
    let spec = ExportSpec {
        path: path.clone(),
        scope: RenderScope::Pattern { index: 0 },
        tail_seconds: 0.0,
        format: ExportFormat::Wav(WavEncoding::Float32),
    };
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                OfflineRenderer::render_with_plugins(
                    project,
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
    read_wav(&path)
}

/// One processor of the test gain at `gain_db`, keyed by `slot`.
fn processor(
    instance: &mut mooloop_plugin_host::clap::ClapInstance,
    lifeline: &Lifeline,
    slot: PluginSlotId,
) -> BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>> {
    BTreeMap::from([(slot, instance.build_processor(lifeline.tie()).expect("a processor"))])
}

/// The address of the test gain's `id` on channel 0's first device.
fn address(project: &Project, id: u32) -> ParamAddr {
    let device = project.channels[0].setup.effects[0].id;
    ParamAddr::plugin_param(EffectTarget::Channel(0), device, id)
}

/// Normalized position of `db` on the test gain's range.
fn gain_normalized(db: f64) -> f32 {
    ((db - test_plugin::GAIN_DB_MIN) / (test_plugin::GAIN_DB_MAX - test_plugin::GAIN_DB_MIN)) as f32
}

/// **A lane on a plugin parameter lands on its tick, offline and live.**
///
/// A lane on the test gain falls from 0 dB to -48 dB across the one-bar
/// pattern. The song is one drum-synth channel with the gain on it, so
/// every sample of the export is the dry song's sample times the gain the
/// lane gives at the start of that sample's control tick -- which is what
/// "sample-accurate at the control rate" means, checked tick by tick. The
/// executor at 64 and at 512 frames a callback plays the same audio.
#[test]
fn a_lane_on_a_plugin_parameter_lands_on_its_tick_offline_and_live() {
    let (mut project, slots) = drum_loop(1, &[0]);
    let slot = slots[0];
    let gain = address(&project, test_plugin::PARAM_GAIN);
    let length = u32::from(project.pattern_lengths[0]) * mooloop_core::TICKS_PER_STEP;
    let mut lane = AutomationLane::new(gain);
    lane.upsert(AutomationPoint::new(1, 0, gain_normalized(0.0)));
    lane.upsert(AutomationPoint::new(2, length, gain_normalized(-48.0)));
    if project.channels[0].automation.is_empty() {
        project.channels[0].automation.push(Vec::new());
    }
    project.channels[0].automation[0].push(lane.clone());

    let dry = export(&project, BTreeMap::new());
    let mut instance = open_gain(0.0, 0);
    let lifeline = Lifeline::new();
    let wet = export(&project, processor(&mut instance, &lifeline, slot));
    assert!(lifeline.is_alone());
    assert_eq!(wet.len(), dry.len());

    // Tick by tick: the lane's value at the tick's first frame, in the
    // plugin's decibels, is the gain every frame of that tick hears.
    let ticks_per_frame = f64::from(project.bpm) * f64::from(mooloop_core::time::DEFAULT_PPQ)
        / 60.0
        / f64::from(SAMPLE_RATE);
    let frames = dry.len() / 2;
    let mut checked = 0;
    for start in (0..frames).step_by(TICK) {
        let position = start as f64 * ticks_per_frame;
        let normalized = lane.value_at(position).expect("the lane has points");
        let db = test_plugin::GAIN_DB_MIN
            + f64::from(normalized) * (test_plugin::GAIN_DB_MAX - test_plugin::GAIN_DB_MIN);
        let expected = 10f64.powf(db / 20.0) as f32;
        for frame in start..(start + TICK).min(frames) {
            for channel in 0..2 {
                let (d, w) = (dry[frame * 2 + channel], wet[frame * 2 + channel]);
                if d.abs() < 1.0e-3 {
                    continue;
                }
                let ratio = w / d;
                assert!(
                    (ratio - expected).abs() <= expected * 1.0e-4,
                    "frame {frame}: heard x{ratio}, the lane says x{expected} ({db:.3} dB)"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 10_000, "only {checked} samples were loud enough to check");

    for block in [64, 512] {
        let played = play_through_executor(
            &project,
            processor(&mut instance, &lifeline, slot),
            SAMPLE_RATE,
            frames,
            block,
        );
        assert!(lifeline.is_alone());
        assert_eq!(
            worst_difference(&played, &wet),
            0.0,
            "playback at {block} frames and the export disagree under a lane"
        );
    }
    assert_eq!(instance.misbehaviour(), 0, "no call on the wrong thread");
}

/// **A route on a plugin parameter is an offset over the plugin's value.**
///
/// An LFO routed to the test gain at a quarter depth: the gain the song
/// hears moves around the plugin's own -6 dB by up to a quarter of the
/// 72 dB range, holds for each control tick, and goes both ways. Playback
/// through the executor is the export.
#[test]
fn a_route_on_a_plugin_parameter_offsets_the_plugins_own_value() {
    let (mut project, slots) = drum_loop(1, &[0]);
    let slot = slots[0];
    let gain = address(&project, test_plugin::PARAM_GAIN);
    let rack = &mut project.channels[0].setup.modulation;
    rack.install(
        0,
        ModulatorParams::Lfo(ModLfoParams {
            rate_hz: 3.0,
            ..ModLfoParams::default()
        }),
    );
    rack.add_route(ModRoute::to_slot(0, gain, 0.25, ModPolarity::Bipolar))
        .expect("a route has room");

    let base = export(&project, BTreeMap::new());
    let mut instance = open_gain(-6.0, 0);
    let lifeline = Lifeline::new();
    let wet = export(&project, processor(&mut instance, &lifeline, slot));
    let frames = base.len() / 2;

    let reach = 0.25 * (test_plugin::GAIN_DB_MAX - test_plugin::GAIN_DB_MIN);
    let (mut lowest, mut highest) = (f64::INFINITY, f64::NEG_INFINITY);
    for start in (0..frames).step_by(TICK) {
        let mut heard: Option<f32> = None;
        for frame in start..(start + TICK).min(frames) {
            let (d, w) = (base[frame * 2], wet[frame * 2]);
            if d.abs() < 1.0e-3 {
                continue;
            }
            let ratio = w / d;
            match heard {
                None => heard = Some(ratio),
                Some(first) => assert!(
                    (ratio - first).abs() <= first.abs() * 1.0e-4,
                    "the offset moved inside the tick at frame {start}"
                ),
            }
        }
        if let Some(ratio) = heard {
            let db = 20.0 * f64::from(ratio).log10();
            assert!(
                (db - -6.0).abs() <= reach + 1.0e-3,
                "{db:.3} dB is further from -6 dB than a quarter of the range"
            );
            lowest = lowest.min(db);
            highest = highest.max(db);
        }
    }
    assert!(
        lowest < -6.0 - 1.0 && highest > -6.0 + 1.0,
        "the route moved the gain only between {lowest:.2} and {highest:.2} dB"
    );
    assert_eq!(
        instance.param_value(test_plugin::PARAM_GAIN),
        Some(-6.0),
        "a route leaves the plugin's own value where it was"
    );

    for block in [64, 512] {
        let played = play_through_executor(
            &project,
            processor(&mut instance, &lifeline, slot),
            SAMPLE_RATE,
            frames,
            block,
        );
        assert_eq!(
            worst_difference(&played, &wet),
            0.0,
            "playback at {block} frames and the export disagree under a route"
        );
    }
}

/// Run `node` over `blocks` blocks of a constant 0.5 on both sides, `frames`
/// each, on a thread of its own, with `events(block)` as each block's
/// events. Returns the left output and the node.
fn run(
    mut node: Box<dyn AudioNode + Send>,
    blocks: usize,
    frames: usize,
    events: impl Fn(usize) -> Vec<TimedEvent> + Send,
) -> (Vec<f32>, Box<dyn AudioNode + Send>) {
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut bus = StereoBus::with_capacity(frames);
                let mut out = Vec::new();
                for block in 0..blocks {
                    let mut list = EventList::empty();
                    for event in events(block) {
                        assert!(list.push_ordered(event));
                    }
                    bus.l[..frames].fill(0.5);
                    bus.r[..frames].fill(0.5);
                    let ctx = ProcessContext {
                        sample_rate: SAMPLE_RATE,
                        frames,
                        playing: true,
                        bpm: 120.0,
                        position_ticks: 0.0,
                        position_frames: (block * frames) as u64,
                    };
                    node.process(&ctx, &mut bus, &list, None);
                    out.extend_from_slice(&bus.l[..frames]);
                }
                (out, node)
            })
            .join()
            .expect("the processing thread did not panic")
    })
}

/// **An offset whose route has gone is set back to zero.** CLAP's
/// modulation holds until it is changed, so a processor that simply stopped
/// passing a route's offset on would leave the plugin offset forever. One
/// block with -12 dB of offset, then one with nothing: the second is back at
/// the plugin's own 0 dB, from its first frame.
#[test]
fn an_offset_whose_route_went_is_zeroed() {
    let mut instance = open_gain(0.0, 0);
    let lifeline = Lifeline::new();
    let node = instance.build_processor(lifeline.tie()).expect("a processor");
    let (out, node) = run(node, 2, 64, |block| {
        if block == 0 {
            vec![TimedEvent {
                offset: 0,
                event: Event::ParamMod {
                    id: test_plugin::PARAM_GAIN,
                    amount: -12.0,
                },
            }]
        } else {
            Vec::new()
        }
    });
    let offset = 0.5 * 10f32.powf(-12.0 / 20.0);
    assert!((out[10] - offset).abs() < 1.0e-6, "the offset is heard: {}", out[10]);
    assert!((out[64] - 0.5).abs() < 1.0e-6, "the offset is gone: {}", out[64]);
    assert_eq!(
        node.hosted_param(test_plugin::PARAM_GAIN).map(|param| (param.min, param.max)),
        Some((test_plugin::GAIN_DB_MIN, test_plugin::GAIN_DB_MAX))
    );
    assert_eq!(node.hosted_param(test_plugin::PARAM_NUDGE).map(|param| param.modulatable), Some(false));
    assert_eq!(node.hosted_param(12345), None);
    drop(node);
    assert!(lifeline.is_alone());
}

/// **What the plugin changes itself comes back on its ring.** A nudge
/// makes the test gain move its own gain up a decibel, as a GUI would, and
/// report it as a gesture: the instance drains exactly that, with the id
/// intact above `i32::MAX`, and its value reads the new gain. Nothing is
/// sent back: there is no command in any of this.
///
/// **And a full ring is a number, not a guess.** Nudged faster than the
/// ring is drained, the processor counts what it dropped.
#[test]
fn the_plugins_own_changes_come_back_on_its_ring_and_a_full_ring_is_counted() {
    let mut instance = open_gain(0.0, 0);
    let lifeline = Lifeline::new();
    let node = instance.build_processor(lifeline.tie()).expect("a processor");
    let nudge = |offset: u32, value: f32| TimedEvent {
        offset,
        event: Event::ParamValue {
            id: test_plugin::PARAM_NUDGE,
            value,
        },
    };
    let (out, node) = run(node, 1, 256, |_| vec![nudge(100, 1.0)]);
    let mut reported = Vec::new();
    instance.drain_param_events(&mut |event| reported.push(event));
    let gain = test_plugin::PARAM_GAIN;
    assert_eq!(
        reported,
        [
            PluginParamEvent::GestureBegin { id: gain },
            PluginParamEvent::Value { id: gain, value: test_plugin::NUDGE_DB },
            PluginParamEvent::GestureEnd { id: gain },
        ]
    );
    assert_eq!(instance.param_value(gain), Some(test_plugin::NUDGE_DB));
    let louder = 0.5 * 10f32.powf(test_plugin::NUDGE_DB as f32 / 20.0);
    assert!((out[99] - 0.5).abs() < 1.0e-6 && (out[100] - louder).abs() < 1.0e-6, "the nudge lands on its frame");
    assert_eq!(instance.dropped_param_events(), 0);

    // 128 rises a block, three reports each, and nothing drains: the
    // 1024-report ring overflows in the third block.
    let (_, node) = run(node, 3, 256, |_| {
        (0..256u32).map(|n| nudge(n, if n % 2 == 1 { 1.0 } else { 0.0 })).collect()
    });
    assert!(instance.dropped_param_events() > 0, "a full ring was not counted");
    drop(node);
    assert!(lifeline.is_alone());
}
