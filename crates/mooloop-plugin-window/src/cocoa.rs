//! A Cocoa window of mooloop's own for a plugin GUI to embed into, on macOS
//! (`11-plugin-gui-windows.md`, policies 2 and 3, in Cocoa).
//!
//! [`PluginWindows`] is every plugin window the process has open, each an
//! `NSPanel`. The pump owns one, makes it the first time a GUI opens, and
//! drives it from the process's **main thread**, the only thread AppKit may
//! be called from; [`PluginWindows::connect`] refuses any other, and the
//! type cannot leave the thread it was made on. The same calls, in the same
//! order, as the X11 windows:
//!
//! 1. [`PluginWindows::create`] makes a hidden panel, titled, centred, fixed
//!    at its size when the plugin cannot resize.
//! 2. Its [`PluginWindowId::native`], the panel's content view, goes to the
//!    plugin's `set_parent` as the `NSView*` CLAP's Cocoa API asks for.
//! 3. [`PluginWindows::set_transient_for`] floats the panel above mooloop's
//!    other windows. Every panel hides while mooloop is not the active
//!    application and comes back with it, which AppKit does on its own.
//! 4. [`PluginWindows::show`] orders it front and gives it the keyboard;
//!    [`PluginWindows::hide`] orders it out.
//! 5. [`PluginWindows::drain_events`], every pump tick, never waits. The
//!    events are queued by the panel's delegate as AppKit calls it, from the
//!    run loop Slint's event loop drives.
//! 6. [`PluginWindows::destroy`] **after** the plugin's GUI `destroy`, never
//!    before: the plugin's view is a subview of the content view.
//!
//! Dropping [`PluginWindows`] closes every panel still open, so the same
//! order holds for it: every GUI in them destroyed first.
//!
//! Sizes are logical points, the units of a Cocoa plugin's `GuiSize`, so a
//! plugin's size is a content size unconverted. A panel's content view is
//! its plugin's parent for as long as the panel is open.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};

use mooloop_plugin_host::{GuiApi, GuiSize, NativeWindow};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSPanel, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowDelegate,
    NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

pub use crate::window::{
    extent, PluginWindowEvent, PluginWindowId, PluginWindowSpec, WindowError, MAX_EXTENT,
};

thread_local! {
    /// Every open panel's content view, by its id: what
    /// [`PluginWindowId::native`] reads. Only the main thread has any.
    static CONTENT_VIEWS: RefCell<HashMap<u32, NativeWindow>> = RefCell::new(HashMap::new());
}

/// The next id a panel is given. Never reused within a process, so an id
/// held past its window's end names nothing rather than another window.
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

/// The content view is the parent, so the plugin is handed that.
impl PluginWindowId {
    /// The window to hand the plugin's `set_parent`: this panel's content
    /// view, as a Cocoa window. Read on the main thread while the panel is
    /// open; for any other id, or on another thread, it is a null view,
    /// which must not reach a plugin.
    pub fn native(self) -> NativeWindow {
        CONTENT_VIEWS
            .with_borrow(|views| views.get(&self.0).copied())
            .unwrap_or_else(|| NativeWindow::cocoa(std::ptr::null_mut()))
    }
}

/// The panel's style: a title bar with a close button, and a resizable
/// edge only when the plugin can be resized. No miniaturize button: a
/// panel that floats and hides with the application has no use for one.
pub fn style_mask(resizable: bool) -> NSWindowStyleMask {
    let fixed = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
    if resizable {
        fixed | NSWindowStyleMask::Resizable
    } else {
        fixed
    }
}

/// `size` as a content size, held to [`extent`].
pub fn content_size(size: GuiSize) -> NSSize {
    let (width, height) = extent(size);
    NSSize::new(f64::from(width), f64::from(height))
}

