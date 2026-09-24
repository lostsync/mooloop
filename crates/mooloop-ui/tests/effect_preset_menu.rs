//! The rack row's preset rail, driven by real pointer events.
//!
//! The load control is a `PopupWindow` hung off an `IconButton`, and the only
//! thing that matters about it is whether picking an entry actually reaches
//! the callback carrying the entry's index. Invoking `preset-selected`
//! directly would pass even if the popup never routed a click, which is the
//! failure this file exists to catch: the first cut of the feature shipped
//! with presets that did not load when chosen.
//!
//! Coordinates are computed rather than searched, for the reason
//! `first_click.rs` records: the `ElementHandle` search API needs a build with
//! debug info, and these controls have fixed geometry anyway.
//!
//! What it caught: the row closed the popup before invoking the callback, and
//! closing a popup destroys the repeater item whose handler is still running,
//! so the call never landed. The menu opened, drew correctly, and did nothing.
//! The insert menu survives the same sequence because its rows are written
//! out rather than repeated, which is why it is here as a control.
//!
//! It also holds the three-list agreement for device kinds, because the
//! insert menu is one of the three lists. A kind exists in `EffectKind`, is
//! offered by a row in `device-rack.slint`, and is drawn by an arm in
//! `main.slint`, and the only thing joining them is the small integer
//! `mooloop_ui::effect_kind_index` hands out. Adding a kind means appending
//! to all three; forgetting one of them produces a device that saves and
//! loads but cannot be inserted, or one that inserts and renders as an empty
//! frame, and neither of those fails anything else.

use mooloop_core::EffectKind;
use mooloop_ui::effect_kind_index;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

const DEVICE_RACK_SLINT: &str = include_str!("../ui/device-rack.slint");
const MAIN_SLINT: &str = include_str!("../ui/main.slint");

slint::slint! {
    import { DeviceFrame, DeviceJoin } from "../ui/device-rack.slint";

    export component PresetHarness inherits Window {
        width: 460px;
        height: 460px;
        background: #101010;
        in property <[string]> options;
        callback preset-selected(int);
        callback save-requested();
        callback kind-selected(int);
        callback wrapped(int);

        DeviceFrame {
            x: 0px; y: 0px;
            width: 280px; height: 268px;
            preset-enabled: true;
            preset-options: root.options;
            preset-selected(i) => { root.preset-selected(i); }
            save-preset-requested => { root.save-requested(); }
            wrap-enabled: true;
            wrap-requested(k) => { root.wrapped(k); }
        }
        // Where a device is added from since MOO-218: the join after a
        // device, not a button on its rail.
        DeviceJoin {
            x: 290px; y: 0px;
            kind-selected(k) => { root.kind-selected(k); }
        }
    }
}

/// The left rail stacks its buttons from the top with 2px of padding and 2px
/// between them, each 24px square: collapse, save preset, load preset, and
/// wrap in a container. The collapse `<` took the insert `+`'s place on
/// 2026-09-24 (MOO-218, MOO-219), so the three below it kept theirs.
const BUTTON_X: f32 = 14.0;
const SAVE_Y: f32 = 40.0;
const LOAD_Y: f32 = 66.0;

/// The popup opens beside the rail at the load button's own height, and its
/// list is inset by 4px with 22px rows. These are the middles of the first
/// two entries.
const FIRST_ENTRY: (f32, f32) = (120.0, 95.0);
const SECOND_ENTRY: (f32, f32) = (120.0, 117.0);

/// The join, and the menu it opens under the device header (28px down): rows
/// are 22px on a 23px pitch from a 4px inset, so the first row's middle is
/// at 43.
const JOIN: (f32, f32) = (300.0, 134.0);
const MENU_X: f32 = 320.0;

/// The rail's own menus (wrap) open under their button, inset from the rail.
const RAIL_MENU_X: f32 = 36.0;
const FIRST_ROW_Y: f32 = 43.0;
const ROW_PITCH: f32 = 23.0;

fn menu_row_y(row: usize) -> f32 {
    FIRST_ROW_Y + row as f32 * ROW_PITCH
}

