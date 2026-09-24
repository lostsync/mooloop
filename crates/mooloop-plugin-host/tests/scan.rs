//! Step 05's scanner (`docs/plans/plugin-hosting/05-the-scanner.md`, MOO-80).
//!
//! **A plugin that crashes or hangs while it is being scanned must not crash
//! or hang mooloop** (Adam's answer 4). These tests scan real misbehaving
//! libraries: the in-repo test plugin copied under the names that make it
//! abort or never return while its entry initialises
//! (`mooloop_test_plugin::CRASHES_ON_SCAN`, `HANGS_ON_SCAN`). If the scanner
//! loaded either in its own process, this test binary would die or never
//! finish; instead each costs one child, recorded as a failure in the cache.
//!
//! The children are `mooloop-scan-child`, this crate's test-only binary,
//! which runs the same `scan::run_child_from_args` the app runs as
//! `mooloop --scan-plugin`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mooloop_core::plugin::PluginFormat;
use mooloop_plugin_host::scan::{
    self, ChildCommand, FailureKind, PluginCache, ScanConfig, SCAN_FLAG,
};
use mooloop_test_plugin as test_plugin;

/// Long enough that a healthy child is never mistaken for a hung one on a
/// loaded CI runner, short enough that the hanging plugin does not make the
/// suite slow.
const TIMEOUT: Duration = Duration::from_secs(8);

/// The test plugin's library, next to this test binary (see `spike.rs`'s
/// `test_plugin_path` for why it is found there).
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

fn child() -> ChildCommand {
    ChildCommand {
        program: PathBuf::from(env!("CARGO_BIN_EXE_mooloop-scan-child")),
        args: vec![SCAN_FLAG.into()],
    }
}

fn config(dir: &Path) -> ScanConfig {
    ScanConfig {
        search_paths: vec![dir.to_path_buf()],
        timeout: TIMEOUT,
        child: child(),
    }
}

/// A scratch directory beside the test binary rather than in the system's
/// temporary directory, which may be mounted `noexec`: a library there would
/// fail to load for a reason that has nothing to do with the scanner.
fn scratch_dir() -> tempfile::TempDir {
    let exe = std::env::current_exe().expect("the test binary has a path");
    tempfile::Builder::new()
        .prefix("scan-test-")
        .tempdir_in(exe.parent().expect("the test binary is in a directory"))
        .expect("a scratch directory")
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).expect("the file exists")
}

fn failure_of(cache: &PluginCache, path: &Path) -> Option<FailureKind> {
    let path = canonical(path);
    cache
        .failures()
        .find(|(failed, _)| *failed == path)
        .map(|(_, failure)| failure.kind)
}

