//! Headless render of the Preferences dialog's Audio page, so a driver
//! control surface change can be checked visually without the live app.

use mooloop_ui::MainWindow;
use mooloop_ui::OutputTargetRow as MainOutputTargetRow;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize, ModelRc, SharedString, VecModel};
use std::rc::Rc;

fn write_snapshot(snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, variable: &str) {
    if let Ok(path) = std::env::var(variable) {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(path, ppm).unwrap();
    }
}

/// Center of the compact "Audio" vertical tab at 800x600, measured from the
/// rendered dialog. The root layout distributes its body height, so use the
/// visible tab rather than deriving this from nominal header dimensions.
const AUDIO_NAV_ITEM: (f32, f32) = (78.0, 132.0);

fn click_at(window: &slint::Window, p: (f32, f32)) {
    let position = LogicalPosition::new(p.0, p.1);
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

slint::slint! {
    import { JackControlSurface, OutputTargetRow } from "../ui/audio-preferences.slint";

    export component JackControlSurfaceHarness inherits Window {
        width: 340px;
        height: 400px;
        in property <[OutputTargetRow]> targets;
        JackControlSurface {
            output-targets: root.targets;
            buffer-size-index: 3;
            sample-rate-text: "48000 Hz — set by the JACK server";
            auto-reconnect: true;
        }
    }
}

#[test]
fn render_preferences_audio_snapshots() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(SharedString::from("software")),
        },
    )))
    .expect("initialize headless renderer");

    // The dialog page, at the size it actually renders at. Its content can
    // be taller than the fixed dialog body and scrolls, so this only shows
    // what fits above the fold.
    let ui = MainWindow::new().unwrap();
    ui.window().set_size(LogicalSize::new(800.0, 600.0));
    ui.set_preferences_open(true);
    ui.set_preferences_audio_output_targets(ModelRc::from(Rc::new(VecModel::from(vec![
        MainOutputTargetRow {
            client: SharedString::from("system"),
            port_l: SharedString::from("system:playback_1"),
            port_r: SharedString::from("system:playback_2"),
            selected: true,
        },
        MainOutputTargetRow {
            client: SharedString::from("Carla"),
            port_l: SharedString::from("Carla:audio-in1"),
            port_r: SharedString::from("Carla:audio-in2"),
            selected: false,
        },
    ]))));
    ui.set_preferences_audio_buffer_size_index(2);
    ui.set_preferences_audio_sample_rate_text(SharedString::from(
        "48000 Hz — set by the JACK server",
    ));
    ui.set_preferences_audio_auto_reconnect(true);

    // The Audio page only becomes visible after clicking its nav item; `page`
    // is private to `PreferencesDialog` and not exposed to Rust.
    click_at(ui.window(), AUDIO_NAV_ITEM);

    let snapshot = ui.window().take_snapshot().expect("headless snapshot");
    write_snapshot(&snapshot, "MOOLOOP_PREFERENCES_AUDIO_SNAPSHOT");
    drop(ui);

    // The surface alone, unclipped, to check the controls below that fold
    // too: buffer size, sample rate, and auto-reconnect.
    let harness = JackControlSurfaceHarness::new().unwrap();
    harness.set_targets(ModelRc::from(Rc::new(VecModel::from(vec![
        OutputTargetRow {
            client: SharedString::from("system"),
            port_l: SharedString::from("system:playback_1"),
            port_r: SharedString::from("system:playback_2"),
            selected: true,
        },
        OutputTargetRow {
            client: SharedString::from("Carla"),
            port_l: SharedString::from("Carla:audio-in1"),
            port_r: SharedString::from("Carla:audio-in2"),
            selected: false,
        },
    ]))));
    let surface_snapshot = harness.window().take_snapshot().expect("headless snapshot");
    write_snapshot(&surface_snapshot, "MOOLOOP_JACK_CONTROL_SURFACE_SNAPSHOT");
}
