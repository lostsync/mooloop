//! A plugin instrument's parameters driven by the engine (MOO-314, the
//! second leg of MOO-312): lanes, routes and knob edits on the test sine's
//! `level`, run by `ClapProcessor` inside the channel's `HostedSource`.
//!
//! - **A lane lands on every control tick**, and the export is what the
//!   executor plays at 512 and at 64 frames, to the sample.
//! - **A route is an offset** (`ParamMod`) over the plugin's own value, and
//!   an offset whose route has gone is zeroed from the next block.
//! - **A knob edit (`SetChannelGeneratorParam`) reaches the plugin**, and is
//!   held back while a lane writes that parameter.
//! - **A replaced instrument's lanes drive nothing.** The sequencer holds
//!   them until the next install, and they name the old device, so the new
//!   instrument does not answer to them.
//! - **Nothing allocates in the callback**: every executor run here counts
//!   each block's allocations and frees on the audio thread.
//!
//! The sine keeps MOO-311's strict mode: every processor starts, runs and
//! stops on one thread, the executor's.

use std::collections::BTreeMap;

use mooloop_core::{
    AutomationLane, AutomationPoint, DeviceId, EffectTarget, EngineCommand, ModLfoParams,
    ModPolarity, ModRoute, ModulatorParams, ParamAddr, PluginSlotId, Project,
};
use mooloop_dsp::{AudioNode, HostedSource};
use mooloop_plugin_host::clap::ClapInstance;
use mooloop_plugin_host::{HostedInstance, Lifeline};
use mooloop_test_plugin as test_plugin;

use crate::plugin_host_tests::{live, read_wav, worst_difference};
use crate::plugin_instrument_tests::{open_sine, song};
use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use crate::{
    ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RealtimeCommand, RenderScope,
    StructuralCommand, WavEncoding,
};

/// The control tick, in frames: `mooloop_dsp::CONTROL_RATE_FRAMES`.
const TICK: usize = mooloop_dsp::CONTROL_RATE_FRAMES;

/// One bar at 120 bpm: `plugin_instrument_tests`' song, whose notes sound
/// on steps 0, 4, 9 and 14 and the last across the loop point.
const BAR: usize = 16 * 6_000;

/// The sine song with its instrument given an identity, as a song saved or
/// made since MOO-313 has, and the address of its `level` on that device.
fn sine_song() -> (Project, PluginSlotId, ParamAddr) {
    let (mut project, slot) = song();
    project.channels[0].setup.assign_device_ids();
    let device = project.channels[0].setup.source_device;
    assert!(device.is_assigned(), "a plugin source is given an id");
    (project, slot, level_on(device))
}

/// The sine's `level` on `device`, channel 0.
fn level_on(device: DeviceId) -> ParamAddr {
    ParamAddr::plugin_param(EffectTarget::Channel(0), device, test_plugin::PARAM_LEVEL)
}

/// Normalized position of `db` on the sine's level range.
fn normalized(db: f64) -> f32 {
    ((db - test_plugin::LEVEL_DB_MIN) / (test_plugin::LEVEL_DB_MAX - test_plugin::LEVEL_DB_MIN)) as f32
}

fn gain(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// A lane on `target` from `from_db` to `to_db` across the bar, in
/// pattern 0 of channel 0.
fn add_lane(project: &mut Project, target: ParamAddr, from_db: f64, to_db: f64) -> AutomationLane {
    let length = u32::from(project.pattern_lengths[0]) * mooloop_core::TICKS_PER_STEP;
    let mut lane = AutomationLane::new(target);
    lane.upsert(AutomationPoint::new(1, 0, normalized(from_db)));
    lane.upsert(AutomationPoint::new(2, length, normalized(to_db)));
    if project.channels[0].automation.is_empty() {
        project.channels[0].automation.push(Vec::new());
    }
    project.channels[0].automation[0].push(lane.clone());
    lane
}

/// Export pattern 0 with the sine's processor from `instance`, float WAV,
/// no tail, and read it back.
fn export(project: &Project, slot: PluginSlotId, instance: &mut ClapInstance, lifeline: &Lifeline) -> Vec<f32> {
    let plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>> =
        BTreeMap::from([(slot, instance.build_processor(lifeline.tie()).expect("a processor"))]);
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
                OfflineRenderer::render_with_plugins(project, &[], SAMPLE_RATE, &spec, &ExportProgress::new(), plugins)
            })
            .join()
            .expect("the export thread did not panic")
    })
    .expect("the export succeeds");
    assert!(lifeline.is_alone(), "the export dropped its processor");
    read_wav(&path)
}

