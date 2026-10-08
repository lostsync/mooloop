//! Notes through the song's patch (`docs/plans/song-patch/07-notes-through-the-patch.md`):
//! Keys' notes wired from its Notes in tag to another channel's Notes out
//! tag reach that channel at the offsets they reach Keys, copied or taken,
//! and no note the patch played is left sounding by anything that releases
//! notes today.

use std::collections::HashSet;

use mooloop_core::harmony::{ChordQuality, Mode};
use mooloop_core::{
    CanvasPoint, ChannelId, InputSource, Jack, LoopRange, ModChanceParams, ModChordParams, ModModalParams,
    ModScaleParams, ModSourceId, ModTransposeParams, ModulatorParams, NoteEvent, PatternPlacement, PlaybackMode,
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

// Note boxes (`docs/plans/song-patch/08-note-boxes.md`).

/// [`song`] with `boxes` wired one after another between the tags: Keys'
/// notes through each in turn to the other channel. Returns the boxes' ids.
fn song_through(steps: &[(u32, u32)], take: bool, boxes: &[ModulatorParams]) -> (Project, Vec<ModSourceId>) {
    let mut project = song(steps, take);
    project.modulation.wires.clear();
    let keys = project.channels[0].id;
    let ids: Vec<ModSourceId> = boxes
        .iter()
        .map(|params| project.modulation.add_module(*params, InputSource::None, keys, "Keys"))
        .collect();
    let mut from = NOTES_IN;
    for &id in &ids {
        project.modulation.connect(Jack::new(from, 0), Jack::new(id, 0)).unwrap();
        from = id;
    }
    if !boxes.last().is_some_and(|params| *params == ModulatorParams::NoteGate) {
        project.modulation.connect(Jack::new(from, 0), Jack::new(NOTES_OUT, 0)).unwrap();
    }
    (project, ids)
}

/// Every note on and off the other channel is sent over `blocks` blocks,
/// as (frame, on, pitch), sorted by frame then pitch, and what is still
/// sounding after.
fn heard_over(project: &Project, blocks: usize, looping: bool) -> (Vec<(usize, bool, u8)>, Sounding) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut heard = Vec::new();
    let mut sounding = Sounding::default();
    for block in 0..blocks {
        if looping {
            render.process_block(BLOCK);
        } else {
            render.process_once_block(BLOCK);
        }
        let events = render.events_of(1);
        sounding.hear(&events);
        heard.extend(
            notes(&events)
                .into_iter()
                .map(|(offset, on, note)| (block * BLOCK + offset as usize, on, note)),
        );
    }
    heard.sort_by_key(|&(frame, on, note)| (frame, on, note));
    (heard, sounding)
}

/// The pitches the other channel starts and ends, in order, ignoring when.
fn pitches(heard: &[(usize, bool, u8)], on: bool) -> Vec<u8> {
    heard.iter().filter(|event| event.1 == on).map(|event| event.2).collect()
}

const BAR_BLOCKS: usize = SAMPLE_RATE as usize * 2 / BLOCK;

#[test]
fn a_chord_box_plays_its_chord_in_its_inversion_and_releases_it() {
    // Keys plays C4 then C#4 (60, 61), two steps each.
    let chord = ModulatorParams::Chord(ModChordParams {
        quality: ChordQuality::Minor7,
        inversion: 1,
    });
    let (project, _) = song_through(&[(0, 2), (4, 2)], true, &[chord]);
    let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
    assert_eq!(pitches(&heard, true), [63, 67, 70, 72, 64, 68, 71, 73]);
    assert_eq!(pitches(&heard, false), [63, 67, 70, 72, 64, 68, 71, 73]);
    assert!(sounding.0.is_empty());
    // Each chord starts where its note does.
    let first = heard.iter().filter(|event| event.1).map(|event| event.0).collect::<Vec<_>>();
    assert!(first[..4].iter().all(|&frame| frame == first[0]));
}

#[test]
fn a_modal_box_snaps_then_stacks_its_degree_and_a_scale_box_only_snaps() {
    // D dorian. Keys plays 60 (C, in the mode) and 61 (C#, not: down to C).
    let modal = ModulatorParams::Modal(ModModalParams {
        root: 2,
        mode: Mode::Dorian,
        seventh: false,
    });
    let (project, _) = song_through(&[(0, 2), (4, 2)], true, &[modal]);
    let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
    // C E G both times: the second is the same three pitches again.
    assert_eq!(pitches(&heard, true), [60, 64, 67, 60, 64, 67]);
    assert_eq!(pitches(&heard, false), [60, 64, 67, 60, 64, 67]);
    assert!(sounding.0.is_empty());

    let scale = ModulatorParams::Scale(ModScaleParams {
        root: 0,
        mode: Mode::Major,
    });
    let (project, _) = song_through(&[(0, 2), (4, 2), (8, 2)], true, &[scale]);
    let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
    // 60 stays; 61 goes down to 60; 62 stays.
    assert_eq!(pitches(&heard, true), [60, 60, 62]);
    assert_eq!(pitches(&heard, false), [60, 60, 62]);
    assert!(sounding.0.is_empty());
}

