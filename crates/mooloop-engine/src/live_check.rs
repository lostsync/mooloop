//! **Test support, not API.** A song played through the real executor, the
//! way a driver's callback plays it, with no driver and no audio server, so
//! that a check outside this crate can hold what the export renders against
//! what playback would have played.
//!
//! It is compiled only for this crate's own tests and behind the
//! `test-support` feature, which only examples and tests enable
//! (`mooloop-session`'s `clap_automation_case`, MOO-82, runs a real CLAP
//! plugin's automation through both paths). Nothing in the application
//! calls it: the application has a driver.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use mooloop_core::{
    ChannelSource, EffectKind, EffectParams, EffectTarget, EngineCommand, EngineEvent, PluginSlotId, Project, MAX_BUSES,
    MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL,
};
use mooloop_dsp::{AudioNode, IntegerDelay, SampleData};

use crate::executor::{prepare_audio_thread, Executor, ExecutorIo};
use crate::load::LoadMeters;
use crate::render::RenderState;
use crate::{RealtimeCommand, StructuralCommand, StructuralReclaim};

/// Play `frames` of `project` from its start through a fresh executor in
/// callbacks of `block` frames, on a thread of its own, and return the
/// master's interleaved stereo.
///
/// Each processor in `plugins` goes into its device the way the session's
/// rack sends it live: `ReplaceEffect` down the command ring, keyed by the
/// plugin's slot, ahead of `Play`. Compensation is what the install derives
/// from the song, as it is live before the session's first
/// `sync_compensation`, so a plugin that adds latency is heard uncompensated
/// here where an export compensates it: hold the two against each other with
/// plugins that add none.
pub fn play_through_executor(
    project: &Project,
    plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>,
    sample_rate: u32,
    frames: usize,
    block: usize,
) -> Vec<f32> {
    let Rig {
        mut executor,
        mut events,
        mut reclaim,
    } = rig(project, &[], plugins, sample_rate);
    let block = block.max(1);
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                prepare_audio_thread();
                let silence = vec![0.0f32; block];
                let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
                let mut out = Vec::with_capacity(frames * 2);
                let mut done = 0;
                while done < frames {
                    let n = block.min(frames - done);
                    executor.process_with_input(
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
                    while events.pop().is_ok() {}
                    while let Ok(reclaimed) = reclaim.pop() {
                        drop(reclaimed);
                    }
                    done += n;
                }
                drop(executor);
                out
            })
            .join()
            .expect("the audio thread did not panic")
    })
}

/// What one callback cost, as [`time_through_executor`] measured it.
#[derive(Debug, Clone, Copy)]
pub struct CallbackCost {
    /// How far into the song the callback started, in frames.
    pub frame: usize,
    /// Wall time inside the callback.
    pub nanos: u64,
    /// How many times the callback called the allocator, as the caller's
    /// counter reads it on the audio thread. Zero is the contract.
    pub allocations: usize,
}

