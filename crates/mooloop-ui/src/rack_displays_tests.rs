//! **A pump tick in which nothing moved writes nothing, and a tick in which
//! only a meter moved writes no row** (MOO-261, MOO-258).
//!
//! The pump used to rebuild every EQ analyzer's, Preamp display's and
//! Buffer's `EffectSlotRow` whole on every tick, and each mixer strip's whole
//! `MixerStripRow` whenever its meter moved. A whole-row write dirties every
//! binding on the face, its texts' layouts included, which then re-measure
//! their strings: `device_rows` was 257-313 µs a tick playing
//! `housey-dropout-factory`, and text a large share of every frame.
//!
//! The per-tick values have their own models now, and `EffectSlotRow` and
//! `MixerStripRow` no longer have fields for them, so a meter cannot reach a
//! row at all. These tests hold what is left: unchanged readings write no
//! entry, moved ones write only what moved and in place, and the slot models
//! stay aligned with the rack's rows when the rack changes under them.

use super::*;
use crate::rack_displays::{resting_slot_meters, resting_strip_level, DisplayWrites};
use crate::window_probe::install_backend;

/// A channel with an EQ (analyzer on) and a compressor in its rack, the rows
/// published the way a selection publishes them.
fn rack() -> (MainWindow, UiState) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    let mut st = UiState::new(None, 48_000, &window);
    st.session
        .insert_effect_at(EffectKind::Eq, 0)
        .expect("an EQ inserts into an empty rack");
    st.session
        .insert_effect_at(EffectKind::Compressor, 1)
        .expect("a compressor inserts after it");
    st.sync_effects();
    (window, st)
}

fn compressing(detector_db: f32, gain_reduction_db: f32) -> EffectSlotMeters {
    EffectSlotMeters {
        input_left_db: -9.0,
        input_right_db: -10.0,
        output_left_db: -12.0,
        output_right_db: -13.0,
        detector_db,
        gain_reduction_db,
        ..resting_slot_meters()
    }
}

fn spectrum(level: f32) -> Vec<f32> {
    (0..32).map(|bin| level - bin as f32).collect()
}

fn trace(st: &UiState, slot: usize) -> ModelRc<f32> {
    st.effect_slot_traces
        .row_data(slot)
        .expect("every rack row has a trace model")
}

fn trace_values(st: &UiState, slot: usize) -> Vec<f32> {
    trace(st, slot).iter().collect()
}

#[test]
fn the_slot_models_are_built_with_the_rows_and_start_at_rest() {
    let (window, st) = rack();
    assert_eq!(st.effect_slot_meters.row_count(), 2);
    assert_eq!(st.effect_slot_traces.row_count(), 2);
    for slot in 0..2 {
        assert_eq!(st.slot_meters(slot), resting_slot_meters());
        assert!(trace_values(&st, slot).is_empty());
    }
    // The window reads the same models the pump writes.
    assert_eq!(window.get_effect_slot_meters().row_count(), 2);
    assert_eq!(window.get_effect_slot_traces().row_count(), 2);
}

/// The resting values are stated twice, in `resting_slot_meters` and in the
/// markup's `slot-meters()` / `StripMeters.level()` fallbacks, which is what
/// a face reads before the models reach it. A face drawn from the fallback
/// must look the same as one drawn from a freshly realigned model.
#[test]
fn the_markup_rests_where_the_models_do() {
    let (window, _st) = rack();
    assert_eq!(window.invoke_slot_meters(99), resting_slot_meters());
    assert_eq!(window.invoke_slot_meters(-1), resting_slot_meters());
    assert_eq!(
        window.global::<StripMeters>().invoke_level(-1),
        resting_strip_level()
    );
}

#[test]
fn a_tick_whose_readings_did_not_move_writes_nothing() {
    let (_window, st) = rack();
    st.publish_slot_display(0, &resting_slot_meters(), Some(&spectrum(-20.0)));
    st.publish_slot_display(1, &compressing(-12.0, -6.0), None);

    let mut again = DisplayWrites::default();
    again.add(st.publish_slot_display(0, &resting_slot_meters(), Some(&spectrum(-20.0))));
    again.add(st.publish_slot_display(1, &compressing(-12.0, -6.0), None));

    assert_eq!(again, DisplayWrites::default(), "a still tick wrote a display");
}

#[test]
fn a_reading_too_small_to_draw_writes_nothing() {
    let (_window, st) = rack();
    st.publish_slot_display(1, &compressing(-12.0, -6.0), None);

    // A tenth of a decibel: under a meter's quarter-decibel step and a
    // dynamics readout's half-decibel one.
    let mut nudged = compressing(-12.1, -6.1);
    nudged.input_left_db -= 0.1;
    let writes = st.publish_slot_display(1, &nudged, None);

    assert_eq!(writes, DisplayWrites::default());
}

