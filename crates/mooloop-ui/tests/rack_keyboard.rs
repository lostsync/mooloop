//! The keyboard has to survive using the rack.
//!
//! Adam reported that after working with device containers, space-to-play and
//! every other shortcut were dead. **This file does not reproduce that**, and
//! that is worth saying plainly: it is the set of rack gestures that were
//! ruled out, kept so they stay ruled out.
//!
//! Why these gestures were the suspects. Slint delivers a key to the focused
//! item and then walks up towards the window, so the root `keys` scope hears
//! a key only while it, or something inside it, holds focus -- and
//! `i-slint-core-1.17.1/window.rs` clears the focus outright when the focused
//! item stops being visible:
//!
//! ```text
//! if item.as_ref().is_some_and(|i| !i.is_visible()) {
//!     // Reset the focus... not great, but better than keeping it.
//!     self.take_focus_item(...);
//!     item = None;
//! }
//! ```
//!
//! With the focus at `None` there is no item to start from, so *nothing* gets
//! the key -- not the control, not the root scope, not the window. Every
//! shortcut is dead until something takes focus again, and nothing in the
//! rack ever does. A rack edit destroys and rebuilds repeater rows, which is
//! how a focused item stops being visible, so a reorder was the obvious
//! candidate. It is not the cause: a click on a header, and a drag that
//! actually moves a device, both leave the shortcuts working.
//!
//! Also ruled out, live through `scripts/mooloop-mcp` rather than here:
//! clicking a knob, clicking a button inside a device face and then
//! destroying that face by switching the source kind, and clicking a value
//! field.
//!
//! Geometry is computed rather than searched, and was read off the running
//! application through `scripts/mooloop-mcp`: at 1280x760 the rack row sits
//! at (8, 364) and the first device frame at (16, 384).

use mooloop_ui::{view, ChannelRow, EffectSlotRow, MainWindow, StepCell};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// The source device is three rack units wide, so the first effect starts
/// after it and its join arrow.
const SOURCE_WIDTH: f32 = 220.0 * 3.0 + 4.0 + 30.0 * 2.0;
const RACK_X: f32 = 16.0;
const RACK_Y: f32 = 384.0;
const CELL: f32 = 220.0 + 30.0 * 2.0 + 20.0;
/// Inside the header strip, right of the left rail and left of the host's
/// bypass and wet/dry controls.
const HEADER_Y: f32 = RACK_Y + 14.0;

fn effect_x(index: usize) -> f32 {
    RACK_X + SOURCE_WIDTH + 20.0 + CELL * index as f32 + 30.0 + 40.0
}

fn channels() -> ModelRc<ChannelRow> {
    ModelRc::from(Rc::new(VecModel::from(vec![ChannelRow {
        name: SharedString::from("Kick"),
        muted: false,
        volume_db: -1.9382,
        pan: 0.0,
        selected: true,
        bus: 0,
        steps: ModelRc::from(Rc::new(VecModel::from(vec![
            StepCell {
                active: false,
                velocity: 0,
                substeps: 0,
                onsets: 0,
            };
            16
        ]))),
    }])))
}

fn effect_slot(kind: i32) -> EffectSlotRow {
    EffectSlotRow {
        kind,
        units: 1,
        preset_options: Vec::<SharedString>::new().as_slice().into(),
        preset_name: Default::default(),
        bypassed: false,
        p0: 0.5,
        p1: 0.5,
        p2: 0.0,
        p3: 0.5,
        p4: 0.5,
        p5: 0.5,
        p6: 0.0,
        p7: 0.0,
        p8: 0.0,
        p9: 0.0,
        modulation_depths: Vec::<f32>::new().as_slice().into(),
        modulation_allowed: Vec::<bool>::new().as_slice().into(),
        modulation_offsets: Vec::<f32>::new().as_slice().into(),
        modulation_route_counts: Vec::<i32>::new().as_slice().into(),
        eq_band_data: Vec::<f32>::new().as_slice().into(),
        eq_spectrum_data: Vec::<f32>::new().as_slice().into(),
        eq_analyzer_enabled: false,
        buffer_collisions: 0,
        wet_dry: 1.0,
        input_trim_db: 0.0,
        output_trim_db: 0.0,
        input_left_db: -60.0,
        input_right_db: -60.0,
        output_left_db: -60.0,
        output_right_db: -60.0,
        detector_db: -60.0,
        gain_reduction_db: 0.0,
        children: 0,
        depth: 0,
        closing: Vec::<i32>::new().as_slice().into(),
        selected: false,
    }
}

struct Harness {
    ui: MainWindow,
    keys: Rc<RefCell<Vec<String>>>,
}

fn harness() -> Harness {
    i_slint_backend_testing::init_no_event_loop();
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(1280.0, 760.0));
    ui.set_channels(channels());
    ui.set_selected_channel_name(SharedString::from("Kick"));
    ui.invoke_show_view(view::DEVICES);
    ui.set_source_kind(1);
    ui.set_effect_slots(ModelRc::from(Rc::new(VecModel::from(vec![
        effect_slot(0),
        effect_slot(1),
        effect_slot(3),
    ]))));

    let keys: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let seen = keys.clone();
    ui.on_shortcut_key(move |name, _ctrl, _shift, _alt, _meta| {
        seen.borrow_mut().push(name.to_string());
        true
    });
    Harness { ui, keys }
}

impl Harness {
    fn space(&self) {
        self.ui
            .window()
            .dispatch_event(WindowEvent::KeyPressed { text: ' '.into() });
        self.ui
            .window()
            .dispatch_event(WindowEvent::KeyReleased { text: ' '.into() });
    }

    fn drag(&self, from: f32, to: f32) {
        let window = self.ui.window();
        let at = |x: f32| LogicalPosition::new(x, HEADER_Y);
        window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
        window.dispatch_event(WindowEvent::PointerPressed {
            position: at(from),
            button: PointerEventButton::Left,
        });
        for i in 1..=24 {
            let x = from + (to - from) * i as f32 / 24.0;
            window.dispatch_event(WindowEvent::PointerMoved { position: at(x) });
        }
        window.dispatch_event(WindowEvent::PointerReleased {
            position: at(to),
            button: PointerEventButton::Left,
        });
    }

    fn took(&self) -> usize {
        self.keys.borrow().len()
    }
}

#[test]
fn space_still_reaches_the_transport_after_the_rack_is_used() {
    let ui = harness();

    ui.space();
    assert_eq!(
        ui.took(),
        1,
        "space did not reach the root scope on a window nobody has touched"
    );

    // A press on a device header that does not leave the row: a selection.
    let first = effect_x(0);
    ui.drag(first, first + 20.0);
    ui.space();
    assert_eq!(
        ui.took(),
        2,
        "space stopped working after a device header was clicked"
    );

    // A real reorder, which rebuilds the rows the pointer was just over.
    ui.drag(first, effect_x(2));
    ui.space();
    assert_eq!(
        ui.took(),
        3,
        "space stopped working after a device was dragged. A reorder \
         rebuilds the rows the pointer was over, so if the focus had been \
         cleared by that, this is where it would show"
    );
}
