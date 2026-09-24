//! A knob's right-click menu and typed entry (MOO-143), by real events.
//!
//! The issue's "done when": right-click a knob, pick "Type a Value", type,
//! and the value lands. Driven through the `TouchArea`, the popups and the
//! `TextInput`, because the change is entirely in which of those an event
//! reaches; invoking `changed` from here would pass with the menu unwired.
//!
//! The parse is the production one, `mooloop_ui::typed_value`, wired into
//! the harness the way `lib.rs` wires it into the window.

use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, SharedString};
use std::cell::RefCell;
use std::rc::Rc;

mod common;

slint::slint! {
    import { ControlRequest, ParameterKnob, ParsedValue, ParamAnswer } from "../ui/controls.slint";

    export { ControlRequest, ParsedValue, ParamAnswer }

    export component EntryHarness inherits Window {
        in-out property <float> value: 0.5;
        in-out property <float> bare-value: 0.5;
        in property <bool> armed;
        callback changed(float);
        callback named();
        callback pressed-for-assignment();

        width: 240px;
        height: 200px;

        // A knob whose face forwards nothing: a harness, a bus chain.
        ParameterKnob {
            x: 120px;
            y: 0px;
            width: 60px;
            height: 80px;
            label: "Bare";
            // The harness's properties are plain, with no binding behind
            // them, so the knobs write them: `controlled: false`, the path
            // the source faces take (MOO-220).
            controlled: false;
            value <=> root.bare-value;
            value-text: round(root.bare-value * 100) + "%";
        }

        ParameterKnob {
            x: 0px;
            y: 0px;
            width: 60px;
            height: 80px;
            label: "Amount";
            controlled: false;
            value <=> root.value;
            modulation-armed: root.armed;
            value-text: round(root.value * 100) + "%";
            changed(v) => { root.changed(v); }
            modulation-edit-started => {
                if (ControlRequest.naming) { root.named(); } else { root.pressed-for-assignment(); }
            }
        }
    }
}

struct Seen {
    changed: RefCell<Vec<f32>>,
    named: RefCell<usize>,
    assigned: RefCell<usize>,
}

fn harness() -> (EntryHarness, Rc<Seen>) {
    common::install_testing_backend();
    let ui = EntryHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(240.0, 200.0));
    let seen = Rc::new(Seen {
        changed: RefCell::new(Vec::new()),
        named: RefCell::new(0),
        assigned: RefCell::new(0),
    });
    {
        let seen = seen.clone();
        ui.on_changed(move |v| seen.changed.borrow_mut().push(v));
    }
    {
        let seen = seen.clone();
        ui.on_named(move || *seen.named.borrow_mut() += 1);
    }
    {
        let seen = seen.clone();
        ui.on_pressed_for_assignment(move || *seen.assigned.borrow_mut() += 1);
    }
    ui.global::<ControlRequest>().on_parse(|typed, shown| {
        match mooloop_ui::typed_value::parse_in_shown_units(&typed, &shown) {
            Some((value, percent)) => ParsedValue {
                ok: true,
                value,
                percent,
            },
            None => ParsedValue {
                ok: false,
                value: 0.0,
                percent: false,
            },
        }
    });
    (ui, seen)
}

fn press(window: &slint::Window, at: (f32, f32), button: PointerEventButton) {
    let position = LogicalPosition::new(at.0, at.1);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed { position, button });
    window.dispatch_event(WindowEvent::PointerReleased { position, button });
}

fn key(window: &slint::Window, text: SharedString) {
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text });
}

fn type_text(window: &slint::Window, text: &str) {
    for c in text.chars() {
        key(window, c.to_string().into());
    }
}

/// Where the right-click lands, and the middle of the menu's first row: the
/// menu opens at the pointer, inset 4px, with 24px rows.
const KNOB: (f32, f32) = (30.0, 40.0);
const BARE_KNOB: (f32, f32) = (150.0, 40.0);
const TYPE_ROW: (f32, f32) = (KNOB.0 + 40.0, KNOB.1 + 4.0 + 12.0);
const RESET_ROW: (f32, f32) = (KNOB.0 + 40.0, KNOB.1 + 4.0 + 24.0 + 12.0);

#[test]
fn a_right_click_then_type_a_value_lands_it() {
    let (ui, seen) = harness();
    press(ui.window(), KNOB, PointerEventButton::Right);
    assert!(
        seen.changed.borrow().is_empty(),
        "a right-click moved the knob"
    );

    press(ui.window(), TYPE_ROW, PointerEventButton::Left);
    // The entry opens with the readout selected, so typing replaces it.
    type_text(ui.window(), "25");
    key(ui.window(), Key::Return.into());

    assert_eq!(
        *seen.changed.borrow(),
        vec![0.25],
        "25 beside a percentage is a quarter"
    );
    assert!((ui.get_value() - 0.25).abs() < 1e-6, "{}", ui.get_value());
    assert_eq!(
        *seen.named.borrow(),
        1,
        "the knob named itself once before asking"
    );
}

#[test]
fn reset_to_default_is_in_the_menu() {
    let (ui, seen) = harness();
    press(ui.window(), KNOB, PointerEventButton::Right);
    press(ui.window(), RESET_ROW, PointerEventButton::Left);
    assert_eq!(
        *seen.changed.borrow(),
        vec![0.0],
        "the default here is the minimum"
    );
}

