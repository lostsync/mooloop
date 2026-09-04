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

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { DeviceFrame } from "../ui/device-rack.slint";

    export component PresetHarness inherits Window {
        width: 320px;
        height: 320px;
        background: #101010;
        in property <[string]> options;
        callback preset-selected(int);
        callback save-requested();
        callback kind-selected(int);

        DeviceFrame {
            x: 0px; y: 0px;
            width: 280px; height: 268px;
            preset-enabled: true;
            preset-options: root.options;
            preset-selected(i) => { root.preset-selected(i); }
            save-preset-requested => { root.save-requested(); }
            effect-kind-selected(k) => { root.kind-selected(k); }
        }
    }
}

/// The left rail stacks its buttons from the top with 2px of padding and 2px
/// between them, each 24px square: insert, save preset, load preset.
const BUTTON_X: f32 = 14.0;
const SAVE_Y: f32 = 40.0;
const LOAD_Y: f32 = 66.0;

/// The popup opens beside the rail at the load button's own height, and its
/// list is inset by 4px with 22px rows. These are the middles of the first
/// two entries.
const FIRST_ENTRY: (f32, f32) = (120.0, 95.0);
const SECOND_ENTRY: (f32, f32) = (120.0, 117.0);

fn harness() -> PresetHarness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = PresetHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(320.0, 320.0));
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

/// The control experiment. The insert menu is the same shape -- a
/// `PopupWindow` hung off a rail `IconButton`, closed by the row it contains
/// -- and it has worked since the shell was drawn. If this passes where the
/// preset menu fails, the difference between them is the bug; if both fail,
/// the harness cannot drive popups and the preset test proves nothing.
#[test]
fn the_insert_menu_reports_its_kind() {
    let ui = harness();
    let picked: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_kind_selected(move |kind| seen.borrow_mut().push(kind));

    click(ui.window(), (BUTTON_X, 14.0));
    click(ui.window(), (36.0, 43.0));
    assert_eq!(*picked.borrow(), vec![0], "the insert menu did not deliver");
}
