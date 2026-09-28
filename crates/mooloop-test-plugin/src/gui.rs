//! The `gui` extension the `.gui` variants declare.
//!
//! This is the extension's **lifecycle**, not a window: it answers which
//! windowing APIs it supports, tracks create / parent / show / hide / resize
//! / destroy, and refuses calls made out of order, but it draws nothing.
//! Opening a real window needs a windowing dependency and a display, and CI
//! has neither, so step 11 (`11-plugin-gui-windows.md`) runs the lifecycle
//! headless against this.
//!
//! **It runs its "event loop" the way JUCE and DPF GUIs do** (step 11,
//! MOO-300): on `create` it registers a timer with the host and a pipe's
//! read end as a file descriptor. Each timer tick writes a byte into the
//! pipe, which makes the fd readable, and each `on_fd` reads it back. So a
//! host that services both is seen doing so, and `destroy` unregisters both
//! and closes the pipe.
//!
//! **What a test can read**, through `params.get_value` on the gain's GUI
//! variant with a probe id no parameter list names ([`PROBE_IDS`]): this
//! instance's timer ticks and fd reads, and, across every instance this
//! library has made, how many GUIs, timers and fds are live and how many
//! GUIs were dropped without being destroyed. The library-wide counts are
//! process-global, so a test that reads them must not run beside another
//! that opens GUIs.
//!
//! Asked for a scale other than 1, it asks the host to resize its window to
//! its size at that scale, as a real GUI does when the scale changes.

use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, HostGui, Window};
#[cfg(unix)]
use clack_extensions::posix_fd::{FdFlags, HostPosixFd};
use clack_extensions::timer::{HostTimer, TimerId};
use clack_plugin::prelude::{HostMainThreadHandle, PluginError};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};

/// The size the GUI reports until the host sets another.
pub const GUI_DEFAULT_SIZE: GuiSize = GuiSize {
    width: 320,
    height: 200,
};

/// The smallest size `adjust_size` and `set_size` allow.
pub const GUI_MIN_SIZE: GuiSize = GuiSize {
    width: 160,
    height: 100,
};

/// The period the GUI's timer asks for, in milliseconds.
pub const GUI_TIMER_MS: u32 = 16;

/// Timer ticks this instance's GUI has received.
pub const PROBE_TIMER_TICKS: u32 = 0xFFFF_0001;
/// Times this instance's GUI was called back for its readable pipe.
pub const PROBE_FD_READS: u32 = 0xFFFF_0002;
/// GUIs created and not yet destroyed, across the library.
pub const PROBE_GUIS_LIVE: u32 = 0xFFFF_0010;
/// Timers registered with a host and not unregistered, across the library.
pub const PROBE_TIMERS_LIVE: u32 = 0xFFFF_0011;
/// File descriptors registered with a host and not unregistered, across
/// the library.
pub const PROBE_FDS_LIVE: u32 = 0xFFFF_0012;
/// GUIs whose plugin was destroyed while they were still open: a host that
/// broke CLAP's order. Zero is the contract.
pub const PROBE_GUIS_LEAKED: u32 = 0xFFFF_0013;

/// Every probe id, none of which any parameter list names.
pub const PROBE_IDS: [u32; 6] = [
    PROBE_TIMER_TICKS,
    PROBE_FD_READS,
    PROBE_GUIS_LIVE,
    PROBE_TIMERS_LIVE,
    PROBE_FDS_LIVE,
    PROBE_GUIS_LEAKED,
];

static GUIS_LIVE: AtomicU64 = AtomicU64::new(0);
static TIMERS_LIVE: AtomicU64 = AtomicU64::new(0);
static FDS_LIVE: AtomicU64 = AtomicU64::new(0);
static GUIS_LEAKED: AtomicU64 = AtomicU64::new(0);

/// The windowing API a real GUI would use on this platform.
#[cfg(target_os = "macos")]
const NATIVE_API: GuiApiType<'static> = GuiApiType::COCOA;
#[cfg(target_os = "windows")]
const NATIVE_API: GuiApiType<'static> = GuiApiType::WIN32;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const NATIVE_API: GuiApiType<'static> = GuiApiType::X11;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    None,
    Embedded { parented: bool },
    Floating,
}

/// The pipe the GUI's timer writes into and its fd callback reads from.
#[cfg(unix)]
struct Pipe {
    reader: std::io::PipeReader,
    writer: std::io::PipeWriter,
}

