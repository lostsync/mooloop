//! MIDI recorded over a looping pattern, through the real capture path
//! (MOO-234): the renderer stamps a note when its key comes up, the pump
//! hands it and the block's position to the session, and the session writes
//! it -- and, in Replace, removes what the playhead crossed.
//!
//! Adam's note was that after one pass "notes just stack at the last tick".
//! `JOURNAL.md` 2026-09-17 records the fix (`Sequencer::recording_tick`);
//! these hold it over three passes in both playback modes, which nothing
//! did before.

use mooloop_core::{EngineCommand, EngineEvent, NoteEvent, DEFAULT_SWING_PERCENT};
use mooloop_engine::record_check::RecordRig;
use mooloop_session::session::Session;
use mooloop_session::transport::RecordMode;

const SAMPLE_RATE: u32 = 48_000;
const BLOCK: usize = 256;
/// Four steps: 96 ticks, a quarter note, 24 000 frames at 120 bpm.
const PATTERN_STEPS: i32 = 4;
const PATTERN_TICKS: u64 = 96;
/// Where each pass is played, and a little past it so rounding at the block
/// edge cannot put it on the tick before.
const PLAYED_AT: u64 = 48;

/// What the pump does with one block's events: positions first, then the
/// notes whose keys came up, as `drain_control_surface` does.
fn pump(session: &mut Session, events: &[EngineEvent]) {
    for event in events {
        if let EngineEvent::Position { tick, playing, .. } = *event {
            if let Some(plan) = session.record_position(tick, playing) {
                let _ = session.apply_replace(plan);
            }
        }
    }
    for event in events {
        if let EngineEvent::RecordedNote {
            channel,
            pattern,
            note,
            velocity,
            start_tick,
            length_ticks,
        } = *event
        {
            let _ = session.record_note(
                usize::from(channel),
                usize::from(pattern),
                note,
                velocity,
                start_tick,
                length_ticks,
            );
        }
    }
}

/// Play `passes` passes of pattern 0, pressing a different key at
/// [`PLAYED_AT`] of each, where pass `n` starts at song tick
/// `first + n * period`. Returns the keys in the order played.
fn record_passes(
    session: &mut Session,
    rig: &mut RecordRig,
    first: u64,
    period: u64,
    passes: u64,
) -> Vec<u8> {
    let frames_per_tick = 1.0 / rig.ticks_per_sample();
    let keys: Vec<u8> = (0..passes).map(|pass| 60 + 2 * pass as u8).collect();
    let presses: Vec<u64> = (0..passes)
        .map(|pass| {
            let tick = first + pass * period + PLAYED_AT;
            (tick as f64 * frames_per_tick) as u64 + 50
        })
        .collect();
    // Past the last pass's note, and short of the next pass reaching it.
    let end = ((first + passes * period) as f64 * frames_per_tick) as u64;
    pump(session, &rig.block(BLOCK, &[]));
    rig.play();
    let mut frame = 0u64;
    let mut held: Option<u8> = None;
    while frame < end {
        let mut keys_here = Vec::new();
        if let Some(key) = held.take() {
            keys_here.push((0, key, false));
        }
        for (pass, &at) in presses.iter().enumerate() {
            if (frame..frame + BLOCK as u64).contains(&at) {
                keys_here.push(((at - frame) as u32, keys[pass], true));
                held = Some(keys[pass]);
            }
        }
        let events = rig.block(BLOCK, &keys_here);
        pump(session, &events);
        frame += BLOCK as u64;
    }
    keys
}

fn notes(session: &Session) -> Vec<NoteEvent> {
    session.channels[0].notes[0].clone()
}

fn start_of(notes: &[NoteEvent], key: u8) -> Option<u32> {
    notes.iter().find(|note| note.note == key).map(|note| note.start_tick)
}

/// A session on a four-step pattern, armed, and a renderer playing the same
/// song.
fn armed(song_mode: bool) -> (Session, RecordRig) {
    let mut session = Session::default();
    session.set_pattern_length(PATTERN_STEPS).expect("a new length");
    if song_mode {
        session.add_playlist_placement(0, 0).expect("a clip");
        let _ = session.set_playback_mode(true);
    }
    let _ = session.set_record_armed(true);
    let project = session.project_snapshot(120, i32::from(DEFAULT_SWING_PERCENT));
    let mut rig = RecordRig::new(&project, SAMPLE_RATE);
    if song_mode {
        rig.command(EngineCommand::SetPlaybackMode(mooloop_core::PlaybackMode::Song));
    }
    (session, rig)
}

/// Three passes of a looping pattern, a key at the same place in each: all
/// three land there, none at the last tick and none a pass late.
#[test]
fn three_passes_in_pattern_mode_land_where_they_were_played() {
    let (mut session, mut rig) = armed(false);
    let keys = record_passes(&mut session, &mut rig, 0, PATTERN_TICKS, 3);
    let notes = notes(&session);
    assert_eq!(notes.len(), 3, "overdub keeps every pass: {notes:?}");
    for key in keys {
        assert_eq!(start_of(&notes, key), Some(PLAYED_AT as u32), "{notes:?}");
    }
}

/// The same in song mode, where each pass is the song coming round to the
/// pattern's clip again.
#[test]
fn three_passes_in_song_mode_land_where_they_were_played() {
    let (mut session, mut rig) = armed(true);
    let song = u64::from(session.song_length_ticks());
    let keys = record_passes(&mut session, &mut rig, 0, song, 3);
    let notes = notes(&session);
    assert_eq!(notes.len(), 3, "{notes:?}");
    for key in keys {
        assert_eq!(start_of(&notes, key), Some(PLAYED_AT as u32), "{notes:?}");
    }
}

/// Replace, over the same three passes: what was there before is crossed
/// and gone, and each pass replaces the one before, so only the last is
/// left.
#[test]
fn three_passes_in_replace_leave_only_the_last() {
    let (mut session, mut rig) = armed(false);
    session.set_record_mode(RecordMode::Replace);
    let earlier = session.channels[0]
        .create_note(0, 24, 12, 50)
        .expect("room")
        .id;
    let keys = record_passes(&mut session, &mut rig, 0, PATTERN_TICKS, 3);
    let notes = notes(&session);
    assert!(notes.iter().all(|note| note.id != earlier), "{notes:?}");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(start_of(&notes, *keys.last().unwrap()), Some(PLAYED_AT as u32));
}
