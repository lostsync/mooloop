//! The status bar's held notice and its audio readout (MOO-132).
//!
//! A failure used to be written into `status-message`, one muted line that
//! the next ordinary event -- a device selected, a note copied -- overwrote.
//! These pin the three things that replaced it: a warning or an error is
//! *held* until it is read, it is *drawn* in its own colour, and a dropout
//! raises a count on screen rather than a line in a log.
//!
//! The colour tests read the rendered pixels rather than a property, because
//! the defect was about what the user sees: a property that said "error"
//! while the text drew muted would pass a property check.

use mooloop_engine::load::{HotSpot, LoadSnapshot, RealtimeStatus, Site};
use mooloop_ui::status_bar::{hot_spot_text, notify, show_audio_load, withdraw, Severity};
use mooloop_ui::{MainWindow, NoticeLevel, Theme};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{Color, ComponentHandle, LogicalPosition, LogicalSize};

mod common;

const WIDTH: f32 = 960.0;
const HEIGHT: f32 = 760.0;
/// Where the pixel scans start: inside the status bar, which runs from about
/// y 726 to 750 (it is not flush with the window's bottom edge), and below
/// everything drawn above it.
const BAR_TOP: usize = 736;
/// How far the status bar stops short of the window's right edge, and the
/// height its clickable row is centred on. Both measured by clicking a
/// software render: the bar answers from y 728 to 750.
const BAR_RIGHT_INSET: f32 = 10.0;
const BAR_MIDDLE: f32 = 740.0;
/// The notice's dot: the segment starts at x 8, the dot 5px into it.
const NOTICE_DOT: (f32, f32) = (17.0, 748.0);

const ERROR: &str = "Sample not loaded: kick.wav: unsupported or malformed audio file";

fn harness() -> MainWindow {
    common::install_testing_backend();
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(WIDTH, HEIGHT));
    ui
}

fn click_at(ui: &MainWindow, p: (f32, f32)) {
    let position = LogicalPosition::new(p.0, p.1);
    let window = ui.window();
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

/// How many pixels in a colour count as "drawn in it". Absent is exactly 0,
/// so this only has to clear a stray antialiased pixel. The figure itself
/// depends on the font rasteriser. Counting exact matches, CI's macOS runner
/// drew the error in 34 pixels, which failed the floor of 40 this used to be.
const DRAWN: usize = 12;

/// The status bar's last row: it ends at y 750, and below it is the window.
const BAR_BOTTOM: usize = 750;

/// Pixels in the status bar between `x0` and `x1` drawn in `colour`: the
/// colour itself, or the colour laid over the bar at half coverage or more.
///
/// Antialiased text is mostly blends, and how many pixels of a glyph a
/// rasteriser covers fully is its own business. CI's macOS runner drew the
/// xrun count's digits without one pixel within 12 levels of exact amber.
/// A blend is recognised by lying on the line from the bar's background to
/// `colour`, so muted text, which lies on a different line, does not count,
/// and neither does another theme colour.
fn pixels_near(ui: &MainWindow, colour: Color, x0: usize, x1: usize) -> usize {
    let snapshot = ui.window().take_snapshot().unwrap();
    let width = snapshot.width() as usize;
    let bytes = snapshot.as_bytes();
    let pixel = |x: usize, y: usize| {
        let at = (y * width + x) * 4;
        [bytes[at], bytes[at + 1], bytes[at + 2]].map(f32::from)
    };
    let rows = BAR_TOP..BAR_BOTTOM.min(snapshot.height() as usize);

    // The bar's background is its commonest colour.
    let mut counts = std::collections::HashMap::<[u8; 3], usize>::new();
    for y in rows.clone() {
        for x in 0..width {
            *counts.entry(pixel(x, y).map(|c| c as u8)).or_default() += 1;
        }
    }
    let background = counts.into_iter().max_by_key(|&(_, n)| n).unwrap().0.map(f32::from);

    let target = [colour.red(), colour.green(), colour.blue()].map(f32::from);
    let delta: [f32; 3] = std::array::from_fn(|c| target[c] - background[c]);
    let key = (0..3).max_by(|&a, &b| delta[a].abs().total_cmp(&delta[b].abs())).unwrap();
    assert!(delta[key].abs() >= 32.0, "{colour:?} is too close to the bar to be told apart");

    rows.flat_map(|y| (x0..x1.min(width)).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let p = pixel(x, y);
            let cover = (p[key] - background[key]) / delta[key];
            (0.5..=1.1).contains(&cover)
                && (0..3).all(|c| (p[c] - (background[c] + cover * delta[c])).abs() <= 12.0)
        })
        .count()
}

#[test]
fn an_info_message_does_not_replace_a_held_error() {
    let ui = harness();
    notify(&ui, Severity::Error, ERROR);
    // The exact event the issue names as the one that erased it.
    notify(&ui, Severity::Info, "Device selected");

    assert_eq!(ui.get_status_notice(), ERROR);
    assert_eq!(ui.get_status_notice_level(), NoticeLevel::Error);
    assert_eq!(
        ui.get_status_message(),
        "Device selected",
        "an info message still lands in its own line"
    );
}