/// Play `frames` of `project` through the executor in `block`-frame
/// callbacks on a thread of its own, the sine's processor from `instance`
/// swapped into channel 0's source first, as the rack does, and each
/// `(block index, command)` of `script` pushed just before that block.
///
/// **Every block is counted**: one that allocates or frees on the audio
/// thread fails the test. What the reclaim ring hands back is dropped
/// between callbacks, as the control thread drops it live.
fn play(
    project: &Project,
    slot: PluginSlotId,
    instance: &mut ClapInstance,
    lifeline: &Lifeline,
    script: Vec<(usize, RealtimeCommand)>,
    frames: usize,
    block: usize,
) -> Vec<f32> {
    let mut live = live(RenderState::from_project(SAMPLE_RATE, project, &[]));
    let node = instance.build_processor(lifeline.tie()).expect("a processor");
    let swap = RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor {
        channel: 0,
        slot,
        node: Some(node),
    });
    assert!(live.commands.push(swap).is_ok());
    assert!(live.commands.push(RealtimeCommand::Engine(EngineCommand::Play)).is_ok());
    let out = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                crate::executor::prepare_audio_thread();
                let silence = vec![0.0f32; block];
                let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
                let mut out = Vec::with_capacity(frames * 2);
                let mut script = script.into_iter().peekable();
                let (mut done, mut index) = (0, 0);
                while done < frames {
                    while let Some((_, command)) = script.next_if(|(at, _)| *at <= index) {
                        assert!(live.commands.push(command).is_ok(), "the command ring has room");
                    }
                    let n = block.min(frames - done);
                    let locks = mooloop_core::lock_check::locks_taken();
                    let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
                    live.executor.process_with_input(
                        std::iter::empty(),
                        &silence[..n],
                        &silence[..n],
                        &mut l[..n],
                        &mut r[..n],
                    );
                    let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
                    assert_eq!(
                        after, before,
                        "block {index} at {block} frames allocated {} and freed {} times",
                        after.0 - before.0,
                        after.1 - before.1
                    );
                    let locked = mooloop_core::lock_check::locks_taken() - locks;
                    assert_eq!(locked, 0, "block {index} at {block} frames took a lock {locked} times");
                    for frame in 0..n {
                        out.push(l[frame]);
                        out.push(r[frame]);
                    }
                    while live.events.pop().is_ok() {}
                    while let Ok(reclaimed) = live.reclaim.pop() {
                        drop(reclaimed);
                    }
                    done += n;
                    index += 1;
                }
                drop(live);
                out
            })
            .join()
            .expect("the audio thread did not panic")
    });
    assert!(lifeline.is_alone(), "the executor dropped its processor");
    assert_eq!(instance.misbehaviour(), 0, "no call on the wrong thread");
    out
}

/// Every sample loud enough to measure, as `(frame, wet / dry)`, left side.
fn ratios<'a>(dry: &'a [f32], wet: &'a [f32], frames: std::ops::Range<usize>) -> impl Iterator<Item = (usize, f32)> + 'a {
    frames.filter_map(move |frame| {
        let (d, w) = (dry[frame * 2], wet[frame * 2]);
        (d.abs() >= 1.0e-3).then(|| (frame, w / d))
    })
}

