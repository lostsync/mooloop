//! A hosted plugin's own GUI, as the rest of mooloop sees it
//! (`docs/plans/plugin-hosting/11-plugin-gui-windows.md`).
//!
//! No plugin format's types are here. The window the GUI goes into is
//! somebody else's, handed over as a plain id: on Linux a bare X11 window
//! the platform layer creates, on macOS an `NSView` in a native window. This
//! module only says what a GUI can be asked to do, in what order, and what
//! it asks back.
//!
//! **Which API.** [`GuiApi::native`] is the one a plugin GUI embeds in on
//! the platform this was built for, and [`GuiConfig::native_order`] the
//! configurations to try in turn, so a caller names no platform itself.
//!
//! **Control thread only.** Every [`HostedGui`] call happens on the thread
//! that opened the instance, which is the pump's. An instance is not `Send`,
//! so the compiler keeps it there; the CLAP adapter checks the thread as
//! well and refuses with [`GuiError::WrongThread`], because a plugin's GUI
//! called from another thread is undefined behaviour in every format. On
//! macOS that thread must also be the process's main thread, the only one
//! AppKit may be called from; the pump runs on Slint's event loop, which is
//! there.
//!
//! **Event loops.** An X11 GUI usually runs its event loop on the host's
//! timers and file descriptors (`timer-support`, `posix-fd-support`, served
//! by [`crate::HostedInstance::service_io`] from the pump). A Cocoa GUI
//! needs neither: CLAP has it run on the main thread's run loop, which
//! AppKit already drives, so its own timers and sources fire without the
//! host. The host offers both extensions on every platform anyway, and a
//! plugin that registers a timer on macOS has it fired from the pump like
//! any other.
//!
//! **Order.** `create`, then (embedded) `set_parent` or (floating)
//! `suggest_title` and `set_transient`, then `show`. `destroy` undoes
//! `create` and is always allowed. Closing a GUI never touches the audio
//! processor.

use std::fmt;

/// A windowing API a GUI can be opened in.
///
/// X11 is what nearly every Linux plugin GUI embeds into, under a Wayland
/// session too, through XWayland. Cocoa is macOS's. Win32 arrives with the
/// platform that needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GuiApi {
    /// X11, by window id. Sizes are physical pixels.
    X11,
    /// Cocoa, by `NSView*`. Sizes are logical points.
    Cocoa,
}

impl GuiApi {
    /// The API a plugin GUI embeds in on the platform this was built for:
    /// Cocoa on macOS, X11 everywhere else (Windows has none yet, and asks
    /// for X11, which no Windows plugin offers).
    pub const fn native() -> Self {
        if cfg!(target_os = "macos") { Self::Cocoa } else { Self::X11 }
    }

    /// Whether a GUI's sizes in this API are logical (points, which the
    /// system scales) rather than physical pixels. Under such an API the
    /// plugin is not told a scale ([`HostedGui::set_scale`]).
    pub const fn uses_logical_size(self) -> bool {
        matches!(self, Self::Cocoa)
    }

    /// The API's name, as the device's badge shows it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::X11 => "X11",
            Self::Cocoa => "Cocoa",
        }
    }
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
    /// first on Linux.
    pub const X11_EMBEDDED: Self = Self::embedded(GuiApi::X11);
    /// Floating in an X11 window of the plugin's own: the fallback for a
    /// plugin that does not embed.
    pub const X11_FLOATING: Self = Self::floating(GuiApi::X11);
    /// Embedded in an `NSView` of the host's: the configuration to try first
    /// on macOS.
    pub const COCOA_EMBEDDED: Self = Self::embedded(GuiApi::Cocoa);
    /// Floating in a Cocoa window of the plugin's own: the fallback for a
    /// plugin that does not embed.
    pub const COCOA_FLOATING: Self = Self::floating(GuiApi::Cocoa);

    /// Embedded in a window of the host's, in `api`.
    pub const fn embedded(api: GuiApi) -> Self {
        Self { api, floating: false }
    }

    /// Floating in a window of the plugin's own, in `api`.
    pub const fn floating(api: GuiApi) -> Self {
        Self { api, floating: true }
    }

    /// Embedded, in [`GuiApi::native`]: what to try first on this platform.
    pub const fn native_embedded() -> Self {
        Self::embedded(GuiApi::native())
    }

    /// Floating, in [`GuiApi::native`]: the fallback on this platform.
    pub const fn native_floating() -> Self {
        Self::floating(GuiApi::native())
    }

    /// The configurations to offer a plugin on this platform, in the order
    /// to try them: embedded first, then floating.
    pub const fn native_order() -> [Self; 2] {
        [Self::native_embedded(), Self::native_floating()]
    }
}

