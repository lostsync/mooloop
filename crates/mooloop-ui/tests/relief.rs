//! The relief adopters (`docs/plans/theming/02-relief.md`, MOO-153): under a
//! bevelled theme the knob caps, the device header, the rack row's plate,
//! the panes and the panels are lit from the top-left, and pressed means lit
//! from the other side. Under a flat theme each draws exactly what it drew
//! before relief existed.
//!
//! Read off software renders, by comparing an edge with the surface it bounds
//! in the same frame, so no colour of any theme is restated here.

use mooloop_ui::{MainWindow, Theme};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, Rgba8Pixel, SharedPixelBuffer};

mod common;

slint::slint! {
    import { Theme } from "../ui/theme.slint";
    import { MiniKnob } from "../ui/controls.slint";
    import { DeviceFrame, DeviceHeader, DeviceRackMetrics } from "../ui/device-rack.slint";

    export component ReliefHarness inherits Window {
        width: 420px;
        height: 260px;
        background: #101010;
        out property <length> header-height: DeviceRackMetrics.header-height;
        public function set-relief(relief: int) { Theme.relief = relief; }

        // A plate with nothing on it: its content area is the frame's fill.
        DeviceFrame { x: 10px; y: 10px; width: 260px; height: 120px; }
        DeviceHeader { x: 10px; y: 150px; width: 200px; name: "X"; role: ""; }
        // 100px, so the cap is 316..384 by 36..104: row 35 is where its top
        // meets the knob's track, row 104 is just under it, in the track's gap.
        MiniKnob { x: 300px; y: 20px; width: 100px; height: 100px; diameter: 100px; }
    }
}

type Snapshot = SharedPixelBuffer<Rgba8Pixel>;

fn pixel(snapshot: &Snapshot, x: u32, y: u32) -> (u8, u8, u8) {
    let p = snapshot.as_slice()[(y * snapshot.width() + x) as usize];
    (p.r, p.g, p.b)
}

fn lum(snapshot: &Snapshot, x: u32, y: u32) -> u32 {
    let (r, g, b) = pixel(snapshot, x, y);
    u32::from(r) + u32::from(g) + u32::from(b)
}

fn harness_at(relief: i32) -> (ReliefHarness, Snapshot) {
    common::install_testing_backend();
    let harness = ReliefHarness::new().unwrap();
    harness.invoke_set_relief(relief);
    let snapshot = harness.window().take_snapshot().expect("headless snapshot");
    (harness, snapshot)
}

const BACKGROUND: (u8, u8, u8) = (0x10, 0x10, 0x10);
const KNOB_X: u32 = 350;
/// Inside the knob's track and clear of the cap: the track and nothing else.
const TRACK: u32 = 30;
const ABOVE_CAP: u32 = 35;
/// Inside the cap: its fill.
const CAP: u32 = 100;
const BELOW_CAP: u32 = 104;

/// The cap's two crescents, as (above, below) against what each lies beside:
/// the track over the cap and the cap's own fill under it.
fn cap_edges(snapshot: &Snapshot) -> ((u32, u32), (u32, u32)) {
    (
        (lum(snapshot, KNOB_X, ABOVE_CAP), lum(snapshot, KNOB_X, TRACK)),
        (lum(snapshot, KNOB_X, BELOW_CAP), lum(snapshot, KNOB_X, CAP)),
    )
}

#[test]
fn a_flat_theme_draws_no_bevel() {
    let (harness, flat) = harness_at(0);
    // The cap ends at its own hairline: over it is the track, under it the
    // window.
    assert_eq!(pixel(&flat, KNOB_X, ABOVE_CAP), pixel(&flat, KNOB_X, TRACK));
    assert_eq!(pixel(&flat, KNOB_X, BELOW_CAP), BACKGROUND);
    // The plate's perimeter is one hairline colour on every side.
    assert_eq!(pixel(&flat, 150, 10), pixel(&flat, 150, 129));
    // The header has a hairline under it and nothing over it.
    let h = harness.get_header_height() as u32;
    let fill = pixel(&flat, 150, 150 + h / 2);
    assert_eq!(pixel(&flat, 150, 150), fill);
    assert_ne!(pixel(&flat, 150, 150 + h - 1), fill);
}

