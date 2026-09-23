//! The `gui` extension the `.gui` variants declare.
//!
//! This is the extension's **lifecycle**, not a window: it answers which
//! windowing APIs it supports, tracks create / parent / show / hide / resize
//! / destroy, and refuses calls made out of order, but it draws nothing. Step
//! 01 asked for "a trivial window"; opening a real one needs a windowing
//! dependency (an X11 client on Linux, AppKit on macOS) and a display, and CI
//! has neither. The host side that would exercise it is step 11
//! (`11-plugin-gui-windows.md`), whose own tests run the lifecycle headless,
//! so the window itself is left to that step.

use clack_extensions::gui::{GuiApiType, GuiConfiguration, GuiSize, Window};
use clack_plugin::prelude::PluginError;
use std::cell::Cell;

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

/// One instance's GUI state. It lives on the plugin's main-thread type, so
/// `Cell` is enough.
pub(crate) struct TestGui {
    stage: Cell<Stage>,
    visible: Cell<bool>,
    size: Cell<GuiSize>,
    scale: Cell<f64>,
}

impl TestGui {
    pub(crate) fn new() -> Self {
        Self {
            stage: Cell::new(Stage::None),
            visible: Cell::new(false),
            size: Cell::new(GUI_DEFAULT_SIZE),
            scale: Cell::new(1.0),
        }
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

    pub(crate) fn create(&self, configuration: GuiConfiguration) -> Result<(), PluginError> {
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
        Ok(())
    }

    pub(crate) fn destroy(&self) {
        self.stage.set(Stage::None);
        self.visible.set(false);
        self.size.set(GUI_DEFAULT_SIZE);
    }

    pub(crate) fn set_scale(&self, scale: f64) -> Result<(), PluginError> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(PluginError::Message("GUI scale must be positive"));
        }
        self.scale.set(scale);
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

/// Implement `PluginGuiImpl` for a main-thread type by delegating to its
/// `gui: TestGui` field. Both plugins need the identical impl, and the
/// extension is only *registered* for the `.gui` variants.
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
                self.gui.create(configuration)
            }

            fn destroy(&self) {
                self.gui.destroy()
            }

            fn set_scale(&self, scale: f64) -> Result<(), clack_plugin::prelude::PluginError> {
                self.gui.set_scale(scale)
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
    };
}

pub(crate) use impl_test_gui;
