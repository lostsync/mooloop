//! The swatch grid: which colour a click actually names.
//!
//! The grid is the one part of the colour controls with arithmetic in it.
//! Each swatch is *placed* from its own index rather than laid out, because a
//! `for` inside a row cannot skip the items belonging to the other row -- a
//! hidden child still costs the layout its spacing, which drew the second row
//! indented by six gaps. Placement means the mapping from a pointer position
//! to a colour is arithmetic, and arithmetic that nothing checks is arithmetic
//! that can be off by one row forever: every swatch would still work, each one
//! would just hand over its neighbour's colour.
//!
//! Coordinates are computed rather than searched, for the reason `picker_chip`
//! records: the `ElementHandle` search API needs a build with debug info. The
//! harness is inline, so this costs a widget compile rather than an app.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

mod common;

slint::slint! {
    import { ColorChooser, ColorChoice } from "../ui/color-picker.slint";

    export component ChooserHarness inherits Window {
        width: 200px;
        height: 120px;
        in property <[ColorChoice]> choices;
        in property <bool> has-color;
        in property <string> color-hex;
        callback chosen(string);

        ColorChooser {
            x: 0;
            y: 0;
            width: 140px;
            choices: root.choices;
            has-color: root.has-color;
            color-hex: root.color-hex;
            chosen(value) => { root.chosen(value); }
        }
    }
}

/// The grid's own geometry, restated here on purpose.
///
/// A test that derived these from the markup would agree with it by
/// construction and check nothing. These are the numbers a *user's* pointer
/// meets, and if the markup moves away from them the swatches have moved,
/// which is the thing worth being told about.
const CELL: f32 = 22.0;
const SWATCH: f32 = 18.0;
const COLUMNS: usize = 6;

fn centre_of(index: usize) -> (f32, f32) {
    let col = (index % COLUMNS) as f32;
    let row = (index / COLUMNS) as f32;
    (col * CELL + SWATCH / 2.0, row * CELL + SWATCH / 2.0)
}

fn palette() -> Vec<ColorChoice> {
    [
        ("#EF4444", slint::Color::from_rgb_u8(0xEF, 0x44, 0x44)),
        ("#F97316", slint::Color::from_rgb_u8(0xF9, 0x73, 0x16)),
        ("#EAB308", slint::Color::from_rgb_u8(0xEA, 0xB3, 0x08)),
        ("#84CC16", slint::Color::from_rgb_u8(0x84, 0xCC, 0x16)),
        ("#22C55E", slint::Color::from_rgb_u8(0x22, 0xC5, 0x5E)),
        ("#14B8A6", slint::Color::from_rgb_u8(0x14, 0xB8, 0xA6)),
        ("#0EA5E9", slint::Color::from_rgb_u8(0x0E, 0xA5, 0xE9)),
        ("#6366F1", slint::Color::from_rgb_u8(0x63, 0x66, 0xF1)),
    ]
    .into_iter()
    .map(|(value, tint)| ColorChoice {
        value: SharedString::from(value),
        tint,
    })
    .collect()
}

fn harness() -> (ChooserHarness, Rc<RefCell<Vec<String>>>) {
    common::install_testing_backend();
    let ui = ChooserHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(200.0, 120.0));
    ui.set_choices(ModelRc::from(Rc::new(VecModel::from(palette()))));

    let chosen = Rc::new(RefCell::new(Vec::new()));
    let sink = chosen.clone();
    ui.on_chosen(move |value| sink.borrow_mut().push(value.to_string()));
    (ui, chosen)
}

fn click(window: &slint::Window, at: (f32, f32)) {
    let pos = LogicalPosition::new(at.0, at.1);
    window.dispatch_event(WindowEvent::PointerMoved { position: pos });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: pos,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: pos,
        button: PointerEventButton::Left,
    });
}

/// Every swatch hands over the colour that is drawn on it.
///
/// All eight, not a sample: an off-by-one in the row arithmetic leaves the
/// first row correct and the second row shifted, and a test that checked one
/// colour from each end would pass while every middle swatch lied.
#[test]
fn each_swatch_names_the_colour_it_draws() {
    let (ui, chosen) = harness();
    for (index, choice) in palette().iter().enumerate() {
        click(ui.window(), centre_of(index));
        assert_eq!(
            chosen.borrow().last().map(String::as_str),
            Some(choice.value.as_str()),
            "swatch {index} handed over the wrong colour"
        );
    }
    assert_eq!(chosen.borrow().len(), palette().len());
}

/// The square after the last colour is "no colour", and it says so with an
/// empty string rather than with a colour meaning nothing.
#[test]
fn the_square_after_the_colours_clears_the_colour() {
    let (ui, chosen) = harness();
    click(ui.window(), centre_of(palette().len()));
    assert_eq!(chosen.borrow().last().map(String::as_str), Some(""));
}

/// The gap between two swatches belongs to neither.
///
/// 18px squares on a 22px pitch leave 4px of panel between them, and a grid
/// that placed 22px squares would still look almost right while making every
/// click land on something.
#[test]
fn the_gap_between_swatches_is_not_a_swatch() {
    let (ui, chosen) = harness();
    click(ui.window(), (SWATCH + 2.0, SWATCH / 2.0));
    assert!(
        chosen.borrow().is_empty(),
        "a click in the gap chose {:?}",
        chosen.borrow()
    );
}
