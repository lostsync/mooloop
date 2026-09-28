//! The OS windows a hosted plugin's GUI goes into, and which display server
//! the process runs on (`docs/plans/plugin-hosting/11-plugin-gui-windows.md`,
//! MOO-301). Platform & Release's, in `docs/TEAMS.md`.
//!
//! Two halves:
//!
//! - [`x11`]: a bare top-level X11 window of mooloop's own, made with
//!   `x11rb`, whose id goes to the plugin's `set_parent`
//!   ([`mooloop_plugin_host::HostedGui`]). It is an X11 window under every
//!   session, because almost every Linux plugin GUI is an X11 program; under
//!   Wayland it runs through XWayland like the plugin does.
//! - [`display`]: the "Run under XWayland" decision -- which display backend
//!   the process asks winit for, given the environment and the setting -- and
//!   reading back which one it ended up on, so the pump knows whether a
//!   plugin window can be made transient for the main window.
//!
//! The pump that drives both is Interface's (MOO-302). Nothing here runs on
//! the audio thread, and nothing here touches audio or MIDI.

pub mod display;
pub mod x11;

pub use display::{BackendPlan, DisplayBackend};
/// The host's neutral window types, as a plugin window speaks them.
pub use mooloop_plugin_host::{GuiSize, NativeWindow};
pub use x11::{PluginWindowEvent, PluginWindowId, PluginWindowSpec, PluginWindows, WindowError};