/// A digit typed at a focused knob opens the entry with it.
#[test]
fn a_digit_at_a_focused_knob_starts_typing() {
    let (ui, seen) = harness();
    press(ui.window(), KNOB, PointerEventButton::Left);
    type_text(ui.window(), "70");
    key(ui.window(), Key::Return.into());
    assert_eq!(*seen.changed.borrow(), vec![0.7]);
}

#[test]
fn escape_leaves_the_value_alone() {
    let (ui, seen) = harness();
    press(ui.window(), KNOB, PointerEventButton::Left);
    key(ui.window(), Key::Return.into());
    type_text(ui.window(), "90");
    key(ui.window(), Key::Escape.into());
    assert!(
        seen.changed.borrow().is_empty(),
        "{:?}",
        seen.changed.borrow()
    );
    assert_eq!(ui.get_value(), 0.5);
}

/// When Rust can name the parameter, its descriptor reads the text, and a
/// knob holding a 0..1 position takes the descriptor's position: "440"
/// beside a cutoff readout, not 440 of the knob's own travel.
#[test]
fn a_named_parameter_is_read_against_its_descriptor() {
    let (ui, seen) = harness();
    let asked: Rc<RefCell<Vec<(String, String)>>> = Rc::default();
    {
        let asked = asked.clone();
        ui.global::<ControlRequest>().on_typed(move |typed, shown| {
            asked
                .borrow_mut()
                .push((typed.to_string(), shown.to_string()));
            ParamAnswer {
                named: true,
                ok: true,
                natural: 440.0,
                normalized: 0.3,
                minimum: 20.0,
                maximum: 20_000.0,
            }
        });
    }
    press(ui.window(), KNOB, PointerEventButton::Right);
    press(ui.window(), TYPE_ROW, PointerEventButton::Left);
    type_text(ui.window(), "440");
    key(ui.window(), Key::Return.into());

    assert_eq!(*asked.borrow(), vec![("440".to_owned(), "50%".to_owned())]);
    assert_eq!(*seen.changed.borrow(), vec![0.3]);
}

/// Text that reads as nothing changes nothing, and says so.
#[test]
fn an_unreadable_value_is_refused() {
    let (ui, seen) = harness();
    let refused: Rc<RefCell<Vec<String>>> = Rc::default();
    {
        let refused = refused.clone();
        ui.global::<ControlRequest>()
            .on_refused(move |text| refused.borrow_mut().push(text.to_string()));
    }
    press(ui.window(), KNOB, PointerEventButton::Left);
    key(ui.window(), Key::Return.into());
    type_text(ui.window(), "loud");
    key(ui.window(), Key::Return.into());
    assert!(seen.changed.borrow().is_empty());
    assert_eq!(*refused.borrow(), vec!["loud".to_owned()]);
}

/// While a modulation source is armed the knob edits route depth, and a
/// typed value would write the base underneath it: no entry opens.
#[test]
fn an_armed_knob_takes_no_typed_value() {
    let (ui, seen) = harness();
    ui.set_armed(true);
    press(ui.window(), KNOB, PointerEventButton::Left);
    key(ui.window(), Key::Return.into());
    type_text(ui.window(), "10");
    key(ui.window(), Key::Return.into());
    assert!(
        seen.changed.borrow().is_empty(),
        "{:?}",
        seen.changed.borrow()
    );
}

/// The naming flag is the knob's to clear, inside the same handler that set
/// it: left set, it would turn the next real press -- a modulation edit, a
/// learn -- into a silent naming. So after a request it is down, and an
/// ordinary armed press afterwards reaches the face as a press.
#[test]
fn naming_is_cleared_and_the_next_press_is_an_ordinary_one() {
    let (ui, seen) = harness();
    press(ui.window(), KNOB, PointerEventButton::Right);
    press(ui.window(), TYPE_ROW, PointerEventButton::Left);
    type_text(ui.window(), "25");
    key(ui.window(), Key::Return.into());
    assert_eq!(*seen.named.borrow(), 1);
    assert!(
        !ui.global::<ControlRequest>().get_naming(),
        "naming was left set"
    );

    ui.set_armed(true);
    press(ui.window(), KNOB, PointerEventButton::Left);
    assert_eq!(
        *seen.named.borrow(),
        1,
        "an ordinary press was taken for a naming"
    );
    assert_eq!(
        *seen.assigned.borrow(),
        1,
        "the armed press did not reach the face"
    );
}

/// The same on a knob whose face forwards nothing: nothing is named, the
/// flag still comes down, and the typed value is read in the readout's
/// units.
#[test]
fn a_face_that_does_not_forward_leaves_naming_down() {
    let (ui, seen) = harness();
    press(ui.window(), BARE_KNOB, PointerEventButton::Left);
    key(ui.window(), Key::Return.into());
    type_text(ui.window(), "40");
    key(ui.window(), Key::Return.into());
    assert!(
        !ui.global::<ControlRequest>().get_naming(),
        "naming was left set"
    );
    assert!(
        (ui.get_bare_value() - 0.4).abs() < 1e-6,
        "{}",
        ui.get_bare_value()
    );
    assert_eq!(*seen.named.borrow(), 0);
}
