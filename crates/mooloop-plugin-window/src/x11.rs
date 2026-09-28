//! A bare X11 window of mooloop's own for a plugin GUI to embed into
//! (`11-plugin-gui-windows.md`, policies 2 and 3; MOO-301).
//!
//! [`PluginWindows`] is one connection to the X server and every plugin
//! window made on it. The pump (Interface's, MOO-302) owns one, opens it the
//! first time a GUI opens, and drives it on the control thread:
//!
//! 1. [`PluginWindows::create`] makes an unmapped top-level window, titled,
//!    typed as a dialog, and with fixed size hints when the plugin cannot
//!    resize -- the three things that make a tiling compositor float it.
//! 2. Its [`PluginWindowId::native`] goes to the plugin's `set_parent`.
//! 3. Where the main window is an X11 one too (an X11 session, or the "Run
//!    under XWayland" setting), [`PluginWindows::set_transient_for`] keeps it
//!    above the main window. Set it before the first `show`, as ICCCM asks.
//! 4. [`PluginWindows::show`] maps it; [`PluginWindows::hide`] unmaps it,
//!    which is how a Wayland session hides plugin windows while mooloop is
//!    not focused.
//! 5. [`PluginWindows::drain_events`], every pump tick, never waits.
//! 6. [`PluginWindows::destroy`] **after** the plugin's GUI `destroy`, never
//!    before: the plugin's window is a child of this one.
//!
//! Dropping [`PluginWindows`] closes the connection, and the X server
//! destroys every window still on it, so the same order holds for it: every
//! GUI in them destroyed first.
//!
//! Everything that can be decided without a server -- the properties, the
//! size hints, which X events mean what -- is a pure function here and
//! tested as one. Only the tests that talk to a real server are `#[ignore]`d.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;

use mooloop_core::log_warn;
use mooloop_plugin_host::{GuiApi, GuiSize, NativeWindow};
use x11rb::connection::Connection;
use x11rb::errors::{ConnectError, ConnectionError, ReplyError, ReplyOrIdError};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConfigureNotifyEvent, ConfigureWindowAux,
    ConnectionExt as _, CreateWindowAux, EventMask, FocusInEvent, NotifyDetail, NotifyMode,
    PropMode, Window, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// The largest width or height X11 can give a window (its coordinates are
/// signed 16-bit). A plugin that asks for more gets this.
pub const MAX_EXTENT: u32 = 32_767;

/// `WM_CLASS`, instance then class. The class is the application's, so a
/// window rule written for mooloop can match its plugin windows by class and
/// tell them apart by instance.
pub const WM_CLASS: (&str, &str) = ("mooloop-plugin", "mooloop");

x11rb::atom_manager! {
    /// The atoms a plugin window names that the core protocol does not
    /// predefine.
    pub Atoms: AtomsCookie {
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
        UTF8_STRING,
        _NET_WM_NAME,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DIALOG,
    }
}

/// Why a plugin window could not be made or used. Its text is what the
/// device's badge (step 08) shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowError {
    /// `DISPLAY` is not set: there is no X server (or XWayland) to open a
    /// plugin window on.
    NoDisplay,
    /// The X server named by `DISPLAY` could not be reached.
    Connect(String),
    /// The connection to the X server failed after it was made.
    Connection(String),
    /// The window is not one of this connection's, or was destroyed.
    UnknownWindow(PluginWindowId),
    /// A parent that is not an X11 window cannot be an X11 window's
    /// transient-for.
    NotX11(NativeWindow),
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
            Self::NotX11(parent) => write!(
                f,
                "the plugin's window cannot belong to a {:?} window",
                parent.api
            ),
        }
    }
}

impl std::error::Error for WindowError {}

impl From<ConnectionError> for WindowError {
    fn from(error: ConnectionError) -> Self {
        Self::Connection(error.to_string())
    }
}

impl From<ReplyError> for WindowError {
    fn from(error: ReplyError) -> Self {
        Self::Connection(error.to_string())
    }
}

impl From<ReplyOrIdError> for WindowError {
    fn from(error: ReplyOrIdError) -> Self {
        Self::Connection(error.to_string())
    }
}

impl From<ConnectError> for WindowError {
    fn from(error: ConnectError) -> Self {
        Self::Connect(error.to_string())
    }
}

/// A plugin window, by its X11 id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PluginWindowId(pub u32);

impl PluginWindowId {
    /// The id to hand the plugin's `set_parent`.
    pub const fn native(self) -> NativeWindow {
        NativeWindow::x11(self.0 as u64)
    }
}

