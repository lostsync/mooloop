//! Step 11's host side, headless (`docs/plans/plugin-hosting/11-plugin-gui-windows.md`,
//! MOO-300): a plugin's GUI opened, resized, closed and reopened through
//! [`HostedGui`], and the timer and fd its event loop registers, serviced
//! without waiting.
//!
//! The test plugin's GUI draws nothing and needs no display: it tracks its
//! lifecycle and refuses calls made out of order. Its counters are read
//! through probe ids its parameter list never names
//! (`mooloop_test_plugin::PROBE_IDS`). The library-wide ones are
//! process-global, so every test here that opens a GUI holds [`SERIAL`].
//!
//! X11 is the only API the host speaks, and the test plugin offers it on
//! Linux and the BSDs only.

#![cfg(all(unix, not(target_os = "macos")))]

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mooloop_core::{PluginFormat, PluginRef, PluginState};
use mooloop_plugin_host::clap::ClapInstance;
use mooloop_plugin_host::{
    AudioConfig, GuiConfig, GuiError, GuiRequest, GuiSize, HostedInstance, IoRegistrations, NativeWindow,
};
use mooloop_test_plugin as test_plugin;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The test plugin's library, next to the running test (see `tests/spike.rs`).
fn test_plugin_path() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary has a path");
    let deps = exe.parent().expect("the test binary is in a directory");
    let name = format!(
        "{}mooloop_test_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    for dir in [deps, deps.parent().unwrap_or(deps)] {
        let candidate = dir.join(&name);
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("{name} is not next to {}", exe.display());
}

fn open(id: &str) -> ClapInstance {
    let plugin = PluginRef {
        format: PluginFormat::Clap,
        id: id.to_owned(),
        name: id.to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    };
    ClapInstance::open(
        &test_plugin_path(),
        &plugin,
        &PluginState::default(),
        AudioConfig {
            sample_rate: 48_000,
            max_frames: 512,
        },
    )
    .expect("the test plugin opens")
}

fn probe(instance: &mut ClapInstance, id: u32) -> u64 {
    instance.param_value(id).expect("the GUI variant answers its probes") as u64
}

/// The library-wide counts: GUIs, timers and fds live, and GUIs leaked.
fn live(instance: &mut ClapInstance) -> [u64; 4] {
    [
        probe(instance, test_plugin::PROBE_GUIS_LIVE),
        probe(instance, test_plugin::PROBE_TIMERS_LIVE),
        probe(instance, test_plugin::PROBE_FDS_LIVE),
        probe(instance, test_plugin::PROBE_GUIS_LEAKED),
    ]
}

/// Open `instance`'s GUI embedded in X11 window `parent` and show it.
fn open_embedded(instance: &mut ClapInstance, parent: u64) {
    let gui = instance.gui().expect("the GUI variant has a GUI");
    assert!(gui.is_api_supported(GuiConfig::X11_EMBEDDED));
    gui.create(GuiConfig::X11_EMBEDDED).expect("it opens");
    gui.set_scale(1.0).expect("it takes a scale");
    gui.set_parent(NativeWindow::x11(parent)).expect("it embeds");
    gui.show().expect("it shows");
}

fn requests(instance: &mut ClapInstance) -> Vec<GuiRequest> {
    let mut requests = Vec::new();
    instance
        .gui()
        .expect("a GUI")
        .take_requests(&mut |request| requests.push(request));
    requests
}

#[test]
fn a_plugin_without_a_gui_has_none_and_nothing_to_service() {
    let mut plain = open(test_plugin::GAIN_ID);
    assert!(plain.gui().is_none());
    assert_eq!(plain.io_registrations(), IoRegistrations::default());
    assert_eq!(plain.service_io(Instant::now() + Duration::from_secs(1)).timers_fired, 0);
}

#[test]
fn the_gui_opens_closes_and_reopens_in_order_and_refuses_calls_out_of_it() {
    let _serial = serial();
    let mut instance = open(test_plugin::GAIN_GUI_ID);
    let gui = instance.gui().expect("a GUI");
    assert_eq!(gui.preferred_api(), Some(GuiConfig::X11_EMBEDDED));
    assert_eq!(gui.show(), Err(GuiError::NotOpen), "nothing shows before it opens");
    assert_eq!(gui.set_parent(NativeWindow::x11(7)), Err(GuiError::NotOpen));
    gui.create(GuiConfig::X11_EMBEDDED).expect("it opens");
    assert_eq!(gui.create(GuiConfig::X11_EMBEDDED), Err(GuiError::AlreadyOpen));
    assert!(
        matches!(gui.show(), Err(GuiError::Refused(_))),
        "an embedded GUI shows only once it has a parent"
    );
    assert!(
        matches!(gui.set_transient(NativeWindow::x11(7)), Err(GuiError::Refused(_))),
        "only a floating GUI is transient"
    );
    gui.set_parent(NativeWindow::x11(7)).expect("it embeds");
    gui.show().expect("it shows");
    assert!(gui.is_visible());
    assert_eq!(gui.open_config(), Some(GuiConfig::X11_EMBEDDED));
    assert_eq!(
        gui.size(),
        Some(GuiSize {
            width: test_plugin::GUI_DEFAULT_SIZE.width,
            height: test_plugin::GUI_DEFAULT_SIZE.height,
        })
    );
    gui.hide().expect("it hides");
    assert!(!gui.is_visible());
    gui.destroy();
    assert_eq!(gui.open_config(), None);
    gui.destroy();

    // Reopened, floating this time.
    assert!(gui.is_api_supported(GuiConfig::X11_FLOATING));
    gui.create(GuiConfig::X11_FLOATING).expect("it opens again");
    gui.suggest_title("Test Gain (GUI) - Channel 1");
    gui.set_transient(NativeWindow::x11(9)).expect("a floating GUI stays above its window");
    gui.show().expect("it shows");
    gui.destroy();

    assert_eq!(live(&mut instance), [0, 0, 0, 0]);
    assert_eq!(instance.misbehaviour(), 0, "the plugin saw every call on its main thread");
}