#[test]
fn a_moving_meter_writes_its_own_entry_and_nothing_else() {
    let (_window, st) = rack();
    st.publish_slot_display(0, &resting_slot_meters(), Some(&spectrum(-20.0)));
    st.publish_slot_display(1, &compressing(-12.0, -6.0), None);
    let eq_before = st.slot_meters(0);

    let writes = st.publish_slot_display(1, &compressing(-20.0, -2.0), None);

    assert_eq!(writes, DisplayWrites { meters: 1, values: 0 });
    assert_eq!(st.slot_meters(1).detector_db, -20.0);
    assert_eq!(st.slot_meters(1).gain_reduction_db, -2.0);
    assert_eq!(st.slot_meters(0), eq_before, "the EQ's meters moved too");
}

#[test]
fn a_moving_spectrum_is_rewritten_in_place() {
    let (_window, st) = rack();
    st.publish_slot_display(0, &resting_slot_meters(), Some(&spectrum(-20.0)));
    let model = trace(&st, 0);

    let mut moved = spectrum(-20.0);
    moved[3] = -1.0;
    moved[7] = -2.0;
    let writes = st.publish_slot_display(0, &resting_slot_meters(), Some(&moved));

    assert_eq!(writes, DisplayWrites { meters: 0, values: 2 });
    assert_eq!(trace(&st, 0), model, "the trace's model was replaced, not written");
    assert_eq!(trace_values(&st, 0), moved);
}

#[test]
fn a_slot_that_stops_drawing_its_trace_is_emptied() {
    let (_window, st) = rack();
    st.publish_slot_display(0, &resting_slot_meters(), Some(&spectrum(-20.0)));

    st.publish_slot_display(0, &resting_slot_meters(), None);

    assert!(trace_values(&st, 0).is_empty(), "an analyzer turned off kept its last bars");
}

/// Owed to main's ack of this change: the slot models are indexed like the
/// rack's rows, so they must move with them. A device inserted mid-rack
/// shifts every slot after it, and a face must never show the meters that
/// were published for whatever device sat at its index before.
#[test]
fn inserting_a_device_mid_rack_keeps_each_face_on_its_own_device() {
    let (_window, mut st) = rack();
    st.publish_slot_display(0, &resting_slot_meters(), Some(&spectrum(-20.0)));
    st.publish_slot_display(1, &compressing(-12.0, -6.0), None);
    let eq_trace = trace(&st, 0);

    st.session
        .insert_effect_at(EffectKind::Filter, 1)
        .expect("a filter inserts between them");
    st.sync_effects();

    assert_eq!(st.effect_slot_model.row_count(), 3);
    assert_eq!(st.effect_slot_meters.row_count(), 3);
    assert_eq!(st.effect_slot_traces.row_count(), 3);
    assert_eq!(
        st.slot_meters(1),
        resting_slot_meters(),
        "the new filter shows the compressor's meters"
    );
    assert_eq!(trace(&st, 0), eq_trace, "the EQ's trace model was replaced");

    // The engine renumbers with the chain, so the next tick publishes the
    // compressor at its new index, and the filter's stays at rest.
    st.publish_slot_display(2, &compressing(-12.0, -6.0), None);
    st.publish_slot_display(1, &resting_slot_meters(), None);
    assert_eq!(st.slot_meters(2).gain_reduction_db, -6.0);
    assert_eq!(st.slot_meters(1), resting_slot_meters());

    // And back out: removing it realigns again.
    st.session
        .remove_effect_at(1)
        .expect("the filter is there");
    st.sync_effects();
    assert_eq!(st.effect_slot_meters.row_count(), 2);
    assert_eq!(st.effect_slot_traces.row_count(), 2);
    assert_eq!(trace(&st, 0), eq_trace);
}

#[test]
fn a_still_strip_meter_writes_nothing_and_a_moving_one_writes_its_level() {
    let (window, st) = rack();
    let level = StripLevel {
        left_db: -6.0,
        right_db: -8.0,
        held_left_db: -3.0,
        held_right_db: -4.0,
        clipping: false,
    };
    assert_eq!(st.publish_strip_level(2, &level).meters, 1);

    assert_eq!(st.publish_strip_level(2, &level), DisplayWrites::default());

    let louder = StripLevel {
        left_db: -1.0,
        ..level.clone()
    };
    assert_eq!(st.publish_strip_level(2, &louder).meters, 1);
    let published = window
        .global::<StripMeters>()
        .get_levels()
        .row_data(2)
        .expect("a level per track");
    assert_eq!(published, louder);
    assert_eq!(
        window.global::<StripMeters>().get_levels().row_data(1),
        Some(resting_strip_level()),
        "another track's meter moved"
    );
}