/// What the pump reads from a plugin window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginWindowEvent {
    /// The close button, or the window manager's close: `WM_DELETE_WINDOW`.
    /// The window is still there; the pump closes the GUI, then destroys it.
    CloseRequested,
    /// The window's size changed from outside -- the user dragged its edge,
    /// or a tiling compositor sized it. Not reported for a size the pump set
    /// itself with [`PluginWindows::resize`].
    Resized(GuiSize),
    /// Keyboard focus came to the window, or to the plugin's window inside
    /// it.
    FocusIn,
    /// Keyboard focus left the window and everything inside it. Hiding a
    /// focused window with [`PluginWindows::hide`] reports this too.
    FocusOut,
}

/// What a new plugin window is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginWindowSpec<'a> {
    /// The plugin's and the track's name, as the spec asks
    /// (`11-plugin-gui-windows.md`, policy 2).
    pub title: &'a str,
    /// The size the plugin's GUI asked for, in physical pixels.
    pub size: GuiSize,
    /// Whether the plugin can be resized (`HostedGui::can_resize`). When it
    /// cannot, the size hints fix the window at its size.
    pub resizable: bool,
}

/// `WM_NORMAL_HINTS`: ICCCM's `WM_SIZE_HINTS`, eighteen `CARD32`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizeHints {
    pub flags: u32,
    pub size: (u32, u32),
    pub min: (u32, u32),
    pub max: (u32, u32),
}

impl SizeHints {
    /// The program chose the size (`PSize`).
    pub const P_SIZE: u32 = 1 << 3;
    /// `min` is set (`PMinSize`).
    pub const P_MIN_SIZE: u32 = 1 << 4;
    /// `max` is set (`PMaxSize`).
    pub const P_MAX_SIZE: u32 = 1 << 5;

    /// The hints for a window of `size`. A plugin that cannot resize gets a
    /// minimum and maximum both equal to its size, which is also what makes
    /// a tiling compositor (Hyprland, sway, i3) float the window instead of
    /// tiling it. One that can resize says only its size.
    pub fn for_window(size: GuiSize, resizable: bool) -> Self {
        let size = extent(size);
        if resizable {
            Self {
                flags: Self::P_SIZE,
                size,
                min: (0, 0),
                max: (0, 0),
            }
        } else {
            Self {
                flags: Self::P_SIZE | Self::P_MIN_SIZE | Self::P_MAX_SIZE,
                size,
                min: size,
                max: size,
            }
        }
    }

    /// The property's eighteen words, in ICCCM's order: flags; x, y, width,
    /// height (obsolete, but written, as Xlib does); min; max; resize
    /// increments; min and max aspect; base size; gravity.
    pub fn encode(&self) -> [u32; 18] {
        let mut words = [0u32; 18];
        words[0] = self.flags;
        words[3] = self.size.0;
        words[4] = self.size.1;
        words[5] = self.min.0;
        words[6] = self.min.1;
        words[7] = self.max.0;
        words[8] = self.max.1;
        words
    }
}

/// A window's size as X11 can hold it: at least 1, at most [`MAX_EXTENT`].
/// X11 refuses a zero-sized window outright, and a plugin that reports 0x0
/// before it has laid itself out is not rare.
pub fn extent(size: GuiSize) -> (u32, u32) {
    (
        size.width.clamp(1, MAX_EXTENT),
        size.height.clamp(1, MAX_EXTENT),
    )
}

/// `title` as `WM_NAME`'s `STRING` type holds it: ISO 8859-1, anything
/// outside it as `?`. `_NET_WM_NAME` carries the real UTF-8; this is for
/// window managers that read only the old property.
pub fn latin1(title: &str) -> Vec<u8> {
    title
        .chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

/// A property's value, in the format X11 stores it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PropertyValue {
    Bytes(Vec<u8>),
    Words(Vec<u32>),
}

/// One property to write on a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    pub name: u32,
    pub kind: u32,
    pub value: PropertyValue,
}

/// The title properties: `WM_NAME` (Latin-1) and `_NET_WM_NAME` (UTF-8).
pub fn title_properties(atoms: &Atoms, title: &str) -> [Property; 2] {
    [
        Property {
            name: AtomEnum::WM_NAME.into(),
            kind: AtomEnum::STRING.into(),
            value: PropertyValue::Bytes(latin1(title)),
        },
        Property {
            name: atoms._NET_WM_NAME,
            kind: atoms.UTF8_STRING,
            value: PropertyValue::Bytes(title.as_bytes().to_vec()),
        },
    ]
}

