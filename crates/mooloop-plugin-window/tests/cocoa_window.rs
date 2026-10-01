//! A real Cocoa plugin window, made, driven and closed on the main thread,
//! which libtest never runs a test on: hence a `main` of its own.
//!
//! It **opens a window on the desktop it runs on**, so it runs only where
//! that is wanted: on CI (`CI` set) or when asked
//! (`MOOLOOP_WINDOW_TESTS=1 cargo test -p mooloop-plugin-window --test
//! cocoa_window`). Off macOS there is nothing to test; the X11 twin is the
//! ignored `a_window_on_a_real_x_server` in `src/x11.rs`.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    let wanted = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if !(wanted("CI") || wanted("MOOLOOP_WINDOW_TESTS")) {
        println!("cocoa_window: skipped; opens a window (set MOOLOOP_WINDOW_TESTS=1)");
        return;
    }
    macos::a_window_on_the_main_thread();
    macos::appkit_is_refused_off_the_main_thread();
    println!("cocoa_window: ok");
}

#[cfg(target_os = "macos")]
mod macos {
    use mooloop_plugin_window::{
        GuiSize, NativeWindow, PluginWindowEvent, PluginWindowSpec, PluginWindows, WindowError,
    };
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSView};
    use objc2_foundation::NSSize;

    const fn size(width: u32, height: u32) -> GuiSize {
        GuiSize { width, height }
    }

    pub fn a_window_on_the_main_thread() {
        let mtm = MainThreadMarker::new().expect("a harness-less test's main is the main thread");
        // What winit has made by the time the pump runs; a bare binary has
        // to make it itself.
        let _app = NSApplication::sharedApplication(mtm);

        let mut windows = PluginWindows::connect().expect("the main thread");
        let id = windows
            .create(&PluginWindowSpec {
                title: "mooloop test window",
                size: size(320, 200),
                resizable: false,
            })
            .expect("a panel");

        let parent = id.native();
        let ns_view = parent.as_ns_view().expect("a Cocoa view");
        assert!(!ns_view.is_null(), "the content view goes to the plugin");
        // SAFETY: the panel is open, and its content view is an NSView.
        let view: &NSView = unsafe { &*ns_view.cast::<NSView>() };
        let window = view.window().expect("the view is the panel's content view");
        assert_eq!(window.title().to_string(), "mooloop test window");
        let content = |window: &objc2_app_kit::NSWindow| {
            let size = window.contentRectForFrameRect(window.frame()).size;
            (size.width, size.height)
        };
        assert_eq!(content(&window), (320.0, 200.0), "points, unconverted");

        windows.set_transient_for(id, Some(parent)).expect("a Cocoa parent");
        assert_eq!(
            windows.set_transient_for(id, Some(NativeWindow::x11(7))),
            Err(WindowError::ForeignParent(NativeWindow::x11(7)))
        );
        windows.show(id).unwrap();
        windows.resize(id, size(400, 250)).unwrap();
        assert_eq!(content(&window), (400.0, 250.0));
        windows.set_resizable(id, true).unwrap();
        windows.set_title(id, "renamed").unwrap();
        assert_eq!(window.title().to_string(), "renamed");
        let mut events = Vec::new();
        windows.drain_events(&mut events).unwrap();
        assert!(
            !events
                .iter()
                .any(|(_, event)| matches!(event, PluginWindowEvent::Resized(_))),
            "a size the pump set is not reported back: {events:?}"
        );

        // The user drags its edge.
        window.setContentSize(NSSize::new(500.0, 300.0));
        events.clear();
        windows.drain_events(&mut events).unwrap();
        assert!(
            events.contains(&(id, PluginWindowEvent::Resized(size(500, 300)))),
            "{events:?}"
        );
        assert_eq!(windows.size(id), Some(size(500, 300)));

        // The close button asks, and the panel stays until destroyed.
        window.performClose(None);
        events.clear();
        windows.drain_events(&mut events).unwrap();
        assert!(events.contains(&(id, PluginWindowEvent::CloseRequested)), "{events:?}");
        assert_eq!(windows.windows().count(), 1, "still open after the close button");

        windows.hide(id).unwrap();
        windows.destroy(id).unwrap();
        assert_eq!(windows.destroy(id), Err(WindowError::UnknownWindow(id)));
        assert_eq!(windows.windows().count(), 0);
        assert_eq!(id.native().as_ns_view(), Some(std::ptr::null_mut()), "names nothing now");
    }

    pub fn appkit_is_refused_off_the_main_thread() {
        let refused = std::thread::spawn(|| PluginWindows::connect().err())
            .join()
            .expect("the thread ran");
        assert_eq!(refused, Some(WindowError::NotMainThread));
    }
}
