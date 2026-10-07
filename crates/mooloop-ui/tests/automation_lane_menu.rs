//! The piano roll's lane picker, driven by real pointer events.
//!
//! It is two columns in one popup: devices on the left, the hovered
//! device's parameters on the right, with **Open lanes** at the top of the
//! device column when the clip has any. The parameter rows are repeated, so
//! a row that closed the popup before reporting would report nothing
//! (`scripts/dupe-audit popup-close-order`); only a click can tell.
//!
//! Coordinates are computed: rows are 20px at 100% text, the menu pads by
//! 4px, and the device column is at least 160px wide.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { AutomationLaneMenu, AutomationTargetRow, AutomationGroupRow } from "../ui/automation-menu.slint";

    export component LaneMenuHarness inherits Window {
        width: 420px;
        height: 400px;
        background: #101010;
        in property <[AutomationTargetRow]> targets;
        in property <[AutomationGroupRow]> groups;
        in property <bool> has-open;
        in property <int> longest-list;
        // The menu's own size, copied out because a popup's insides cannot
        // be read from outside it.
        out property <length> menu-width;
        out property <length> menu-height;
        callback picked(int);

        opener := TouchArea {
            x: 0px;
            y: 0px;
            width: 28px;
            height: 24px;
            clicked => { menu.show(); }
        }
        menu := PopupWindow {
            x: 0px;
            y: 30px;
            width: content.preferred-width;
            height: content.preferred-height;
            close-policy: close-on-click-outside;
            // The box, as the roll draws it, so the snapshot shows its edge.
            Rectangle {
                background: #303030;
                content := AutomationLaneMenu {
                    targets: root.targets;
                    groups: root.groups;
                    has-open: root.has-open;
                    longest-list: root.longest-list;
                    init => {
                        root.menu-width = self.preferred-width;
                        root.menu-height = self.preferred-height;
                    }
                    changed preferred-width => { root.menu-width = self.preferred-width; }
                    changed preferred-height => { root.menu-height = self.preferred-height; }
                    picked(index) => {
                        root.picked(index);
                        menu.close();
                    }
                }
            }
        }
    }
}

const OPENER: (f32, f32) = (14.0, 12.0);
const POPUP_Y: f32 = 30.0;
const DEVICE_X: f32 = 40.0;
const PARAM_X: f32 = 220.0;

/// A parameter row's middle: 4px padding, then 20px rows.
fn param_y(row: usize) -> f32 {
    POPUP_Y + 4.0 + 10.0 + row as f32 * 20.0
}

/// A device row's middle. With Open lanes on top, its row and the 5px rule
/// (each 1px apart) come first.
fn device_y(row: usize, has_open: bool) -> f32 {
    let top = if has_open { 4.0 + 20.0 + 1.0 + 5.0 + 1.0 } else { 4.0 };
    POPUP_Y + top + 10.0 + row as f32 * 21.0
}

fn target(param: &str, device: &str, group: i32, open: bool) -> AutomationTargetRow {
    AutomationTargetRow {
        param_name: param.into(),
        device: device.into(),
        starts_group: false,
        group,
        open,
        current: false,
        missing: false,
    }
}


fn harness(has_open: bool) -> (LaneMenuHarness, Rc<RefCell<Vec<i32>>>) {
    harness_with(has_open, 0)
}

/// `extra` more parameters under a third device, Drum 1, after the four
/// above, so one column is longer than the other.
fn harness_with(has_open: bool, extra: usize) -> (LaneMenuHarness, Rc<RefCell<Vec<i32>>>) {
    // The software renderer, so the snapshot test below can draw.
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(slint::SharedString::from("software")),
        },
    )))
    .ok();
    let ui = LaneMenuHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(420.0, 400.0));
    let mut targets = vec![
        target("Cutoff", "Filter 1", 0, has_open),
        target("Resonance", "Filter 1", 0, false),
        target("Time", "Delay 1", 1, false),
        target("Feedback", "Delay 1", 1, false),
    ];
    let mut groups = vec![
        AutomationGroupRow { name: "Filter 1".into(), open: has_open },
        AutomationGroupRow { name: "Delay 1".into(), open: false },
    ];
    if extra > 0 {
        targets.extend((0..extra).map(|n| target(&format!("Param {n}"), "Drum 1", 2, false)));
        groups.push(AutomationGroupRow { name: "Drum 1".into(), open: false });
    }
    // As `refresh_automation` counts it: Open lanes lists one row.
    ui.set_longest_list(2.max(extra) as i32);
    ui.set_targets(ModelRc::from(Rc::new(VecModel::from(targets))));
    ui.set_groups(ModelRc::from(Rc::new(VecModel::from(groups))));
    ui.set_has_open(has_open);
    let picked = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_picked(move |index| seen.borrow_mut().push(index));
    (ui, picked)
}

