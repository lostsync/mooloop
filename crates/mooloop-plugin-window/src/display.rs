//! Which display server mooloop runs on, and the "Run under XWayland"
//! setting (`11-plugin-gui-windows.md`, *The XWayland setting*; MOO-301).
//!
//! winit chooses X11 or Wayland once per process, when its event loop is
//! built, and allows one event loop per process. So the choice is made before
//! Slint's backend starts, from the setting and the environment, and it is the
//! whole process's.
//!
//! **How it is forced.** With the setting on, the UI builds Slint's winit
//! backend with an event loop builder on which winit's `with_x11()` has been
//! called (`slint::BackendSelector::with_winit_event_loop_builder`, the
//! `unstable-winit-030` feature). That sets winit's forced backend, which it
//! consults before `WAYLAND_DISPLAY`, and it is the mechanism Slint's own
//! winit backend uses to put itself on XWayland under WSL. It was chosen over
//! clearing `WAYLAND_DISPLAY` because it changes nothing else: the process
//! environment -- which every thread, the audio client's libraries, a plugin
//! loaded in-process and every child process read -- is left as it was, and
//! nothing mutates the environment of a process that may already have
//! threads (`00-status.md`, step 11).
//!
//! A forced X11 event loop that cannot connect is not recoverable: winit
//! refuses to build a second event loop even after the first one failed. So
//! the plan forces X11 only when `DISPLAY` is set, and the UI probes the X
//! server ([`crate::x11::probe`]) before asking for it.

use std::ffi::OsString;
use std::fmt;

use mooloop_plugin_host::NativeWindow;
pub use raw_window_handle;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

/// The display server a window is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplayBackend {
    /// X11: a real X server, or XWayland under a Wayland session.
    X11,
    /// A native Wayland client.
    Wayland,
}

impl DisplayBackend {
    /// Whether a plugin's X11 window can be made transient for the main
    /// window (`WM_TRANSIENT_FOR`, and the plugin's `set_transient`). Only
    /// when both are X11 windows: Wayland has no protocol for one client's
    /// surface to belong to another's, so under Wayland the plugin windows
    /// are hidden while mooloop is not focused instead (policy 2).
    pub fn can_set_transient(self) -> bool {
        matches!(self, Self::X11)
    }

    /// What the log calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::X11 => "X11",
            Self::Wayland => "Wayland",
        }
    }

    /// The backend a Slint window's display handle belongs to, or `None` for
    /// one that is neither (another platform, or the headless testing
    /// backend).
    pub fn of_display(handle: RawDisplayHandle) -> Option<Self> {
        match handle {
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => Some(Self::X11),
            RawDisplayHandle::Wayland(_) => Some(Self::Wayland),
            _ => None,
        }
    }
}

impl fmt::Display for DisplayBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A window's X11 id, as a parent a plugin window can be transient for, or
/// `None` when the window is not an X11 one.
// `c_ulong` is 64 bits here and 32 on some targets, so the conversion that
// is a no-op on this one is not on every one.
#[allow(clippy::useless_conversion)]
pub fn x11_window_of(handle: RawWindowHandle) -> Option<NativeWindow> {
    match handle {
        RawWindowHandle::Xlib(window) => Some(NativeWindow::x11(u64::from(window.window))),
        RawWindowHandle::Xcb(window) => Some(NativeWindow::x11(u64::from(window.window.get()))),
        _ => None,
    }
}

/// What the process asks winit for, decided before the first window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendPlan {
    /// Build the event loop with winit's `with_x11()`. Only ever true when a
    /// Wayland session was found, the setting is on, and `DISPLAY` is set.
    pub force_x11: bool,
    /// The backend winit will pick under this plan, by winit's own rule;
    /// `None` when the environment names no display at all. A guess until
    /// the window exists: [`DisplayBackend::of_display`] on the real window
    /// is the answer.
    pub expected: Option<DisplayBackend>,
    /// Why the setting was not honoured, for the log. `None` when it was, or
    /// was off.
    pub note: Option<String>,
}