#[test]
fn a_warning_waits_behind_an_error_and_a_newer_error_replaces_it() {
    let ui = harness();
    notify(&ui, Severity::Error, ERROR);
    notify(&ui, Severity::Warning, "Busy — could not start the preview");
    assert_eq!(ui.get_status_notice(), ERROR, "the unread error must not be displaced");

    notify(&ui, Severity::Error, "The take could not be written: disk full");
    assert_eq!(ui.get_status_notice(), "The take could not be written: disk full");

    // And a warning does replace a warning, or a condition that has moved
    // on would keep reporting its first state.
    let ui = harness();
    notify(&ui, Severity::Warning, "first");
    notify(&ui, Severity::Warning, "second");
    assert_eq!(ui.get_status_notice(), "second");
    assert_eq!(ui.get_status_notice_level(), NoticeLevel::Warning);
}

#[test]
fn withdrawing_a_notice_takes_down_only_the_one_it_names() {
    let ui = harness();
    notify(&ui, Severity::Warning, "Audio stopped");
    notify(&ui, Severity::Error, ERROR);
    withdraw(&ui, "Audio stopped");
    assert_eq!(ui.get_status_notice(), ERROR, "a newer notice is not the caller's to clear");

    withdraw(&ui, ERROR);
    assert_eq!(ui.get_status_notice(), "");
}

#[test]
fn an_error_draws_in_the_destructive_colour_until_it_is_clicked_away() {
    let ui = harness();
    let theme = ui.global::<Theme>();
    let (destructive, warning) = (theme.get_destructive(), theme.get_warning());
    let left_half = WIDTH as usize / 2;

    assert_eq!(pixels_near(&ui, destructive, 0, left_half), 0, "nothing held yet");

    notify(&ui, Severity::Error, ERROR);
    let red = pixels_near(&ui, destructive, 0, left_half);
    assert!(red >= DRAWN, "an error should draw its dot and text in red, found {red} pixels");
    assert_eq!(pixels_near(&ui, warning, 0, left_half), 0);

    // An info message beside it does not take the colour away.
    notify(&ui, Severity::Info, "Device selected");
    assert!(pixels_near(&ui, destructive, 0, left_half) >= DRAWN);

    click_at(&ui, NOTICE_DOT);
    assert_eq!(ui.get_status_notice(), "", "clicking the notice dismisses it");
    assert_eq!(pixels_near(&ui, destructive, 0, left_half), 0);

    notify(&ui, Severity::Warning, "Busy — could not start the preview");
    assert!(pixels_near(&ui, warning, 0, left_half) >= DRAWN, "a warning draws in amber");
    assert_eq!(pixels_near(&ui, destructive, 0, left_half), 0);
}

fn window_with(blocks: u32, over_budget: u32, realtime: RealtimeStatus) -> LoadSnapshot {
    LoadSnapshot {
        blocks,
        mean_load: 0.23,
        peak_load: 0.41,
        over_budget,
        late_wakeups: 0,
        peak_period: 1.0,
        realtime,
        faults: 0,
        hot_spot: None,
        hot_spots: 0,
    }
}

#[test]
fn an_xrun_raises_a_visible_count_that_a_click_resets() {
    let ui = harness();
    let warning = ui.global::<Theme>().get_warning();
    let right_half = WIDTH as usize / 2;
    assert_eq!(ui.get_audio_load(), -1.0, "no window read yet");

    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0, "");
    assert_eq!(ui.get_audio_dropouts(), 0);
    assert!((ui.get_audio_load() - 0.23).abs() < 1e-6);
    assert_eq!(pixels_near(&ui, warning, right_half, WIDTH as usize), 0);

    // One driver xrun and the block that caused it: one dropout, not two.
    show_audio_load(&ui, &window_with(750, 1, RealtimeStatus::Realtime), 1, "");
    assert_eq!(ui.get_audio_dropouts(), 1);
    // And the next window adds to it rather than replacing it.
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 2, "");
    assert_eq!(ui.get_audio_dropouts(), 3);
    assert!(
        pixels_near(&ui, warning, right_half, WIDTH as usize) > 0,
        "the count is drawn in amber"
    );

    // The readout ends 12px before the first of the four 26px layout chips,
    // and its right edge is the DSP figure. A fifth chip moves it, and then
    // this click misses and the assertion below fails -- loudly, not
    // silently, which is why the count is written here rather than derived
    // from a list the markup keeps private. The bar stops `BAR_RIGHT_INSET`
    // short of the window's edge; a click placed from the window's edge
    // instead landed in the 12px gap and missed (measured on a software
    // render, 2026-09-23: the readout answers from x 728 to 832 at this
    // width, the first chip from 846).
    let chips_left = WIDTH - BAR_RIGHT_INSET - 4.0 * 26.0;
    click_at(&ui, (chips_left - 16.0, BAR_MIDDLE));
    assert_eq!(ui.get_audio_dropouts(), 0, "clicking the readout resets the count");
    // And a later window counts from there.
    show_audio_load(&ui, &window_with(750, 1, RealtimeStatus::Realtime), 0, "");
    assert_eq!(ui.get_audio_dropouts(), 1);
}

