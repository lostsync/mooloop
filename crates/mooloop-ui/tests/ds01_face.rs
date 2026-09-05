//! The DS-01 face's one property that no snapshot can see.
//!
//! Every other device face declares a property per parameter, so Rust sets
//! each control's value directly. DS-01 has ninety-two and declares none of
//! them: the face takes arrays indexed by descriptor id, and a control's value
//! is a *binding* onto a model row.
//!
//! Slint drops a binding at the first assignment to the property it feeds, and
//! assigning to its own value is exactly what a knob does while it is dragged.
//! Nothing makes that visible at the time — the edit handler writes the same
//! value straight back — so it only shows up the next time the patch changes
//! from somewhere else, which is what loading a factory preset over a face
//! somebody has been turning does. `ParameterKnob.controlled` is the fix, and
//! this is the test that says so: it failed before that flag existed.
//!
//! The test closes the same loop the application closes. A controlled knob
//! reports its change and does not write it, so the owner has to put the new
//! value into the model — `on_ds01_value_changed` does that through
//! `touch_ds01_param`, and `apply_edit` below does it here. Without that half,
//! the knob would not move at all, which is why the drag is asserted to have
//! changed the picture before anything else is concluded from it.
//!
//! The face is driven through `Ds01DeviceDragHarness` with real pointer
//! events at known coordinates, for the reason `first_click.rs` records: the
//! `ElementHandle` search API needs a build with `SLINT_EMIT_DEBUG_INFO=1`,
//! which this workspace only does under the `mcp` feature.

use mooloop_core::ds01;
use mooloop_core::{Ds01Params, ParamCurve};
use mooloop_ui::Ds01DeviceDragHarness;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, Model, ModelRc, SharedString, VecModel};
use std::rc::Rc;

/// Four rack units of `DeviceRackMetrics` minus the two rails, by
/// `face-height` minus the device header. The same size the rack gives it.
const FACE_WIDTH: f32 = 884.0;
const FACE_HEIGHT: f32 = 240.0;

/// The centre of the BODY page's DAMPING dial, which is the second control in
/// the second row of the leftmost module. Fixed rather than searched; if the
/// layout moves, the "the drag reached the knob" assertion fails rather than
/// the test quietly proving nothing.
const DAMPING_DIAL: (f32, f32) = (99.0, 161.0);

/// The BODY page.
const BODY_PAGE: i32 = 3;

/// `KnobStack`'s dial, which is what a comparison has to be about.
const DIAL_SIZE: u32 = 34;

fn init_software_backend() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .ok();
}

/// The per-id arrays the face reads, built the same way `refresh_ds01` builds
/// them: normalized values, defaults, step counts and polarity straight off
/// the descriptor table.
fn push_patch(harness: &Ds01DeviceDragHarness, params: &Ds01Params) {
    let len = ds01::DESCRIPTORS
        .iter()
        .map(|descriptor| descriptor.id as usize + 1)
        .max()
        .unwrap_or(0);
    let mut values = vec![0.0_f32; len];
    let mut defaults = vec![0.0_f32; len];
    let mut steps = vec![0_i32; len];
    let mut bipolars = vec![false; len];
    let mut texts = vec![SharedString::new(); len];
    for descriptor in ds01::DESCRIPTORS.iter() {
        let index = descriptor.id as usize;
        let natural = ds01::get(params, descriptor.id).unwrap_or(descriptor.default);
        values[index] = descriptor.to_normalized(natural);
        defaults[index] = descriptor.to_normalized(descriptor.default);
        bipolars[index] = descriptor.min < 0.0;
        texts[index] = SharedString::from(format!("{natural:.2}"));
        if let ParamCurve::Stepped(count) = descriptor.curve {
            steps[index] = i32::from(count);
        }
    }
    harness.set_values(ModelRc::from(Rc::new(VecModel::from(values))));
    harness.set_defaults(ModelRc::from(Rc::new(VecModel::from(defaults))));
    harness.set_step_counts(ModelRc::from(Rc::new(VecModel::from(steps))));
    harness.set_bipolars(ModelRc::from(Rc::new(VecModel::from(bipolars))));
    harness.set_value_texts(ModelRc::from(Rc::new(VecModel::from(texts))));
    harness.set_modulation_depths(ModelRc::from(Rc::new(VecModel::from(vec![0.0_f32; len]))));
    harness.set_modulation_allowed(ModelRc::from(Rc::new(VecModel::from(vec![true; len]))));
    harness.set_modulation_offsets(ModelRc::from(Rc::new(VecModel::from(vec![0.0_f32; len]))));
    harness.set_modulation_route_counts(ModelRc::from(Rc::new(VecModel::from(vec![0_i32; len]))));
    harness.set_body_decay_fraction(0.5);
}

fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    let at = |p: (f32, f32)| LogicalPosition::new(p.0, p.1);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button: PointerEventButton::Left,
    });
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        window.dispatch_event(WindowEvent::PointerMoved {
            position: at((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)),
        });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button: PointerEventButton::Left,
    });
}

/// The pixels of one control, so a comparison can be about that control.
///
/// A whole-frame comparison cannot answer this question: a drag leaves the
/// knob focused and the pointer hovering it, so two renders of the same patch
/// differ for reasons that have nothing to do with the patch.
fn dial_pixels(
    snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    centre: (f32, f32),
    size: u32,
) -> Vec<u8> {
    let bytes = snapshot.as_bytes();
    let stride = snapshot.width() as usize * 4;
    let left = (centre.0 as u32).saturating_sub(size / 2) as usize;
    let top = (centre.1 as u32).saturating_sub(size / 2) as usize;
    let mut out = Vec::with_capacity(size as usize * size as usize * 4);
    for row in top..top + size as usize {
        let from = row * stride + left * 4;
        out.extend_from_slice(&bytes[from..from + size as usize * 4]);
    }
    out
}

/// A control that has been turned must still follow the patch afterwards.
///
/// Drag the knob first, so whatever the drag does to the binding has already
/// happened. Then push two patches that differ only in that knob's parameter:
/// the dial has to draw differently for each. With the binding dropped it
/// draws the value it was dragged to for both, and this fails.
///
/// The dial rather than the frame, and the dial rather than the whole
/// `KnobStack`: the value field beside it is a separate binding that the knob
/// never writes, so it would keep following the model and hide the defect.
#[test]
fn a_dragged_knob_still_follows_the_patch() {
    init_software_backend();
    let harness = Ds01DeviceDragHarness::new().unwrap();
    harness
        .window()
        .set_size(LogicalSize::new(FACE_WIDTH, FACE_HEIGHT));
    harness.set_page(BODY_PAGE);

    // The owner's half of a controlled knob: the edit lands in the model, and
    // the model is what the control reads. This is `touch_ds01_param` with
    // the engine and the formatting left out.
    let weak = harness.as_weak();
    harness.on_value_changed(move |id, normalized| {
        let Some(harness) = weak.upgrade() else { return };
        let values = harness.get_values();
        let index = id.max(0) as usize;
        if index < values.row_count() {
            values.set_row_data(index, normalized);
        }
    });

    let damped = |damping: f32| Ds01Params {
        body_damping: damping,
        ..Ds01Params::default()
    };
    let dial = |harness: &Ds01DeviceDragHarness| {
        dial_pixels(
            &harness.window().take_snapshot().unwrap(),
            DAMPING_DIAL,
            DIAL_SIZE,
        )
    };

    push_patch(&harness, &damped(0.3));
    let untouched = dial(&harness);

    drag(
        harness.window(),
        DAMPING_DIAL,
        (DAMPING_DIAL.0, DAMPING_DIAL.1 - 40.0),
    );
    assert_ne!(
        untouched,
        dial(&harness),
        "the drag did not reach the dial, so the rest of this test proves nothing"
    );

    push_patch(&harness, &damped(0.85));
    let high = dial(&harness);
    push_patch(&harness, &damped(0.05));
    let low = dial(&harness);
    assert_ne!(
        high, low,
        "the dial drew the same thing for two different patches, so it stopped \
         following the model once it had been dragged"
    );
}


/// The stepped controls that read as a count are knobs with a step, not chips
/// to click through. Tune has ninety-seven positions and Bits sixteen; a
/// cycling chip would be ninety-six clicks to get back where you started.
///
/// This is a property of the descriptor table rather than of the face, and it
/// is the reason the face can decide chip-or-knob without a second list: a
/// stepped parameter with more than a handful of positions is a number.
#[test]
fn the_long_stepped_controls_are_countable() {
    for (id, least) in [
        (ds01::PARAM_TUNE, 97),
        (ds01::PARAM_BITS, 16),
        (ds01::PARAM_BURST_REPEATS, 8),
    ] {
        let descriptor = ds01::descriptor(id).expect("a descriptor for every id");
        let ParamCurve::Stepped(count) = descriptor.curve else {
            panic!("{} is not stepped", descriptor.name);
        };
        assert_eq!(i32::from(count), least, "{}", descriptor.name);
    }
}
