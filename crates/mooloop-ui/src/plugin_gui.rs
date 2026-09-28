//! A hosted plugin's own GUI, in a window of its own: the face's control
//! that opens it, and the pump's part in keeping it alive
//! (`docs/plans/plugin-hosting/11-plugin-gui-windows.md`, MOO-302).
//!
//! Three pieces meet here, and none of them knows the others:
//!
//! - **The session's GUI verbs** (Engine's, MOO-300): `open_plugin_gui`,
//!   `close_plugin_gui`, `resize_plugin_gui`, `set_plugin_gui_scale`,
//!   `service_plugin_io` and `drain_plugin_gui_events`. The plugin's GUI is
//!   only ever reached through them.
//! - **The bare X11 window** a GUI embeds into (Platform's, MOO-301):
//!   `mooloop_plugin_window::PluginWindows`, reached here through the
//!   [`GuiWindows`] seam so the pump's logic runs in a test with no display.
//! - **The main window**: which display server it is on, its X11 id, its
//!   focus and its scale ([`MainWindowState`]).
//!
//! **The order that matters**: a plugin's GUI is always destroyed before the
//! window it lives in. The window goes when the session says the GUI is gone
//! ([`PluginGuiEvent::Closed`], or `close_plugin_gui` returning), never
//! before; a window whose close button was pressed is closed through the
//! session first.
//!
//! **Focus on a native Wayland session.** A plugin's X11 window cannot be
//! made to belong to a Wayland window, so it would sit above every other
//! application. Instead every plugin window is unmapped while neither the
//! main window nor any plugin window has focus, and mapped again when focus
//! comes back ([`FOCUS_GRACE`] explains the wait). On X11, or with "Run under
//! XWayland" on, each window is made transient for the main window and
//! nothing hides.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use mooloop_core::{log_warn, PluginSlotId};
use mooloop_plugin_window::{
    DisplayBackend, GuiSize, NativeWindow, PluginWindowEvent, PluginWindowId, PluginWindowSpec,
    PluginWindows, WindowError,
};
use mooloop_session::plugin_rack::{GuiPlacement, PluginGuiEvent, PluginGuiOpen};
use mooloop_session::session::Session;

/// The size a plugin window is made at, before the plugin has said its own.
/// It is resized as soon as the GUI is open, before it is ever mapped.
const FIRST_SIZE: GuiSize = GuiSize { width: 640, height: 480 };

/// How long focus must be away from mooloop before its plugin windows are
/// hidden (native Wayland only). Focus moving from the main window to a
/// plugin window leaves both unfocused for a moment -- Slint hears the main
/// window lose it before X11 reports the plugin window gaining it -- and
/// hiding in that gap would take away the very window being clicked.
pub(crate) const FOCUS_GRACE: Duration = Duration::from_millis(300);