/// Every kind's index, sorted, which is what a complete menu reports.
fn every_kind_index() -> Vec<i32> {
    let mut indices: Vec<i32> = EffectKind::ALL.iter().copied().map(effect_kind_index).collect();
    indices.sort();
    indices
}


fn harness() -> PresetHarness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = PresetHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(460.0, 460.0));
    ui.set_options(ModelRc::from(Rc::new(VecModel::from(vec![
        SharedString::from("Factory — Telephone"),
        SharedString::from("Factory — Warm Low-Pass"),
    ]))));
    ui
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

/// The wrap button, fourth on the rail, and the two-row menu it opens below
/// itself (`containers/10`): 4px inset, 22px rows on a 23px pitch.
const WRAP_Y: f32 = 92.0;
const WRAP_ROWS_Y: [f32; 2] = [121.0, 144.0];

/// The wrap menu reaches its callback with the kind of each row, clicked.
/// A menu that closed itself before calling back would open, draw, and do
/// nothing -- `dupe-audit popup-close-order` is the search for that, and
/// this is the press.
#[test]
fn the_wrap_menu_reports_the_kind_each_row_offers() {
    let ui = harness();
    let seen: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let log = seen.clone();
    ui.on_wrapped(move |kind| log.borrow_mut().push(kind));
    for row_y in WRAP_ROWS_Y {
        click(ui.window(), (BUTTON_X, WRAP_Y));
        click(ui.window(), (RAIL_MENU_X, row_y));
    }
    assert_eq!(
        *seen.borrow(),
        [
            effect_kind_index(EffectKind::Chain),
            effect_kind_index(EffectKind::Layer)
        ],
        "the wrap menu's rows did not report Chain then Layer"
    );
}

#[test]
fn picking_an_entry_reports_its_index() {
    let ui = harness();
    let picked: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_preset_selected(move |index| seen.borrow_mut().push(index));

    click(ui.window(), (BUTTON_X, LOAD_Y));
    click(ui.window(), FIRST_ENTRY);
    assert_eq!(
        *picked.borrow(),
        vec![0],
        "choosing the first preset did not reach the callback"
    );

    click(ui.window(), (BUTTON_X, LOAD_Y));
    click(ui.window(), SECOND_ENTRY);
    assert_eq!(
        *picked.borrow(),
        vec![0, 1],
        "the second entry reported the wrong index"
    );
}

#[test]
fn the_save_button_asks_for_a_save() {
    let ui = harness();
    let asked = Rc::new(RefCell::new(0));
    let count = asked.clone();
    ui.on_save_requested(move || *count.borrow_mut() += 1);

    click(ui.window(), (BUTTON_X, SAVE_Y));
    assert_eq!(*asked.borrow(), 1, "the save rail button did nothing");
}

/// **Every effect kind is reachable from the insert menu.**
///
/// `EffectTypeMenu` is a hand-written list in `device-rack.slint` and
/// `EffectKind::ALL` is a list in Rust, and nothing but this holds them
/// together. A kind added to the enum and forgotten here is a device that
/// exists, saves, loads and renders, and that a musician has no way to put on
/// a chain -- which is exactly what happened when the container kind landed
/// and shipped unreachable.
///
/// Every row is clicked, and what comes back has to be every kind's index
/// exactly once. That is the reachability question asked directly, and it is
/// indifferent to the order the menu lists things in. The previous version
/// clicked only the last row and asserted it reported `ALL.len() - 1`, which
/// quietly required the menu's order to match the numbering: it failed on
/// 2026-09-10 because the preamp's row was put where it reads best rather
/// than where its number fell, and the failure said nothing about order.
///
/// Clicking rather than counting, because the count is the thing in question:
/// a menu one entry short would still have a last row, and it would report
/// the wrong kind. A click at the missing row's coordinates lands on nothing
/// and reports nothing, which is the gap this sees.
#[test]
fn the_insert_menu_offers_every_kind() {
    let ui = harness();
    let picked: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_kind_selected(move |kind| seen.borrow_mut().push(kind));

    let kinds = EffectKind::ALL.len();
    // A row past the bottom of the harness window cannot be clicked, and an
    // unclicked row reads exactly like a missing one.
    let window_height = ui.window().size().height as f32;
    assert!(
        menu_row_y(kinds - 1) + ROW_PITCH < window_height,
        "the harness window is {window_height}px tall and the menu now needs \
         {kinds} rows: make it taller, or this test starts passing by failing \
         to click"
    );

    for row in 0..kinds {
        click(ui.window(), JOIN);
        click(ui.window(), (MENU_X, menu_row_y(row)));
    }

    let mut reported = picked.borrow().clone();
    reported.sort();
    assert_eq!(
        reported,
        every_kind_index(),
        "clicking every row of the insert menu did not report every kind \
         exactly once. A short value list means a row is missing and that \
         kind cannot be put on a chain; a value no kind owns means a row \
         carries a number `effect_kind_index` does not hand out"
    );
}