/// Play `frames` of `project` through a fresh executor in callbacks of
/// `block` frames, as [`play_through_executor`] does, and time every
/// callback instead of keeping the audio.
///
/// For finding what a real song costs where (`mooloop-session`'s
/// `tests/song_block_cost.rs`): a synthetic project only exercises what its
/// author thought of, and MOO-195 cost 40% of a block at two bars of one song
/// and nothing anywhere else. `samples` is in seat order, as the install
/// takes it. `allocations` reads a per-thread allocation count; it is called
/// on the audio thread either side of each callback, so the caller's global
/// allocator decides what counts. Only the callback is timed: draining the
/// event and reclaim rings is the control thread's work live, and happens
/// here between callbacks.
pub fn time_through_executor(
    project: &Project,
    samples: &[Option<Arc<SampleData>>],
    sample_rate: u32,
    frames: usize,
    block: usize,
    allocations: fn() -> usize,
) -> Vec<CallbackCost> {
    let Rig {
        mut executor,
        mut events,
        mut reclaim,
    } = rig(project, samples, BTreeMap::new(), sample_rate);
    let block = block.max(1);
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                prepare_audio_thread();
                let silence = vec![0.0f32; block];
                let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
                let mut costs = Vec::with_capacity(frames.div_ceil(block));
                let mut done = 0;
                while done < frames {
                    let n = block.min(frames - done);
                    let before = allocations();
                    let started = Instant::now();
                    executor.process_with_input(
                        std::iter::empty(),
                        &silence[..n],
                        &silence[..n],
                        &mut l[..n],
                        &mut r[..n],
                    );
                    let nanos = started.elapsed().as_nanos() as u64;
                    let allocations = allocations() - before;
                    costs.push(CallbackCost {
                        frame: done,
                        nanos,
                        allocations,
                    });
                    while events.pop().is_ok() {}
                    while let Ok(reclaimed) = reclaim.pop() {
                        drop(reclaimed);
                    }
                    done += n;
                }
                drop(executor);
                costs
            })
            .join()
            .expect("the audio thread did not panic")
    })
}

/// A fresh executor over `project`, with its plugins swapped in and `Play`
/// queued, and the two rings the control thread would drain.
struct Rig {
    executor: Executor,
    events: rtrb::Consumer<EngineEvent>,
    reclaim: rtrb::Consumer<StructuralReclaim>,
}

fn rig(
    project: &Project,
    samples: &[Option<Arc<SampleData>>],
    mut plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>,
    sample_rate: u32,
) -> Rig {
    let (mut commands, cmd_rx) = rtrb::RingBuffer::new(256);
    let (evt_tx, events) = rtrb::RingBuffer::new(4096);
    let (reclaim_tx, reclaim) = rtrb::RingBuffer::new(256);
    let executor = Executor::new(
        ExecutorIo {
            cmd_rx,
            evt_tx,
            reclaim_tx,
        },
        Box::new(RenderState::from_project(sample_rate, project, samples)),
        Arc::new(AtomicU64::new(0)),
        sample_rate,
        LoadMeters::new(),
    );
    let channels = project
        .channels
        .iter()
        .take(MAX_CHANNELS)
        .enumerate()
        .map(|(index, channel)| (EffectTarget::Channel(index as u8), &channel.setup.effects));
    let buses = project
        .buses
        .iter()
        .take(MAX_BUSES)
        .enumerate()
        .map(|(index, bus)| (EffectTarget::Bus(index as u8), &bus.effects));
    for (target, effects) in channels.chain(buses) {
        for (row, effect) in effects.iter().take(MAX_EFFECTS_PER_CHANNEL).enumerate() {
            let EffectParams::Plugin(slot) = effect.params else {
                continue;
            };
            let Some(node) = plugins.remove(&slot) else {
                continue;
            };
            let key = u64::from(slot.0);
            let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
            let swap = RealtimeCommand::Structural(StructuralCommand::ReplaceEffect {
                target,
                slot: row as u8,
                expected_kind: EffectKind::Plugin,
                expected_resource_key: key,
                resource_key: key,
                node,
                align,
            });
            assert!(commands.push(swap).is_ok(), "the command ring has room");
        }
    }
    // A hosted instrument's processor goes into its channel's source the
    // way the rack sends it (MOO-84).
    for (index, channel) in project.channels.iter().take(MAX_CHANNELS).enumerate() {
        let ChannelSource::Plugin(slot) = channel.setup.source else {
            continue;
        };
        let Some(node) = plugins.remove(&slot) else {
            continue;
        };
        let swap = RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor {
            channel: index as u8,
            slot,
            node: Some(node),
        });
        assert!(commands.push(swap).is_ok(), "the command ring has room");
    }
    assert!(commands
        .push(RealtimeCommand::Engine(EngineCommand::Play))
        .is_ok());
    Rig {
        executor,
        events,
        reclaim,
    }
}