/// The window side of a plugin GUI, as the pump uses it: the calls of
/// `mooloop_plugin_window::PluginWindows`, and a fake in the tests, which
/// must not open windows on the desktop they run on.
pub(crate) trait GuiWindows {
    fn create(&mut self, spec: &PluginWindowSpec<'_>) -> Result<PluginWindowId, WindowError>;
    fn set_title(&mut self, id: PluginWindowId, title: &str) -> Result<(), WindowError>;
    fn resize(&mut self, id: PluginWindowId, size: GuiSize) -> Result<(), WindowError>;
    fn set_resizable(&mut self, id: PluginWindowId, resizable: bool) -> Result<(), WindowError>;
    fn set_transient_for(&mut self, id: PluginWindowId, parent: Option<NativeWindow>) -> Result<(), WindowError>;
    fn show(&mut self, id: PluginWindowId) -> Result<(), WindowError>;
    fn hide(&mut self, id: PluginWindowId) -> Result<(), WindowError>;
    fn destroy(&mut self, id: PluginWindowId) -> Result<(), WindowError>;
    fn drain_events(&mut self, out: &mut Vec<(PluginWindowId, PluginWindowEvent)>) -> Result<(), WindowError>;
}

impl GuiWindows for PluginWindows {
    fn create(&mut self, spec: &PluginWindowSpec<'_>) -> Result<PluginWindowId, WindowError> {
        PluginWindows::create(self, spec)
    }
    fn set_title(&mut self, id: PluginWindowId, title: &str) -> Result<(), WindowError> {
        PluginWindows::set_title(self, id, title)
    }
    fn resize(&mut self, id: PluginWindowId, size: GuiSize) -> Result<(), WindowError> {
        PluginWindows::resize(self, id, size)
    }
    fn set_resizable(&mut self, id: PluginWindowId, resizable: bool) -> Result<(), WindowError> {
        PluginWindows::set_resizable(self, id, resizable)
    }
    fn set_transient_for(&mut self, id: PluginWindowId, parent: Option<NativeWindow>) -> Result<(), WindowError> {
        PluginWindows::set_transient_for(self, id, parent)
    }
    fn show(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
        PluginWindows::show(self, id)
    }
    fn hide(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
        PluginWindows::hide(self, id)
    }
    fn destroy(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
        PluginWindows::destroy(self, id)
    }
    fn drain_events(&mut self, out: &mut Vec<(PluginWindowId, PluginWindowEvent)>) -> Result<(), WindowError> {
        PluginWindows::drain_events(self, out)
    }
}

/// How the window side is reached: `PluginWindows::connect` in the app, the
/// first time a GUI opens; a fake in the tests.
pub(crate) type Connector = Box<dyn FnMut() -> Result<Box<dyn GuiWindows>, WindowError>>;

/// What the pump reads from the main window each tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MainWindowState {
    /// The display server it is on; `None` before it is shown, and under
    /// the testing backend.
    pub backend: Option<DisplayBackend>,
    /// Its X11 id, on X11 and under XWayland.
    pub x11_parent: Option<NativeWindow>,
    /// Whether it has keyboard focus.
    pub focused: bool,
    /// Its scale factor, which every plugin GUI is given.
    pub scale: f64,
    pub now: Instant,
}

impl MainWindowState {
    /// `window` as it is now. A window with no winit window behind it (the
    /// testing backend) counts as focused, so nothing is hidden for it.
    pub(crate) fn of(window: &slint::Window) -> Self {
        use slint::winit_030::WinitWindowAccessor;
        Self {
            backend: crate::display_backend::window_display_backend(window),
            x11_parent: crate::display_backend::window_x11_parent(window),
            focused: window.with_winit_window(|winit| winit.has_focus()).unwrap_or(true),
            scale: f64::from(window.scale_factor()),
            now: Instant::now(),
        }
    }

    /// For a tick with no plugin window open, which reads none of it.
    pub(crate) fn idle() -> Self {
        Self {
            backend: None,
            x11_parent: None,
            focused: true,
            scale: 1.0,
            now: Instant::now(),
        }
    }

    /// The main window's X11 id, when a plugin window can be made
    /// transient for it (an X11 session, or "Run under XWayland").
    fn transient_parent(&self) -> Option<NativeWindow> {
        if self.backend.is_some_and(DisplayBackend::can_set_transient) {
            self.x11_parent
        } else {
            None
        }
    }

    /// Whether plugin windows hide while mooloop is not focused: a native
    /// Wayland session only.
    fn hides_on_focus_loss(&self) -> bool {
        self.backend == Some(DisplayBackend::Wayland)
    }
}

/// One plugin GUI that is open.
#[derive(Debug)]
struct OpenGui {
    /// The bare window it is embedded in, or `None` for a floating GUI,
    /// which is in a window of the plugin's own.
    window: Option<PluginWindowId>,
    title: String,
    /// The plugin wants it on screen (it has not hidden itself).
    shown: bool,
}

/// Every plugin GUI that is open, and the windows they are in.
pub(crate) struct PluginGuis {
    connector: Connector,
    windows: Option<Box<dyn GuiWindows>>,
    open: HashMap<PluginSlotId, OpenGui>,
    /// Plugin windows that have focus (X11's word for it).
    focused: HashSet<PluginWindowId>,
    /// When mooloop last had focus nowhere, for [`FOCUS_GRACE`].
    unfocused_since: Option<Instant>,
    /// The plugin windows are unmapped because mooloop lost focus.
    hidden_for_focus: bool,
    /// The scale every open GUI was last given.
    scale: Option<f64>,
    /// Why a plugin's window could not open, for its face's badge.
    problems: HashMap<PluginSlotId, String>,
    /// Scratch for the window events, kept so a tick allocates nothing.
    events: Vec<(PluginWindowId, PluginWindowEvent)>,
}