/// Two notes held at once that a box lands on one pitch play it once, and
/// it ends with the last of them.
#[test]
fn two_notes_on_one_pitch_play_once_and_end_with_the_last() {
    let scale = ModulatorParams::Scale(ModScaleParams::default());
    // 60 for eight steps; 61 (snapped to 60) from step 2 for two.
    let (project, _) = song_through(&[(0, 8), (2, 2)], true, &[scale]);
    let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
    assert_eq!(pitches(&heard, true), [60]);
    assert_eq!(pitches(&heard, false), [60]);
    let off = heard.iter().find(|event| !event.1).unwrap().0;
    assert!(off >= 8 * SAMPLE_RATE as usize / 8 - BLOCK, "it ends with the long note, at {off}");
    assert!(sounding.0.is_empty());
}

#[test]
fn a_transpose_box_moves_notes_and_drops_what_leaves_the_range() {
    let down = ModulatorParams::Transpose(ModTransposeParams { semitones: -12 });
    let (project, _) = song_through(&[(0, 2), (4, 2)], true, &[down]);
    let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
    assert_eq!(pitches(&heard, true), [48, 49]);
    assert_eq!(pitches(&heard, false), [48, 49]);
    assert!(sounding.0.is_empty());

    // Two transposes of +48 take 60 past 127: it falls, NoteOff and all.
    let far = ModulatorParams::Transpose(ModTransposeParams { semitones: 48 });
    let (project, _) = song_through(&[(0, 2)], true, &[far, far]);
    let (heard, _) = heard_over(&project, BAR_BLOCKS, false);
    assert!(heard.is_empty(), "{heard:?}");
}

/// A wire into a transpose's second inlet adds whole semitones, read once
/// a block: a `+ 0.25` box, an offset with nothing in, adds three.
#[test]
fn a_wire_into_transpose_adds_semitones() {
    let transpose = ModulatorParams::Transpose(ModTransposeParams::default());
    let (mut project, ids) = song_through(&[(4, 2)], true, &[transpose]);
    let keys = project.channels[0].id;
    let offset = project.modulation.add_module(
        ModulatorParams::Math(mooloop_core::ModMathParams {
            op: mooloop_core::ModMathOp::Add,
            operand: 0.25,
            ..Default::default()
        }),
        InputSource::None,
        keys,
        "Keys",
    );
    project.modulation.connect(Jack::new(offset, 0), Jack::new(ids[0], 1)).unwrap();
    let (heard, _) = heard_over(&project, BAR_BLOCKS, false);
    assert_eq!(pitches(&heard, true), [63]);
    assert_eq!(pitches(&heard, false), [63]);
}

/// A chance box drops some notes and passes the rest, NoteOffs following,
/// and the same ones played and bounced.
#[test]
fn a_chance_box_plays_the_same_notes_bounced_as_played() {
    let chance = ModulatorParams::Chance(ModChanceParams { probability: 0.5 });
    // Every step but the last, whose NoteOff would land on the next bar.
    let steps: Vec<(u32, u32)> = (0..15).map(|step| (step, 1)).collect();
    let (project, _) = song_through(&steps, true, &[chance]);
    let (played, sounding) = heard_over(&project, BAR_BLOCKS, true);
    let (bounced, _) = heard_over(&project, BAR_BLOCKS, false);
    assert_eq!(played, bounced);
    assert!(sounding.0.is_empty());
    let passed = pitches(&played, true).len();
    assert!((1..15).contains(&passed), "some of the fifteen, not none or all: {passed}");
    assert_eq!(pitches(&played, false).len(), passed, "each NoteOff follows its NoteOn");

    let never = ModulatorParams::Chance(ModChanceParams { probability: 0.0 });
    let (project, _) = song_through(&steps, true, &[never]);
    assert!(heard_over(&project, BAR_BLOCKS, false).0.is_empty());
}