/// **Both lists that carry a kind's number agree with the number itself.**
///
/// `effect_kind_index` is a contract with two pieces of markup: the insert
/// menu calls `kind-selected(n)` and `main.slint` draws the face with
/// `if slot.kind == n`. Neither is reachable from Rust, so they are read back
/// out of the markup here rather than exercised. The clicking test above
/// covers the menu end for real; this adds the half that no rendered test
/// can see, because a kind with no arm in `main.slint` does not fail -- it
/// draws an empty device frame and carries on.
///
/// A face is expected to be named after its kind. That is the convention all
/// fourteen already follow, and holding it means the pairing can be checked
/// rather than just the count: an arm that instantiated the wrong face would
/// otherwise look exactly like a correct one.
#[test]
fn the_menu_and_the_faces_cover_every_kind() {
    let menu = menu_indices();
    let faces = face_branches();
    let predicate_faces = predicate_face_branches();
    let known = every_kind_index();

    for index in &menu {
        assert!(
            known.contains(index),
            "the insert menu offers kind {index}, which effect_kind_index \
             never produces: inserting it would build nothing"
        );
    }
    for (index, face) in &faces {
        assert!(
            known.contains(index),
            "main.slint draws {face} for kind {index}, which \
             effect_kind_index never produces: that arm is dead markup"
        );
    }

    for kind in EffectKind::ALL {
        let index = effect_kind_index(kind);
        assert_eq!(
            menu.iter().filter(|offered| **offered == index).count(),
            1,
            "{} (kind {index}) needs exactly one `kind-selected({index})` row \
             in device-rack.slint's EffectTypeMenu",
            kind.label()
        );

        // A container is drawn by a predicate arm rather than by its index:
        // the chain by `is-container && !is-layer`, the layer by `is-layer`
        // (`containers/09` gave it its own face). The question is unchanged
        // -- exactly one arm draws this kind -- but it is asked of the
        // predicates rather than of the number.
        if kind.is_container() {
            let expected = if kind.container_flow() == Some(mooloop_core::ContainerFlow::Parallel)
            {
                "LayerDeviceFace"
            } else {
                "ContainerDeviceFace"
            };
            let drawing: Vec<&String> = predicate_faces
                .iter()
                .filter(|(parallel, _)| {
                    *parallel
                        == (kind.container_flow() == Some(mooloop_core::ContainerFlow::Parallel))
                })
                .map(|(_, face)| face)
                .collect();
            assert_eq!(
                drawing,
                vec![expected],
                "{} is a container, so exactly one predicate arm in main.slint \
                 draws it, and that arm draws {expected}",
                kind.label()
            );
            assert!(
                !faces.iter().any(|(at, _)| *at == index),
                "{} (kind {index}) has an indexed face arm as well as the \
                 predicate one, so it would draw two faces",
                kind.label()
            );
            continue;
        }

        let expected_face = format!("{kind:?}DeviceFace");
        let drawn: Vec<&str> = faces
            .iter()
            .filter(|(at, _)| *at == index)
            .map(|(_, face)| face.as_str())
            .collect();
        assert_eq!(
            drawn,
            vec![expected_face.as_str()],
            "{} (kind {index}) needs exactly one \
             `if slot.kind == {index} : {expected_face}` arm in main.slint",
            kind.label()
        );
    }
}