impl Default for PluginGuis {
    /// On the display `DISPLAY` names, connected the first time a GUI opens.
    fn default() -> Self {
        Self::new(Box::new(|| {
            PluginWindows::connect().map(|windows| Box::new(windows) as Box<dyn GuiWindows>)
        }))
    }
}

impl PluginGuis {
    pub(crate) fn new(connector: Connector) -> Self {
        Self {
            connector,
            windows: None,
            open: HashMap::new(),
            focused: HashSet::new(),
            unfocused_since: None,
            hidden_for_focus: false,
            scale: None,
            problems: HashMap::new(),
            events: Vec::new(),
        }
    }

    /// Whether plugin `slot`'s GUI is open.
    pub(crate) fn is_open(&self, slot: PluginSlotId) -> bool {
        self.open.contains_key(&slot)
    }

    /// Why plugin `slot`'s window last failed to open, if it did.
    pub(crate) fn problem(&self, slot: PluginSlotId) -> Option<&str> {
        self.problems.get(&slot).map(String::as_str)
    }

    /// Whether any plugin GUI is open.
    pub(crate) fn any_open(&self) -> bool {
        !self.open.is_empty()
    }

    fn slot_of(&self, id: PluginWindowId) -> Option<PluginSlotId> {
        self.open
            .iter()
            .find(|(_, gui)| gui.window == Some(id))
            .map(|(slot, _)| *slot)
    }

    /// The face's control: open plugin `slot`'s GUI, or bring it to the
    /// front when it is open. A failure is kept for the face's badge and
    /// returned; the face stays either way.
    pub(crate) fn open_or_raise(
        &mut self,
        session: &mut Session,
        slot: PluginSlotId,
        main: &MainWindowState,
    ) -> Result<(), String> {
        let result = if self.open.contains_key(&slot) {
            self.raise(session, slot)
        } else {
            self.open(session, slot, main)
        };
        match &result {
            Ok(()) => {
                self.problems.remove(&slot);
            }
            Err(why) => {
                log_warn!("plugin-window", "slot {}: the window did not open: {why}", slot.0);
                self.problems.insert(slot, why.clone());
            }
        }
        result
    }

    /// Bring an open GUI's window to the front: unmapped and mapped again,
    /// which every window manager places on top and none refuses. A
    /// floating GUI is asked to show itself.
    fn raise(&mut self, session: &mut Session, slot: PluginSlotId) -> Result<(), String> {
        // A press on the face means mooloop has focus again.
        self.reveal();
        let Some(gui) = self.open.get_mut(&slot) else {
            return Ok(());
        };
        gui.shown = true;
        match (gui.window, self.windows.as_mut()) {
            (Some(id), Some(windows)) => {
                let _ = windows.hide(id);
                windows.show(id).map_err(|error| error.to_string())
            }
            (Some(_), None) => Ok(()),
            (None, _) => session.show_plugin_gui(slot).map_err(|error| error.to_string()),
        }
    }

    fn open(&mut self, session: &mut Session, slot: PluginSlotId, main: &MainWindowState) -> Result<(), String> {
        let kind = session.plugin_gui_kind(slot).map_err(|error| error.to_string())?;
        let title = session.plugin_gui_title(slot).unwrap_or_else(|| "Plugin".to_string());
        let transient = main.transient_parent();
        if kind.floating {
            // Policy 4: the plugin only floats, in a window of its own.
            let open = PluginGuiOpen {
                placement: GuiPlacement::Floating {
                    transient_for: transient.map(|parent| parent.id),
                },
                title: title.clone(),
                scale: Some(main.scale),
            };
            session.open_plugin_gui(slot, &open).map_err(|error| error.to_string())?;
            self.open.insert(slot, OpenGui { window: None, title, shown: true });
            return Ok(());
        }
        if self.windows.is_none() {
            self.windows = Some((self.connector)().map_err(|error| error.to_string())?);
        }
        let windows = self.windows.as_mut().expect("connected above");
        let id = windows
            .create(&PluginWindowSpec {
                title: &title,
                size: FIRST_SIZE,
                resizable: true,
            })
            .map_err(|error| error.to_string())?;
        let open = PluginGuiOpen {
            placement: GuiPlacement::Embedded { parent: id.native().id },
            title: title.clone(),
            scale: Some(main.scale),
        };
        let opened = match session.open_plugin_gui(slot, &open) {
            Ok(opened) => opened,
            Err(error) => {
                // Nothing is in the window: the session destroyed a GUI
                // that failed half way, or never made one.
                let _ = windows.destroy(id);
                return Err(error.to_string());
            }
        };
        let placed = (|| {
            if let Some(size) = opened.size {
                windows.resize(id, size)?;
            }
            if !opened.can_resize {
                // Fixed size hints: what makes a tiling compositor float it.
                windows.set_resizable(id, false)?;
            }
            if transient.is_some() {
                windows.set_transient_for(id, transient)?;
            }
            windows.show(id)
        })();
        if let Err(error) = placed {
            // The GUI first, then its window.
            session.close_plugin_gui(slot);
            let _ = windows.destroy(id);
            return Err(error.to_string());
        }
        self.open.insert(slot, OpenGui { window: Some(id), title, shown: true });
        // A window just mapped is not focused until X11 says so; the press
        // that opened it came from the main window, which had focus.
        self.unfocused_since = None;
        Ok(())
    }

