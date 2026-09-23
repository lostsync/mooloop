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

use mooloop_engine::load::{LoadSnapshot, RealtimeStatus};
use mooloop_ui::status_bar::{notify, show_audio_load, withdraw, Severity};
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
/// depends on the font rasteriser: CI's macOS runner draws the error in 34
/// pixels, which failed the floor of 40 this used to be.
const DRAWN: usize = 12;

/// Pixels in the status bar between `x0` and `x1` within a few levels of
/// `colour`. Close rather than equal, so antialiased text counts; nothing
/// else on a near-black bar comes within that distance of red or amber.
fn pixels_near(ui: &MainWindow, colour: Color, x0: usize, x1: usize) -> usize {
    let snapshot = ui.window().take_snapshot().unwrap();
    let width = snapshot.width() as usize;
    let bytes = snapshot.as_bytes();
    let near = |a: u8, b: u8| a.abs_diff(b) <= 12;
    (BAR_TOP..snapshot.height() as usize)
        .flat_map(|y| (x0..x1.min(width)).map(move |x| (y * width + x) * 4))
        .filter(|&at| {
            near(bytes[at], colour.red())
                && near(bytes[at + 1], colour.green())
                && near(bytes[at + 2], colour.blue())
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
    }
}

#[test]
fn an_xrun_raises_a_visible_count_that_a_click_resets() {
    let ui = harness();
    let warning = ui.global::<Theme>().get_warning();
    let right_half = WIDTH as usize / 2;
    assert_eq!(ui.get_audio_load(), -1.0, "no window read yet");

    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0);
    assert_eq!(ui.get_audio_dropouts(), 0);
    assert!((ui.get_audio_load() - 0.23).abs() < 1e-6);
    assert_eq!(pixels_near(&ui, warning, right_half, WIDTH as usize), 0);

    // One driver xrun and the block that caused it: one dropout, not two.
    show_audio_load(&ui, &window_with(750, 1, RealtimeStatus::Realtime), 1);
    assert_eq!(ui.get_audio_dropouts(), 1);
    // And the next window adds to it rather than replacing it.
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 2);
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
    show_audio_load(&ui, &window_with(750, 1, RealtimeStatus::Realtime), 0);
    assert_eq!(ui.get_audio_dropouts(), 1);
}

#[test]
fn a_time_shared_callback_raises_a_badge_and_a_stopped_one_clears_the_load() {
    let ui = harness();
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::TimeShared), 0);
    assert!(ui.get_audio_time_shared());
    show_audio_load(&ui, &window_with(750, 0, RealtimeStatus::Realtime), 0);
    assert!(!ui.get_audio_time_shared());

    // A window with no blocks is an engine that is not running, and every
    // other field of it is meaningless; the last live number must not stay
    // up saying the audio is fine.
    show_audio_load(&ui, &window_with(0, 0, RealtimeStatus::Realtime), 0);
    assert_eq!(ui.get_audio_load(), -1.0);
}