/// One instance's GUI state. It lives on the plugin's main-thread type, so
/// `Cell` is enough.
pub(crate) struct TestGui {
    stage: Cell<Stage>,
    visible: Cell<bool>,
    size: Cell<GuiSize>,
    scale: Cell<f64>,
    timer: Cell<Option<TimerId>>,
    #[cfg(unix)]
    pipe: RefCell<Option<Pipe>>,
    timer_ticks: Cell<u64>,
    fd_reads: Cell<u64>,
}

impl TestGui {
    pub(crate) fn new() -> Self {
        Self {
            stage: Cell::new(Stage::None),
            visible: Cell::new(false),
            size: Cell::new(GUI_DEFAULT_SIZE),
            scale: Cell::new(1.0),
            timer: Cell::new(None),
            #[cfg(unix)]
            pipe: RefCell::new(None),
            timer_ticks: Cell::new(0),
            fd_reads: Cell::new(0),
        }
    }

    /// The value of probe `id`, or `None` when `id` is not a probe.
    pub(crate) fn probe(&self, id: u32) -> Option<f64> {
        let global = |counter: &AtomicU64| counter.load(Ordering::Relaxed) as f64;
        Some(match id {
            PROBE_TIMER_TICKS => self.timer_ticks.get() as f64,
            PROBE_FD_READS => self.fd_reads.get() as f64,
            PROBE_GUIS_LIVE => global(&GUIS_LIVE),
            PROBE_TIMERS_LIVE => global(&TIMERS_LIVE),
            PROBE_FDS_LIVE => global(&FDS_LIVE),
            PROBE_GUIS_LEAKED => global(&GUIS_LEAKED),
            _ => return None,
        })
    }

    pub(crate) fn is_api_supported(&self, configuration: GuiConfiguration) -> bool {
        // Wayland has no embedding API, so a Wayland GUI is floating or
        // nothing (`11-plugin-gui-windows.md`, "Platform facts").
        let wayland_floating = cfg!(all(unix, not(target_os = "macos")))
            && configuration.api_type == GuiApiType::WAYLAND
            && configuration.is_floating;
        configuration.api_type == NATIVE_API || wayland_floating
    }