#[test]
fn still_lamps_and_playheads_write_nothing() {
    let (_window, st) = rack();
    let mut reduction = [0.0_f32; MAX_BUSES];
    reduction[3] = 4.5;
    assert_eq!(st.publish_strip_reductions(&reduction).values, 1);
    assert_eq!(st.publish_strip_reductions(&reduction), DisplayWrites::default());

    assert_eq!(st.publish_playheads(&[0.25, 0.5]).values, 2);
    assert_eq!(st.publish_playheads(&[0.25, 0.5]), DisplayWrites::default());
    // Two voices moving: in place, two entries.
    assert_eq!(st.publish_playheads(&[0.26, 0.51]).values, 2);
    assert_eq!(st.playhead_model.row_count(), 2);
    assert_eq!(st.publish_playheads(&[]).values, 1);
    assert_eq!(st.publish_playheads(&[]), DisplayWrites::default());
}

// ---- `playlist-live-from`: the clips' borders (MOO-261) ----------------

/// A song's clips: overlapping, abutting, one at zero, one after a gap, on
/// two patterns.
fn song_clips() -> Vec<PlaylistClip> {
    let clip = |pattern, start_tick, length_steps| PlaylistClip {
        pattern,
        start_tick,
        length_steps,
    };
    let step = TICKS_PER_STEP as i32;
    vec![
        clip(0, 0, 16),
        clip(1, 8 * step, 16),
        clip(0, 16 * step, 16),
        clip(1, 24 * step, 4),
        clip(0, 64 * step, 32),
    ]
}

/// What a clip's border said before: lit while the playhead is inside it.
fn lit_by_position(clip: &PlaylistClip, position: i32) -> bool {
    let (start, end) = rack_displays::clip_span(clip);
    position >= start && position < end
}

/// What it says now, reading `playlist-live-from` exactly as the markup does.
fn lit_by_live_from(window: &MainWindow, clip: &PlaylistClip) -> bool {
    let (start, end) = rack_displays::clip_span(clip);
    let from = window.get_playlist_live_from();
    from >= start && from < end
}

/// Every boundary, one tick either side of it, and the song's end.
fn positions_around(clips: &[PlaylistClip]) -> Vec<i32> {
    let mut positions = vec![0, 1];
    for clip in clips {
        let (start, end) = rack_displays::clip_span(clip);
        for edge in [start, end] {
            positions.extend([edge - 1, edge, edge + 1]);
        }
    }
    positions.retain(|position| *position >= 0);
    positions
}

fn assert_borders_agree(window: &MainWindow, clips: &[PlaylistClip], sweep: &[i32]) {
    for &position in sweep {
        rack_displays::set_playlist_position(window, position);
        assert_eq!(window.get_playlist_position_ticks(), position);
        for (index, clip) in clips.iter().enumerate() {
            assert_eq!(
                lit_by_live_from(window, clip),
                lit_by_position(clip, position),
                "clip {index} {clip:?} at tick {position}: live-from {}",
                window.get_playlist_live_from()
            );
        }
    }
}

#[test]
fn every_clip_lights_where_the_playhead_is_across_every_edge() {
    let (window, _st) = rack();
    let clips = song_clips();
    window.set_playlist_clips(ModelRc::from(Rc::new(VecModel::from(clips.clone()))));
    let mut forward = positions_around(&clips);
    forward.sort_unstable();
    forward.dedup();

    // Forward, as a song plays.
    assert_borders_agree(&window, &clips, &forward);
    // A loop wrap: from past the last clip back to the start, and on.
    assert_borders_agree(&window, &clips, &[forward[forward.len() - 1], 0, 5, 8 * 24]);
    // Backwards seeks, every edge in reverse.
    let backward: Vec<i32> = forward.iter().rev().copied().collect();
    assert_borders_agree(&window, &clips, &backward);
}

#[test]
fn a_moved_clip_is_lit_by_its_new_edges() {
    let (window, _st) = rack();
    let mut clips = song_clips();
    let model = Rc::new(VecModel::from(clips.clone()));
    window.set_playlist_clips(ModelRc::from(model.clone()));
    rack_displays::set_playlist_position(&window, 20 * TICKS_PER_STEP as i32);

    // Move the second clip later and shorten the last, as an edit does,
    // without the playhead moving first.
    clips[1].start_tick = 40 * TICKS_PER_STEP as i32;
    clips[4].length_steps = 3;
    model.set_vec(clips.clone());

    let mut sweep = positions_around(&clips);
    sweep.sort_unstable();
    sweep.dedup();
    assert_borders_agree(&window, &clips, &sweep);
}

#[test]
fn live_from_moves_only_when_the_playhead_crosses_an_edge() {
    let (window, _st) = rack();
    let clips = song_clips();
    window.set_playlist_clips(ModelRc::from(Rc::new(VecModel::from(clips))));
    let step = TICKS_PER_STEP as i32;

    rack_displays::set_playlist_position(&window, 2 * step);
    let inside = window.get_playlist_live_from();
    for position in (2 * step..8 * step).step_by(7) {
        rack_displays::set_playlist_position(&window, position);
        assert_eq!(window.get_playlist_live_from(), inside);
    }
    rack_displays::set_playlist_position(&window, 8 * step);
    assert_eq!(window.get_playlist_live_from(), 8 * step);
}