/// **A lane on the instrument's parameter lands on every control tick,
/// offline and live.**
///
/// A lane on the sine's level falls from 0 dB to -48 dB across the bar.
/// Every sample of the export is the lane-free export's sample times the
/// level the lane gives at the start of that sample's control tick, which is
/// what "sample-accurate at the control rate" means, checked tick by tick.
/// The executor at 512 and at 64 frames a callback plays the export exactly,
/// and allocates nothing doing it.
#[test]
fn a_lane_on_a_plugin_instruments_parameter_lands_on_every_control_tick() {
    let (plain, slot, level) = sine_song();
    let mut project = plain.clone();
    let lane = add_lane(&mut project, level, 0.0, -48.0);

    let mut instance = open_sine();
    let lifeline = Lifeline::new();
    let dry = export(&plain, slot, &mut instance, &lifeline);
    let wet = export(&project, slot, &mut instance, &lifeline);
    assert_eq!(wet.len(), dry.len());
    let frames = dry.len() / 2;
    assert!(frames >= BAR - 1, "the export is the bar: {frames} frames");

    let ticks_per_frame = f64::from(project.bpm) * f64::from(mooloop_core::time::DEFAULT_PPQ)
        / 60.0
        / f64::from(SAMPLE_RATE);
    let mut checked = 0;
    for (frame, ratio) in ratios(&dry, &wet, 0..frames) {
        let start = frame / TICK * TICK;
        let value = lane.value_at(start as f64 * ticks_per_frame).expect("the lane has points");
        let db = test_plugin::LEVEL_DB_MIN
            + f64::from(value) * (test_plugin::LEVEL_DB_MAX - test_plugin::LEVEL_DB_MIN);
        let expected = gain(db);
        assert!(
            (ratio - expected).abs() <= expected * 1.0e-4,
            "frame {frame}: heard x{ratio}, the lane says x{expected} ({db:.3} dB)"
        );
        checked += 1;
    }
    assert!(checked > 10_000, "only {checked} samples were loud enough to check");

    for block in [512, 64] {
        let played = play(&project, slot, &mut instance, &lifeline, Vec::new(), frames, block);
        assert_eq!(
            worst_difference(&played, &wet),
            0.0,
            "the executor at {block} frames and the export disagree under a lane"
        );
    }
}

/// **A route on the instrument's parameter is an offset over the plugin's
/// own value, and an offset whose route has gone is zeroed.**
///
/// An LFO routed to the sine's level at a quarter depth: the level moves
/// around the plugin's own 0 dB by up to a quarter of its 60 dB range, holds
/// for each control tick, goes both ways, and leaves the plugin's value
/// where it was. The executor plays the export. Then, live, the route is
/// removed partway: from that block on, the song is the route-free song to
/// the sample.
#[test]
fn a_route_on_a_plugin_instruments_parameter_is_an_offset_and_is_zeroed_when_it_goes() {
    let (plain, slot, level) = sine_song();
    let mut project = plain.clone();
    let rack = &mut project.channels[0].setup.modulation;
    rack.install(
        0,
        ModulatorParams::Lfo(ModLfoParams {
            rate_hz: 3.0,
            ..ModLfoParams::default()
        }),
    );
    rack.add_route(ModRoute::to_slot(0, level, 0.25, ModPolarity::Bipolar))
        .expect("a route has room");
    let source = rack.routes.iter().flatten().next().expect("the route").source;

    let mut instance = open_sine();
    let lifeline = Lifeline::new();
    let dry = export(&plain, slot, &mut instance, &lifeline);
    let wet = export(&project, slot, &mut instance, &lifeline);
    let frames = dry.len() / 2;

    let reach = 0.25 * (test_plugin::LEVEL_DB_MAX - test_plugin::LEVEL_DB_MIN);
    let (mut lowest, mut highest) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut tick_ratio: Option<(usize, f32)> = None;
    for (frame, ratio) in ratios(&dry, &wet, 0..frames) {
        let tick = frame / TICK;
        match tick_ratio {
            Some((at, first)) if at == tick => assert!(
                (ratio - first).abs() <= first.abs() * 1.0e-4,
                "the offset moved inside the tick at frame {frame}"
            ),
            _ => tick_ratio = Some((tick, ratio)),
        }
        let db = 20.0 * f64::from(ratio).log10();
        assert!(db.abs() <= reach + 1.0e-3, "{db:.3} dB is further from 0 dB than a quarter of the range");
        lowest = lowest.min(db);
        highest = highest.max(db);
    }
    assert!(
        lowest < -1.0 && highest > 1.0,
        "the route moved the level only between {lowest:.2} and {highest:.2} dB"
    );
    assert_eq!(
        instance.param_value(test_plugin::PARAM_LEVEL),
        Some(test_plugin::LEVEL_DB_DEFAULT),
        "a route leaves the plugin's own value where it was"
    );

    for block in [512, 64] {
        let played = play(&project, slot, &mut instance, &lifeline, Vec::new(), frames, block);
        assert_eq!(
            worst_difference(&played, &wet),
            0.0,
            "the executor at {block} frames and the export disagree under a route"
        );
    }

    // Removed a third of the way in, while the first notes ring.
    let block = 256;
    let removed_at = BAR / 3 / block;
    let remove = RealtimeCommand::Engine(EngineCommand::RemoveModRoute {
        channel: 0,
        source,
        destination: level,
    });
    let routed = play(&project, slot, &mut instance, &lifeline, vec![(removed_at, remove)], BAR, block);
    let unrouted = play(&plain, slot, &mut instance, &lifeline, Vec::new(), BAR, block);
    let cut = removed_at * block * 2;
    assert!(
        worst_difference(&routed[..cut], &unrouted[..cut]) > 1.0e-3,
        "the route was heard before it went"
    );
    assert_eq!(
        worst_difference(&routed[cut..], &unrouted[cut..]),
        0.0,
        "an offset outlived its route"
    );
}