/// `WM_NORMAL_HINTS` for a window of `size`.
pub fn size_hints_property(size: GuiSize, resizable: bool) -> Property {
    Property {
        name: AtomEnum::WM_NORMAL_HINTS.into(),
        kind: AtomEnum::WM_SIZE_HINTS.into(),
        value: PropertyValue::Words(SizeHints::for_window(size, resizable).encode().to_vec()),
    }
}

/// Every property a new plugin window is made with: its title, `WM_CLASS`,
/// `WM_PROTOCOLS` naming `WM_DELETE_WINDOW` (so the close button asks
/// rather than kills the connection), `_NET_WM_WINDOW_TYPE_DIALOG`, and
/// its size hints.
pub fn window_properties(atoms: &Atoms, spec: &PluginWindowSpec<'_>) -> Vec<Property> {
    let mut class = Vec::new();
    for part in [WM_CLASS.0, WM_CLASS.1] {
        class.extend_from_slice(part.as_bytes());
        class.push(0);
    }
    let mut properties = title_properties(atoms, spec.title).to_vec();
    properties.extend([
        Property {
            name: AtomEnum::WM_CLASS.into(),
            kind: AtomEnum::STRING.into(),
            value: PropertyValue::Bytes(class),
        },
        Property {
            name: atoms.WM_PROTOCOLS,
            kind: AtomEnum::ATOM.into(),
            value: PropertyValue::Words(vec![atoms.WM_DELETE_WINDOW]),
        },
        Property {
            name: atoms._NET_WM_WINDOW_TYPE,
            kind: AtomEnum::ATOM.into(),
            value: PropertyValue::Words(vec![atoms._NET_WM_WINDOW_TYPE_DIALOG]),
        },
        size_hints_property(spec.size, spec.resizable),
    ]);
    properties
}

/// `WM_TRANSIENT_FOR` naming `parent`, which must be an X11 window.
pub fn transient_for_property(parent: NativeWindow) -> Result<Property, WindowError> {
    let id = match parent.api {
        GuiApi::X11 => u32::try_from(parent.id).map_err(|_| WindowError::NotX11(parent))?,
        // `GuiApi` is non-exhaustive: Cocoa and Win32 arrive later.
        _ => return Err(WindowError::NotX11(parent)),
    };
    Ok(Property {
        name: AtomEnum::WM_TRANSIENT_FOR.into(),
        kind: AtomEnum::WINDOW.into(),
        value: PropertyValue::Words(vec![id]),
    })
}

/// Whether a focus change is one the pump should hear about.
///
/// A plugin's GUI is a child of the window, so when it takes the keyboard
/// the window hears `FocusOut` with detail `Inferior`: focus went *inside*
/// it, not away. That one is dropped, and so are the `Grab` and `Ungrab`
/// modes a keyboard grab (a window manager's Alt-Tab, say) reports around
/// itself, and the pointer-focus details. What is left is focus arriving at
/// the window or anything in it, and leaving all of it.
pub fn focus_change(entered: bool, mode: NotifyMode, detail: NotifyDetail) -> Option<PluginWindowEvent> {
    if mode == NotifyMode::GRAB || mode == NotifyMode::UNGRAB {
        return None;
    }
    if detail == NotifyDetail::POINTER
        || detail == NotifyDetail::POINTER_ROOT
        || detail == NotifyDetail::NONE
    {
        return None;
    }
    if !entered && detail == NotifyDetail::INFERIOR {
        return None;
    }
    Some(if entered {
        PluginWindowEvent::FocusIn
    } else {
        PluginWindowEvent::FocusOut
    })
}

/// What the pump knows about one of its windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WindowState {
    size: (u32, u32),
    resizable: bool,
}