/// A content size in whole points, held to [`extent`].
pub fn gui_size(size: NSSize) -> GuiSize {
    let points = |value: f64| value.round().clamp(1.0, f64::from(MAX_EXTENT)) as u32;
    GuiSize {
        width: points(size.width),
        height: points(size.height),
    }
}

/// The frame a window at `frame` takes when it is resized to `resized`'s
/// size (the frame for its new content) with its top-left corner where it
/// was. Cocoa's origin is the bottom-left, so a plain content resize would
/// move the title bar.
pub fn keep_top_left(frame: NSRect, resized: NSRect) -> NSRect {
    let top = frame.origin.y + frame.size.height;
    NSRect::new(
        NSPoint::new(frame.origin.x, top - resized.size.height),
        resized.size,
    )
}

type Events = Rc<RefCell<Vec<(PluginWindowId, PluginWindowEvent)>>>;

/// What one panel's delegate knows.
struct DelegateState {
    id: PluginWindowId,
    events: Events,
    /// The content size last set or reported, so a size the pump set is not
    /// reported back.
    size: Cell<GuiSize>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `PanelDelegate`
    // does not implement `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateState]
    struct PanelDelegate;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for PanelDelegate {}

    // SAFETY: `NSWindowDelegate` has no safety requirements, and each
    // method's signature is the protocol's.
    unsafe impl NSWindowDelegate for PanelDelegate {
        // The close button asks; the pump closes the GUI first, then
        // destroys the panel, so the panel never closes itself.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            self.push(PluginWindowEvent::CloseRequested);
            false
        }

        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, notification: &NSNotification) {
            let Some(window) = notification
                .object()
                .and_then(|object| object.downcast::<NSWindow>().ok())
            else {
                return;
            };
            let size = gui_size(window.contentRectForFrameRect(window.frame()).size);
            if size != self.ivars().size.get() {
                self.ivars().size.set(size);
                self.push(PluginWindowEvent::Resized(size));
            }
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            self.push(PluginWindowEvent::FocusIn);
        }

        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            self.push(PluginWindowEvent::FocusOut);
        }
    }
);

impl PanelDelegate {
    fn new(mtm: MainThreadMarker, state: DelegateState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(state);
        // SAFETY: NSObject's `init` takes nothing and returns the object.
        unsafe { msg_send![super(this), init] }
    }

    fn push(&self, event: PluginWindowEvent) {
        let state = self.ivars();
        state.events.borrow_mut().push((state.id, event));
    }
}

/// One open panel.
struct Panel {
    panel: Retained<NSPanel>,
    /// The panel's delegate. AppKit does not keep it alive, so this does.
    delegate: Retained<PanelDelegate>,
    /// The content view the plugin embeds into, kept so its address stays
    /// the plugin's parent for as long as the panel is open.
    _view: Retained<NSView>,
}

impl Drop for Panel {
    fn drop(&mut self) {
        // The delegate goes with this, and AppKit may not hold its pointer
        // past it.
        self.panel.setDelegate(None);
        self.panel.orderOut(None);
        self.panel.close();
    }
}

/// Every plugin window the process has open. Main thread only.
pub struct PluginWindows {
    mtm: MainThreadMarker,
    panels: HashMap<u32, Panel>,
    events: Events,
}

impl fmt::Debug for PluginWindows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginWindows")
            .field("windows", &self.panels.len())
            .finish_non_exhaustive()
    }
}

impl PluginWindows {
    /// Start making plugin windows on this thread, which must be the
    /// process's main thread ([`WindowError::NotMainThread`] otherwise).
    /// Cocoa has no display to reach, so nothing else can fail.
    pub fn connect() -> Result<Self, WindowError> {
        let mtm = MainThreadMarker::new().ok_or(WindowError::NotMainThread)?;
        Ok(Self {
            mtm,
            panels: HashMap::new(),
            events: Rc::default(),
        })
    }

