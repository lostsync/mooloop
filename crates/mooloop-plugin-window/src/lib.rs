//! The OS windows a hosted plugin's GUI goes into, and which display server
//! the process runs on (`docs/plans/plugin-hosting/11-plugin-gui-windows.md`).
//!
//! - [`PluginWindows`]: a bare window of mooloop's own whose native id goes
//!   to the plugin's `set_parent` ([`mooloop_plugin_host::HostedGui`]), in
//!   the platform's windowing API (`GuiApi::native`). Off macOS it is an X11
//!   window ([`x11`]), under every session, because almost every Linux
//!   plugin GUI is an X11 program; under Wayland it runs through XWayland
//!   like the plugin does. On macOS it is an `NSPanel` (`cocoa`), whose
//!   content view the plugin's Cocoa GUI embeds into. Both speak the types
//!   in [`window`], with the same calls, so the pump spells no `#[cfg]`.
//! - [`display`]: the "Run under XWayland" decision -- which display backend
//!   the process asks winit for, given the environment and the setting -- and
//!   reading back which one it ended up on, so the pump knows whether a
//!   plugin window can be made transient for the main window.
//!
//! Nothing here runs on the audio thread, and nothing here touches audio or
//! MIDI.

#[cfg(target_os = "macos")]
pub mod cocoa;
pub mod display;
pub mod window;
#[cfg(not(target_os = "macos"))]
pub mod x11;

#[cfg(target_os = "macos")]
pub use cocoa::PluginWindows;
pub use display::{BackendPlan, DisplayBackend};
/// The host's neutral window types, as a plugin window speaks them.
pub use mooloop_plugin_host::{GuiSize, NativeWindow};
pub use window::{PluginWindowEvent, PluginWindowId, PluginWindowSpec, WindowError};
#[cfg(not(target_os = "macos"))]
pub use x11::PluginWindows;