/// What an X event means for the windows in `windows`, if anything. A size
/// is reported only when it differs from the last one known, and becomes the
/// one known.
fn translate(
    event: &Event,
    atoms: &Atoms,
    windows: &mut HashMap<Window, WindowState>,
) -> Option<(PluginWindowId, PluginWindowEvent)> {
    match event {
        Event::ClientMessage(ClientMessageEvent {
            window,
            type_,
            format: 32,
            data,
            ..
        }) if *type_ == atoms.WM_PROTOCOLS
            && data.as_data32()[0] == atoms.WM_DELETE_WINDOW
            && windows.contains_key(window) =>
        {
            Some((PluginWindowId(*window), PluginWindowEvent::CloseRequested))
        }
        Event::ConfigureNotify(ConfigureNotifyEvent {
            window,
            width,
            height,
            ..
        }) => {
            let state = windows.get_mut(window)?;
            let size = (u32::from(*width), u32::from(*height));
            if size == state.size {
                return None;
            }
            state.size = size;
            Some((
                PluginWindowId(*window),
                PluginWindowEvent::Resized(GuiSize {
                    width: size.0,
                    height: size.1,
                }),
            ))
        }
        Event::FocusIn(FocusInEvent {
            event, mode, detail, ..
        }) if windows.contains_key(event) => {
            focus_change(true, *mode, *detail).map(|change| (PluginWindowId(*event), change))
        }
        Event::FocusOut(FocusInEvent {
            event, mode, detail, ..
        }) if windows.contains_key(event) => {
            focus_change(false, *mode, *detail).map(|change| (PluginWindowId(*event), change))
        }
        _ => None,
    }
}

/// The X server `DISPLAY` names, or [`WindowError::NoDisplay`].
pub fn display_name_in(env: impl Fn(&str) -> Option<OsString>) -> Result<String, WindowError> {
    env("DISPLAY")
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
        .ok_or(WindowError::NoDisplay)
}

/// Whether the X server `DISPLAY` names can be reached: connect, and hang
/// up. Opens no window. What the "Run under XWayland" setting asks before it
/// commits the process to X11, which cannot be undone once asked.
pub fn probe() -> Result<(), WindowError> {
    let display = display_name_in(|name| std::env::var_os(name))?;
    let (connection, _) = x11rb::connect(Some(&display))?;
    drop(connection);
    Ok(())
}

/// One connection to the X server, and the plugin windows made on it.
/// Control thread only.
pub struct PluginWindows {
    connection: RustConnection,
    screen: usize,
    atoms: Atoms,
    windows: HashMap<Window, WindowState>,
}

impl fmt::Debug for PluginWindows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginWindows")
            .field("screen", &self.screen)
            .field("windows", &self.windows.len())
            .finish_non_exhaustive()
    }
}

impl PluginWindows {
    /// Connect to the X server `DISPLAY` names. Under a Wayland session that
    /// is XWayland. With no `DISPLAY`, [`WindowError::NoDisplay`].
    pub fn connect() -> Result<Self, WindowError> {
        let display = display_name_in(|name| std::env::var_os(name))?;
        let (connection, screen) = x11rb::connect(Some(&display))?;
        let atoms = Atoms::new(&connection)?.reply()?;
        Ok(Self {
            connection,
            screen,
            atoms,
            windows: HashMap::new(),
        })
    }