    pub(crate) fn preferred_api(&self) -> Option<GuiConfiguration<'static>> {
        Some(GuiConfiguration {
            api_type: NATIVE_API,
            is_floating: false,
        })
    }

    pub(crate) fn create(
        &self,
        host: &HostMainThreadHandle,
        configuration: GuiConfiguration,
    ) -> Result<(), PluginError> {
        if !self.is_api_supported(configuration) {
            return Err(PluginError::Message("unsupported GUI API"));
        }
        if self.stage.get() != Stage::None {
            return Err(PluginError::Message("GUI already created"));
        }
        self.stage.set(if configuration.is_floating {
            Stage::Floating
        } else {
            Stage::Embedded { parented: false }
        });
        self.visible.set(false);
        GUIS_LIVE.fetch_add(1, Ordering::Relaxed);
        self.start_event_loop(host);
        Ok(())
    }

    /// Register the timer and the pipe's fd, where the host offers them.
    fn start_event_loop(&self, host: &HostMainThreadHandle) {
        if let Some(timer) = host.get_extension::<HostTimer>() {
            if let Ok(id) = timer.register_timer(host, GUI_TIMER_MS) {
                self.timer.set(Some(id));
                TIMERS_LIVE.fetch_add(1, Ordering::Relaxed);
            }
        }
        #[cfg(unix)]
        if let (Some(fds), Ok((reader, writer))) = (host.get_extension::<HostPosixFd>(), std::io::pipe()) {
            use std::os::fd::AsRawFd;
            if fds.register_fd(host, reader.as_raw_fd(), FdFlags::READ).is_ok() {
                FDS_LIVE.fetch_add(1, Ordering::Relaxed);
                *self.pipe.borrow_mut() = Some(Pipe { reader, writer });
            }
        }
    }

    fn stop_event_loop(&self, host: &HostMainThreadHandle) {
        if let Some(id) = self.timer.take() {
            if let Some(timer) = host.get_extension::<HostTimer>() {
                let _ = timer.unregister_timer(host, id);
            }
            TIMERS_LIVE.fetch_sub(1, Ordering::Relaxed);
        }
        #[cfg(unix)]
        if let Some(pipe) = self.pipe.borrow_mut().take() {
            use std::os::fd::AsRawFd;
            if let Some(fds) = host.get_extension::<HostPosixFd>() {
                let _ = fds.unregister_fd(host, pipe.reader.as_raw_fd());
            }
            FDS_LIVE.fetch_sub(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn destroy(&self, host: &HostMainThreadHandle) {
        if self.stage.get() != Stage::None {
            GUIS_LIVE.fetch_sub(1, Ordering::Relaxed);
        }
        self.stop_event_loop(host);
        self.stage.set(Stage::None);
        self.visible.set(false);
        self.size.set(GUI_DEFAULT_SIZE);
    }

    pub(crate) fn on_timer(&self, id: TimerId) {
        if self.timer.get() != Some(id) {
            return;
        }
        self.timer_ticks.set(self.timer_ticks.get() + 1);
        #[cfg(unix)]
        if let Some(pipe) = self.pipe.borrow_mut().as_mut() {
            use std::io::Write;
            let _ = pipe.writer.write_all(&[1]);
        }
    }

    #[cfg(unix)]
    pub(crate) fn on_fd(&self, fd: std::os::unix::io::RawFd, flags: FdFlags) {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        let mut pipe = self.pipe.borrow_mut();
        let Some(pipe) = pipe.as_mut().filter(|pipe| pipe.reader.as_raw_fd() == fd) else {
            return;
        };
        if !flags.contains(FdFlags::READ) {
            return;
        }
        // Readable, so this returns at once. One read per tick holds at most
        // a handful of bytes, one per timer tick.
        let mut bytes = [0u8; 64];
        if pipe.reader.read(&mut bytes).is_ok_and(|read| read > 0) {
            self.fd_reads.set(self.fd_reads.get() + 1);
        }
    }

    pub(crate) fn set_scale(&self, host: &HostMainThreadHandle, scale: f64) -> Result<(), PluginError> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(PluginError::Message("GUI scale must be positive"));
        }
        let changed = scale != self.scale.get();
        self.scale.set(scale);
        if changed && self.stage.get() != Stage::None {
            if let Some(gui) = host.get_extension::<HostGui>() {
                let size = self.size.get();
                let scaled = |pixels: u32| (f64::from(pixels) * scale).round() as u32;
                let _ = gui.request_resize(host, scaled(size.width), scaled(size.height));
            }
        }
        Ok(())
    }

    pub(crate) fn size(&self) -> Option<GuiSize> {
        (self.stage.get() != Stage::None).then(|| self.size.get())
    }

    pub(crate) fn adjust_size(&self, size: GuiSize) -> Option<GuiSize> {
        Some(GuiSize {
            width: size.width.max(GUI_MIN_SIZE.width),
            height: size.height.max(GUI_MIN_SIZE.height),
        })
    }

    pub(crate) fn set_size(&self, size: GuiSize) -> Result<(), PluginError> {
        self.created()?;
        match self.adjust_size(size) {
            Some(adjusted) if adjusted == size => {
                self.size.set(size);
                Ok(())
            }
            _ => Err(PluginError::Message("GUI size below the minimum")),
        }
    }

    pub(crate) fn set_parent(&self, _window: Window) -> Result<(), PluginError> {
        match self.stage.get() {
            Stage::Embedded { .. } => {
                self.stage.set(Stage::Embedded { parented: true });
                Ok(())
            }
            Stage::Floating => Err(PluginError::Message("a floating GUI has no parent")),
            Stage::None => Err(PluginError::Message("GUI not created")),
        }
    }

    pub(crate) fn set_transient(&self, _window: Window) -> Result<(), PluginError> {
        match self.stage.get() {
            Stage::Floating => Ok(()),
            _ => Err(PluginError::Message("only a floating GUI is transient")),
        }
    }

    pub(crate) fn show(&self) -> Result<(), PluginError> {
        match self.stage.get() {
            Stage::Embedded { parented: false } => {
                Err(PluginError::Message("an embedded GUI needs a parent first"))
            }
            Stage::None => Err(PluginError::Message("GUI not created")),
            _ => {
                self.visible.set(true);
                Ok(())
            }
        }
    }

    pub(crate) fn hide(&self) -> Result<(), PluginError> {
        self.created()?;
        self.visible.set(false);
        Ok(())
    }

    fn created(&self) -> Result<(), PluginError> {
        match self.stage.get() {
            Stage::None => Err(PluginError::Message("GUI not created")),
            _ => Ok(()),
        }
    }
}