    /// The pump's tick: the plugins' own requests and the GUIs the session
    /// closed, then the windows' events, then the scale, the titles and the
    /// focus. Costs a branch or two when no GUI is open.
    pub(crate) fn tick(&mut self, session: &mut Session, main: &MainWindowState) {
        for (slot, event) in session.drain_plugin_gui_events() {
            self.on_gui_event(session, slot, event);
        }
        if let Some(windows) = self.windows.as_mut() {
            let mut events = std::mem::take(&mut self.events);
            events.clear();
            let drained = windows.drain_events(&mut events);
            for &(id, event) in &events {
                self.on_window_event(session, id, event);
            }
            events.clear();
            self.events = events;
            if let Err(error) = drained {
                self.lost_connection(session, &error);
            }
        }
        if self.open.is_empty() {
            self.unfocused_since = None;
            self.hidden_for_focus = false;
            return;
        }
        if self.scale != Some(main.scale) {
            self.scale = Some(main.scale);
            let slots: Vec<PluginSlotId> = self.open.keys().copied().collect();
            for slot in slots {
                let _ = session.set_plugin_gui_scale(slot, main.scale);
            }
        }
        self.follow_titles(session);
        self.follow_focus(main);
    }

    fn on_gui_event(&mut self, session: &mut Session, slot: PluginSlotId, event: PluginGuiEvent) {
        let Some(gui) = self.open.get_mut(&slot) else {
            return;
        };
        let window = gui.window;
        match event {
            PluginGuiEvent::Resize(size) => {
                if let (Some(id), Some(windows)) = (window, self.windows.as_mut()) {
                    let _ = windows.resize(id, size);
                }
            }
            PluginGuiEvent::Show => {
                gui.shown = true;
                if let (Some(id), Some(windows), false) = (window, self.windows.as_mut(), self.hidden_for_focus) {
                    let _ = windows.show(id);
                }
            }
            PluginGuiEvent::Hide => {
                gui.shown = false;
                if let (Some(id), Some(windows)) = (window, self.windows.as_mut()) {
                    let _ = windows.hide(id);
                }
            }
            PluginGuiEvent::ResizeHintsChanged => {
                let resizable = session
                    .plugin_rack
                    .instance_mut(slot)
                    .and_then(|instance| instance.gui())
                    .is_some_and(|gui| gui.can_resize());
                if let (Some(id), Some(windows)) = (window, self.windows.as_mut()) {
                    let _ = windows.set_resizable(id, resizable);
                }
            }
            PluginGuiEvent::Closed => {
                // The GUI is gone -- the plugin closed it, or the session
                // did for a removal, a song closing or a quit -- so now,
                // and only now, its window may go.
                self.forget(slot);
            }
        }
    }

    fn on_window_event(&mut self, session: &mut Session, id: PluginWindowId, event: PluginWindowEvent) {
        match event {
            PluginWindowEvent::FocusIn => {
                self.focused.insert(id);
            }
            PluginWindowEvent::FocusOut => {
                self.focused.remove(&id);
            }
            PluginWindowEvent::CloseRequested => {
                let Some(slot) = self.slot_of(id) else { return };
                // The GUI is hidden and destroyed first; the processor keeps
                // running. Then the window.
                session.close_plugin_gui(slot);
                self.forget(slot);
            }
            PluginWindowEvent::Resized(size) => {
                let Some(slot) = self.slot_of(id) else { return };
                match session.resize_plugin_gui(slot, size) {
                    Ok(taken) if taken != size => {
                        if let Some(windows) = self.windows.as_mut() {
                            let _ = windows.resize(id, taken);
                        }
                    }
                    Ok(_) => {}
                    Err(error) => log_warn!("plugin-window", "slot {}: resize: {error}", slot.0),
                }
            }
        }
    }