/// The kind each `EffectTypeRow` in `device-rack.slint` inserts.
fn menu_indices() -> Vec<i32> {
    DEVICE_RACK_SLINT
        .lines()
        // A row is an element that *starts* its line. The wrap menu's
        // `WrapKindRow` is declared as inheriting `EffectTypeRow`, and that
        // declaration is not a row of the insert menu (`containers/10`).
        .filter(|line| line.trim_start().starts_with("EffectTypeRow {"))
        .map(|line| {
            let call = line
                .split_once("kind-selected(")
                .unwrap_or_else(|| panic!("an EffectTypeRow that inserts nothing: {line:?}"))
                .1;
            call.split_once(')')
                .and_then(|(index, _)| index.trim().parse().ok())
                .unwrap_or_else(|| panic!("kind-selected's argument is not a literal: {line:?}"))
        })
        .collect()
}

/// The kind each face arm in `main.slint` draws, and what it draws.
fn face_branches() -> Vec<(i32, String)> {
    MAIN_SLINT
        .lines()
        .filter_map(|line| line.split_once("if slot.kind == "))
        .filter_map(|(_, rest)| rest.split_once(" : "))
        .filter_map(|(index, face)| {
            let face = face.trim().trim_end_matches('{').trim();
            if !face.ends_with("DeviceFace") {
                return None;
            }
            Some((index.trim().parse().ok()?, face.to_string()))
        })
        .collect()
}

/// The faces drawn by a predicate rather than by an index, each with whether
/// its predicate admits a layer.
///
/// Two arms since `containers/09`: `if slot.is-container && !slot.is-layer :
/// ContainerDeviceFace` and `if slot.is-layer : LayerDeviceFace`. Keyed that
/// way because containment is a property of the row and not of its number:
/// the markup asked `slot.kind == 13` in seven places until 2026-09-21, and
/// a second container kind would have had to be added to all seven as
/// `|| kind == 14`.
///
/// It means face dispatch is no longer one arm per kind, so the cover test
/// below asks a container kind a different question -- but the same one
/// underneath: **is there exactly one arm that draws it.**
fn predicate_face_branches() -> Vec<(bool, String)> {
    let mut out = Vec::new();
    for (predicate, parallel) in [
        ("if slot.is-container && !slot.is-layer : ", false),
        ("if slot.is-layer : ", true),
    ] {
        out.extend(
            MAIN_SLINT
                .lines()
                .filter_map(|line| line.split_once(predicate))
                .filter_map(|(_, face)| {
                    let face = face.trim().trim_end_matches('{').trim();
                    face.ends_with("DeviceFace").then(|| (parallel, face.to_string()))
                }),
        );
    }
    out
}

/// The control experiment. The insert menu is the same shape -- a
/// `PopupWindow` hung off a control, closed by the row it contains -- and it
/// has worked since the shell was drawn; it hangs off the join between two
/// devices now (MOO-218) rather than a rail button. If this passes where the
/// preset menu fails, the difference between them is the bug; if both fail,
/// the harness cannot drive popups and the preset test proves nothing.
#[test]
fn the_insert_menu_reports_its_kind() {
    let ui = harness();
    let picked: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_kind_selected(move |kind| seen.borrow_mut().push(kind));

    click(ui.window(), JOIN);
    click(ui.window(), (MENU_X, menu_row_y(0)));

    // Which kind is first is the menu's business -- it lists them in the
    // order that reads best, not in numbering order -- so this asks only that
    // one click produced one kind that exists.
    let picked = picked.borrow();
    assert_eq!(picked.len(), 1, "the insert menu did not deliver");
    assert!(
        every_kind_index().contains(&picked[0]),
        "the insert menu's first row reported {}, which is not a kind",
        picked[0]
    );
}
