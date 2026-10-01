//! Which display server the process runs on: the "Run under XWayland"
//! setting applied before Slint's backend starts, and read back from the
//! main window afterwards (plugin-hosting step 11, MOO-301). Platform &
//! Release's, like the rest of startup.
//!
//! The decision is `mooloop_plugin_window::display`'s, which tests it
//! without a display; this is only where it meets Slint. Nothing here
//! touches audio or MIDI, and the environment only as
//! `mooloop_plugin_window::display::apply_x11_scale` allows (MOO-343).

use mooloop_core::log_warn;
use mooloop_plugin_window::NativeWindow;
use mooloop_plugin_window::display::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
pub use mooloop_plugin_window::display::{BackendPlan, DisplayBackend};

use crate::settings::UiSettings;

/// Choose Slint's display backend from the saved "Run under XWayland"
/// setting. Call once, before the first window is made: winit fixes the
/// backend when it builds its event loop, and builds one per process.
///
/// With the setting off, or on anything but a Wayland session with an X
/// server beside it, this changes nothing and Slint chooses as it always
/// has. With it on, the X server is probed first, because an event loop
/// forced onto an X server that is not there cannot be rebuilt on Wayland;
/// if the probe fails the process stays on Wayland and the log says why.
pub fn select_display_backend() -> BackendPlan {
    let setting = UiSettings::load_or_default().plugins.run_under_xwayland;
    let plan = mooloop_plugin_window::display::plan_backend(setting);
    let plan = if plan.force_x11 {
        force_x11(plan)
    } else {
        plan
    };
    if let Some(note) = &plan.note {
        log_warn!("display", "{note}");
    }
    plan
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn force_x11(plan: BackendPlan) -> BackendPlan {
    // Here, not at the top: this is its only use, and an import no Apple
    // build reads fails macOS clippy under `-D warnings` (MOO-323).
    use mooloop_core::log_info;
    use slint::winit_030::{winit, SlintEvent};
    use winit::platform::x11::EventLoopBuilderExtX11;

    use mooloop_plugin_window::display::{apply_x11_scale, x11_scale_in};

    let server = match mooloop_plugin_window::x11::probe() {
        Ok(server) => server,
        Err(error) => {
            return plan.without_x11(format!(
                "Run under XWayland is on, but {error}, so mooloop runs on Wayland"
            ))
        }
    };
    // Before the event loop exists: winit reads the scale when it first
    // asks for the monitors.
    let scale = x11_scale_in(|name| std::env::var_os(name), server.xft_dpi);
    match apply_x11_scale(scale) {
        Ok(()) => log_info!("display", "the X11 window is drawn at {scale}"),
        Err(why) => log_warn!("display", "{why}"),
    }
    let mut builder = winit::event_loop::EventLoop::<SlintEvent>::with_user_event();
    builder.with_x11();
    match slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_event_loop_builder(builder)
        .select()
    {
        Ok(()) => {
            log_info!("display", "running under XWayland, as the setting asks");
            plan
        }
        Err(error) => plan.without_x11(format!(
            "Run under XWayland is on, but Slint refused X11 ({error}); mooloop may not \
             start until the setting is turned off"
        )),
    }
}

#[cfg(not(all(unix, not(target_vendor = "apple"))))]
fn force_x11(plan: BackendPlan) -> BackendPlan {
    plan.without_x11("Run under XWayland only applies on Linux and the BSDs")
}

/// The display server `window` is on, once it has been shown; `None`
/// before that, and under the headless testing backend. What the pump asks
/// before a plugin window is made transient for the main window
/// ([`DisplayBackend::can_set_transient`]).
pub fn window_display_backend(window: &slint::Window) -> Option<DisplayBackend> {
    let handle = window.window_handle();
    let display = handle.display_handle().ok()?;
    DisplayBackend::of_display(display.as_raw())
}

/// `window` as the parent a plugin window is kept above
/// (`PluginWindows::set_transient_for`) and a floating plugin GUI's
/// `set_transient`, in the platform's windowing API: its X11 id on X11, its
/// `NSView*` on macOS. `None` on Wayland, and before the window is shown.
pub fn window_x11_parent(window: &slint::Window) -> Option<NativeWindow> {
    let handle = window.window_handle();
    let raw = handle.window_handle().ok()?;
    mooloop_plugin_window::display::native_window_of(raw.as_raw())
}