    /// Plugin `slot`'s GUI is destroyed: destroy its window, if it has one.
    fn forget(&mut self, slot: PluginSlotId) {
        let Some(gui) = self.open.remove(&slot) else {
            return;
        };
        if let Some(id) = gui.window {
            self.focused.remove(&id);
            if let Some(windows) = self.windows.as_mut() {
                if let Err(error) = windows.destroy(id) {
                    log_warn!("plugin-window", "slot {}: {error}", slot.0);
                }
            }
        }
    }

    /// The X connection failed, and every window on it with it: close each
    /// embedded GUI (its window is already gone) and say why on its face.
    fn lost_connection(&mut self, session: &mut Session, error: &WindowError) {
        log_warn!("plugin-window", "{error}; closing every embedded plugin window");
        let embedded: Vec<PluginSlotId> = self
            .open
            .iter()
            .filter(|(_, gui)| gui.window.is_some())
            .map(|(slot, _)| *slot)
            .collect();
        for slot in embedded {
            session.close_plugin_gui(slot);
            self.open.remove(&slot);
            self.problems.insert(slot, error.to_string());
        }
        self.windows = None;
        self.focused.clear();
    }

    /// Retitle a window whose plugin or track was renamed.
    fn follow_titles(&mut self, session: &Session) {
        for (slot, gui) in &mut self.open {
            let Some(id) = gui.window else { continue };
            let Some(title) = session.plugin_gui_title(*slot) else { continue };
            if title != gui.title {
                if let Some(windows) = self.windows.as_mut() {
                    let _ = windows.set_title(id, &title);
                }
                gui.title = title;
            }
        }
    }

    /// Native Wayland: hide every plugin window once neither the main window
    /// nor any plugin window has had focus for [`FOCUS_GRACE`], and show them
    /// again when focus comes back. Anywhere else, nothing hides.
    ///
    /// A floating GUI is left alone: its window is the plugin's, whose focus
    /// mooloop cannot see, so hiding it would hide it from under the click.
    fn follow_focus(&mut self, main: &MainWindowState) {
        if !main.hides_on_focus_loss() {
            self.unfocused_since = None;
            self.reveal();
            return;
        }
        if main.focused || !self.focused.is_empty() {
            self.unfocused_since = None;
            self.reveal();
            return;
        }
        let since = *self.unfocused_since.get_or_insert(main.now);
        if self.hidden_for_focus || main.now.duration_since(since) < FOCUS_GRACE {
            return;
        }
        self.hidden_for_focus = true;
        let Some(windows) = self.windows.as_mut() else { return };
        for gui in self.open.values().filter(|gui| gui.shown) {
            if let Some(id) = gui.window {
                let _ = windows.hide(id);
            }
        }
    }

    /// Map again every window hidden for focus that its plugin still wants
    /// shown.
    fn reveal(&mut self) {
        if !std::mem::take(&mut self.hidden_for_focus) {
            return;
        }
        let Some(windows) = self.windows.as_mut() else { return };
        for gui in self.open.values().filter(|gui| gui.shown) {
            if let Some(id) = gui.window {
                let _ = windows.show(id);
            }
        }
    }

    /// Destroy every window left, for a quit: after `close_plugins` has
    /// destroyed every GUI and [`Self::tick`] or this has read its `Closed`.
    pub(crate) fn close_all(&mut self, session: &mut Session) {
        for (slot, event) in session.drain_plugin_gui_events() {
            if event == PluginGuiEvent::Closed {
                self.forget(slot);
            }
        }
        let slots: Vec<PluginSlotId> = self.open.keys().copied().collect();
        for slot in slots {
            session.close_plugin_gui(slot);
            self.forget(slot);
        }
    }
}

impl crate::UiState {
    /// The pump's plugin-GUI work for one tick: every hosted plugin's timers
    /// and fds (a GUI's event loop runs on them), then the GUIs and their
    /// windows. After `service_plugins`, so a device removed this tick has
    /// already had its GUI closed and its window goes now.
    pub(crate) fn pump_plugin_guis(&mut self, main: &MainWindowState) {
        self.session.service_plugin_io();
        self.plugin_guis.tick(&mut self.session, main);
    }

