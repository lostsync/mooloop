//! A hosted plugin's own GUI, as the rest of mooloop sees it
//! (`docs/plans/plugin-hosting/11-plugin-gui-windows.md`).
//!
//! No plugin format's types are here. The window the GUI goes into is
//! somebody else's: on Linux a bare X11 window the platform layer creates
//! and hands over as a plain id. This module only says what a GUI
//! can be asked to do, in what order, and what it asks back.
//!
//! **Control thread only.** Every [`HostedGui`] call happens on the thread
//! that opened the instance, which is the pump's. An instance is not `Send`,
//! so the compiler keeps it there; the CLAP adapter checks the thread as
//! well and refuses with [`GuiError::WrongThread`], because a plugin's GUI
//! called from another thread is undefined behaviour in every format.
//!
//! **Order.** `create`, then (embedded) `set_parent` or (floating)
//! `suggest_title` and `set_transient`, then `show`. `destroy` undoes
//! `create` and is always allowed. Closing a GUI never touches the audio
//! processor.

use std::fmt;

/// A windowing API a GUI can be opened in.
///
/// Only X11 for now: it is what nearly every Linux plugin GUI embeds into,
/// under a Wayland session too, through XWayland. Cocoa and Win32 arrive
/// with the platforms that need them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GuiApi {
    /// X11, by window id.
    X11,
}

/// How a GUI is opened: in which API, and whether it floats in a window of
/// its own (`floating`) or is embedded in the host's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GuiConfig {
    /// The windowing API.
    pub api: GuiApi,
    /// In a window of the plugin's own, rather than embedded in the host's.
    pub floating: bool,
}

impl GuiConfig {
    /// Embedded in an X11 window of the host's: the configuration to try
    /// first.
    pub const X11_EMBEDDED: Self = Self {
        api: GuiApi::X11,
        floating: false,
    };
    /// Floating in a window of the plugin's own: the fallback for a plugin
    /// that does not embed.
    pub const X11_FLOATING: Self = Self {
        api: GuiApi::X11,
        floating: true,
    };
}

/// A GUI's size, in the API's own pixels (physical for X11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GuiSize {
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
}

/// A window of the host's, by its native id: an X11 window id for
/// [`GuiApi::X11`]. The caller keeps the window alive until the GUI in it
/// has been destroyed ([`HostedGui::destroy`]): the plugin's window is a
/// child of it, and destroying the parent first pulls the plugin's window
/// out from under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeWindow {
    /// The API `id` belongs to.
    pub api: GuiApi,
    /// The window's id in that API.
    pub id: u64,
}

impl NativeWindow {
    /// An X11 window, by the id the X server gave it.
    pub const fn x11(id: u64) -> Self {
        Self {
            api: GuiApi::X11,
            id,
        }
    }
}

/// Something a plugin asked of its GUI's window, from any thread, queued
/// until the pump drains it ([`HostedGui::take_requests`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiRequest {
    /// Resize the window to this. The plugin need not be told again.
    Resize(GuiSize),
    /// Show the window.
    Show,
    /// Hide the window.
    Hide,
    /// The plugin's resize hints changed: ask [`HostedGui::can_resize`]
    /// again.
    ResizeHintsChanged,
    /// The plugin's floating window was closed, or its connection to the
    /// display was lost.
    Closed {
        /// The plugin has already torn its GUI down and wants `destroy`
        /// called to acknowledge it.
        destroyed: bool,
    },
}

/// Why a GUI could not do what it was asked. Its `Display` text is what the
/// device's badge shows the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiError {
    /// The plugin has no GUI at all: its face is the only one.
    NoGui,
    /// The plugin does not open in this API, embedded or floating as asked.
    Unsupported(GuiConfig),
    /// The GUI is not open, and the call needs it to be.
    NotOpen,
    /// The GUI is already open.
    AlreadyOpen,
    /// Called off the control thread.
    WrongThread,
    /// The plugin refused, saying what it was asked to do.
    Refused(&'static str),
    /// There is no live plugin in that slot.
    Missing,
}