/// A GUI's size, in the units of the API it is open in: **physical pixels**
/// for [`GuiApi::X11`], **logical points** for [`GuiApi::Cocoa`] (an
/// `NSView`'s own units, which the system scales;
/// [`GuiApi::uses_logical_size`]). A size passes between the plugin and the
/// window it is in unconverted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GuiSize {
    /// Across, in the API's units.
    pub width: u32,
    /// Down, in the API's units.
    pub height: u32,
}

/// A window of the host's, by its native id: an X11 window id for
/// [`GuiApi::X11`], an `NSView*`'s address for [`GuiApi::Cocoa`]. The caller
/// keeps the window alive until the GUI in it has been destroyed
/// ([`HostedGui::destroy`]): the plugin's window is a child of it, and
/// destroying the parent first pulls the plugin's window out from under it.
/// For Cocoa that is a memory-safety contract too, because the plugin
/// dereferences the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NativeWindow {
    /// The API `id` belongs to.
    pub api: GuiApi,
    /// The window's id in that API: for Cocoa, the pointer's address.
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

    /// A Cocoa view, by its `NSView*`. Only the address is kept; the view
    /// must outlive the GUI embedded in it (see [`NativeWindow`]).
    pub fn cocoa(ns_view: *mut std::ffi::c_void) -> Self {
        Self {
            api: GuiApi::Cocoa,
            id: ns_view.expose_provenance() as u64,
        }
    }

    /// A window of [`GuiApi::native`], by its id in that API (for Cocoa, the
    /// `NSView*`'s address): what a [`GuiConfig::native_order`]
    /// configuration is parented to or kept above.
    pub const fn native(id: u64) -> Self {
        Self {
            api: GuiApi::native(),
            id,
        }
    }

    /// The `NSView*` a Cocoa window was made from, or `None` for another
    /// API or an address this target's pointers cannot hold.
    pub fn as_ns_view(self) -> Option<*mut std::ffi::c_void> {
        if self.api != GuiApi::Cocoa {
            return None;
        }
        usize::try_from(self.id).ok().map(std::ptr::with_exposed_provenance_mut)
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
                config.api.name()
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
    /// the system itself may refuse, which is not a failure to open. Under an
    /// API with logical sizes ([`GuiApi::uses_logical_size`]) the scale is
    /// the system's: the plugin is not told, and this returns `Ok`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_native_api_is_cocoa_on_macos_and_x11_elsewhere() {
        let expected = if cfg!(target_os = "macos") { GuiApi::Cocoa } else { GuiApi::X11 };
        assert_eq!(GuiApi::native(), expected);
        assert_eq!(
            GuiConfig::native_order(),
            [GuiConfig::embedded(expected), GuiConfig::floating(expected)]
        );
        assert_eq!(NativeWindow::native(7), NativeWindow { api: expected, id: 7 });
    }

    #[test]
    fn the_named_configs_are_the_api_embedded_then_floating() {
        assert_eq!(GuiConfig::X11_EMBEDDED, GuiConfig { api: GuiApi::X11, floating: false });
        assert_eq!(GuiConfig::X11_FLOATING, GuiConfig { api: GuiApi::X11, floating: true });
        assert_eq!(GuiConfig::COCOA_EMBEDDED, GuiConfig { api: GuiApi::Cocoa, floating: false });
        assert_eq!(GuiConfig::COCOA_FLOATING, GuiConfig { api: GuiApi::Cocoa, floating: true });
    }

    #[test]
    fn only_cocoa_sizes_are_logical() {
        assert!(GuiApi::Cocoa.uses_logical_size());
        assert!(!GuiApi::X11.uses_logical_size());
    }

    #[test]
    fn a_cocoa_window_carries_its_views_address_and_gives_it_back() {
        let mut view = 0u8;
        let ns_view = (&raw mut view).cast::<std::ffi::c_void>();
        let window = NativeWindow::cocoa(ns_view);
        assert_eq!(window.api, GuiApi::Cocoa);
        assert_eq!(window.id, ns_view.addr() as u64);
        assert_eq!(window.as_ns_view(), Some(ns_view));
        assert_eq!(NativeWindow::x11(window.id).as_ns_view(), None);
    }

    #[test]
    fn the_badge_names_the_api_it_could_not_open_in() {
        assert_eq!(
            GuiError::Unsupported(GuiConfig::COCOA_EMBEDDED).to_string(),
            "the plugin cannot open an embedded window in Cocoa"
        );
        assert_eq!(
            GuiError::Unsupported(GuiConfig::X11_FLOATING).to_string(),
            "the plugin cannot open a floating window in X11"
        );
    }
}