impl BackendPlan {
    /// This plan, with X11 no longer forced because `why`: the X server
    /// could not be reached, or Slint refused the backend.
    pub fn without_x11(self, why: impl Into<String>) -> Self {
        if !self.force_x11 {
            return self;
        }
        Self {
            force_x11: false,
            expected: Some(DisplayBackend::Wayland),
            note: Some(why.into()),
        }
    }
}

/// The plan for this process's own environment.
pub fn plan_backend(run_under_xwayland: bool) -> BackendPlan {
    plan_backend_in(run_under_xwayland, |name| std::env::var_os(name))
}

/// The plan given `run_under_xwayland` and an environment, so the rule can
/// be tested without changing the process's.
///
/// winit's rule, which this mirrors: a non-empty `WAYLAND_DISPLAY` or
/// `WAYLAND_SOCKET` means Wayland, else a non-empty `DISPLAY` means X11.
/// The setting only changes the first case, and only when there is an X
/// server (`DISPLAY`) to go to.
pub fn plan_backend_in(
    run_under_xwayland: bool,
    env: impl Fn(&str) -> Option<OsString>,
) -> BackendPlan {
    let set = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    let wayland = set("WAYLAND_DISPLAY") || set("WAYLAND_SOCKET");
    let x11 = set("DISPLAY");
    let natural = if wayland {
        Some(DisplayBackend::Wayland)
    } else if x11 {
        Some(DisplayBackend::X11)
    } else {
        None
    };
    if !run_under_xwayland || !wayland {
        // Off, or already on X11 (or on nothing): nothing to force.
        return BackendPlan {
            force_x11: false,
            expected: natural,
            note: None,
        };
    }
    if !x11 {
        return BackendPlan {
            force_x11: false,
            expected: natural,
            note: Some(
                "Run under XWayland is on, but there is no X server to run on (DISPLAY is \
                 not set), so mooloop runs on Wayland"
                    .to_owned(),
            ),
        };
    }
    BackendPlan {
        force_x11: true,
        expected: Some(DisplayBackend::X11),
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raw_window_handle::{
        WaylandDisplayHandle, XcbDisplayHandle, XcbWindowHandle, XlibDisplayHandle,
        XlibWindowHandle,
    };
    use std::num::NonZeroU32;
    use std::ptr::NonNull;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    const WAYLAND_AND_X: &[(&str, &str)] = &[("WAYLAND_DISPLAY", "wayland-1"), ("DISPLAY", ":0")];

    #[test]
    fn the_setting_off_changes_nothing() {
        for vars in [
            WAYLAND_AND_X,
            &[("WAYLAND_DISPLAY", "wayland-1")][..],
            &[("DISPLAY", ":0")][..],
            &[][..],
        ] {
            let plan = plan_backend_in(false, env(vars));
            assert!(!plan.force_x11, "{vars:?}");
            assert_eq!(plan.note, None, "{vars:?}");
        }
    }

    #[test]
    fn without_the_setting_a_wayland_session_stays_on_wayland() {
        assert_eq!(
            plan_backend_in(false, env(WAYLAND_AND_X)),
            BackendPlan {
                force_x11: false,
                expected: Some(DisplayBackend::Wayland),
                note: None,
            }
        );
    }

    #[test]
    fn the_setting_on_a_wayland_session_with_xwayland_forces_x11() {
        assert_eq!(
            plan_backend_in(true, env(WAYLAND_AND_X)),
            BackendPlan {
                force_x11: true,
                expected: Some(DisplayBackend::X11),
                note: None,
            }
        );
    }

    #[test]
    fn a_wayland_socket_counts_as_a_wayland_session() {
        let plan = plan_backend_in(true, env(&[("WAYLAND_SOCKET", "5"), ("DISPLAY", ":0")]));
        assert!(plan.force_x11);
    }

    #[test]
    fn empty_variables_count_as_unset_the_way_winit_counts_them() {
        let plan = plan_backend_in(true, env(&[("WAYLAND_DISPLAY", ""), ("DISPLAY", ":0")]));
        assert_eq!(plan.expected, Some(DisplayBackend::X11));
        assert!(!plan.force_x11, "an X11 session needs nothing forced");

        let plan = plan_backend_in(true, env(&[("WAYLAND_DISPLAY", "wayland-1"), ("DISPLAY", "")]));
        assert!(!plan.force_x11, "an empty DISPLAY is no X server");
    }

    #[test]
    fn the_setting_with_no_x_server_stays_on_wayland_and_says_why() {
        let plan = plan_backend_in(true, env(&[("WAYLAND_DISPLAY", "wayland-1")]));
        assert!(!plan.force_x11);
        assert_eq!(plan.expected, Some(DisplayBackend::Wayland));
        assert!(plan.note.as_deref().is_some_and(|note| note.contains("DISPLAY")));
    }

    #[test]
    fn the_setting_on_an_x11_session_needs_nothing_forced() {
        assert_eq!(
            plan_backend_in(true, env(&[("DISPLAY", ":0")])),
            BackendPlan {
                force_x11: false,
                expected: Some(DisplayBackend::X11),
                note: None,
            }
        );
    }

    #[test]
    fn no_display_at_all_expects_nothing() {
        for setting in [false, true] {
            let plan = plan_backend_in(setting, env(&[]));
            assert_eq!(plan.expected, None);
            assert!(!plan.force_x11);
        }
    }

    #[test]
    fn a_failed_probe_falls_back_to_wayland_and_keeps_the_reason() {
        let plan = plan_backend_in(true, env(WAYLAND_AND_X)).without_x11("no X server");
        assert_eq!(
            plan,
            BackendPlan {
                force_x11: false,
                expected: Some(DisplayBackend::Wayland),
                note: Some("no X server".to_owned()),
            }
        );
    }

    #[test]
    fn falling_back_from_a_plan_that_forced_nothing_leaves_it_alone() {
        let plan = plan_backend_in(false, env(WAYLAND_AND_X));
        assert_eq!(plan.clone().without_x11("irrelevant"), plan);
    }

    #[test]
    fn only_x11_can_set_transient() {
        assert!(DisplayBackend::X11.can_set_transient());
        assert!(!DisplayBackend::Wayland.can_set_transient());
    }

    #[test]
    fn a_display_handle_says_which_backend_it_is() {
        assert_eq!(
            DisplayBackend::of_display(XlibDisplayHandle::new(None, 0).into()),
            Some(DisplayBackend::X11)
        );
        assert_eq!(
            DisplayBackend::of_display(XcbDisplayHandle::new(None, 0).into()),
            Some(DisplayBackend::X11)
        );
        let mut marker = 0u8;
        let surface = NonNull::from(&mut marker).cast();
        assert_eq!(
            DisplayBackend::of_display(WaylandDisplayHandle::new(surface).into()),
            Some(DisplayBackend::Wayland)
        );
        assert_eq!(
            DisplayBackend::of_display(RawDisplayHandle::Drm(
                raw_window_handle::DrmDisplayHandle::new(3)
            )),
            None
        );
    }

    #[test]
    fn an_x11_window_handle_gives_its_id() {
        assert_eq!(
            x11_window_of(XlibWindowHandle::new(0x0120_0007).into()),
            Some(NativeWindow::x11(0x0120_0007))
        );
        assert_eq!(
            x11_window_of(XcbWindowHandle::new(NonZeroU32::new(0x0340_0002).unwrap()).into()),
            Some(NativeWindow::x11(0x0340_0002))
        );
        let mut marker = 0u8;
        let surface = NonNull::from(&mut marker).cast();
        assert_eq!(
            x11_window_of(raw_window_handle::WaylandWindowHandle::new(surface).into()),
            None
        );
    }
}
