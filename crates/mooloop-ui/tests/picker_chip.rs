//! `PickerChip`'s popup, driven by real pointer events.
//!
//! The chip is the app's one-of-many control: the toolbar's channel source,
//! Aux In's channel and outlet, ML-P8's route ends, DS-01's matrix ends. All
//! of them are a `PopupWindow` whose rows are a repeater, and the only thing
//! that matters about that shape is whether clicking a row reaches `picked`
//! carrying the row's index. Setting the selection from Rust would pass even
//! if no click ever routed, which is exactly the failure this file exists to
//! catch, and which `effect_preset_menu.rs` caught once already: the menu
//! opened, drew correctly, and did nothing.
//!
//! Coordinates are computed rather than searched, for the reason `first_click`
//! records: the `ElementHandle` search API needs a build with debug info, and
//! the chip has fixed geometry anyway. The harness is inline rather than a
//! face out of `main.slint`, so this test costs a widget compile, not an app.

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { PickerChip } from "../ui/controls.slint";

    export component PickerHarness inherits Window {
        width: 200px;
        height: 200px;
        background: #101010;
        in property <[string]> options;
        in property <int> selected-index;
        callback picked(int);

        PickerChip {
            x: 0px;
            y: 0px;
            width: 120px;
            options: root.options;
            selected-index: root.selected-index;
            tooltip: "Pick a source";
            picked(i) => { root.picked(i); }
        }
    }
}

/// The chip is 18px tall and hangs its popup 2px below itself, inset by 4px,
/// with 18px rows on a 19px pitch. These are the middles of the first three.
const ROW_X: f32 = 40.0;
fn row_y(index: usize) -> f32 {
    18.0 + 2.0 + 4.0 + 9.0 + index as f32 * 19.0
}

fn harness() -> PickerHarness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = PickerHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(200.0, 200.0));
    ui.set_options(ModelRc::from(Rc::new(VecModel::from(vec![
        SharedString::from("Sampler"),
        SharedString::from("Wavetable"),
        SharedString::from("Aux In"),
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
fn picking_a_row_reports_its_index() {
    let ui = harness();
    let picked: Rc<RefCell<Vec<i32>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = picked.clone();
    ui.on_picked(move |index| seen.borrow_mut().push(index));

    click(ui.window(), (60.0, 9.0));
    click(ui.window(), (ROW_X, row_y(1)));
    assert_eq!(
        *picked.borrow(),
        vec![1],
        "choosing the second entry did not reach the callback"
    );

    click(ui.window(), (60.0, 9.0));
    click(ui.window(), (ROW_X, row_y(2)));
    assert_eq!(
        *picked.borrow(),
        vec![1, 2],
        "the third entry reported the wrong index"
    );
}