/// Stopping, seeking, muting or panicking mid-chord leaves nothing the
/// boxes played sounding.
#[test]
fn nothing_hangs_when_a_chord_is_choked_mid_note() {
    let chord = ModulatorParams::Chord(ModChordParams::default());
    let scale = ModulatorParams::Scale(ModScaleParams::default());
    let (project, _) = song_through(&[(0, 8)], false, &[chord, scale]);
    let commands: [Vec<EngineCommand>; 4] = [
        vec![EngineCommand::Stop],
        vec![EngineCommand::Seek { tick: f64::from(12 * TICKS_PER_STEP) }],
        vec![EngineCommand::SetChannelMuted { channel: 0, muted: true }],
        vec![EngineCommand::Panic],
    ];
    for commands in commands {
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();
        let mut sounding = Sounding::default();
        run(&mut render, STEP_BLOCKS, false, &mut sounding);
        assert_eq!(sounding.0.len(), 3, "the chord sounds");
        for command in commands {
            render.apply_command(command);
        }
        run(&mut render, 2, false, &mut sounding);
        assert!(sounding.0.is_empty(), "{:?}", sounding.0);
    }
}

/// Removing the box mid-chord releases the chord; retyping it keeps the
/// chord it played and releases it when the note ends.
#[test]
fn a_box_edited_mid_chord_releases_what_it_played() {
    let chord = ModulatorParams::Chord(ModChordParams::default());
    let (project, ids) = song_through(&[(0, 4)], false, &[chord]);
    let mut retyped = project.clone();
    retyped.modulation.module_mut(ids[0]).unwrap().params = ModulatorParams::Chord(ModChordParams {
        quality: ChordQuality::Minor,
        inversion: 0,
    });
    let mut removed = project.clone();
    removed.modulation.remove_module(ids[0]);
    for edited in [retyped, removed] {
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.play();
        let mut sounding = Sounding::default();
        run(&mut render, 1, false, &mut sounding);
        let chord: Vec<u64> = sounding.0.iter().copied().collect();
        assert_eq!(chord.len(), 3);
        let set = SongModulator::of_project(&edited);
        let _ = render.apply_structural(StructuralCommand::SetModulation { set });
        run(&mut render, 6 * STEP_BLOCKS, false, &mut sounding);
        for id in chord {
            assert!(!sounding.0.contains(&id));
        }
    }
}

/// A gate box puts out 1 while a note reaches it, and the latest NoteOn's
/// pitch and velocity on its other outlets.
#[test]
fn a_gate_box_puts_out_gate_pitch_and_velocity() {
    let (mut project, ids) = song_through(&[(0, 2)], false, &[ModulatorParams::NoteGate]);
    let keys = project.channels[0].id;
    let follow = |project: &mut Project, port: u8| {
        let id = project.modulation.add_module(
            ModulatorParams::Math(mooloop_core::ModMathParams {
                op: mooloop_core::ModMathOp::Add,
                operand: 0.0,
                ..Default::default()
            }),
            InputSource::None,
            keys,
            "Keys",
        );
        project.modulation.connect(Jack::new(ids[0], port), Jack::new(id, 0)).unwrap();
        id
    };
    let pitch = follow(&mut project, 1);
    let velocity = follow(&mut project, 2);
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    render.play();
    let at = |render: &RenderState, id| render.song_modulation().plan().position_of(id).unwrap();
    let read = |render: &RenderState| {
        let outputs = render.song_modulation().outputs();
        (outputs[at(render, ids[0])], outputs[at(render, pitch)], outputs[at(render, velocity)])
    };
    render.process_once_block(BLOCK);
    let (gate, held_pitch, held_velocity) = read(&render);
    assert_eq!(gate, 1.0);
    assert!((held_pitch - 60.0 / 127.0).abs() < 1e-6, "{held_pitch}");
    assert!((held_velocity - 100.0 / 127.0).abs() < 1e-6, "{held_velocity}");
    for _ in 0..3 * STEP_BLOCKS {
        render.process_once_block(BLOCK);
    }
    let (gate, held_pitch, _) = read(&render);
    assert_eq!(gate, 0.0, "the note ended");
    assert!((held_pitch - 60.0 / 127.0).abs() < 1e-6, "the pitch holds");
}

/// The boxes allocate nothing either, chords made, notes dropped and a
/// gate filed.
#[test]
fn the_note_boxes_allocate_nothing() {
    let chord = ModulatorParams::Chord(ModChordParams::default());
    let chance = ModulatorParams::Chance(ModChanceParams { probability: 0.5 });
    let (mut project, ids) = song_through(&[(0, 2), (1, 2), (2, 2), (3, 2)], true, &[chord, chance]);
    let gate = project
        .modulation
        .add_module(ModulatorParams::NoteGate, InputSource::None, project.channels[0].id, "Keys");
    project.modulation.connect(Jack::new(ids[0], 0), Jack::new(gate, 0)).unwrap();
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
    assert_eq!(after, before, "the callback allocated or freed with note boxes in the patch");
}
