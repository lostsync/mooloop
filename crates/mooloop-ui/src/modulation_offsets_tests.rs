//! **A pump tick whose modulation did not move writes nothing** (MOO-257).
//!
//! `refresh_modulation_offsets` runs on every tick in which a channel's
//! modulators moved, and a free-running LFO moves on every tick, stopped
//! included. It used to replace the source's offsets model and write every
//! insert's whole rack row each time, modulated or not. Every binding on every
//! face was re-evaluated, and the window repainted on every tick with nothing
//! else changing: 0.53 ms a tick on `housey-dropout-factory`, plus a full
//! frame.
//!
//! These tests hold the rule, not the mechanism: unchanged offsets write no
//! row and no model, and moved offsets reach the knobs through the models
//! they already read, without a row write.

use super::*;
use crate::window_probe::install_backend;
use mooloop_core::ModRoute;

/// A channel with an LFO in its rack, routed to the first insert's first
/// parameter, and a second insert nothing reaches. The rows are published the
/// way a selection publishes them.
fn routed_rack() -> (MainWindow, UiState) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    let mut st = UiState::new(None, 48_000, &window);
    st.session
        .add_modulation_source(ModulatorKind::Lfo)
        .expect("an empty rack has a free slot");
    st.session
        .insert_effect_at(EffectKind::Filter, 0)
        .expect("a filter inserts into an empty rack");
    st.session
        .insert_effect_at(EffectKind::Drive, 1)
        .expect("a drive inserts after it");
    let scope = EffectTarget::Channel(0);
    let routed = st.session.channels[0].effects[0].id;
    st.session.channels[0]
        .modulation
        .add_route(ModRoute::to_slot(
            0,
            ParamAddr::effect(scope, routed, 0),
            0.5,
            ModPolarity::Bipolar,
        ))
        .expect("the matrix is empty");
    st.sync_effects();
    st.refresh_modulation(&window);
    (window, st)
}

fn set_lfo(st: &UiState, value: f32) {
    let mut outputs = st.session.modulation_outputs.get();
    outputs[0] = value;
    st.session.modulation_outputs.set(outputs);
}

fn first_offset(st: &UiState, slot: usize) -> f32 {
    st.effect_slot_model
        .row_data(slot)
        .and_then(|row| row.modulation_offsets.row_data(0))
        .expect("the row carries its offsets")
}

#[test]
fn a_tick_whose_offsets_did_not_move_writes_nothing() {
    let (window, st) = routed_rack();
    set_lfo(&st, 0.4);
    st.refresh_modulation_offsets(&window);

    // The same outputs again: the LFO is where it was.
    let again = st.refresh_modulation_offsets(&window);

    assert_eq!(
        again,
        ModulationOffsetsRefresh::default(),
        "a tick with nothing moved wrote offsets or rows"
    );
}

#[test]
fn a_moving_lfo_reaches_its_knob_without_writing_a_row() {
    let (window, st) = routed_rack();
    set_lfo(&st, 0.4);
    st.refresh_modulation_offsets(&window);
    let before = first_offset(&st, 0);

    set_lfo(&st, -0.6);
    let moved = st.refresh_modulation_offsets(&window);

    assert_eq!(
        moved.rows_written, 0,
        "the offsets went in by replacing whole rack rows"
    );
    assert_eq!(
        moved.values_moved, 1,
        "only the routed insert's offsets should have moved: {moved:?}"
    );
    let after = first_offset(&st, 0);
    assert_ne!(before, after, "the routed knob's offset did not follow the LFO");
    assert_eq!(
        first_offset(&st, 1),
        0.0,
        "an insert nothing reaches was given an offset"
    );
}