    /// Make a plugin window, hidden and centred on the screen. Show it with
    /// [`Self::show`] once the plugin's GUI is in it.
    pub fn create(&mut self, spec: &PluginWindowSpec<'_>) -> Result<PluginWindowId, WindowError> {
        let id = PluginWindowId(NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let size = content_size(spec.size);
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(self.mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), size),
            style_mask(spec.resizable),
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: the panel is owned by `Retained` alone, so a close must
        // not release it as well.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setTitle(&NSString::from_str(spec.title));
        // Above a full-screen main window too, not on a space of its own.
        panel.setCollectionBehavior(NSWindowCollectionBehavior::FullScreenAuxiliary);
        panel.setHidesOnDeactivate(true);
        panel.center();
        let delegate = PanelDelegate::new(
            self.mtm,
            DelegateState {
                id,
                events: Rc::clone(&self.events),
                size: Cell::new(gui_size(size)),
            },
        );
        panel.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let Some(view) = panel.contentView() else {
            panel.setDelegate(None);
            panel.close();
            return Err(WindowError::NoContentView);
        };
        let ns_view = Retained::as_ptr(&view).cast_mut().cast::<std::ffi::c_void>();
        CONTENT_VIEWS.with_borrow_mut(|views| views.insert(id.0, NativeWindow::cocoa(ns_view)));
        self.panels.insert(
            id.0,
            Panel {
                panel,
                delegate,
                _view: view,
            },
        );
        Ok(id)
    }

    /// Retitle a window: the plugin or the track was renamed.
    pub fn set_title(&mut self, id: PluginWindowId, title: &str) -> Result<(), WindowError> {
        self.panel(id)?.panel.setTitle(&NSString::from_str(title));
        Ok(())
    }

    /// Resize a window's content to `size`, in points: the plugin's own
    /// request or its answer to `adjust_size`. Its top-left corner stays
    /// where it is. Reports no [`PluginWindowEvent::Resized`] for this size.
    pub fn resize(&mut self, id: PluginWindowId, size: GuiSize) -> Result<(), WindowError> {
        let panel = self.panel(id)?;
        let content = content_size(size);
        panel.delegate.ivars().size.set(gui_size(content));
        let frame = panel.panel.frame();
        let resized = panel
            .panel
            .frameRectForContentRect(NSRect::new(frame.origin, content));
        panel.panel.setFrame_display(keep_top_left(frame, resized), true);
        Ok(())
    }

    /// The plugin's resize hints changed: fix the window at its size, or
    /// free it.
    pub fn set_resizable(&mut self, id: PluginWindowId, resizable: bool) -> Result<(), WindowError> {
        self.panel(id)?.panel.setStyleMask(style_mask(resizable));
        Ok(())
    }

    /// Float the window above mooloop's own (`Some`), or stop (`None`).
    /// `parent` must be a Cocoa window, the main window's view; the panel
    /// is kept above every mooloop window rather than tied to that one, and
    /// hides with the application whether it floats or not.
    pub fn set_transient_for(
        &mut self,
        id: PluginWindowId,
        parent: Option<NativeWindow>,
    ) -> Result<(), WindowError> {
        let panel = self.panel(id)?;
        match parent {
            Some(parent) if parent.api != GuiApi::Cocoa => {
                return Err(WindowError::ForeignParent(parent))
            }
            floating => panel.panel.setFloatingPanel(floating.is_some()),
        }
        Ok(())
    }

    /// Order the window front and give it the keyboard.
    pub fn show(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
        self.panel(id)?.panel.makeKeyAndOrderFront(None);
        Ok(())
    }

    /// Order the window out. The plugin's GUI stays in it, and comes back
    /// with it on [`Self::show`].
    pub fn hide(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
        self.panel(id)?.panel.orderOut(None);
        Ok(())
    }

