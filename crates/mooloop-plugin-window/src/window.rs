//! What a plugin window is on every platform: its id, what it is made from,
//! what it reports and how it fails. Each platform's `PluginWindows`
//! ([`crate::x11`] off macOS, [`crate::cocoa`] on it) speaks these, so the
//! pump that drives them spells no `#[cfg]`.

use std::fmt;

use mooloop_plugin_host::{GuiSize, NativeWindow};

/// The largest width or height a plugin window is given. X11 cannot hold
/// more (its coordinates are signed 16-bit), and Cocoa is held to the same
/// rule so a plugin sees one. A plugin that asks for more gets this.
pub const MAX_EXTENT: u32 = 32_767;

/// A size as a plugin window holds it: at least 1, at most [`MAX_EXTENT`].
/// X11 refuses a zero-sized window outright, and a plugin that reports 0x0
/// before it has laid itself out is not rare.
pub fn extent(size: GuiSize) -> (u32, u32) {
    (
        size.width.clamp(1, MAX_EXTENT),
        size.height.clamp(1, MAX_EXTENT),
    )
}

/// Why a plugin window could not be made or used. Its text is what the
/// device's badge shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowError {
    /// `DISPLAY` is not set: there is no X server (or XWayland) to open a
    /// plugin window on. X11 only.
    NoDisplay,
    /// The X server named by `DISPLAY` could not be reached. X11 only.
    Connect(String),
    /// The connection to the X server failed after it was made. X11 only.
    Connection(String),
    /// The window is not one of these windows, or was destroyed.
    UnknownWindow(PluginWindowId),
    /// A parent of another windowing API than the plugin window's cannot be
    /// kept above: an X11 window needs an X11 parent, a Cocoa one a Cocoa
    /// view.
    ForeignParent(NativeWindow),
    /// AppKit was asked from a thread other than the process's main thread.
    /// Cocoa only.
    NotMainThread,
    /// A new Cocoa window came without a content view to embed into.
    NoContentView,
}

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDisplay => f.write_str(
                "there is no X server to open the plugin's window on (DISPLAY is not set)",
            ),
            Self::Connect(why) => write!(f, "could not reach the X server: {why}"),
            Self::Connection(why) => write!(f, "the connection to the X server failed: {why}"),
            Self::UnknownWindow(id) => write!(f, "no plugin window {:#x}", id.0),
            Self::ForeignParent(parent) => write!(
                f,
                "the plugin's window cannot belong to a {:?} window",
                parent.api
            ),
            Self::NotMainThread => {
                f.write_str("a plugin window can only be made on the main thread")
            }
            Self::NoContentView => {
                f.write_str("the plugin's window has no view for the plugin to draw in")
            }
        }
    }
}

impl std::error::Error for WindowError {}

/// A plugin window, by a number that names it among the open ones: the
/// window's X11 id under X11, a number mooloop gives it under Cocoa.
/// [`PluginWindowId::native`] is what goes to the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PluginWindowId(pub u32);

/// What the pump reads from a plugin window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginWindowEvent {
    /// The close button, or the window manager's close. The window is still
    /// there; the pump closes the GUI, then destroys it.
    CloseRequested,
    /// The window's size changed from outside -- the user dragged its edge,
    /// or a tiling compositor sized it -- in the units of
    /// [`PluginWindowSpec::size`]. Not reported for a size the pump set
    /// itself with `PluginWindows::resize`.
    Resized(GuiSize),
    /// Keyboard focus came to the window, or to the plugin's view inside it.
    FocusIn,
    /// Keyboard focus left the window and everything inside it. Hiding a
    /// focused window reports this too.
    FocusOut,
}

/// What a new plugin window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginWindowSpec<'a> {
    /// The plugin's and the track's name
    /// (`11-plugin-gui-windows.md`, policy 2).
    pub title: &'a str,
    /// The size the plugin's GUI asked for, in the platform API's units
    /// (`GuiApi::uses_logical_size`): physical pixels under X11, logical
    /// points under Cocoa.
    pub size: GuiSize,
    /// Whether the plugin can be resized (`HostedGui::can_resize`). When it
    /// cannot, the window is fixed at its size.
    pub resizable: bool,
}

/// The X11 id is the window, so the plugin is handed it as it is.
#[cfg(not(target_os = "macos"))]
impl PluginWindowId {
    /// The window to hand the plugin's `set_parent`: this X11 window.
    pub const fn native(self) -> NativeWindow {
        NativeWindow::x11(self.0 as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_no_window_can_hold_is_clamped() {
        assert_eq!(extent(GuiSize { width: 0, height: 0 }), (1, 1));
        assert_eq!(
            extent(GuiSize {
                width: 100_000,
                height: 480,
            }),
            (MAX_EXTENT, 480)
        );
    }

    #[test]
    fn every_reason_reads_as_a_sentence_for_the_badge() {
        assert!(WindowError::NoDisplay.to_string().contains("DISPLAY is not set"));
        assert!(WindowError::NotMainThread.to_string().contains("main thread"));
        let parent = NativeWindow::x11(7);
        assert!(WindowError::ForeignParent(parent).to_string().contains("X11"));
    }
}