    /// The face's control on chain row `row`: open that plugin's GUI, or
    /// bring it to the front. The face is refreshed either way, so the
    /// control lights and a failure's badge appears at once.
    pub(crate) fn open_plugin_gui_at(&mut self, row: usize, main: &MainWindowState) -> Result<(), String> {
        let slot = self
            .session
            .effect_chain()
            .and_then(|chain| chain.get(row))
            .and_then(|effect| match effect.params {
                mooloop_core::EffectParams::Plugin(slot) => Some(slot),
                _ => None,
            });
        let Some(slot) = slot else {
            return Err("that device is not a plugin".to_string());
        };
        let result = self.plugin_guis.open_or_raise(&mut self.session, slot, main);
        self.refresh_plugin_faces();
        result
    }

    /// The control on the selected channel's instrument face (MOO-304): the
    /// same window a plugin on a chain opens, for the source's slot.
    pub(crate) fn open_source_plugin_gui(&mut self, main: &MainWindowState) -> Result<(), String> {
        let Some(slot) = self.source_plugin_slot() else {
            return Err("this channel's source is not a plugin".to_string());
        };
        let result = self.plugin_guis.open_or_raise(&mut self.session, slot, main);
        self.refresh_plugin_faces();
        result
    }

    /// Quit: destroy every plugin window once `close_plugins` has destroyed
    /// the GUIs in them.
    pub(crate) fn close_plugin_windows(&mut self) {
        self.plugin_guis.close_all(&mut self.session);
    }
}

/// The window side faked, for the tests here and in `plugin_ui_tests`: no
/// test may open a window on the desktop it runs on.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// What the fake window side was asked, in order.
    #[derive(Default)]
    pub(crate) struct Log {
        pub calls: Vec<String>,
        pub events: Vec<(PluginWindowId, PluginWindowEvent)>,
        pub next: u32,
    }

    pub(crate) struct FakeWindows(pub Rc<RefCell<Log>>);

    impl GuiWindows for FakeWindows {
        fn create(&mut self, spec: &PluginWindowSpec<'_>) -> Result<PluginWindowId, WindowError> {
            let mut log = self.0.borrow_mut();
            log.next += 1;
            let id = PluginWindowId(0x100 + log.next);
            log.calls.push(format!("create {:#x} {}", id.0, spec.title));
            Ok(id)
        }
        fn set_title(&mut self, id: PluginWindowId, title: &str) -> Result<(), WindowError> {
            self.0.borrow_mut().calls.push(format!("title {:#x} {title}", id.0));
            Ok(())
        }
        fn resize(&mut self, id: PluginWindowId, size: GuiSize) -> Result<(), WindowError> {
            self.0
                .borrow_mut()
                .calls
                .push(format!("resize {:#x} {}x{}", id.0, size.width, size.height));
            Ok(())
        }
        fn set_resizable(&mut self, id: PluginWindowId, resizable: bool) -> Result<(), WindowError> {
            self.0.borrow_mut().calls.push(format!("resizable {:#x} {resizable}", id.0));
            Ok(())
        }
        fn set_transient_for(&mut self, id: PluginWindowId, parent: Option<NativeWindow>) -> Result<(), WindowError> {
            self.0
                .borrow_mut()
                .calls
                .push(format!("transient {:#x} {:?}", id.0, parent.map(|parent| parent.id)));
            Ok(())
        }
        fn show(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
            self.0.borrow_mut().calls.push(format!("show {:#x}", id.0));
            Ok(())
        }
        fn hide(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
            self.0.borrow_mut().calls.push(format!("hide {:#x}", id.0));
            Ok(())
        }
        fn destroy(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
            self.0.borrow_mut().calls.push(format!("destroy {:#x}", id.0));
            Ok(())
        }
        fn drain_events(&mut self, out: &mut Vec<(PluginWindowId, PluginWindowEvent)>) -> Result<(), WindowError> {
            out.append(&mut self.0.borrow_mut().events);
            Ok(())
        }
    }

    /// A `PluginGuis` on the fake, and its log.
    pub(crate) fn fake() -> (PluginGuis, Rc<RefCell<Log>>) {
        let log = Rc::new(RefCell::new(Log::default()));
        let shared = log.clone();
        let guis = PluginGuis::new(Box::new(move || {
            Ok(Box::new(FakeWindows(shared.clone())) as Box<dyn GuiWindows>)
        }));
        (guis, log)
    }

    pub(crate) fn main_on(backend: Option<DisplayBackend>, focused: bool, now: Instant) -> MainWindowState {
        MainWindowState {
            backend,
            x11_parent: (backend == Some(DisplayBackend::X11)).then_some(NativeWindow::x11(0x42)),
            focused,
            scale: 1.0,
            now,
        }
    }

}

