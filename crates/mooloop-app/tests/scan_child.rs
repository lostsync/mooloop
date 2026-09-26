//! `mooloop --scan-plugin <path>` is the plugin scanner's child (MOO-80),
//! and it is the shipped binary itself -- the rpm, the deb, the AppImage and
//! the macOS bundle all carry it with nothing added. These tests run the real
//! `mooloop` binary in that mode.
//!
//! The child must load its one file and touch nothing else: no log file, no
//! settings, no audio client, no window. It is run here with every directory
//! mooloop writes to pointed at an empty scratch directory and no display or
//! JACK server to reach; it must answer, exit 0 promptly, and leave the
//! directory empty. A child that had started logging would have written
//! `mooloop.log` there, and one that had loaded settings or opened a window
//! would have failed or stalled without a display.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use mooloop_plugin_host::scan::{self, ChildCommand, ChildReport, SCAN_FLAG};

const MOOLOOP: &str = env!("CARGO_BIN_EXE_mooloop");

/// The test plugin's library, which cargo built next to this test because
/// this crate names it as a dev-dependency.
fn test_plugin_path() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary has a path");
    let deps = exe.parent().expect("the test binary is in a directory");
    let name = format!(
        "{}mooloop_test_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let found = [deps, deps.parent().unwrap_or(deps)]
        .into_iter()
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file());
    found.unwrap_or_else(|| panic!("{name} is not next to {}", exe.display()))
}

/// Run `mooloop --scan-plugin <plugin>` with nowhere to write and nothing to
/// connect to. Returns its exit status, its stdout, and what it left behind.
fn run_isolated(plugin: &Path) -> (std::process::ExitStatus, String, Vec<PathBuf>, Duration) {
    let home = tempfile::tempdir().expect("a scratch home");
    let started = Instant::now();
    let output = Command::new(MOOLOOP)
        .arg(SCAN_FLAG)
        .arg(plugin)
        .env_clear()
        .env("HOME", home.path())
        .env("MOOLOOP_CONFIG_DIR", home.path().join("config"))
        .env("MOOLOOP_STATE_DIR", home.path().join("state"))
        .env("MOOLOOP_DATA_DIR", home.path().join("data"))
        .env("XDG_RUNTIME_DIR", home.path().join("run"))
        .env("JACK_NO_START_SERVER", "1")
        .output()
        .expect("mooloop starts");
    let took = started.elapsed();
    let left: Vec<PathBuf> = std::fs::read_dir(home.path())
        .expect("the scratch home is readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    (
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
        left,
        took,
    )
}

#[test]
fn the_shipped_binary_scans_a_plugin_and_touches_nothing_else() {
    let plugin = test_plugin_path();
    let (status, stdout, left, took) = run_isolated(&plugin);
    assert!(status.success(), "{status}; stdout: {stdout}");
    assert!(left.is_empty(), "the scan child wrote {left:?}");
    assert!(took < Duration::from_secs(10), "the scan child took {took:?}");
    let Some(ChildReport::Scanned { plugins }) = scan::parse_report(&stdout) else {
        panic!("no report in: {stdout}");
    };
    let mut ids: Vec<&str> = plugins.iter().map(|p| p.plugin.id.as_str()).collect();
    ids.sort_unstable();
    let mut expected = mooloop_test_plugin::PLUGIN_IDS.to_vec();
    expected.sort_unstable();
    assert_eq!(ids, expected);
}

/// A file that is not a plugin is a report of a failure, not a crash.
#[test]
fn the_shipped_binary_reports_a_file_that_will_not_load() {
    let home = tempfile::tempdir().expect("a scratch directory");
    let bogus = home.path().join("bogus.clap");
    std::fs::write(&bogus, b"not a library").expect("a bogus file");
    let failure = scan::run_child(
        &ChildCommand {
            program: PathBuf::from(MOOLOOP),
            args: vec![SCAN_FLAG.into()],
        },
        &bogus,
        Duration::from_secs(10),
    )
    .expect_err("a text file is not a plugin");
    assert_eq!(failure.kind, scan::FailureKind::Load, "{failure:?}");
}
