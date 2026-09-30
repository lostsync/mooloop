//! The song's length is written twice: `Sequencer::song_length_ticks` in the
//! engine, which the sequencer schedules Song mode on and wraps a recorded
//! note by, and `Session::song_length_ticks`, which the session wraps a
//! Replace take's crossing by, resolves the export range against and folds the
//! playhead readout with (MOO-393).
//!
//! Each is tested against itself only, so this holds them to each other over
//! the same songs. If it fails, one copy has learned something the other has
//! not, and Replace will remove notes at positions the playhead did not cross.

use mooloop_core::TICKS_PER_STEP;
use mooloop_engine::record_check::song_length_ticks;
use mooloop_session::session::Session;

/// Both lengths, for the song `session` holds.
fn both(session: &Session) -> (u32, u32) {
    let project = session.project_snapshot(120, 50);
    (song_length_ticks(&project), session.song_length_ticks())
}

#[test]
fn the_engine_and_the_session_agree_how_long_the_song_is() {
    // No placement at all: one bar.
    let mut session = Session::default();
    let (engine, document) = both(&session);
    assert_eq!(engine, document, "an empty playlist");

    // A clip that ends mid-bar rounds up to the bar.
    session.set_pattern_length(20);
    session.add_playlist_placement(0, 0).expect("room");
    let (engine, document) = both(&session);
    assert_eq!(engine, document, "a clip ending mid-bar");
    assert_eq!(engine, 2 * 16 * TICKS_PER_STEP, "the premise: two bars");

    // Two patterns, the later one placed further out.
    session.add_pattern().expect("room");
    session.set_pattern_length(48);
    session
        .add_playlist_placement(1, 5 * 16 * TICKS_PER_STEP as i32 + 24)
        .expect("room");
    let (engine, document) = both(&session);
    assert_eq!(engine, document, "two patterns");

    // A shorter clip does not shorten the song.
    session.add_pattern().expect("room");
    session.set_pattern_length(4);
    session.add_playlist_placement(2, 0).expect("room");
    let (engine, document) = both(&session);
    assert_eq!(engine, document, "a short clip under a long one");
}