#[test]
fn a_bevel_lights_every_adopter_from_the_top_left() {
    let (harness, bevel) = harness_at(1);

    // The knob's cap: a lit crescent over it, brighter than the track it
    // lies on, and a shaded one under it, darker than the cap.
    let ((above, track), (below, cap)) = cap_edges(&bevel);
    assert!(above > track + 30, "cap lit above: {above} over the track's {track}");
    assert!(below + 30 < cap, "cap shaded below: {below} under the cap's {cap}");
    assert_ne!(pixel(&bevel, KNOB_X, BELOW_CAP), BACKGROUND, "the cap's shaded edge");

    // The rack row's plate.
    let plate = lum(&bevel, 150, 70);
    let (top, bottom) = (lum(&bevel, 150, 10), lum(&bevel, 150, 129));
    assert!(top > plate && plate > bottom, "plate {top} > {plate} > {bottom}");
    let (left, right) = (lum(&bevel, 10, 70), lum(&bevel, 269, 70));
    assert!(left > right, "plate lit from the left: {left} over {right}");

    // The device header.
    let h = harness.get_header_height() as u32;
    let fill = lum(&bevel, 150, 150 + h / 2);
    let (top, bottom) = (lum(&bevel, 150, 150), lum(&bevel, 150, 150 + h - 1));
    assert!(top > fill && fill > bottom, "header {top} > {fill} > {bottom}");
}

/// The cap's crescents swapped: shaded over it, lit under it.
fn lit_from_below(snapshot: &Snapshot, what: &str) {
    let ((above, track), (below, cap)) = cap_edges(snapshot);
    assert!(above + 30 < track, "{what} cap shaded above: {above} under the track's {track}");
    assert!(below > cap + 30, "{what} cap lit below: {below} over the cap's {cap}");
}

#[test]
fn a_held_knob_and_an_inset_theme_are_lit_from_the_other_side() {
    let (harness, _) = harness_at(1);
    let centre = LogicalPosition::new(KNOB_X as f32, 70.0);
    harness
        .window()
        .dispatch_event(WindowEvent::PointerMoved { position: centre });
    harness.window().dispatch_event(WindowEvent::PointerPressed {
        position: centre,
        button: PointerEventButton::Left,
    });
    let held = harness.window().take_snapshot().expect("headless snapshot");
    lit_from_below(&held, "held");

    let (harness, inset) = harness_at(2);
    lit_from_below(&inset, "inset");
    let h = harness.get_header_height() as u32;
    let (top, bottom) = (lum(&inset, 150, 150), lum(&inset, 150, 150 + h - 1));
    assert!(bottom > top, "inset header lit from below: {bottom} over {top}");
}

/// The shell: the panes (the dock's included) and the status bar, in the
/// real window at the layout a first run gets.
#[test]
fn the_panes_and_the_status_bar_are_plates_under_a_bevel() {
    common::install_testing_backend();
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(1280.0, 760.0));

    let flat = ui.window().take_snapshot().expect("headless snapshot");
    // The status bar is the bottom 24 rows, and flat it has no lower edge.
    assert_eq!(pixel(&flat, 400, 759), pixel(&flat, 400, 748));
    // The top pane's two side edges are the same colour.
    assert_eq!(pixel(&flat, 0, 200), pixel(&flat, 1279, 200));

    ui.global::<Theme>().set_relief(1);
    let bevel = ui.window().take_snapshot().expect("headless snapshot");
    let fill = lum(&bevel, 400, 748);
    let (top, bottom) = (lum(&bevel, 400, 736), lum(&bevel, 400, 759));
    assert!(top > fill && fill > bottom, "status bar {top} > {fill} > {bottom}");
    let (left, right) = (lum(&bevel, 0, 200), lum(&bevel, 1279, 200));
    assert!(left > right, "pane lit from the left: {left} over {right}");
}
