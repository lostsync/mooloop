//! Finding and pressing the real window's controls from a unit test.
//!
//! `ElementHandle`'s search API is the obvious tool and cannot be used here:
//! it needs element debug info, which only the `mcp` feature compiles in, and
//! CI builds without it -- so a test written on it would find nothing and
//! pass. This walks the item tree instead, by accessible role, which every
//! build carries, including the open popups a menu or a chip puts up.

use crate::MainWindow;
use i_slint_core::accessibility::AccessibleStringProperty;
use i_slint_core::item_tree::ItemRc;
use i_slint_core::items::AccessibleRole;
use i_slint_core::window::{PopupWindowLocation, WindowInner};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition};
use std::ops::ControlFlow;

/// One control, as a user meets it: where it is, what it is called, and what
/// its readout says.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Control {
    pub label: String,
    pub value: String,
    pub checked: bool,
    pub centre: (f32, f32),
}

fn collect(
    root: ItemRc,
    offset: (f32, f32),
    role: AccessibleRole,
    bounds: (f32, f32),
    found: &mut Vec<Control>,
) {
    root.visit_descendants(|item| {
        if item.accessible_role() != role || !item.is_visible() {
            return ControlFlow::<()>::Continue(());
        }
        let geometry = item.geometry();
        let origin = item.map_to_window(geometry.origin);
        let centre = (
            offset.0 + origin.x + geometry.size.width / 2.0,
            offset.1 + origin.y + geometry.size.height / 2.0,
        );
        let inside = centre.0 > 0.0 && centre.1 > 0.0 && centre.0 < bounds.0 && centre.1 < bounds.1;
        if inside && geometry.size.width > 0.0 && geometry.size.height > 0.0 {
            let text = |what| {
                item.accessible_string_property(what)
                    .map(|text| text.to_string())
                    .unwrap_or_default()
            };
            found.push(Control {
                label: text(AccessibleStringProperty::Label),
                value: text(AccessibleStringProperty::Value),
                checked: text(AccessibleStringProperty::Checked) == "true",
                centre,
            });
        }
        ControlFlow::Continue(())
    });
}

/// Every visible element with `role`, in tree order -- the window first,
/// then each open popup -- that lies inside the window.
pub(crate) fn controls(window: &MainWindow, role: AccessibleRole) -> Vec<Control> {
    let size = window.window().size().to_logical(window.window().scale_factor());
    let bounds = (size.width, size.height);
    let inner = WindowInner::from_pub(window.window());
    let mut found = Vec::new();
    collect(ItemRc::new_root(inner.component()), (0.0, 0.0), role, bounds, &mut found);
    for popup in inner.active_popups().iter() {
        let offset = match popup.location {
            PopupWindowLocation::ChildWindow(at) => (at.x, at.y),
            _ => (0.0, 0.0),
        };
        collect(ItemRc::new_root(popup.component.clone()), offset, role, bounds, &mut found);
    }
    found
}

pub(crate) fn sliders(window: &MainWindow) -> Vec<Control> {
    controls(window, AccessibleRole::Slider)
}

fn at(point: (f32, f32)) -> LogicalPosition {
    LogicalPosition::new(point.0, point.1)
}

/// One wheel step up at the control's centre: the smallest edit a control
/// can be given, and it goes through the same write a drag does.
pub(crate) fn wheel(window: &MainWindow, control: &Control) {
    let window = window.window();
    window.dispatch_event(WindowEvent::PointerMoved { position: at(control.centre) });
    window.dispatch_event(WindowEvent::PointerScrolled {
        position: at(control.centre),
        delta_x: 0.0,
        delta_y: -60.0,
    });
}

pub(crate) fn click(window: &MainWindow, point: (f32, f32)) {
    let window = window.window();
    window.dispatch_event(WindowEvent::PointerMoved { position: at(point) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(point),
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(point),
        button: PointerEventButton::Left,
    });
}

/// Typing, one character at a time, into whatever holds the focus.
pub(crate) fn type_text(window: &MainWindow, text: &str) {
    for character in text.chars() {
        let text = slint::SharedString::from(character.to_string());
        window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window.window().dispatch_event(WindowEvent::KeyReleased { text });
    }
}

/// The software-less testing backend, installed at most once per thread:
/// `init_no_event_loop` panics on the second window a test builds.
pub(crate) fn install_backend() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            ..Default::default()
        },
    )))
    .ok();
}
