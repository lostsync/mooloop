//! A CLAP instrument as a channel's source (MOO-85,
//! `docs/plans/plugin-hosting/10-clap-instruments.md`), through the engine:
//! the in-repo test sine (`mooloop.test.sine`: a sine voice per note, a
//! 50 ms release, `note_end` when a voice finishes), loaded by path and run
//! by `ClapProcessor` inside the channel's `HostedSource`.
//!
//! - A pattern played by the export path and by the executor at 512 and at
//!   64 frames is the same to the sample, including a note held across the
//!   loop point, and every note starts on its own frame.
//! - A choke releases every held note, and a note-off that arrives after it
//!   for a note the choke already released reaches nothing: it is dropped,
//!   not sent as a wildcard that would release a newer voice on its key.
//! - A stop resets the plugin: silence, and a note-off from before the stop
//!   is stale after it.
//!
//! Every processor runs on a thread of its own: the test sine checks that
//! `process` is never called on its main thread.

use std::collections::BTreeMap;

use mooloop_core::{
    ChannelSource, DeviceKind, NoteEvent, PluginFormat, PluginRef, PluginSlotId,
    PluginSlotState, PluginState, Project, ProjectChannel, TICKS_PER_STEP,
};
use mooloop_dsp::node::Discontinuity;
use mooloop_dsp::{AudioNode, Event, EventList, ProcessContext, StereoBus, TimedEvent};
use mooloop_plugin_host::clap::{ClapInstance, MAX_FRAMES};
use mooloop_plugin_host::{AudioConfig, HostedInstance, Lifeline};
use mooloop_test_plugin as test_plugin;

use crate::plugin_host_tests::{play_with, render_hosted, test_plugin_path, worst_difference};
use crate::render_test_support::SAMPLE_RATE;
use crate::{RealtimeCommand, StructuralCommand};

fn sine_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: test_plugin::SINE_ID.to_owned(),
        name: "Test Sine".to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    }
}

fn open_sine() -> ClapInstance {
    ClapInstance::open(
        &test_plugin_path(),
        &sine_ref(),
        &PluginState::default(),
        AudioConfig {
            sample_rate: SAMPLE_RATE,
            max_frames: MAX_FRAMES,
        },
    )
    .expect("the test sine opens")
}

const FRAMES_PER_STEP: usize = 6_000;
/// `(step, length in steps, key)`; the last is held across the loop point.
const NOTES: [(u32, u32, u8); 4] = [(0, 1, 57), (4, 1, 60), (9, 2, 64), (14, 4, 69)];

/// One channel whose source is the sine, playing [`NOTES`] in a one-bar
/// loop at 120 bpm.
fn song() -> (Project, PluginSlotId) {
    let mut project = Project {
        bpm: 120,
        ..Project::default()
    };
    project.pattern_lengths[0] = 16;
    let slot = project.add_plugin_slot(PluginSlotState::new(sine_ref()));
    let mut channel = ProjectChannel::drum_synth(0, 1);
    channel.setup.source = ChannelSource::Plugin(slot);
    channel.setup.channel.kind = DeviceKind::Plugin;
    channel.setup.channel.volume = 1.0;
    for (id, (step, steps, key)) in NOTES.into_iter().enumerate() {
        channel.notes[0].push(NoteEvent::new(
            id as u32 + 1,
            step * TICKS_PER_STEP,
            steps * TICKS_PER_STEP,
            key,
            100,
        ));
    }
    project.channels = vec![channel];
    project.assign_channel_ids();
    (project, slot)
}

/// The first frame of each run of sound that follows a hundred frames of
/// silence. The sine starts at phase 0, so a note is first heard one frame
/// after it starts: the same offset for every note, which the differences
/// below cancel.
fn onsets(interleaved: &[f32]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut quiet = FRAMES_PER_STEP;
    for (frame, pair) in interleaved.chunks(2).enumerate() {
        if pair[0] == 0.0 {
            quiet += 1;
        } else {
            if quiet >= 100 {
                out.push(frame);
            }
            quiet = 0;
        }
    }
    out
}

/// **Offline and realtime agree, and every note is on its frame.** The
/// export's path (one pass of the pattern) and the executor at 512 and at 64
/// frames are the same to the sample over the bar; and the executor, which
/// loops, plays two passes the same at 512 and at 64, the note held across
/// the loop point included.
#[test]
fn a_clap_instrument_plays_a_pattern_the_same_offline_and_through_the_executor() {
    let (project, slot) = song();
    let frames = 16 * FRAMES_PER_STEP;
    let build = || {
        let mut instance = open_sine();
        assert!(instance.fits_source() && !instance.fits_effect(), "an instrument, and only that");
        let life = Lifeline::new();
        let node = instance.build_processor(life.tie()).expect("a processor");
        (instance, life, node)
    };

    let (_offline_instance, _offline_life, node) = build();
    let offline = render_hosted(&project, BTreeMap::from([(slot, node)]), frames, 512);
    let peak = offline.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.1, "the sine is heard, peak {peak}");

    let heard = onsets(&offline);
    assert!(heard.len() >= 4, "every note is heard: {heard:?}");
    let first = heard[0];
    for (index, (step, _, _)) in NOTES.iter().take(3).enumerate() {
        assert_eq!(
            heard[index] - first,
            *step as usize * FRAMES_PER_STEP,
            "note {index} is off its frame: {heard:?}"
        );
    }

    let mut looped = Vec::new();
    for block in [512, 64] {
        let (instance, life, node) = build();
        let swap = RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor {
            channel: 0,
            slot,
            node: Some(node),
        });
        let played = play_with(&project, vec![swap], 2 * frames, block);
        assert!(life.is_alone(), "the executor dropped its processor");
        assert_eq!(
            worst_difference(&played[..offline.len()], &offline),
            0.0,
            "the executor at {block} frames and the export disagree"
        );
        assert_eq!(instance.misbehaviour(), 0, "no call on the wrong thread");
        looped.push(played);
    }
    assert_eq!(worst_difference(&looped[0], &looped[1]), 0.0, "the second pass differs between block sizes");
    let second = onsets(&looped[0][offline.len()..]);
    assert!(!second.is_empty(), "the loop plays again");
}