    /// Make a plugin window, unmapped. Show it with [`Self::show`] once the
    /// plugin's GUI is in it.
    pub fn create(&mut self, spec: &PluginWindowSpec<'_>) -> Result<PluginWindowId, WindowError> {
        let screen = &self.connection.setup().roots[self.screen];
        let (root, black) = (screen.root, screen.black_pixel);
        let (width, height) = extent(spec.size);
        let window = self.connection.generate_id()?;
        let aux = CreateWindowAux::new()
            .background_pixel(black)
            .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::FOCUS_CHANGE);
        self.connection.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            width as u16,
            height as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &aux,
        )?;
        for property in window_properties(&self.atoms, spec) {
            self.write(window, &property)?;
        }
        self.connection.flush()?;
        self.windows.insert(
            window,
            WindowState {
                size: (width, height),
                resizable: spec.resizable,
            },
        );
        Ok(PluginWindowId(window))
    }

    /// Retitle a window: the plugin or the track was renamed.
    pub fn set_title(&mut self, id: PluginWindowId, title: &str) -> Result<(), WindowError> {
        self.state(id)?;
        for property in title_properties(&self.atoms, title) {
            self.write(id.0, &property)?;
        }
        self.connection.flush()?;
        Ok(())
    }

    /// Resize a window to `size`, the plugin's own request or its answer to
    /// `adjust_size`. A window that cannot be resized has its fixed hints
    /// moved to the new size first, or the window manager would refuse it.
    /// Reports no [`PluginWindowEvent::Resized`] for this size.
    pub fn resize(&mut self, id: PluginWindowId, size: GuiSize) -> Result<(), WindowError> {
        let state = self.state(id)?;
        let (width, height) = extent(size);
        if !state.resizable {
            self.write(id.0, &size_hints_property(size, false))?;
        }
        self.connection
            .configure_window(id.0, &ConfigureWindowAux::new().width(width).height(height))?;
        self.connection.flush()?;
        if let Some(state) = self.windows.get_mut(&id.0) {
            state.size = (width, height);
        }
        Ok(())
    }

    /// The plugin's resize hints changed (`GuiRequest::ResizeHintsChanged`):
    /// fix the window at its size, or free it.
    pub fn set_resizable(&mut self, id: PluginWindowId, resizable: bool) -> Result<(), WindowError> {
        let state = self.state(id)?;
        let size = GuiSize {
            width: state.size.0,
            height: state.size.1,
        };
        self.write(id.0, &size_hints_property(size, resizable))?;
        self.connection.flush()?;
        if let Some(state) = self.windows.get_mut(&id.0) {
            state.resizable = resizable;
        }
        Ok(())
    }

    /// Keep the window above `parent`, the main window, or stop (`None`).
    /// `parent` must be an X11 window: only an X11 session or the "Run under
    /// XWayland" setting has one ([`crate::DisplayBackend::can_set_transient`]).
    pub fn set_transient_for(
        &mut self,
        id: PluginWindowId,
        parent: Option<NativeWindow>,
    ) -> Result<(), WindowError> {
        self.state(id)?;
        match parent {
            Some(parent) => {
                let property = transient_for_property(parent)?;
                self.write(id.0, &property)?;
            }
            None => {
                self.connection
                    .delete_property(id.0, AtomEnum::WM_TRANSIENT_FOR.into())?;
            }
        }
        self.connection.flush()?;
        Ok(())
    }

    /// Map the window.
    pub fn show(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
        self.state(id)?;
        self.connection.map_window(id.0)?;
        self.connection.flush()?;
        Ok(())
    }

    /// Unmap the window. The plugin's GUI stays in it, and comes back with
    /// it on [`Self::show`].
    pub fn hide(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
        self.state(id)?;
        self.connection.unmap_window(id.0)?;
        self.connection.flush()?;
        Ok(())
    }

    /// Destroy the window. Only after the plugin's GUI in it has been
    /// destroyed.
    pub fn destroy(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
        if self.windows.remove(&id.0).is_none() {
            return Err(WindowError::UnknownWindow(id));
        }
        self.connection.destroy_window(id.0)?;
        self.connection.flush()?;
        Ok(())
    }

    /// The window's size as last set or reported.
    pub fn size(&self, id: PluginWindowId) -> Option<GuiSize> {
        self.windows.get(&id.0).map(|state| GuiSize {
            width: state.size.0,
            height: state.size.1,
        })
    }

    /// Every window still open on this connection.
    pub fn windows(&self) -> impl Iterator<Item = PluginWindowId> + '_ {
        self.windows.keys().map(|&window| PluginWindowId(window))
    }

    /// Append everything that happened to the windows since the last call to
    /// `out`, without waiting. An X protocol error is logged and skipped: it
    /// answers a request already made, and the window it names is either
    /// gone or about to be. A failed connection is an error, and every
    /// window on it is gone with it.
    pub fn drain_events(
        &mut self,
        out: &mut Vec<(PluginWindowId, PluginWindowEvent)>,
    ) -> Result<(), WindowError> {
        while let Some(event) = self.connection.poll_for_event()? {
            if let Event::Error(error) = &event {
                log_warn!("plugin-window", "X error: {error:?}");
                continue;
            }
            if let Some(change) = translate(&event, &self.atoms, &mut self.windows) {
                out.push(change);
            }
        }
        Ok(())
    }

    fn state(&self, id: PluginWindowId) -> Result<WindowState, WindowError> {
        self.windows
            .get(&id.0)
            .copied()
            .ok_or(WindowError::UnknownWindow(id))
    }

    fn write(&self, window: Window, property: &Property) -> Result<(), WindowError> {
        match &property.value {
            PropertyValue::Bytes(bytes) => {
                self.connection.change_property8(
                    PropMode::REPLACE,
                    window,
                    property.name,
                    property.kind,
                    bytes,
                )?;
            }
            PropertyValue::Words(words) => {
                self.connection.change_property32(
                    PropMode::REPLACE,
                    window,
                    property.name,
                    property.kind,
                    words,
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xproto::{FocusOutEvent, CONFIGURE_NOTIFY_EVENT, FOCUS_IN_EVENT, FOCUS_OUT_EVENT};

    /// Atoms as a server might intern them: any distinct numbers above the
    /// predefined ones.
    fn atoms() -> Atoms {
        Atoms {
            WM_PROTOCOLS: 300,
            WM_DELETE_WINDOW: 301,
            UTF8_STRING: 302,
            _NET_WM_NAME: 303,
            _NET_WM_WINDOW_TYPE: 304,
            _NET_WM_WINDOW_TYPE_DIALOG: 305,
        }
    }

    const fn size(width: u32, height: u32) -> GuiSize {
        GuiSize { width, height }
    }

    fn find(properties: &[Property], name: impl Into<u32>) -> &Property {
        let name = name.into();
        properties
            .iter()
            .find(|property| property.name == name)
            .unwrap_or_else(|| panic!("no property {name}"))
    }

    fn spec(resizable: bool) -> PluginWindowSpec<'static> {
        PluginWindowSpec {
            title: "Surge XT — Lead",
            size: size(800, 600),
            resizable,
        }
    }

    #[test]
    fn a_fixed_size_window_has_equal_min_and_max_hints() {
        let hints = SizeHints::for_window(size(800, 600), false).encode();
        assert_eq!(
            hints,
            [
                SizeHints::P_SIZE | SizeHints::P_MIN_SIZE | SizeHints::P_MAX_SIZE,
                0,
                0,
                800,
                600,
                800,
                600,
                800,
                600,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ]
        );
        // ICCCM's bit values, not just self-consistent ones.
        assert_eq!(hints[0], 8 | 16 | 32);
    }

    #[test]
    fn a_resizable_window_says_only_its_size() {
        let hints = SizeHints::for_window(size(640, 480), true).encode();
        assert_eq!(hints[0], SizeHints::P_SIZE);
        assert_eq!((hints[3], hints[4]), (640, 480));
        assert!(hints[5..].iter().all(|&word| word == 0));
    }

    #[test]
    fn a_size_x11_cannot_hold_is_clamped() {
        assert_eq!(extent(size(0, 0)), (1, 1));
        assert_eq!(extent(size(100_000, 70_000)), (MAX_EXTENT, MAX_EXTENT));
        assert_eq!(extent(size(1024, 768)), (1024, 768));
        let hints = SizeHints::for_window(size(0, 90_000), false).encode();
        assert_eq!((hints[5], hints[6], hints[7], hints[8]), (1, MAX_EXTENT, 1, MAX_EXTENT));
    }

    #[test]
    fn the_title_is_utf8_in_net_wm_name_and_latin1_in_wm_name() {
        let atoms = atoms();
        let properties = window_properties(&atoms, &spec(false));
        let net = find(&properties, atoms._NET_WM_NAME);
        assert_eq!(net.kind, atoms.UTF8_STRING);
        assert_eq!(net.value, PropertyValue::Bytes("Surge XT — Lead".as_bytes().to_vec()));
        let old = find(&properties, AtomEnum::WM_NAME);
        assert_eq!(old.kind, u32::from(AtomEnum::STRING));
        assert_eq!(old.value, PropertyValue::Bytes(b"Surge XT ? Lead".to_vec()));
    }

    #[test]
    fn latin1_keeps_what_it_can_hold() {
        assert_eq!(latin1("Café"), vec![b'C', b'a', b'f', 0xE9]);
        assert_eq!(latin1("鍵"), b"?".to_vec());
    }

    #[test]
    fn the_window_asks_to_be_told_about_a_close() {
        let atoms = atoms();
        let properties = window_properties(&atoms, &spec(true));
        let protocols = find(&properties, atoms.WM_PROTOCOLS);
        assert_eq!(protocols.kind, u32::from(AtomEnum::ATOM));
        assert_eq!(protocols.value, PropertyValue::Words(vec![atoms.WM_DELETE_WINDOW]));
    }

    #[test]
    fn the_window_is_a_dialog() {
        let atoms = atoms();
        let properties = window_properties(&atoms, &spec(true));
        let kind = find(&properties, atoms._NET_WM_WINDOW_TYPE);
        assert_eq!(kind.kind, u32::from(AtomEnum::ATOM));
        assert_eq!(kind.value, PropertyValue::Words(vec![atoms._NET_WM_WINDOW_TYPE_DIALOG]));
    }

    #[test]
    fn the_window_carries_its_class_and_its_size_hints() {
        let atoms = atoms();
        let properties = window_properties(&atoms, &spec(false));
        assert_eq!(
            find(&properties, AtomEnum::WM_CLASS).value,
            PropertyValue::Bytes(b"mooloop-plugin\0mooloop\0".to_vec())
        );
        let hints = find(&properties, AtomEnum::WM_NORMAL_HINTS);
        assert_eq!(hints.kind, u32::from(AtomEnum::WM_SIZE_HINTS));
        assert_eq!(
            hints.value,
            PropertyValue::Words(SizeHints::for_window(size(800, 600), false).encode().to_vec())
        );
    }

    #[test]
    fn transient_for_names_an_x11_parent() {
        let property = transient_for_property(NativeWindow::x11(0x0340_0002)).unwrap();
        assert_eq!(property.name, u32::from(AtomEnum::WM_TRANSIENT_FOR));
        assert_eq!(property.kind, u32::from(AtomEnum::WINDOW));
        assert_eq!(property.value, PropertyValue::Words(vec![0x0340_0002]));
    }

    #[test]
    fn transient_for_refuses_an_id_x11_cannot_have() {
        let parent = NativeWindow::x11(u64::from(u32::MAX) + 1);
        assert_eq!(transient_for_property(parent), Err(WindowError::NotX11(parent)));
    }

    #[test]
    fn a_window_id_goes_to_the_plugin_as_an_x11_parent() {
        assert_eq!(PluginWindowId(0x0120_0007).native(), NativeWindow::x11(0x0120_0007));
    }

    #[test]
    fn focus_moving_into_the_plugin_is_not_focus_leaving() {
        assert_eq!(focus_change(false, NotifyMode::NORMAL, NotifyDetail::INFERIOR), None);
        assert_eq!(
            focus_change(true, NotifyMode::NORMAL, NotifyDetail::INFERIOR),
            Some(PluginWindowEvent::FocusIn)
        );
    }

    #[test]
    fn focus_arriving_or_leaving_from_outside_is_reported() {
        for detail in [
            NotifyDetail::ANCESTOR,
            NotifyDetail::VIRTUAL,
            NotifyDetail::NONLINEAR,
            NotifyDetail::NONLINEAR_VIRTUAL,
        ] {
            for mode in [NotifyMode::NORMAL, NotifyMode::WHILE_GRABBED] {
                assert_eq!(focus_change(true, mode, detail), Some(PluginWindowEvent::FocusIn));
                assert_eq!(focus_change(false, mode, detail), Some(PluginWindowEvent::FocusOut));
            }
        }
    }

    #[test]
    fn a_keyboard_grab_and_pointer_focus_are_not_focus_changes() {
        for entered in [true, false] {
            assert_eq!(focus_change(entered, NotifyMode::GRAB, NotifyDetail::NONLINEAR), None);
            assert_eq!(focus_change(entered, NotifyMode::UNGRAB, NotifyDetail::NONLINEAR), None);
            for detail in [NotifyDetail::POINTER, NotifyDetail::POINTER_ROOT, NotifyDetail::NONE] {
                assert_eq!(focus_change(entered, NotifyMode::NORMAL, detail), None);
            }
        }
    }

    fn known(window: Window, size: (u32, u32)) -> HashMap<Window, WindowState> {
        HashMap::from([(
            window,
            WindowState {
                size,
                resizable: true,
            },
        )])
    }

    fn configure(window: Window, width: u16, height: u16) -> Event {
        Event::ConfigureNotify(ConfigureNotifyEvent {
            response_type: CONFIGURE_NOTIFY_EVENT,
            sequence: 0,
            event: window,
            window,
            above_sibling: 0,
            x: 40,
            y: 40,
            width,
            height,
            border_width: 0,
            override_redirect: false,
        })
    }

    #[test]
    fn delete_window_is_a_close_request() {
        let atoms = atoms();
        let mut windows = known(7, (800, 600));
        let event = Event::ClientMessage(ClientMessageEvent::new(
            32,
            7,
            atoms.WM_PROTOCOLS,
            [atoms.WM_DELETE_WINDOW, 0, 0, 0, 0],
        ));
        assert_eq!(
            translate(&event, &atoms, &mut windows),
            Some((PluginWindowId(7), PluginWindowEvent::CloseRequested))
        );
    }

    #[test]
    fn other_client_messages_and_strangers_windows_are_ignored() {
        let atoms = atoms();
        let mut windows = known(7, (800, 600));
        let ping = Event::ClientMessage(ClientMessageEvent::new(
            32,
            7,
            atoms.WM_PROTOCOLS,
            [999, 0, 0, 0, 0],
        ));
        assert_eq!(translate(&ping, &atoms, &mut windows), None);
        let elsewhere = Event::ClientMessage(ClientMessageEvent::new(
            32,
            8,
            atoms.WM_PROTOCOLS,
            [atoms.WM_DELETE_WINDOW, 0, 0, 0, 0],
        ));
        assert_eq!(translate(&elsewhere, &atoms, &mut windows), None);
        assert_eq!(translate(&configure(8, 10, 10), &atoms, &mut windows), None);
    }

    #[test]
    fn a_new_size_is_reported_once_and_a_move_not_at_all() {
        let atoms = atoms();
        let mut windows = known(7, (800, 600));
        assert_eq!(translate(&configure(7, 800, 600), &atoms, &mut windows), None);
        assert_eq!(
            translate(&configure(7, 1000, 700), &atoms, &mut windows),
            Some((PluginWindowId(7), PluginWindowEvent::Resized(size(1000, 700))))
        );
        assert_eq!(translate(&configure(7, 1000, 700), &atoms, &mut windows), None);
        assert_eq!(windows[&7].size, (1000, 700));
    }

    #[test]
    fn focus_events_become_focus_changes() {
        let atoms = atoms();
        let mut windows = known(7, (800, 600));
        let focus_in = Event::FocusIn(FocusInEvent {
            response_type: FOCUS_IN_EVENT,
            detail: NotifyDetail::NONLINEAR,
            sequence: 0,
            event: 7,
            mode: NotifyMode::NORMAL,
        });
        assert_eq!(
            translate(&focus_in, &atoms, &mut windows),
            Some((PluginWindowId(7), PluginWindowEvent::FocusIn))
        );
        let into_plugin = Event::FocusOut(FocusOutEvent {
            response_type: FOCUS_OUT_EVENT,
            detail: NotifyDetail::INFERIOR,
            sequence: 0,
            event: 7,
            mode: NotifyMode::NORMAL,
        });
        assert_eq!(translate(&into_plugin, &atoms, &mut windows), None);
        let away = Event::FocusOut(FocusOutEvent {
            response_type: FOCUS_OUT_EVENT,
            detail: NotifyDetail::NONLINEAR_VIRTUAL,
            sequence: 0,
            event: 7,
            mode: NotifyMode::NORMAL,
        });
        assert_eq!(
            translate(&away, &atoms, &mut windows),
            Some((PluginWindowId(7), PluginWindowEvent::FocusOut))
        );
    }

    #[test]
    fn no_display_is_a_reason_the_badge_can_show() {
        assert_eq!(display_name_in(|_| None), Err(WindowError::NoDisplay));
        assert_eq!(display_name_in(|_| Some(OsString::new())), Err(WindowError::NoDisplay));
        assert_eq!(
            display_name_in(|_| Some(OsString::from(":1"))),
            Ok(":1".to_owned())
        );
        assert!(WindowError::NoDisplay.to_string().contains("DISPLAY is not set"));
    }

    /// Opens, shows, resizes, hides and destroys a real window. **Opens a
    /// window on whatever X server `DISPLAY` names**, so it is not run by
    /// default: run it on a disposable server, e.g.
    /// `Xvfb :99 & DISPLAY=:99 cargo test -p mooloop-plugin-window -- --ignored`.
    #[test]
    #[ignore = "needs an X server; opens a window"]
    fn a_window_on_a_real_x_server() {
        let mut windows = PluginWindows::connect().expect("an X server");
        let id = windows
            .create(&PluginWindowSpec {
                title: "mooloop test window",
                size: size(320, 200),
                resizable: false,
            })
            .unwrap();
        windows
            .set_transient_for(id, Some(NativeWindow::x11(u64::from(
                windows.connection.setup().roots[windows.screen].root,
            ))))
            .unwrap();
        windows.show(id).unwrap();
        windows.resize(id, size(400, 250)).unwrap();
        windows.set_resizable(id, true).unwrap();
        windows.set_title(id, "renamed").unwrap();
        let mut events = Vec::new();
        windows.drain_events(&mut events).unwrap();
        assert!(
            !events
                .iter()
                .any(|(_, event)| *event == PluginWindowEvent::Resized(size(400, 250))),
            "a size the pump set is not reported back: {events:?}"
        );
        windows.hide(id).unwrap();
        windows.destroy(id).unwrap();
        assert_eq!(windows.destroy(id), Err(WindowError::UnknownWindow(id)));
        assert_eq!(windows.windows().count(), 0);
    }

    #[test]
    #[ignore = "needs an X server"]
    fn the_probe_reaches_a_real_x_server() {
        probe().expect("an X server");
    }
}