/// **A knob edit reaches the plugin, and a lane holds it back.**
///
/// `SetChannelGeneratorParam` on a plugin source used to be dropped without
/// a word (`GeneratorParams::Plugin` has no parameter to set). Now it goes to
/// the plugin in its own units at the top of the next block: -12 dB sent
/// while a note rings is every sample from that block on at -12 dB, and the
/// plugin's value reads it. With a lane on the level, the same edit changes
/// nothing: the song is the lane's to the sample, and so is the value.
#[test]
fn a_knob_edit_reaches_a_plugin_instrument_unless_a_lane_writes_that_parameter() {
    let (plain, slot, level) = sine_song();
    let block = 256;
    let edit_at = BAR / 3 / block;
    let edit = |db: f32| {
        RealtimeCommand::Engine(EngineCommand::SetChannelGeneratorParam {
            channel: 0,
            id: test_plugin::PARAM_LEVEL,
            value: db,
        })
    };
    let mut instance = open_sine();
    let lifeline = Lifeline::new();

    let dry = play(&plain, slot, &mut instance, &lifeline, Vec::new(), BAR, block);
    let edited = play(&plain, slot, &mut instance, &lifeline, vec![(edit_at, edit(-12.0))], BAR, block);
    let cut = edit_at * block;
    assert_eq!(worst_difference(&edited[..cut * 2], &dry[..cut * 2]), 0.0, "the edit was heard early");
    let expected = gain(-12.0);
    let mut checked = 0;
    for (frame, ratio) in ratios(&dry, &edited, cut..BAR) {
        assert!(
            (ratio - expected).abs() <= expected * 1.0e-4,
            "frame {frame}: heard x{ratio}, the knob says x{expected}"
        );
        checked += 1;
    }
    assert!(checked > 10_000, "only {checked} samples after the edit were loud enough to check");
    assert_eq!(instance.param_value(test_plugin::PARAM_LEVEL), Some(-12.0), "the plugin holds the edit");

    // Under a lane holding -6 dB, the same edit is not heard, and the
    // plugin's value is the lane's.
    let mut laned = plain.clone();
    add_lane(&mut laned, level, -6.0, -6.0);
    let lane_only = play(&laned, slot, &mut instance, &lifeline, Vec::new(), BAR, block);
    let lane_and_knob = play(&laned, slot, &mut instance, &lifeline, vec![(edit_at, edit(-30.0))], BAR, block);
    assert!(worst_difference(&lane_only, &dry) > 1.0e-3, "the lane is heard");
    assert_eq!(worst_difference(&lane_and_knob, &lane_only), 0.0, "a knob edit got past a lane");
    let value = instance.param_value(test_plugin::PARAM_LEVEL).expect("the level has a value");
    assert!((value - -6.0).abs() < 1.0e-3, "the plugin's value is {value} dB, not the lane's -6 dB");
}