#[cfg(test)]
mod tests {
    use super::fake::main_on;
    use super::*;

    /// With no display to connect to, nothing opens, and the face is told
    /// why rather than the app failing.
    #[test]
    fn no_display_is_a_reason_not_a_crash() {
        let mut guis = PluginGuis::new(Box::new(|| Err(WindowError::NoDisplay)));
        assert!(guis.windows.is_none());
        let error = (guis.connector)().err().expect("refused");
        assert!(error.to_string().contains("DISPLAY"), "{error}");
        assert!(!guis.any_open());
    }

    /// **On native Wayland, plugin windows hide once mooloop has had focus
    /// nowhere for the grace period, and come back with it**; a plugin
    /// window's own focus counts as mooloop's, and a window its plugin hid
    /// stays hidden. On X11 nothing hides.
    #[test]
    fn plugin_windows_hide_while_mooloop_is_not_focused_on_wayland() {
        let (mut guis, log) = super::fake::fake();
        guis.windows = Some((guis.connector)().expect("the fake connects"));
        let (shown, hidden) = (PluginWindowId(0x201), PluginWindowId(0x202));
        let gui = |window, shown| OpenGui {
            window: Some(window),
            title: String::new(),
            shown,
        };
        guis.open.insert(PluginSlotId(1), gui(shown, true));
        guis.open.insert(PluginSlotId(2), gui(hidden, false));
        let calls = |log: &std::rc::Rc<std::cell::RefCell<super::fake::Log>>| log.borrow_mut().calls.split_off(0);
        let start = Instant::now();
        let wayland = |focused, after: Duration| main_on(Some(DisplayBackend::Wayland), focused, start + after);

        guis.follow_focus(&wayland(true, Duration::ZERO));
        assert!(calls(&log).is_empty(), "focused: nothing moves");
        guis.follow_focus(&wayland(false, Duration::ZERO));
        guis.follow_focus(&wayland(false, FOCUS_GRACE / 2));
        assert!(calls(&log).is_empty(), "a moment without focus is focus moving between windows");
        guis.follow_focus(&wayland(false, FOCUS_GRACE));
        assert_eq!(calls(&log), ["hide 0x201"], "only the window its plugin still shows");
        guis.follow_focus(&wayland(false, FOCUS_GRACE * 2));
        assert!(calls(&log).is_empty(), "hidden once");
        guis.follow_focus(&wayland(true, FOCUS_GRACE * 3));
        assert_eq!(calls(&log), ["show 0x201"], "back with focus");

        // A plugin window with focus is mooloop with focus.
        guis.focused.insert(shown);
        guis.follow_focus(&wayland(false, FOCUS_GRACE * 4));
        guis.follow_focus(&wayland(false, FOCUS_GRACE * 10));
        assert!(calls(&log).is_empty(), "the plugin window has focus");
        guis.focused.clear();

        // X11, or "Run under XWayland": the window manager keeps it above
        // its parent, and nothing hides however long focus is away.
        let x11 = |after| main_on(Some(DisplayBackend::X11), false, start + after);
        guis.follow_focus(&x11(FOCUS_GRACE * 20));
        guis.follow_focus(&x11(FOCUS_GRACE * 30));
        assert!(calls(&log).is_empty());
    }

    /// The transient parent is offered only where the backend allows it.
    #[test]
    fn only_x11_is_transient() {
        let now = Instant::now();
        assert_eq!(
            main_on(Some(DisplayBackend::X11), true, now).transient_parent(),
            Some(NativeWindow::x11(0x42))
        );
        assert_eq!(main_on(Some(DisplayBackend::Wayland), true, now).transient_parent(), None);
        assert_eq!(main_on(None, true, now).transient_parent(), None);
        assert!(main_on(Some(DisplayBackend::Wayland), true, now).hides_on_focus_loss());
        assert!(!main_on(Some(DisplayBackend::X11), true, now).hides_on_focus_loss());
    }
}