/// The plan's directory, plus the two misbehaving copies: one good plugin
/// file, a zero-byte `.clap`, a shell script that sleeps, a library that
/// aborts while loading and one that never returns. The scan finishes, finds
/// the four good plugins, and records each bad file with its own kind of
/// failure. All of it survives a save and a load, and a second scan over the
/// same files launches no child at all -- a crash or a hang is recorded once,
/// not paid for on every startup.
#[test]
fn a_crashing_or_hanging_plugin_costs_one_child_and_is_never_relaunched() {
    let dir = scratch_dir();
    let good = dir.path().join("good.clap");
    let empty = dir.path().join("empty.clap");
    let script = dir.path().join("sleeps.clap");
    let crashes = dir.path().join(format!("nested/{}", test_plugin::CRASHES_ON_SCAN));
    let hangs = dir.path().join(test_plugin::HANGS_ON_SCAN);
    std::fs::create_dir_all(dir.path().join("nested")).expect("a nested directory");
    for copy in [&good, &crashes, &hangs] {
        std::fs::copy(test_plugin_path(), copy).expect("the test plugin copies");
    }
    std::fs::write(&empty, b"").expect("an empty file");
    std::fs::write(&script, b"#!/bin/sh\nsleep 30\n").expect("a script");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("the script is executable");
    }
    // Not a candidate: only `*.clap` is.
    std::fs::copy(test_plugin_path(), dir.path().join("also-good.so")).expect("copies");

    let mut cache = PluginCache::default();
    let started = Instant::now();
    let summary = scan::scan(&config(dir.path()), &mut cache, |_, _, _| {});
    let took = started.elapsed();

    assert_eq!(summary.candidates, 5, "{summary:?}");
    assert_eq!(summary.launched, 5, "{summary:?}");
    assert_eq!(summary.plugins, 4, "{summary:?}");
    assert_eq!(summary.failed, 4, "{summary:?}");
    // The hang is cut off at the timeout rather than waited out.
    assert!(took < TIMEOUT * 3, "the scan took {took:?}");

    let mut ids: Vec<&str> = cache.plugins().map(|p| p.plugin.id.as_str()).collect();
    ids.sort_unstable();
    let mut expected = vec![
        test_plugin::GAIN_ID,
        test_plugin::GAIN_GUI_ID,
        test_plugin::SINE_ID,
        test_plugin::SINE_GUI_ID,
    ];
    expected.sort_unstable();
    assert_eq!(ids, expected);
    assert!(cache.plugins().all(|p| p.path == canonical(&good)));

    assert_eq!(failure_of(&cache, &empty), Some(FailureKind::Load));
    assert_eq!(failure_of(&cache, &script), Some(FailureKind::Load));
    assert_eq!(failure_of(&cache, &crashes), Some(FailureKind::Crashed));
    assert_eq!(failure_of(&cache, &hangs), Some(FailureKind::TimedOut));
    assert_eq!(failure_of(&cache, &good), None);

    // Through the file, as the next startup would see it.
    let cache_path = dir.path().join("config/plugins.toml");
    cache.save(&cache_path).expect("the cache saves");
    let mut reloaded = PluginCache::load(&cache_path);
    assert_eq!(reloaded, cache);

    let started = Instant::now();
    let again = scan::scan(&config(dir.path()), &mut reloaded, |_, _, _| {});
    assert_eq!(again.launched, 0, "{again:?}");
    assert_eq!(again.reused, 5, "{again:?}");
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    assert_eq!(reloaded, cache, "an unchanged directory scans to the same cache");

    // A file that changes is scanned again, and only that one.
    std::fs::write(&empty, b"x").expect("the empty file changes");
    let changed = scan::scan(&config(dir.path()), &mut reloaded, |_, _, _| {});
    assert_eq!(changed.launched, 1, "{changed:?}");
    assert_eq!(failure_of(&reloaded, &empty), Some(FailureKind::Load));

    // A file that is gone leaves the cache.
    std::fs::remove_file(&hangs).expect("the hanging plugin goes");
    let gone = scan::scan(&config(dir.path()), &mut reloaded, |_, _, _| {});
    assert_eq!(gone.removed, 1, "{gone:?}");
    assert_eq!(gone.launched, 0, "{gone:?}");

    // "Rescan all" forgets the failures, so the next scan tries them again.
    reloaded.clear_failures();
    let rescan = scan::scan(&config(dir.path()), &mut reloaded, |_, _, _| {});
    assert_eq!(rescan.launched, 3, "{rescan:?}");
    assert_eq!(failure_of(&reloaded, &crashes), Some(FailureKind::Crashed));
}

/// The child's report on the test plugin parses back into the plugins it
/// holds, with what a browser and step 06 need: the saved reference, the
/// features, the ports and whether there is a GUI.
#[test]
fn the_child_describes_every_plugin_in_the_file() {
    let path = test_plugin_path();
    let plugins = scan::run_child(&child(), &path, TIMEOUT).expect("the test plugin scans");
    let find = |id: &str| {
        plugins
            .iter()
            .find(|p| p.plugin.id == id)
            .unwrap_or_else(|| panic!("{id} is missing from {plugins:#?}"))
    };

    let gain = find(test_plugin::GAIN_ID);
    assert_eq!(gain.plugin.format, PluginFormat::Clap);
    assert_eq!(gain.plugin.name, "Test Gain");
    assert_eq!(gain.plugin.vendor, test_plugin::VENDOR);
    assert_eq!(gain.plugin.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(gain.path, path);
    assert!(gain.is_effect() && !gain.is_instrument(), "{gain:?}");
    assert_eq!(gain.audio_inputs, vec![2]);
    assert_eq!(gain.audio_outputs, vec![2]);
    assert_eq!((gain.note_inputs, gain.note_outputs), (0, 0));
    assert!(!gain.has_gui);
    assert!(gain.is_usable());

    let sine = find(test_plugin::SINE_ID);
    assert!(sine.is_instrument() && !sine.is_effect(), "{sine:?}");
    assert!(sine.audio_inputs.is_empty());
    assert_eq!(sine.audio_outputs, vec![2]);
    assert_eq!((sine.note_inputs, sine.note_outputs), (1, 0));

    assert!(find(test_plugin::GAIN_GUI_ID).has_gui);
    assert!(find(test_plugin::SINE_GUI_ID).has_gui);

    // What a saved song would ask for finds the file again.
    let mut cache = PluginCache::default();
    let scan_dir = scratch_dir();
    std::fs::copy(&path, scan_dir.path().join("test.clap")).expect("copies");
    scan::scan(&config(scan_dir.path()), &mut cache, |_, _, _| {});
    let found = cache.resolve(&gain.plugin).expect("the gain resolves");
    assert_eq!(found.path, canonical(&scan_dir.path().join("test.clap")));
}