fn hover(window: &slint::Window, at: (f32, f32)) {
    window.dispatch_event(WindowEvent::PointerMoved {
        position: LogicalPosition::new(at.0, at.1),
    });
}

fn click(window: &slint::Window, at: (f32, f32)) {
    let position = LogicalPosition::new(at.0, at.1);
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

/// **Hovering a device lists its parameters, and choosing one reports its
/// index in the full list.** With no lane open, the first device is shown.
#[test]
fn a_device_then_a_parameter_reports_the_parameter() {
    let (ui, picked) = harness(false);

    click(ui.window(), OPENER);
    click(ui.window(), (PARAM_X, param_y(1)));
    assert_eq!(*picked.borrow(), vec![1], "Filter 1's second row is Resonance");

    click(ui.window(), OPENER);
    hover(ui.window(), (DEVICE_X, device_y(1, false)));
    click(ui.window(), (PARAM_X, param_y(0)));
    assert_eq!(
        *picked.borrow(),
        vec![1, 2],
        "after hovering Delay 1, its first row is Time, index 2"
    );
}

/// **Open lanes is the first thing the picker shows when the clip has a
/// lane**, and lists only the open ones, so a lane already drawn is one
/// click away.
#[test]
fn open_lanes_come_first() {
    let (ui, picked) = harness(true);

    click(ui.window(), OPENER);
    click(ui.window(), (PARAM_X, param_y(0)));
    assert_eq!(*picked.borrow(), vec![0], "Open lanes lists Cutoff first");

    // Nothing else is under Open lanes: a click on its second row lands on
    // no row, so nothing is reported.
    click(ui.window(), OPENER);
    click(ui.window(), (PARAM_X, param_y(1)));
    assert_eq!(*picked.borrow(), vec![0], "Open lanes listed a closed parameter");

    // The devices are still below it. The miss above left the menu open.
    hover(ui.window(), (DEVICE_X, device_y(1, true)));
    click(ui.window(), (PARAM_X, param_y(1)));
    assert_eq!(*picked.borrow(), vec![0, 3], "Delay 1's second row is Feedback");
}

/// **A device with more parameters than the device column has rows still
/// gets a box tall enough for all of them** (Joam, 2026-10-07: the box was
/// sized to the column shown when it opened, and a longer list hung out of
/// it, unclickable below the box).
#[test]
fn a_long_parameter_list_fits_the_box() {
    let (ui, picked) = harness_with(true, 12);

    click(ui.window(), OPENER);
    // A real window keeps the size the popup had when it opened, which this
    // backend does not, so what is checked is that the size never changes:
    // Open lanes shows one row, Drum 1 twelve, in the same box.
    let opened = (ui.get_menu_width(), ui.get_menu_height());
    assert!(opened.1 >= 4.0 + 12.0 * 20.0, "the box is too short for Drum 1: {opened:?}");
    hover(ui.window(), (DEVICE_X, device_y(2, true)));
    assert_eq!((ui.get_menu_width(), ui.get_menu_height()), opened, "the box changed size");
    click(ui.window(), (PARAM_X, param_y(11)));
    assert_eq!(*picked.borrow(), vec![15], "Drum 1's last row is Param 11, index 15");
}

/// Dumps the open menu, Drum 1 hovered, when `MOOLOOP_LANE_MENU_SNAPSHOT`
/// names a file, so its layout can be looked at without a compositor.
#[test]
fn the_open_menu_renders() {
    let (ui, _) = harness_with(true, 12);
    click(ui.window(), OPENER);
    hover(ui.window(), (DEVICE_X, device_y(2, true)));
    let snapshot = ui.window().take_snapshot().unwrap();
    if let Ok(path) = std::env::var("MOOLOOP_LANE_MENU_SNAPSHOT") {
        let mut ppm =
            format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(&path, ppm).unwrap();
    }
    assert!(snapshot.as_bytes().iter().any(|byte| *byte != 0));
}
