//! Regression tests for dragging handles whose own position is bound to the
//! value they control: the EQ band points and the filter's cutoff handle.
//!
//! Their `TouchArea` re-centres under the pointer every time the drag
//! callback updates the underlying value, so `mouse-x`/`mouse-y` are not a
//! stable, parent-relative coordinate system - see the fix on
//! eq-device.slint and filter-device.slint (the same pitfall documented on
//! controls.slint's `MixerFader`). The old code mapped mouse position
//! directly and produced a recurrence that flips sign each event instead of
//! converging, which shows up here as a non-monotonic value sequence during
//! a drag that moves steadily in one direction - invoking the `*-changed`
//! callbacks directly would not catch this, since the bug is entirely in how
//! the `TouchArea`'s own coordinate system interacts with hit-testing.
//!
//! The reverb's capture dot was the third such handle. The FDN hall that
//! replaced the generated-room convolution reverb has no plan view and no
//! self-positioning control, only shared `ParameterKnob`s, so that case is
//! gone rather than untested.

use mooloop_ui::{EqDeviceDragHarness, FilterDeviceDragHarness};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// The EQ fits two rack units.
const EQ_FACE_WIDTH: f32 = 220.0 * 2.0 + 4.0;
const FACE_HEIGHT: f32 = 268.0;
const HEADER_HEIGHT: f32 = 28.0;

/// Initialize the testing backend with the software renderer, the only one
/// that implements `take_snapshot`, so the drag tests can also compare
/// rendered pixels before and after a drag.
fn init_software_backend() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(slint::SharedString::from("software")),
        },
    )))
    .ok();
}

/// Write the snapshot to a PPM file when `variable` names a path, for
/// before/after visual inspection of a drag.
fn write_snapshot(snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, variable: &str) {
    if let Ok(path) = std::env::var(variable) {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }
}

/// Press at `from`, travel to `to` in small increments the way a real
/// pointer would, then release.
fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32), steps: usize) {
    let at = |p: (f32, f32)| LogicalPosition::new(p.0, p.1);
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button: PointerEventButton::Left,
    });
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let p = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        window.dispatch_event(WindowEvent::PointerMoved { position: at(p) });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button: PointerEventButton::Left,
    });
}

/// A drag moving steadily in one direction must produce a value sequence
/// that moves the same direction throughout - a coordinate system that
/// isn't actually parent-relative shows up here as zig-zag instead.
fn assert_monotonic(values: &[f32], increasing: bool, what: &str) {
    assert!(values.len() >= 2, "{what}: drag produced no updates");
    for pair in values.windows(2) {
        if increasing {
            assert!(
                pair[1] >= pair[0] - 1e-4,
                "{what}: not monotonically increasing: {values:?}"
            );
        } else {
            assert!(
                pair[1] <= pair[0] + 1e-4,
                "{what}: not monotonically decreasing: {values:?}"
            );
        }
    }
}

#[test]
fn eq_point_drag_tracks_the_pointer() {
    init_software_backend();
    let ui = EqDeviceDragHarness::new().unwrap();
    ui.window()
        .set_size(LogicalSize::new(EQ_FACE_WIDTH, FACE_HEIGHT));

    // Band 0 active at the plot centre; the other six disabled so their
    // (irrelevant) hit areas can't shadow band 0's.
    let mut band_data = vec![0.0f32; 35];
    band_data[0] = 0.5; // frequency
    band_data[1] = 0.5; // gain
    band_data[2] = 0.707; // q
    band_data[3] = 1.0; // enabled
    band_data[4] = 0.0; // bell
    ui.set_band_data(ModelRc::from(Rc::new(VecModel::from(band_data))));

    let frequencies = Rc::new(RefCell::new(Vec::new()));
    let gains = Rc::new(RefCell::new(Vec::new()));
    ui.on_frequency_changed({
        let frequencies = frequencies.clone();
        move |v| frequencies.borrow_mut().push(v)
    });
    ui.on_gain_changed({
        let gains = gains.clone();
        move |v| gains.borrow_mut().push(v)
    });

    // Plot geometry from eq-device.slint's EqResponseDisplay: its face has
    // 6px padding (34px on top, for the header strip) and begins immediately
    // under that inset; FFT now lives in the selector strip below the plot.
    let plot_x = 6.0;
    let plot_y = HEADER_HEIGHT + 6.0;
    let plot_w = EQ_FACE_WIDTH - 12.0;
    let plot_h = 126.0;
    let start = (plot_x + 0.5 * plot_w, plot_y + 0.5 * plot_h);
    let dx = 100.0;
    let dy = 30.0;
    let end = (start.0 + dx, start.1 + dy);

    drag(ui.window(), start, end, 20);

    assert_monotonic(&frequencies.borrow(), true, "frequency");
    assert_monotonic(&gains.borrow(), false, "gain");
    let final_freq = *frequencies.borrow().last().unwrap();
    let final_gain = *gains.borrow().last().unwrap();
    assert!(
        (final_freq - (0.5 + dx / plot_w)).abs() < 0.02,
        "frequency should track the pointer 1:1: got {final_freq}"
    );
    assert!(
        (final_gain - (0.5 - dy / plot_h)).abs() < 0.02,
        "gain should track the pointer 1:1: got {final_gain}"
    );
}