/// **A replaced instrument's lanes do not drive the new one** (MOO-314,
/// the gap MOO-313 left).
///
/// A source change is one `InstallSource`, and the sequencer keeps the lanes
/// it was installed with until the next project install. A lane on the old
/// instrument's level at -24 dB must not reach the instrument that replaces
/// it: the new one has its own device id, and from the block it lands in,
/// the song is the one with no lane at all. The same install carrying the
/// *old* id -- which nothing sends, and is here to show what the id is
/// doing -- does hand the new instrument the lane.
#[test]
fn a_replaced_plugin_instruments_lanes_do_not_drive_the_new_one() {
    let (plain, slot, level) = sine_song();
    let mut laned = plain.clone();
    add_lane(&mut laned, level, -24.0, -24.0);
    let old = plain.channels[0].setup.source_device;
    let new = DeviceId(old.0 + 1);
    let next = PluginSlotId(slot.0 + 1);
    let block = 256;
    let swap_at = BAR / 3 / block;

    let mut instance = open_sine();
    let lifeline = Lifeline::new();
    let mut replacement = open_sine();
    let replacement_life = Lifeline::new();
    let mut run = |project: &Project, device: DeviceId| {
        let node = replacement.build_processor(replacement_life.tie()).expect("a processor");
        let install = RealtimeCommand::Structural(StructuralCommand::InstallSource {
            channel: 0,
            node: Box::new(HostedSource::with_processor(next, node)),
            device,
        });
        let out = play(project, slot, &mut instance, &lifeline, vec![(swap_at, install)], BAR, block);
        assert!(replacement_life.is_alone(), "the replacement came back");
        assert_eq!(replacement.misbehaviour(), 0);
        out
    };
    let without = run(&plain, new);
    let replaced = run(&laned, new);
    let misaddressed = run(&laned, old);

    let cut = swap_at * block * 2;
    assert!(
        worst_difference(&replaced[..cut], &without[..cut]) > 1.0e-3,
        "the lane drove the old instrument"
    );
    assert_eq!(
        worst_difference(&replaced[cut..], &without[cut..]),
        0.0,
        "the old instrument's lane drove the one that replaced it"
    );
    assert!(
        worst_difference(&misaddressed[cut..], &without[cut..]) > 1.0e-3,
        "a lane naming the instrument's id did not drive it"
    );
}

/// **Nine knob edits on a muted plugin instrument all reach it** (MOO-344).
///
/// A muted channel is not rendered, so its instrument's edits wait in an
/// eight-entry box. The level and eight more ids (ones the sine does not
/// have, which it ignores) arrive in one gap between blocks: the ninth
/// used to overwrite the level. The full box is flushed to the plugin
/// instead, on the audio thread, through CLAP's `params.flush` -- with no
/// allocation, no lock and no call on the wrong thread, which `play`
/// checks -- and the plugin holds the level.
#[test]
fn nine_knob_edits_on_a_muted_plugin_instrument_all_reach_it() {
    let (mut project, slot, _) = sine_song();
    project.channels[0].setup.channel.muted = true;
    let edit = |id: u32, value: f32| {
        RealtimeCommand::Engine(EngineCommand::SetChannelGeneratorParam {
            channel: 0,
            id,
            value,
        })
    };
    let block = 256;
    let mut script = vec![(4, edit(test_plugin::PARAM_LEVEL, -12.0))];
    script.extend((0..8u32).map(|index| (4, edit(9_000 + index, 0.5))));
    let mut instance = open_sine();
    let lifeline = Lifeline::new();
    play(&project, slot, &mut instance, &lifeline, script, 12 * block, block);
    assert_eq!(
        instance.param_value(test_plugin::PARAM_LEVEL),
        Some(-12.0),
        "the plugin lost the first of nine edits made while muted"
    );
}
