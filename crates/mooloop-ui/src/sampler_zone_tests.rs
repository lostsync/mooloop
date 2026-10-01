//! **The SAMPLE page shows one key zone at a time** (MOO-463): picking a
//! zone puts its region on the page, only its voices draw a playhead, and
//! FOLLOW picks the zone a key has just started.

use super::*;
use crate::window_probe::install_backend;
use mooloop_core::{KeyRange, ProjectChannel, SampleReference, SampleZone};
use mooloop_dsp::sampler::encode_playhead;
use std::path::PathBuf;

fn tone(len: usize) -> Arc<SampleData> {
    Arc::new(SampleData {
        frames: (0..len).map(|n| [(n as f32 * 0.01).sin(); 2]).collect(),
        sample_rate: 48_000,
        root_note: 60,
    })
}

/// One sampler: its own sample below C4, and two zones above it, the first
/// cut to its second half.
fn zoned() -> (MainWindow, Rc<RefCell<UiState>>) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    let mut st = UiState::new(None, 48_000, &window);
    let path = |name: &str| PathBuf::from(format!("/nonexistent/moo-463/{name}.wav"));
    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().unwrap();
    state.keys = KeyRange::new(0, 59);
    state.zones = vec![
        SampleZone {
            keys: KeyRange::new(60, 71),
            root_note: 60,
            sample: SampleReference::File { path: path("b"), embedded: false },
            region: Some(ZoneRegion { start: 0.5, end: 0.9, tune_semitones: 3.0, ..ZoneRegion::default() }),
            ..SampleZone::default()
        },
        SampleZone {
            keys: KeyRange::new(72, 127),
            root_note: 72,
            sample: SampleReference::File { path: path("c"), embedded: false },
            ..SampleZone::default()
        },
    ];
    let project = Project {
        channels: vec![channel],
        pattern_lengths: vec![16],
        ..Project::default()
    };
    st.session.admit_zone_audio(vec![(path("b"), tone(2_000)), (path("c"), tone(3_000))], true);
    st.session.replace_project(&project, &[Some(tone(1_000))]);
    st.refresh_editor(&window);
    (window, Rc::new(RefCell::new(st)))
}

fn playheads(st: &Rc<RefCell<UiState>>) -> Vec<f32> {
    st.borrow().playhead_model.iter().collect()
}

#[test]
fn picking_a_zone_puts_its_region_on_the_page() {
    let (window, st) = zoned();
    assert_eq!(window.get_sampler_zone_selected(), 0);
    assert_eq!(window.get_sample_frames(), 1_000);
    st.borrow_mut().select_sampler_zone(&window, 1);
    assert_eq!(window.get_sampler_zone_selected(), 1);
    assert_eq!(window.get_start_pos(), 0.5);
    assert_eq!(window.get_end_pos(), 0.9);
    assert_eq!(window.get_tune_semitones(), 3.0);
    assert_eq!(window.get_root_note(), 60);
    assert_eq!(window.get_sample_frames(), 2_000);
    assert_eq!(window.get_sampler_zone_name(), "b.wav");
    // A zone that is not there falls back to zone 1.
    st.borrow_mut().select_sampler_zone(&window, 9);
    assert_eq!(window.get_sampler_zone_selected(), 0);
    assert_eq!(window.get_sample_frames(), 1_000);
}

/// Only the selected zone's voices draw, decoded back to positions in its
/// own sample, through the pump's own step.
#[test]
fn only_the_selected_zones_voices_draw_a_playhead() {
    let (window, st) = zoned();
    let raw = vec![0.25, encode_playhead(1, 0.5), encode_playhead(2, 0.75), encode_playhead(1, 1.0)];
    pump_sampler_playheads(&st, &window, true, || raw.clone());
    assert_eq!(playheads(&st), vec![0.25]);
    st.borrow_mut().select_sampler_zone(&window, 1);
    pump_sampler_playheads(&st, &window, true, || raw.clone());
    assert_eq!(playheads(&st), vec![0.5, 1.0]);
    pump_sampler_playheads(&st, &window, false, || raw.clone());
    assert!(playheads(&st).is_empty(), "a hidden rack draws no playhead");
}

/// FOLLOW picks the zone a key has just started, and leaves the pick alone
/// while that zone keeps sounding; off, it picks nothing.
#[test]
fn follow_picks_the_zone_a_key_just_started() {
    let (window, st) = zoned();
    pump_sampler_playheads(&st, &window, true, || vec![encode_playhead(2, 0.1)]);
    assert_eq!(window.get_sampler_zone_selected(), 0, "FOLLOW is off by default");

    st.borrow_mut().sampler_zone_follow = true;
    pump_sampler_playheads(&st, &window, true, Vec::new);
    // Positions a float holds exactly, so the decode round-trips them.
    pump_sampler_playheads(&st, &window, true, || vec![encode_playhead(2, 0.125)]);
    assert_eq!(window.get_sampler_zone_selected(), 2);
    assert_eq!(playheads(&st), vec![0.125]);
    pump_sampler_playheads(&st, &window, true, || vec![encode_playhead(2, 0.25), 0.375]);
    assert_eq!(window.get_sampler_zone_selected(), 0, "a key in zone 1 started");
    pump_sampler_playheads(&st, &window, true, || vec![encode_playhead(2, 0.5), 0.625]);
    assert_eq!(window.get_sampler_zone_selected(), 0, "nothing new started");
}