#[test]
fn coincident_eq_points_are_separately_selectable() {
    init_software_backend();
    let ui = EqDeviceDragHarness::new().unwrap();
    ui.window()
        .set_size(LogicalSize::new(EQ_FACE_WIDTH, FACE_HEIGHT));

    let mut band_data = vec![0.0f32; 35];
    for index in 0..2 {
        let base = index * 5;
        band_data[base] = 0.5;
        band_data[base + 1] = 0.5;
        band_data[base + 2] = 0.707;
        band_data[base + 3] = 1.0;
    }
    ui.set_band_data(ModelRc::from(Rc::new(VecModel::from(band_data))));

    let selected = Rc::new(RefCell::new(Vec::new()));
    ui.on_target_changed({
        let selected = selected.clone();
        move |v| selected.borrow_mut().push(v)
    });

    let center = (
        6.0 + 0.5 * (EQ_FACE_WIDTH - 12.0),
        HEADER_HEIGHT + 6.0 + 0.5 * 126.0,
    );
    // Coincident points are spread by 18px, so the two hit targets have
    // distinct centres at ±9px rather than sharing a z-ordered area.
    for x_offset in [-9.0, 9.0] {
        let at = LogicalPosition::new(center.0 + x_offset, center.1);
        ui.window().dispatch_event(WindowEvent::PointerMoved { position: at });
        ui.window().dispatch_event(WindowEvent::PointerPressed {
            position: at,
            button: PointerEventButton::Left,
        });
        ui.window().dispatch_event(WindowEvent::PointerReleased {
            position: at,
            button: PointerEventButton::Left,
        });
    }
    assert_eq!(&*selected.borrow(), &[0.0, 0.125]);
}

#[test]
fn filter_point_drag_and_wheel_update_only_the_bound_parameters() {
    init_software_backend();
    let ui = FilterDeviceDragHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(220.0, FACE_HEIGHT));
    ui.set_cutoff(0.5);
    ui.set_resonance(0.0);

    let before = ui.window().take_snapshot().unwrap();
    write_snapshot(&before, "MOOLOOP_FILTER_BEFORE_DRAG_SNAPSHOT");

    let cutoffs = Rc::new(RefCell::new(Vec::new()));
    let resonances = Rc::new(RefCell::new(Vec::new()));
    ui.on_cutoff_changed({
        let cutoffs = cutoffs.clone();
        move |v| cutoffs.borrow_mut().push(v)
    });
    ui.on_resonance_changed({
        let resonances = resonances.clone();
        move |v| resonances.borrow_mut().push(v)
    });

    // The Filter face's response display starts beneath the 28px device
    // header, 6px layout padding, 26px mode selector, 4px spacing, and 4px
    // inset, and spans 208x96px. At cutoff 0.5 the point sits where the
    // curve crosses its own cutoff: magnitude 1/sqrt(1 + damping^2) = 0.5
    // for damping 2, i.e. plot-y 0.6 of the height (see plot-y-at in
    // device-displays.slint).
    let start = (110.0, HEADER_HEIGHT + 6.0 + 26.0 + 4.0 + 4.0 + 0.6 * 96.0);
    let end = (140.0, start.1 - 20.0);
    drag(ui.window(), start, end, 12);

    assert_monotonic(&cutoffs.borrow(), true, "filter cutoff");
    assert_monotonic(&resonances.borrow(), true, "filter resonance");
    let cutoff = *cutoffs.borrow().last().unwrap();
    let resonance = *resonances.borrow().last().unwrap();
    assert!(
        (cutoff - 0.65).abs() < 0.02,
        "cutoff should track horizontal drag: got {cutoff}"
    );
    assert!(
        (resonance - 20.0 / 96.0).abs() < 0.03,
        "resonance should track vertical drag: got {resonance}"
    );

    // Write the dragged values back into the harness the way the real
    // application does via the DSP parameter, so the handle follows the
    // values and the wheel acts on the post-drag resonance.
    ui.set_cutoff(cutoff);
    ui.set_resonance(resonance);
    let cutoff_updates = cutoffs.borrow().len();
    // Hover the handle's post-drag position: centered on the cutoff's x;
    // the curve crosses its own cutoff at magnitude 1/sqrt(1 + damping^2)
    // (damping now 2 - resonance * 1.9), so plot-y is just under 0.6.
    let damping = 2.0 - resonance * 1.9;
    let magnitude = 1.0 / (1.0 + damping * damping).sqrt();
    let hover = LogicalPosition::new(
        10.0 + cutoff * 208.0,
        HEADER_HEIGHT + 40.0 + (0.25 + 0.7 * (1.0 - magnitude)) * 96.0,
    );
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: hover });
    ui.window().dispatch_event(WindowEvent::PointerScrolled {
        position: hover,
        delta_x: 0.0,
        // In Slint's convention this backend hands wheel-down a negative
        // delta-y; the handler subtracts it, so this raises resonance.
        delta_y: -120.0,
    });
    assert_eq!(
        cutoffs.borrow().len(),
        cutoff_updates,
        "wheel input must not change cutoff"
    );
    assert!(
        *resonances.borrow().last().unwrap() > resonance,
        "wheel input should increase resonance"
    );

    ui.set_cutoff(cutoff);
    ui.set_resonance(*resonances.borrow().last().unwrap());
    let after = ui.window().take_snapshot().unwrap();
    write_snapshot(&after, "MOOLOOP_FILTER_AFTER_DRAG_SNAPSHOT");
    assert_ne!(
        before.as_bytes(),
        after.as_bytes(),
        "the filter point and curve should move together after a drag"
    );
}