#[test]
fn resizing_goes_through_can_resize_adjust_size_and_set_size() {
    let _serial = serial();
    let mut instance = open(test_plugin::GAIN_GUI_ID);
    open_embedded(&mut instance, 11);
    let gui = instance.gui().expect("a GUI");
    assert!(gui.can_resize());
    let small = GuiSize {
        width: 10,
        height: 10,
    };
    let min = GuiSize {
        width: test_plugin::GUI_MIN_SIZE.width,
        height: test_plugin::GUI_MIN_SIZE.height,
    };
    assert_eq!(gui.adjust_size(small), Some(min));
    assert!(matches!(gui.set_size(small), Err(GuiError::Refused(_))));
    let big = GuiSize {
        width: 640,
        height: 480,
    };
    let adjusted = gui.adjust_size(big).expect("it adjusts");
    gui.set_size(adjusted).expect("it takes the adjusted size");
    assert_eq!(gui.size(), Some(big));
    gui.destroy();
    assert_eq!(gui.size(), None, "a closed GUI has no size");
}

#[test]
fn the_plugins_requests_wait_for_the_pump_and_are_dropped_once_it_closes() {
    let _serial = serial();
    let mut instance = open(test_plugin::GAIN_GUI_ID);
    open_embedded(&mut instance, 12);
    assert!(requests(&mut instance).is_empty());
    // A new scale makes the test GUI ask for its size at that scale, as a
    // real GUI does.
    instance.gui().expect("a GUI").set_scale(2.0).expect("a scale");
    assert_eq!(
        requests(&mut instance),
        [GuiRequest::Resize(GuiSize {
            width: test_plugin::GUI_DEFAULT_SIZE.width * 2,
            height: test_plugin::GUI_DEFAULT_SIZE.height * 2,
        })]
    );
    assert!(requests(&mut instance).is_empty(), "drained once");
    let gui = instance.gui().expect("a GUI");
    gui.set_scale(1.0).expect("a scale");
    gui.destroy();
    assert!(requests(&mut instance).is_empty(), "a request for a GUI that closed is dropped");
}

#[test]
fn the_guis_timer_and_fd_fire_from_the_service_call_and_are_gone_once_it_closes() {
    let _serial = serial();
    let mut instance = open(test_plugin::GAIN_GUI_ID);
    assert_eq!(instance.io_registrations(), IoRegistrations::default());
    open_embedded(&mut instance, 13);
    assert_eq!(instance.io_registrations(), IoRegistrations { timers: 1, fds: 1 });

    let start = Instant::now();
    let early = instance.service_io(start);
    assert_eq!(early.timers_fired, 0, "the timer is not due before its period");
    assert_eq!(early.fds_fired, 0, "and the pipe is empty");

    // One period on: the timer fires, writes into the pipe, and the same
    // pass finds the pipe readable and calls the plugin back for it.
    let period = Duration::from_millis(u64::from(test_plugin::GUI_TIMER_MS) + 1);
    let mut fired = instance.service_io(start + period);
    assert_eq!(fired.timers_fired, 1);
    assert_eq!(fired.fds_fired, 1);
    for tick in 2..=5u32 {
        fired += instance.service_io(start + period * tick);
    }
    assert_eq!(fired.timers_fired, 5);
    assert_eq!(probe(&mut instance, test_plugin::PROBE_TIMER_TICKS), 5, "the plugin saw each tick");
    assert_eq!(probe(&mut instance, test_plugin::PROBE_FD_READS), 5, "and read its pipe each time");

    // Nothing waits: a pass with nothing due returns at once.
    let quiet = Instant::now();
    let idle = instance.service_io(start + period * 5);
    assert_eq!((idle.timers_fired, idle.fds_fired), (0, 0), "the pipe was drained");
    assert!(quiet.elapsed() < Duration::from_millis(50));

    instance.gui().expect("a GUI").destroy();
    assert_eq!(instance.io_registrations(), IoRegistrations::default(), "gone with the GUI");
    let after = instance.service_io(start + period * 100);
    assert_eq!((after.timers_fired, after.fds_fired), (0, 0));
    assert_eq!(live(&mut instance), [0, 0, 0, 0]);
}

#[test]
fn an_instance_dropped_with_its_gui_open_destroys_the_gui_first() {
    let _serial = serial();
    // Held open so the library stays loaded and its counts can be read
    // after the other instance is gone.
    let mut witness = open(test_plugin::GAIN_GUI_ID);
    for cycle in 0..3 {
        let mut instance = open(test_plugin::GAIN_GUI_ID);
        open_embedded(&mut instance, 100 + cycle);
        instance.service_io(Instant::now() + Duration::from_secs(1));
        assert_eq!(live(&mut witness), [1, 1, 1, 0], "cycle {cycle}: one GUI, timer and fd live");
        drop(instance);
        assert_eq!(live(&mut witness), [0, 0, 0, 0], "cycle {cycle}: none leaked");
    }
}