impl fmt::Display for GuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoGui => f.write_str("the plugin has no window of its own"),
            Self::Unsupported(config) => write!(
                f,
                "the plugin cannot open {} in {}",
                if config.floating { "a floating window" } else { "an embedded window" },
                match config.api {
                    GuiApi::X11 => "X11",
                }
            ),
            Self::NotOpen => f.write_str("the plugin's window is not open"),
            Self::AlreadyOpen => f.write_str("the plugin's window is already open"),
            Self::WrongThread => f.write_str("the plugin's window was asked for off the control thread"),
            Self::Refused(what) => write!(f, "the plugin's window refused to {what}"),
            Self::Missing => f.write_str("the plugin is not loaded"),
        }
    }
}

impl std::error::Error for GuiError {}

/// A hosted plugin's GUI, with no format's types in it. Reached through
/// [`crate::HostedInstance::gui`], on the control thread only.
pub trait HostedGui {
    /// Whether the plugin can open in `config`. Ask before [`Self::create`].
    fn is_api_supported(&mut self, config: GuiConfig) -> bool;

    /// The configuration the plugin would rather have, as a hint.
    fn preferred_api(&mut self) -> Option<GuiConfig>;

    /// The configuration the GUI is open in, or `None` while it is not.
    fn open_config(&self) -> Option<GuiConfig>;

    /// Whether the plugin's window is showing, as far as the host last told
    /// it.
    fn is_visible(&self) -> bool;

    /// Open the GUI. It shows nothing until [`Self::show`].
    fn create(&mut self, config: GuiConfig) -> Result<(), GuiError>;

    /// Tell the plugin the window's scale. A plugin that reads the scale from
    /// the system itself may refuse, which is not a failure to open.
    fn set_scale(&mut self, scale: f64) -> Result<(), GuiError>;

    /// The GUI's size now.
    fn size(&mut self) -> Option<GuiSize>;

    /// Whether the user may resize an embedded GUI's window.
    fn can_resize(&mut self) -> bool;

    /// The nearest size the plugin can take to `size`.
    fn adjust_size(&mut self, size: GuiSize) -> Option<GuiSize>;

    /// Resize an embedded GUI to `size`, which [`Self::adjust_size`] gave.
    fn set_size(&mut self, size: GuiSize) -> Result<(), GuiError>;

    /// Embed the GUI in `window`, which must outlive it (see [`NativeWindow`]).
    fn set_parent(&mut self, window: NativeWindow) -> Result<(), GuiError>;

    /// Keep a floating GUI above `window`, which must outlive it.
    fn set_transient(&mut self, window: NativeWindow) -> Result<(), GuiError>;

    /// The title a floating GUI's window should have.
    fn suggest_title(&mut self, title: &str);

    /// Show the GUI: once it is created, and embedded, once it has a parent.
    fn show(&mut self) -> Result<(), GuiError>;

    /// Hide the GUI without closing it.
    fn hide(&mut self) -> Result<(), GuiError>;

    /// Tear the GUI down. Allowed whatever state it is in, and a no-op when
    /// it is not open. After this the host's window may go.
    fn destroy(&mut self);

    /// Hand every request the plugin made of its window since the last call
    /// to `sink`. A request made while the GUI was not open is dropped.
    fn take_requests(&mut self, sink: &mut dyn FnMut(GuiRequest));
}

/// What one pass over a plugin's timers and file descriptors did
/// ([`crate::HostedInstance::service_io`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoActivity {
    /// Timers whose period had elapsed, each called once.
    pub timers_fired: u32,
    /// File descriptors that were ready, each called once.
    pub fds_fired: u32,
}

impl std::ops::AddAssign for IoActivity {
    fn add_assign(&mut self, other: Self) {
        self.timers_fired += other.timers_fired;
        self.fds_fired += other.fds_fired;
    }
}

/// What a plugin has registered with the host's event loop right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoRegistrations {
    /// Timers registered.
    pub timers: usize,
    /// File descriptors registered.
    pub fds: usize,
}