fn context(frames: usize, at: u64) -> ProcessContext {
    ProcessContext {
        sample_rate: SAMPLE_RATE,
        frames,
        playing: true,
        bpm: 120.0,
        position_ticks: 0.0,
        position_frames: at,
    }
}

fn events(list: &[(u32, Event)]) -> EventList {
    let mut events = EventList::empty();
    for &(offset, event) in list {
        assert!(events.push_ordered(TimedEvent { offset, event }));
    }
    events
}

/// Render `blocks` through `node` on a thread of its own, each block's
/// events from `script`, and return the left channel.
fn run(node: &mut Box<dyn AudioNode + Send>, script: Vec<(EventList, Option<Discontinuity>)>) -> Vec<f32> {
    const BLOCK: usize = 512;
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let mut out = Vec::new();
                let mut bus = StereoBus::with_capacity(BLOCK);
                for (index, (events, discontinuity)) in script.iter().enumerate() {
                    if let Some(kind) = discontinuity {
                        node.on_discontinuity(*kind);
                    }
                    bus.clear(BLOCK);
                    node.process(&context(BLOCK, (index * BLOCK) as u64), &mut bus, events, None);
                    out.extend_from_slice(&bus.l[..BLOCK]);
                }
                out
            })
            .join()
            .expect("the audio thread did not panic")
    })
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

/// **A choke releases everything, and a stale note-off reaches nothing.**
/// Note 1 on key 60, a choke, then note 2 on the same key, then note 1's
/// note-off arriving late. Were the stale note-off sent with a wildcard id
/// it would release note 2 as well; dropped, note 2 sounds on.
#[test]
fn a_note_off_after_a_choke_does_not_release_a_newer_note_on_its_key() {
    let mut instance = open_sine();
    let life = Lifeline::new();
    let mut node = instance.build_processor(life.tie()).expect("a processor");
    let on = |id| Event::NoteOn {
        id,
        note: 60,
        velocity: 127,
    };
    let quiet = || (EventList::empty(), None);
    let mut script = vec![(events(&[(0, on(1))]), None)];
    script.push((events(&[(0, Event::Choke), (100, on(2)), (200, Event::NoteOff { id: 1, note: 60 })]), None));
    script.extend((0..10).map(|_| quiet()));
    let out = run(&mut node, script);
    let tail = &out[out.len() - 512..];
    assert!(rms(tail) > 0.1, "note 2 was released by note 1's stale note-off: rms {}", rms(tail));

    // And its own note-off does release it: silence once the 50 ms release
    // has run.
    let mut script = vec![(events(&[(0, Event::NoteOff { id: 2, note: 60 })]), None)];
    script.extend((0..10).map(|_| quiet()));
    let out = run(&mut node, script);
    assert_eq!(rms(&out[out.len() - 512..]), 0.0, "note 2 did not end");
    drop(node);
    assert!(life.is_alone());
}

/// **A stop silences the plugin, and what was held before it is stale.**
#[test]
fn a_stop_resets_the_instrument_and_its_held_notes() {
    let mut instance = open_sine();
    let life = Lifeline::new();
    let mut node = instance.build_processor(life.tie()).expect("a processor");
    let script = vec![
        (events(&[(0, Event::NoteOn { id: 7, note: 64, velocity: 127 })]), None),
        (EventList::empty(), None),
        (EventList::empty(), Some(Discontinuity::Stop)),
        // Note 8 on the same key, then note 7's note-off from before the
        // stop: stale, so note 8 holds.
        (events(&[(0, Event::NoteOn { id: 8, note: 64, velocity: 127 }), (50, Event::NoteOff { id: 7, note: 64 })]), None),
        (EventList::empty(), None),
        (EventList::empty(), None),
        (EventList::empty(), None),
        (EventList::empty(), None),
        (EventList::empty(), None),
    ];
    let out = run(&mut node, script);
    assert_eq!(rms(&out[2 * 512..3 * 512]), 0.0, "the stop did not silence the held note");
    assert!(rms(&out[out.len() - 512..]) > 0.1, "note 8 was released by a note-off from before the stop");
}