#[test]
fn a_time_shared_callback_raises_a_badge_and_a_stopped_one_clears_the_load() {
    let ui = harness();
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::TimeShared), 0, "");
    assert!(ui.get_audio_time_shared());
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0, "");
    assert!(!ui.get_audio_time_shared());

    // A window with no blocks is an engine that is not running, and every
    // other field of it is meaningless; the last live number must not stay
    // up saying the audio is fine.
    show_audio_load(&ui, &window_with(0, 0, RealtimeStatus::Realtime), 0, "");
    assert_eq!(ui.get_audio_load(), -1.0);
}

/// **A slow callback says where it was and what took it** (MOO-236): the
/// text names the bar and each site's share of the budget, is drawn in
/// amber beside the readout, holds through a quiet window, and a click on
/// the readout clears it with the dropouts.
#[test]
fn a_slow_callback_names_its_bar_and_channels_until_the_readout_is_clicked() {
    // 2.67 ms, a 128-frame block at 48 kHz; bar 3, beat 2 at 96 PPQ.
    let spot = HotSpot {
        tick: 2 * 384 + 96,
        frames: 128,
        work_nanos: 2_400_000,
        budget_nanos: 2_666_666,
        sites: [
            Some((Site::Channel(1), 1_093_333)),
            Some((Site::Bus(0), 586_666)),
            None,
        ],
    };
    let text = hot_spot_text(&spot, |site| match site {
        Site::Channel(index) => format!("Ch{index}"),
        Site::Bus(_) => "Master".to_owned(),
    });
    assert_eq!(text, "bar 3.2 · Ch1 41%, Master 22%");
    let unnamed = HotSpot { sites: [None; 3], ..spot };
    assert_eq!(hot_spot_text(&unnamed, |_| String::new()), "bar 3.2 · 90%");

    let ui = harness();
    let warning = ui.global::<Theme>().get_warning();
    let right_half = WIDTH as usize / 2;
    let chips_left = WIDTH - BAR_RIGHT_INSET - 4.0 * 26.0;
    // The right end of the readout, "DSP n%", and everything right of it.
    let readout_end = (chips_left - 40.0) as usize;
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0, "");
    assert_eq!(ui.get_audio_hot_spot(), "");
    assert_eq!(pixels_near(&ui, warning, right_half, WIDTH as usize), 0, "nothing amber yet");
    let readout_before = bar_pixels(&ui, readout_end, WIDTH as usize);

    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0, &text);
    assert_eq!(ui.get_audio_hot_spot(), text.as_str());
    assert!(pixels_near(&ui, warning, right_half, WIDTH as usize) > 0, "the hot spot is drawn in amber");
    // It sits beside the readout in a box of its own width: the readout does
    // not move when it appears, nor when a longer one replaces it.
    assert!(
        bar_pixels(&ui, readout_end, WIDTH as usize) == readout_before,
        "the readout moved when the hot spot appeared"
    );
    let long = "bar 118.4.3 · A channel with a very long name 61%, Another long one 30%, Master 4%";
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0, long);
    assert!(
        bar_pixels(&ui, readout_end, WIDTH as usize) == readout_before,
        "the readout moved for a longer hot spot"
    );
    // Elided into its box: nothing amber left of it.
    assert_eq!(
        pixels_near(&ui, warning, 0, (chips_left - 360.0) as usize),
        0,
        "a long hot spot spilled out of its box"
    );
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0, &text);
    // A quiet window after it leaves it up.
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0, "");
    assert_eq!(ui.get_audio_hot_spot(), text.as_str());

    click_at(&ui, (chips_left - 16.0, BAR_MIDDLE));
    assert_eq!(ui.get_audio_hot_spot(), "", "clicking the readout clears it");
    assert_eq!(pixels_near(&ui, warning, right_half, WIDTH as usize), 0);

    // And a click on the hot spot itself clears it too: it is part of the
    // readout.
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0, &text);
    click_at(&ui, (chips_left - 150.0, BAR_MIDDLE));
    assert_eq!(ui.get_audio_hot_spot(), "", "clicking the hot spot clears it");
}

/// The status bar's pixels between `x0` and `x1`, row by row.
fn bar_pixels(ui: &MainWindow, x0: usize, x1: usize) -> Vec<u8> {
    let snapshot = ui.window().take_snapshot().unwrap();
    let width = snapshot.width() as usize;
    let bytes = snapshot.as_bytes();
    (BAR_TOP..BAR_BOTTOM.min(snapshot.height() as usize))
        .flat_map(|y| bytes[(y * width + x0) * 4..(y * width + x1.min(width)) * 4].to_vec())
        .collect()
}
