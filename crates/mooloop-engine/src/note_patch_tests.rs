//! Notes through the song's patch (`docs/plans/song-patch/07-notes-through-the-patch.md`):
//! Keys' notes wired from its Notes in tag to another channel's Notes out
//! tag reach that channel at the offsets they reach Keys, copied or taken,
//! and no note the patch played is left sounding by anything that releases
//! notes today.

use std::collections::HashSet;

use mooloop_core::{
    CanvasPoint, ChannelId, Jack, LoopRange, ModSourceId, NoteEvent, PatternPlacement, PlaybackMode,
    Project, ProjectChannel, SongTag, TagKind, Wire, TICKS_PER_STEP,
};
use mooloop_dsp::{Event, TimedEvent};

use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use crate::{EngineCommand, SongModulator, StructuralCommand};

const BLOCK: usize = 256;
/// A step at 120 BPM, in blocks, rounded up.
const STEP_BLOCKS: usize = (SAMPLE_RATE as usize / 8).div_ceil(BLOCK);

const NOTES_IN: ModSourceId = ModSourceId(1);
const NOTES_OUT: ModSourceId = ModSourceId(2);

/// Keys on channel 0 playing two-step notes on `steps` of a one-bar
/// pattern, a second channel, and Keys' notes wired to it, taken or copied.
fn song(steps: &[(u32, u32)], take: bool) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    let mut keys = ProjectChannel::sampler(0, 2);
    for (id, &(step, length)) in steps.iter().enumerate() {
        keys.notes[0].push(NoteEvent::new(
            id as u32 + 1,
            step * TICKS_PER_STEP,
            length * TICKS_PER_STEP,
            60 + id as u8,
            100,
        ));
    }
    project.channels.push(keys);
    project.channels.push(ProjectChannel::sampler(1, 2));
    project.assign_channel_ids();
    project.pattern_lengths = vec![16, 16];
    let (keys, other) = (project.channels[0].id, project.channels[1].id);
    project.modulation.tags = vec![
        SongTag {
            id: NOTES_IN,
            at: CanvasPoint::default(),
            kind: TagKind::NotesIn {
                channel: Some(keys),
                take,
            },
        },
        SongTag {
            id: NOTES_OUT,
            at: CanvasPoint::default(),
            kind: TagKind::NotesOut { channel: Some(other) },
        },
    ];
    project.modulation.wires = vec![Wire {
        from: Jack::new(NOTES_IN, 0),
        to: Jack::new(NOTES_OUT, 0),
        bend: None,
        late: false,
    }];
    project.modulation.next_source_id = 3;
    project
}

fn notes(events: &[TimedEvent]) -> Vec<(u32, bool, u8)> {
    events
        .iter()
        .filter_map(|event| match event.event {
            Event::NoteOn { note, .. } => Some((event.offset, true, note)),
            Event::NoteOff { note, .. } => Some((event.offset, false, note)),
            _ => None,
        })
        .collect()
}

/// Every NoteOn a channel was sent that no NoteOff or Choke has ended yet.
#[derive(Default)]
struct Sounding(HashSet<u64>);

impl Sounding {
    fn hear(&mut self, events: &[TimedEvent]) {
        for event in events {
            match event.event {
                Event::NoteOn { id, .. } => {
                    self.0.insert(id);
                }
                Event::NoteOff { id, .. } => {
                    self.0.remove(&id);
                }
                Event::Choke => self.0.clear(),
                _ => {}
            }
        }
    }
}

fn run(render: &mut RenderState, blocks: usize, looping: bool, sounding: &mut Sounding) {
    for _ in 0..blocks {
        if looping {
            render.process_block(BLOCK);
        } else {
            render.process_once_block(BLOCK);
        }
        sounding.hear(&render.events_of(1));
    }
}

/// Keys' notes reach the other channel at the offsets they reach Keys, in
/// the realtime path and the offline one. Copied, Keys still plays them;
/// taken, only the other channel does.
#[test]
fn keys_notes_reach_another_channel_copied_and_taken() {
    for take in [false, true] {
        for looping in [true, false] {
            let project = song(&[(0, 2), (3, 1), (8, 4), (13, 2)], take);
            let plain = song(&[(0, 2), (3, 1), (8, 4), (13, 2)], false);
            let mut patched = RenderState::from_project(SAMPLE_RATE, &project, &[]);
            let mut unpatched = RenderState::from_project(SAMPLE_RATE, &Project {
                modulation: Default::default(),
                ..plain
            }, &[]);
            patched.play();
            unpatched.play();
            let mut heard = 0;
            // One bar exactly.
            for _ in 0..SAMPLE_RATE as usize * 2 / BLOCK {
                for render in [&mut patched, &mut unpatched] {
                    if looping {
                        render.process_block(BLOCK);
                    } else {
                        render.process_once_block(BLOCK);
                    }
                }
                let keys = notes(&unpatched.events_of(0));
                heard += keys.len();
                assert_eq!(notes(&patched.events_of(1)), keys, "the other channel plays Keys' part");
                let own = notes(&patched.events_of(0));
                if take {
                    assert!(own.is_empty(), "taken, Keys plays nothing of its own: {own:?}");
                } else {
                    assert_eq!(own, keys, "copied, Keys still plays its part");
                }
            }
            assert_eq!(heard, 8, "every note on and off of the bar was compared");
            let counts: Vec<_> = patched.song_modulation().tag_activity().map(|(notes, _)| notes).collect();
            assert_eq!(counts, [4, 4], "each tag counts the four notes it ran");
        }
    }
}

