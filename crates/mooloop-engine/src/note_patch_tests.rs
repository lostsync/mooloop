//! Notes through the song's patch (`docs/plans/archive/song-patch/07-notes-through-the-patch.md`):
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

// Note boxes (`docs/plans/archive/song-patch/08-note-boxes.md`).

/// [`song`] with `boxes` wired one after another between the tags: Keys'
/// notes through each in turn to the other channel. Returns the boxes' ids.
fn song_through(steps: &[(u32, u32)], take: bool, boxes: &[ModulatorParams]) -> (Project, Vec<ModSourceId>) {
    let mut project = song(steps, take);
    project.modulation.wires.clear();
    let ids: Vec<ModSourceId> = boxes
        .iter()
        .map(|params| project.modulation.add_module(*params, InputSource::None, "Keys"))
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
    let offset = project.modulation.add_module(
        ModulatorParams::Math(mooloop_core::ModMathParams {
            op: mooloop_core::ModMathOp::Add,
            operand: 0.25,
            ..Default::default()
        }),
        InputSource::None,
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
    let follow = |project: &mut Project, port: u8| {
        let id = project.modulation.add_module(
            ModulatorParams::Math(mooloop_core::ModMathParams {
                op: mooloop_core::ModMathOp::Add,
                operand: 0.0,
                ..Default::default()
            }),
            InputSource::None,
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
        .add_module(ModulatorParams::NoteGate, InputSource::None, "Keys");
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

// Note boxes with many jacks (`docs/plans/archive/song-patch-boxes/04-note-boxes-with-many-jacks.md`).

/// A box of `params` added to `project`'s patch.
fn add(project: &mut Project, params: ModulatorParams) -> ModSourceId {
    project.modulation.add_module(params, InputSource::None, "Keys")
}

fn wire(project: &mut Project, from: (ModSourceId, u8), to: (ModSourceId, u8)) {
    project.modulation.connect(Jack::new(from.0, from.1), Jack::new(to.0, to.1)).unwrap();
}

/// A box sending `value` with nothing wired in.
fn constant(project: &mut Project, value: f32) -> ModSourceId {
    add(
        project,
        ModulatorParams::Math(mooloop_core::ModMathParams {
            op: mooloop_core::ModMathOp::Add,
            operand: value,
            ..Default::default()
        }),
    )
}

/// Keys' notes into a major chord, and the chord into the other channel
/// through `then`, which returns the jack the other channel hears.
fn through_chord(then: impl FnOnce(&mut Project, ModSourceId) -> (ModSourceId, u8)) -> Project {
    chords_on(&[(0, 2), (4, 2)], then)
}

/// [`through_chord`] with Keys playing `steps`.
fn chords_on(steps: &[(u32, u32)], then: impl FnOnce(&mut Project, ModSourceId) -> (ModSourceId, u8)) -> Project {
    let (mut project, ids) = song_through(steps, true, &[ModulatorParams::Chord(ModChordParams::default())]);
    project.modulation.wires.retain(|wire| wire.to.node != NOTES_OUT);
    let out = then(&mut project, ids[0]);
    wire(&mut project, out, (NOTES_OUT, 0));
    project
}

/// The step's patch: a chord, and its lowest note an octave down under it,
/// met at one notes-out by a merge.
#[test]
fn pick_low_transposed_and_merged_plays_a_left_hand_under_the_chord() {
    let project = through_chord(|project, chord| {
        let pick = add(project, ModulatorParams::Pick(mooloop_core::ModPickParams { which: 1 }));
        let down = add(project, ModulatorParams::Transpose(ModTransposeParams { semitones: -12 }));
        let merge = add(project, ModulatorParams::Merge(mooloop_core::ModMergeParams { inputs: 2 }));
        wire(project, (chord, 0), (pick, 0));
        wire(project, (pick, 0), (down, 0));
        wire(project, (chord, 0), (merge, 0));
        wire(project, (down, 0), (merge, 1));
        (merge, 0)
    });
    let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
    // C major on 60, then C# major on 61, each with its root an octave down.
    let mut on = pitches(&heard, true);
    on.sort_unstable();
    assert_eq!(on, [48, 49, 60, 61, 64, 65, 67, 68]);
    assert_eq!(pitches(&heard, false).len(), 8);
    assert!(sounding.0.is_empty());
}

#[test]
fn pick_keeps_the_note_it_names_from_either_end() {
    for (which, expected) in [(1, [60, 61]), (-1, [67, 68]), (2, [64, 65]), (-2, [64, 65]), (3, [67, 68])] {
        let project = through_chord(|project, chord| {
            let pick = add(project, ModulatorParams::Pick(mooloop_core::ModPickParams { which }));
            wire(project, (chord, 0), (pick, 0));
            (pick, 0)
        });
        let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
        assert_eq!(pitches(&heard, true), expected, "pick {which}");
        assert_eq!(pitches(&heard, false), expected, "pick {which}");
        assert!(sounding.0.is_empty());
    }
    // Past the chord's size there is nothing to keep.
    let project = through_chord(|project, chord| {
        let pick = add(project, ModulatorParams::Pick(mooloop_core::ModPickParams { which: 4 }));
        wire(project, (chord, 0), (pick, 0));
        (pick, 0)
    });
    assert!(heard_over(&project, BAR_BLOCKS, false).0.is_empty());
}

#[test]
fn unpack_puts_each_note_of_a_chord_on_its_own_outlet() {
    for (port, expected) in [(0, [60, 61]), (1, [64, 65]), (2, [67, 68])] {
        let project = through_chord(|project, chord| {
            let unpack = add(project, ModulatorParams::Unpack(mooloop_core::ModUnpackParams { outputs: 3 }));
            wire(project, (chord, 0), (unpack, 0));
            (unpack, port)
        });
        let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
        assert_eq!(pitches(&heard, true), expected, "outlet {port}");
        assert_eq!(pitches(&heard, false), expected, "outlet {port}");
        assert!(sounding.0.is_empty());
    }
}

/// A switch or a gate takes a note wire into its `in` and works on notes.
/// Its control inlet is read a block late, as a transpose's is, so the
/// notes start a step in.
#[test]
fn a_switch_on_notes_sends_them_to_the_outlet_which_picks() {
    for (which, expected) in [(1.0, vec![60, 64, 67, 61, 65, 68]), (0.0, vec![])] {
        let project = chords_on(&[(1, 2), (5, 2)], |project, chord| {
            let switch = add(project, ModulatorParams::Switch(mooloop_core::ModSwitchParams::default()));
            wire(project, (chord, 0), (switch, 0));
            assert!(project.modulation.module(switch).unwrap().params.is_note_box(), "it took the note wire's sort");
            let picks = constant(project, which);
            wire(project, (picks, 0), (switch, 1));
            (switch, 1)
        });
        let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
        assert_eq!(pitches(&heard, true), expected, "which {which}");
        assert_eq!(pitches(&heard, false), expected, "which {which}");
        assert!(sounding.0.is_empty());
    }
}

#[test]
fn a_gate_on_notes_passes_them_while_open_and_closing_releases_them() {
    // Keys holds one note for eight steps from step 1: 60, then the chord
    // on it.
    let gated = |open: f32| {
        let (mut project, ids) =
            song_through(&[(1, 8)], true, &[ModulatorParams::Chord(ModChordParams::default())]);
        project.modulation.wires.retain(|wire| wire.to.node != NOTES_OUT);
        let gate = add(&mut project, ModulatorParams::Gate(mooloop_core::ModGateParams::default()));
        wire(&mut project, (ids[0], 0), (gate, 0));
        let level = constant(&mut project, open);
        wire(&mut project, (level, 0), (gate, 1));
        wire(&mut project, (gate, 0), (NOTES_OUT, 0));
        project
    };
    let (heard, _) = heard_over(&gated(0.0), BAR_BLOCKS, false);
    assert!(heard.is_empty(), "closed: {heard:?}");

    let open = gated(1.0);
    let mut render = RenderState::from_project(SAMPLE_RATE, &open, &[]);
    render.play();
    let mut sounding = Sounding::default();
    run(&mut render, STEP_BLOCKS + 2, false, &mut sounding);
    assert_eq!(sounding.0.len(), 3, "open, the chord sounds");
    let set = SongModulator::of_project(&gated(0.0));
    let _ = render.apply_structural(StructuralCommand::SetModulation { set });
    run(&mut render, 3, false, &mut sounding);
    assert!(sounding.0.is_empty(), "closing let go mid-note: {:?}", sounding.0);
}

/// The many-jack boxes allocate nothing either.
#[test]
fn the_many_jack_boxes_allocate_nothing() {
    let project = through_chord(|project, chord| {
        let unpack = add(project, ModulatorParams::Unpack(mooloop_core::ModUnpackParams { outputs: 3 }));
        let pick = add(project, ModulatorParams::Pick(mooloop_core::ModPickParams { which: -1 }));
        let merge = add(project, ModulatorParams::Merge(mooloop_core::ModMergeParams { inputs: 3 }));
        wire(project, (chord, 0), (unpack, 0));
        wire(project, (chord, 0), (pick, 0));
        wire(project, (unpack, 0), (merge, 0));
        wire(project, (unpack, 2), (merge, 1));
        wire(project, (pick, 0), (merge, 2));
        (merge, 0)
    });
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
    assert_eq!(after, before, "the callback allocated or freed with many-jack note boxes in the patch");
}

// wait (`docs/plans/archive/song-patch-boxes/05-wait.md`).

fn wait_of(typed: &str) -> ModulatorParams {
    mooloop_core::box_text::parse(typed).unwrap()
}

/// A wait plays each note, NoteOff and all, the set time later, live and
/// in a bounce alike.
#[test]
fn a_wait_plays_its_notes_the_set_time_later() {
    let (straight, _) = song_through(&[(0, 2), (4, 2)], true, &[]);
    let (waited, _) = song_through(&[(0, 2), (4, 2)], true, &[wait_of("wait 1/8")]);
    let (before, _) = heard_over(&straight, BAR_BLOCKS, false);
    // An eighth at 120 BPM is a quarter of a second.
    let later = SAMPLE_RATE as usize / 4;
    let shifted: Vec<_> = before.iter().map(|&(frame, on, note)| (frame + later, on, note)).collect();
    for looping in [false, true] {
        let (heard, sounding) = heard_over(&waited, BAR_BLOCKS, looping);
        assert_eq!(heard, shifted, "looping {looping}");
        assert!(sounding.0.is_empty());
    }
    let (heard, _) = heard_over(&song_through(&[(0, 2)], true, &[wait_of("wait 10ms")]).0, BAR_BLOCKS, false);
    assert_eq!(heard.first().map(|event| event.0), Some(SAMPLE_RATE as usize / 100));
}

/// Stopping, seeking or panicking while a wait holds notes back drops
/// them, and releases what it already played.
#[test]
fn nothing_hangs_when_the_transport_moves_mid_wait() {
    // Keys holds 60 for eight steps; the wait holds it back an eighth (two
    // steps).
    let (project, _) = song_through(&[(0, 8)], false, &[wait_of("wait 1/8")]);
    let commands = [
        EngineCommand::Stop,
        EngineCommand::Seek { tick: f64::from(12 * TICKS_PER_STEP) },
        EngineCommand::Panic,
    ];
    for command in commands {
        for (blocks, played) in [(STEP_BLOCKS, false), (3 * STEP_BLOCKS, true)] {
            let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
            render.play();
            let mut sounding = Sounding::default();
            run(&mut render, blocks, false, &mut sounding);
            assert_eq!(sounding.0.len(), usize::from(played), "{command:?} after {blocks} blocks");
            render.apply_command(command);
            for _ in 0..4 * STEP_BLOCKS {
                render.process_once_block(BLOCK);
                let events = render.events_of(1);
                sounding.hear(&events);
                assert!(
                    !events.iter().any(|event| matches!(event.event, Event::NoteOn { .. })),
                    "{command:?}: a held-back note played after"
                );
            }
            assert!(sounding.0.is_empty(), "{command:?}: {:?}", sounding.0);
        }
    }
}

/// The step's patch: an arp built from parts. A held chord unpacked into
/// four held boxes, a counter on the Beat tag picking one pitch of the four,
/// and a note box playing it on each beat, a block late, each note ending
/// as the next starts.
#[test]
fn an_arp_from_parts_plays_a_chord_one_note_a_beat() {
    let project = chords_on(&[(0, 16)], |project, chord| {
        if let Some(module) = project.modulation.modules.iter_mut().find(|module| module.id == chord) {
            module.params = mooloop_core::box_text::parse("chord maj7").unwrap();
        }
        let beat = project.modulation.add_tag(
            TagKind::Inlet {
                bind: Some(mooloop_core::InletSource::Beat),
            },
            CanvasPoint::default(),
        );
        let unpack = add(project, ModulatorParams::Unpack(mooloop_core::ModUnpackParams { outputs: 4 }));
        let counter = add(project, ModulatorParams::Counter(mooloop_core::ModCounterParams { steps: 4 }));
        let select = add(project, ModulatorParams::Select(mooloop_core::ModSelectParams { inputs: 4 }));
        let note = add(project, ModulatorParams::Note);
        wire(project, (chord, 0), (unpack, 0));
        wire(project, (beat, 0), (counter, 0));
        wire(project, (counter, 0), (select, 0));
        for at in 0..4 {
            let held = add(project, ModulatorParams::NoteGate);
            wire(project, (unpack, at), (held, 0));
            wire(project, (held, 1), (select, at + 1));
        }
        wire(project, (select, 0), (note, 0));
        wire(project, (beat, 0), (note, 2));
        (note, 0)
    });
    let (heard, sounding) = heard_over(&project, BAR_BLOCKS, false);
    let beat = SAMPLE_RATE as usize / 2;
    let on: Vec<(usize, u8)> = heard.iter().filter(|event| event.1).map(|event| (event.0, event.2)).collect();
    // The counter's first count is the play's start: E, G, B, then C.
    assert_eq!(on.iter().map(|&(_, note)| note).collect::<Vec<_>>(), [64, 67, 71, 60]);
    for (at, &(frame, _)) in on.iter().enumerate() {
        let late = frame - at * beat;
        assert!((BLOCK..BLOCK + 32).contains(&late), "note {at} at {frame}");
    }
    // Each ends as the next starts; the last still sounds.
    let off: Vec<usize> = heard.iter().filter(|event| !event.1).map(|event| event.0).collect();
    assert_eq!(off, on[1..].iter().map(|&(frame, _)| frame).collect::<Vec<_>>());
    assert_eq!(sounding.0.len(), 1);
}

// arp (`docs/plans/archive/song-patch-boxes/07-arp.md`).

fn arp_of(typed: &str) -> ModulatorParams {
    let params = mooloop_core::box_text::parse(typed).expect(typed);
    assert!(matches!(params, ModulatorParams::Arp(_)), "{typed}");
    params
}

/// Keys' chord, from the chord box, into an arp typed `typed`.
fn arped(steps: &[(u32, u32)], typed: &str) -> Project {
    let params = arp_of(typed);
    chords_on(steps, |project, chord| {
        let arp = add(project, params);
        wire(project, (chord, 0), (arp, 0));
        (arp, 0)
    })
}

/// On its own clock an arp plays the chord up, a note every sixteenth on
/// the grid, each for half its step, live and bounced alike.
#[test]
fn an_arp_plays_the_chord_up_on_the_grid_live_and_bounced() {
    let step = SAMPLE_RATE as usize / 8;
    for looping in [true, false] {
        let (heard, sounding) = heard_over(&arped(&[(0, 6)], "arp up 1/16"), 8 * STEP_BLOCKS, looping);
        let on: Vec<(usize, u8)> = heard.iter().filter(|event| event.1).map(|event| (event.0, event.2)).collect();
        // C major held for six steps: C E G C E G.
        let expected: Vec<(usize, u8)> = [60, 64, 67, 60, 64, 67]
            .iter()
            .enumerate()
            .map(|(at, &note)| (at * step, note))
            .collect();
        assert_eq!(on, expected, "looping {looping}");
        let off: Vec<usize> = heard.iter().filter(|event| !event.1).map(|event| event.0).collect();
        assert_eq!(off, expected.iter().map(|&(frame, _)| frame + step / 2).collect::<Vec<_>>());
        assert!(sounding.0.is_empty());
    }
}

/// Down, updown across two octaves, and played order.
#[test]
fn an_arp_plays_its_modes_and_octaves() {
    for (typed, expected) in [
        ("arp down 1/16", vec![67, 64, 60, 67]),
        ("arp updown 1/16 2", vec![60, 64, 67, 72, 76, 79, 76, 72]),
    ] {
        let (heard, _) = heard_over(&arped(&[(0, 16)], typed), 16 * STEP_BLOCKS, false);
        let on = pitches(&heard, true);
        assert_eq!(on[..expected.len()], expected[..], "{typed}");
    }
}

/// A chord change mid-arp is picked up at the next step, and nothing is
/// left sounding when the chord goes, a seek or a stop.
#[test]
fn an_arp_follows_chord_changes_without_hung_notes() {
    let (heard, sounding) = heard_over(&arped(&[(0, 3), (3, 3)], "arp up 1/16"), 8 * STEP_BLOCKS, false);
    // C major for three steps, then C# major.
    assert_eq!(pitches(&heard, true), [60, 64, 67, 61, 65, 68]);
    assert!(sounding.0.is_empty());
    for command in [EngineCommand::Stop, EngineCommand::Panic] {
        let mut render = RenderState::from_project(SAMPLE_RATE, &arped(&[(0, 16)], "arp up 1/32"), &[]);
        render.play();
        let mut sounding = Sounding::default();
        run(&mut render, STEP_BLOCKS + 3, false, &mut sounding);
        assert_eq!(sounding.0.len(), 1, "a note sounds");
        render.apply_command(command);
        run(&mut render, 4, false, &mut sounding);
        assert!(sounding.0.is_empty(), "{command:?} let it go");
    }
}

/// A random arp plays the same notes on every bounce.
#[test]
fn a_random_arp_plays_the_same_on_every_bounce() {
    let project = arped(&[(0, 16)], "arp random 1/16 2");
    let (first, _) = heard_over(&project, 16 * STEP_BLOCKS, false);
    let (second, _) = heard_over(&project, 16 * STEP_BLOCKS, false);
    assert_eq!(first, second);
    let on = pitches(&first, true);
    assert!(on.iter().collect::<HashSet<_>>().len() > 2, "it moves: {on:?}");
}

/// Wired, `advance` clocks the arp in place of its own clock, a block late.
#[test]
fn an_advance_wire_clocks_the_arp_in_place_of_its_own_clock() {
    let project = chords_on(&[(0, 16)], |project, chord| {
        let arp = add(project, arp_of("arp up 1/16"));
        let held = add(project, ModulatorParams::NoteGate);
        let clock = add(project, ModulatorParams::Chord(ModChordParams::default()));
        // Keys' own notes, as a gate, are the clock: one advance a note.
        wire(project, (chord, 0), (arp, 0));
        wire(project, (NOTES_IN, 0), (clock, 0));
        wire(project, (clock, 0), (held, 0));
        wire(project, (held, 0), (arp, 1));
        (arp, 0)
    });
    let (heard, _) = heard_over(&project, 16 * STEP_BLOCKS, false);
    // One Keys note, one advance: the first note only, a block late.
    let on: Vec<(usize, u8)> = heard.iter().filter(|event| event.1).map(|event| (event.0, event.2)).collect();
    assert_eq!(on, [(BLOCK, 60)]);
}

#[test]
fn the_arp_allocates_nothing() {
    let project = arped(&[(0, 4), (8, 4)], "arp random 1/32 3");
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    render.play();
    render.process_once_block(BLOCK);
    let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    for _ in 0..16 * STEP_BLOCKS {
        render.process_once_block(BLOCK);
    }
    render.apply_command(EngineCommand::Panic);
    render.process_once_block(BLOCK);
    let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    assert_eq!(after, before, "the callback allocated or freed with an arp in the patch");
}