impl Drop for TestGui {
    /// The plugin is being destroyed. A GUI still open here is one the host
    /// never destroyed, which CLAP forbids: counted, and its registrations
    /// forgotten so the live counts stay true.
    fn drop(&mut self) {
        if self.stage.get() != Stage::None {
            GUIS_LEAKED.fetch_add(1, Ordering::Relaxed);
            GUIS_LIVE.fetch_sub(1, Ordering::Relaxed);
        }
        if self.timer.take().is_some() {
            TIMERS_LIVE.fetch_sub(1, Ordering::Relaxed);
        }
        #[cfg(unix)]
        if self.pipe.borrow_mut().take().is_some() {
            FDS_LIVE.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// Implement the GUI, timer and fd extensions for a main-thread type by
/// delegating to its `gui: TestGui` field, with its `host` handle. Both
/// plugins need the identical impls, and the extensions are only
/// *registered* for the `.gui` variants.
macro_rules! impl_test_gui {
    ($main:ident) => {
        impl clack_extensions::gui::PluginGuiImpl for $main<'_> {
            fn is_api_supported(
                &self,
                configuration: clack_extensions::gui::GuiConfiguration,
            ) -> bool {
                self.gui.is_api_supported(configuration)
            }

            fn get_preferred_api(&self) -> Option<clack_extensions::gui::GuiConfiguration<'_>> {
                self.gui.preferred_api()
            }

            fn create(
                &self,
                configuration: clack_extensions::gui::GuiConfiguration,
            ) -> Result<(), clack_plugin::prelude::PluginError> {
                self.gui.create(&self.host, configuration)
            }

            fn destroy(&self) {
                self.gui.destroy(&self.host)
            }

            fn set_scale(&self, scale: f64) -> Result<(), clack_plugin::prelude::PluginError> {
                self.gui.set_scale(&self.host, scale)
            }

            fn get_size(&self) -> Option<clack_extensions::gui::GuiSize> {
                self.gui.size()
            }

            fn can_resize(&self) -> bool {
                true
            }

            fn adjust_size(
                &self,
                size: clack_extensions::gui::GuiSize,
            ) -> Option<clack_extensions::gui::GuiSize> {
                self.gui.adjust_size(size)
            }

            fn set_size(
                &self,
                size: clack_extensions::gui::GuiSize,
            ) -> Result<(), clack_plugin::prelude::PluginError> {
                self.gui.set_size(size)
            }

            fn set_parent(
                &self,
                window: clack_extensions::gui::Window,
            ) -> Result<(), clack_plugin::prelude::PluginError> {
                self.gui.set_parent(window)
            }

            fn set_transient(
                &self,
                window: clack_extensions::gui::Window,
            ) -> Result<(), clack_plugin::prelude::PluginError> {
                self.gui.set_transient(window)
            }

            fn show(&self) -> Result<(), clack_plugin::prelude::PluginError> {
                self.gui.show()
            }

            fn hide(&self) -> Result<(), clack_plugin::prelude::PluginError> {
                self.gui.hide()
            }
        }

        impl clack_extensions::timer::PluginTimerImpl for $main<'_> {
            fn on_timer(&self, timer_id: clack_extensions::timer::TimerId) {
                self.gui.on_timer(timer_id)
            }
        }

        #[cfg(unix)]
        impl clack_extensions::posix_fd::PluginPosixFdImpl for $main<'_> {
            fn on_fd(&self, fd: std::os::unix::io::RawFd, flags: clack_extensions::posix_fd::FdFlags) {
                self.gui.on_fd(fd, flags)
            }
        }
    };
}

pub(crate) use impl_test_gui;

/// Register the GUI extension and the two its event loop answers on.
macro_rules! register_test_gui {
    ($builder:expr) => {{
        $builder
            .register::<clack_extensions::gui::PluginGui>()
            .register::<clack_extensions::timer::PluginTimer>();
        #[cfg(unix)]
        $builder.register::<clack_extensions::posix_fd::PluginPosixFd>();
    }};
}

pub(crate) use register_test_gui;