/// Play Keys' first note into the other channel, do `then`, and check every
/// note the other channel was sent has ended `blocks` later.
fn released_by(project: &Project, looping: bool, then: impl FnOnce(&mut RenderState), blocks: usize) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut sounding = Sounding::default();
    run(&mut render, STEP_BLOCKS, looping, &mut sounding);
    let held: Vec<u64> = sounding.0.iter().copied().collect();
    assert_eq!(held.len(), 1, "the first note sounds on the other channel");
    then(&mut render);
    run(&mut render, blocks, looping, &mut sounding);
    for id in held {
        assert!(!sounding.0.contains(&id), "the note the patch played is still sounding");
    }
}

#[test]
fn stop_pause_seek_switch_mute_and_panic_release_what_the_patch_played() {
    let project = song(&[(0, 8)], false);
    let commands: [(&str, Vec<EngineCommand>); 6] = [
        ("stop", vec![EngineCommand::Stop]),
        ("pause", vec![EngineCommand::Pause]),
        ("seek", vec![EngineCommand::Seek { tick: f64::from(12 * TICKS_PER_STEP) }]),
        ("pattern switch", vec![EngineCommand::SetCurrentPattern(1)]),
        ("mute", vec![EngineCommand::SetChannelMuted { channel: 0, muted: true }]),
        ("panic", vec![EngineCommand::Panic]),
    ];
    for (what, commands) in commands {
        eprintln!("{what}");
        released_by(
            &project,
            false,
            |render| {
                for command in commands {
                    render.apply_command(command);
                }
            },
            2,
        );
    }
}

/// A note held across a loop's end is let go at the fold, on the channel the
/// patch played it on as well as on Keys.
#[test]
fn a_loop_fold_releases_what_the_patch_played() {
    let mut project = song(&[(0, 8)], true);
    project.playback_mode = PlaybackMode::Song;
    project.playlist = vec![PatternPlacement::new(0, 0)];
    project.loop_range = LoopRange {
        start_tick: 0,
        end_tick: 2 * TICKS_PER_STEP,
        enabled: true,
    };
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    render.play();
    let mut sounding = Sounding::default();
    run(&mut render, 1, true, &mut sounding);
    let first: Vec<u64> = sounding.0.iter().copied().collect();
    assert_eq!(first.len(), 1);
    run(&mut render, 3 * STEP_BLOCKS, true, &mut sounding);
    assert!(!sounding.0.contains(&first[0]), "the fold released it");
    assert!(sounding.0.len() <= 1, "one lap's note at most: {:?}", sounding.0);
}

/// Deleting the wire, or rebinding its tag, while a note sounds releases it
/// on the next block.
#[test]
fn removing_the_wire_mid_note_releases_it() {
    let project = song(&[(0, 8)], false);
    let unwired = Project {
        modulation: mooloop_core::SongModulation {
            wires: Vec::new(),
            ..project.modulation.clone()
        },
        ..project.clone()
    };
    let mut rebound = project.clone();
    let keys = rebound.channels[0].id;
    rebound.modulation.tags[1].kind = TagKind::NotesOut { channel: Some(keys) };
    for edited in [unwired, rebound] {
        released_by(
            &project,
            false,
            |render| {
                let set = SongModulator::of_project(&edited);
                let _ = render.apply_structural(StructuralCommand::SetModulation { set });
            },
            1,
        );
    }
}

/// The note pass allocates nothing, notes flowing, taken and released.
#[test]
fn the_note_pass_allocates_nothing() {
    let project = song(&[(0, 2), (1, 2), (2, 2), (3, 2)], true);
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    render.play();
    render.process_once_block(BLOCK);
    let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    for _ in 0..6 * STEP_BLOCKS {
        render.process_once_block(BLOCK);
    }
    render.apply_command(EngineCommand::Panic);
    render.process_once_block(BLOCK);
    let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    assert_eq!(after, before, "the callback allocated or freed with notes in the patch");
    let counts: Vec<_> = render.song_modulation().tag_activity().map(|(notes, _)| notes).collect();
    assert_eq!(counts, [4, 4]);
}

/// A notes tag on a channel the song does not have sends nothing.
#[test]
fn a_link_to_a_channel_the_song_lacks_is_left_out() {
    let mut project = song(&[(0, 2)], false);
    project.modulation.tags[1].kind = TagKind::NotesOut {
        channel: Some(ChannelId(999)),
    };
    let plan = SongModulator::compile(&project);
    assert!(plan.notes.is_empty());
}
