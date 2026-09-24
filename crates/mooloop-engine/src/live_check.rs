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

use mooloop_core::{
    EffectKind, EffectParams, EffectTarget, EngineCommand, PluginSlotId, Project, MAX_BUSES,
    MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL,
};
use mooloop_dsp::{AudioNode, IntegerDelay};

use crate::executor::{prepare_audio_thread, Executor, ExecutorIo};
use crate::load::LoadMeters;
use crate::render::RenderState;
use crate::{RealtimeCommand, StructuralCommand};

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
    mut plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>,
    sample_rate: u32,
    frames: usize,
    block: usize,
) -> Vec<f32> {
    let (mut commands, cmd_rx) = rtrb::RingBuffer::new(256);
    let (evt_tx, mut events) = rtrb::RingBuffer::new(4096);
    let (reclaim_tx, mut reclaim) = rtrb::RingBuffer::new(256);
    let mut executor = Executor::new(
        ExecutorIo {
            cmd_rx,
            evt_tx,
            reclaim_tx,
        },
        Box::new(RenderState::from_project(sample_rate, project, &[])),
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
    assert!(commands
        .push(RealtimeCommand::Engine(EngineCommand::Play))
        .is_ok());
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