    /// Close the window for good. Only after the plugin's GUI in it has been
    /// destroyed. Its id names nothing afterwards.
    pub fn destroy(&mut self, id: PluginWindowId) -> Result<(), WindowError> {
        let panel = self.panels.remove(&id.0).ok_or(WindowError::UnknownWindow(id))?;
        CONTENT_VIEWS.with_borrow_mut(|views| views.remove(&id.0));
        drop(panel);
        Ok(())
    }

    /// The window's content size as last set or reported, in points.
    pub fn size(&self, id: PluginWindowId) -> Option<GuiSize> {
        self.panels
            .get(&id.0)
            .map(|panel| panel.delegate.ivars().size.get())
    }

    /// Every window still open.
    pub fn windows(&self) -> impl Iterator<Item = PluginWindowId> + '_ {
        self.panels.keys().map(|&id| PluginWindowId(id))
    }

    /// Append everything that happened to the windows since the last call to
    /// `out`, without waiting. Never fails: Cocoa has no connection to lose.
    pub fn drain_events(
        &mut self,
        out: &mut Vec<(PluginWindowId, PluginWindowEvent)>,
    ) -> Result<(), WindowError> {
        out.append(&mut self.events.borrow_mut());
        Ok(())
    }

    fn panel(&self, id: PluginWindowId) -> Result<&Panel, WindowError> {
        self.panels.get(&id.0).ok_or(WindowError::UnknownWindow(id))
    }
}

impl Drop for PluginWindows {
    fn drop(&mut self) {
        CONTENT_VIEWS.with_borrow_mut(|views| {
            for id in self.panels.keys() {
                views.remove(id);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // libtest runs each test off the main thread, so nothing here can make
    // a panel; `tests/cocoa_window.rs` does, from a main of its own.

    #[test]
    fn a_fixed_size_panel_has_no_resizable_edge() {
        let fixed = style_mask(false);
        assert!(fixed.contains(NSWindowStyleMask::Titled | NSWindowStyleMask::Closable));
        assert!(!fixed.contains(NSWindowStyleMask::Resizable));
        assert!(style_mask(true).contains(NSWindowStyleMask::Resizable));
    }

    #[test]
    fn a_plugin_size_is_a_content_size_in_points_unconverted() {
        let size = GuiSize { width: 640, height: 480 };
        let content = content_size(size);
        assert_eq!((content.width, content.height), (640.0, 480.0));
        assert_eq!(gui_size(content), size);
    }

    #[test]
    fn a_size_no_window_can_hold_is_clamped_both_ways() {
        let content = content_size(GuiSize { width: 0, height: 100_000 });
        assert_eq!((content.width, content.height), (1.0, f64::from(MAX_EXTENT)));
        assert_eq!(gui_size(NSSize::new(-3.0, 199.6)), GuiSize { width: 1, height: 200 });
    }

    #[test]
    fn a_resize_keeps_the_title_bar_where_it_was() {
        // Cocoa's y grows upwards: a frame at y 100, 300 tall, has its top
        // at 400, and keeps it when it grows to 500.
        let frame = NSRect::new(NSPoint::new(50.0, 100.0), NSSize::new(200.0, 300.0));
        let resized = NSRect::new(NSPoint::new(50.0, 100.0), NSSize::new(260.0, 500.0));
        let kept = keep_top_left(frame, resized);
        assert_eq!((kept.origin.x, kept.origin.y), (50.0, -100.0));
        assert_eq!((kept.size.width, kept.size.height), (260.0, 500.0));
    }

    #[test]
    fn an_id_that_names_no_open_panel_is_a_null_view() {
        let native = PluginWindowId(u32::MAX).native();
        assert_eq!(native.api, GuiApi::Cocoa);
        assert_eq!(native.as_ns_view(), Some(std::ptr::null_mut()));
    }

    #[test]
    fn appkit_is_refused_off_the_main_thread() {
        let refused = std::thread::spawn(|| PluginWindows::connect().err())
            .join()
            .expect("the thread ran");
        assert_eq!(refused, Some(WindowError::NotMainThread));
    }
}
