//! The out-of-process scanner and its cache, end to end.
//!
//! **A plugin that crashes or hangs while it is being scanned must not crash
//! or hang mooloop** (`docs/plans/plugin-hosting/05-the-scanner.md`). These
//! tests scan real misbehaving libraries: the in-repo test plugin copied
//! under the names that make it abort or never return while its entry
//! initialises (`mooloop_test_plugin::CRASHES_ON_SCAN`, `HANGS_ON_SCAN`). If
//! the scanner loaded either in its own process, this test binary would die
//! or never finish; instead each costs one child, recorded as a failure in
//! the cache.
//!
//! The children are `mooloop-scan-child`, this crate's test-only binary,
//! which runs the same `scan::run_child_from_args` the app runs as
//! `mooloop --scan-plugin`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mooloop_core::plugin::PluginFormat;
use mooloop_plugin_host::scan::{
    self, ChildCommand, FailureKind, PluginCache, Refusal, ScanConfig, ScannedPlugin, SCAN_FLAG,
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

/// One directory holding every way a candidate can go wrong: one good plugin
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
    assert_eq!(summary.plugins, test_plugin::PLUGIN_IDS.len(), "{summary:?}");
    assert_eq!(summary.failed, 4, "{summary:?}");
    // The hang is cut off at the timeout rather than waited out.
    assert!(took < TIMEOUT * 3, "the scan took {took:?}");

    let mut ids: Vec<&str> = cache.plugins().map(|p| p.plugin.id.as_str()).collect();
    ids.sort_unstable();
    let mut expected = test_plugin::PLUGIN_IDS.to_vec();
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

    // Forgetting only the failures makes the next scan try those files, and
    // no others, again.
    reloaded.clear_failures();
    let rescan = scan::scan(&config(dir.path()), &mut reloaded, |_, _, _| {});
    assert_eq!(rescan.launched, 3, "{rescan:?}");
    assert_eq!(failure_of(&reloaded, &crashes), Some(FailureKind::Crashed));
}

/// The child's report on the test plugin parses back into the plugins it
/// holds, with what the browser and the CLAP adapter need: the saved
/// reference, the features, the ports and whether there is a GUI.
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

    // Each flags its one port main; the scan records which.
    assert_eq!((gain.main_audio_input, gain.main_audio_output), (0, 0));
    assert_eq!((gain.main_input_channels(), gain.main_output_channels()), (Some(2), Some(2)));
    assert_eq!((sine.main_input_channels(), sine.main_output_channels()), (None, Some(2)));

    // A plugin whose main ports are not port 0, and not at the same index
    // each way: the scan records where they are, and the rules judge those,
    // not the sidechain or the extra outputs.
    let sidechain = find(test_plugin::SIDECHAIN_ID);
    assert_eq!(sidechain.audio_inputs, vec![1, 2]);
    assert_eq!(sidechain.audio_outputs, vec![2, 1, 2]);
    assert_eq!((sidechain.main_audio_input, sidechain.main_audio_output), (1, 2));
    assert_eq!(
        (sidechain.main_input_channels(), sidechain.main_output_channels()),
        (Some(2), Some(2))
    );
    assert_eq!(sidechain.effect_refusal(), None);
    assert!(sidechain.source_refusal().is_some());
}

/// A cache file holding one plugin, `fields` being its keys after the saved
/// reference, in the file's own spelling.
fn cache_with(fields: &str) -> String {
    format!(
        "version = 1\n\n\
         [[file]]\npath = \"/usr/lib/clap/example.clap\"\nmodified-ns = 0\nsize = 0\n\n\
         [[file.plugin]]\npath = \"/usr/lib/clap/example.clap\"\nformat = \"clap\"\n\
         id = \"org.example.plugin\"\nname = \"Example\"\n{fields}\n"
    )
}

/// The one plugin in [`cache_with`]'s file, as a load reads it.
fn cached_plugin(fields: &str) -> ScannedPlugin {
    let text = cache_with(fields);
    let cache = PluginCache::from_toml(&text).unwrap_or_else(|error| panic!("{error}\n{text}"));
    let mut plugins: Vec<ScannedPlugin> = cache.plugins().cloned().collect();
    assert_eq!(plugins.len(), 1, "{text}");
    plugins.remove(0)
}

/// A cache entry with no `main-audio-input` or `main-audio-output` (every
/// entry written before the scan recorded them) loads, reads port 0 as main
/// until a rescan, and is judged by it, so Surge XT's layouts, whose main
/// port is port 0, are offered. Port 0 is never written, so such an entry
/// writes back as it was; any other main port is written and read back.
#[test]
fn an_old_cache_entry_without_main_ports_loads_with_port_zero_as_main() {
    let surge = "features = [\"instrument\"]\naudio-outputs = [2, 2, 2]\nnote-inputs = 1";
    let plugin = cached_plugin(surge);
    assert_eq!((plugin.main_audio_input, plugin.main_audio_output), (0, 0));
    assert_eq!((plugin.main_input_channels(), plugin.main_output_channels()), (None, Some(2)));
    assert_eq!(plugin.source_refusal(), None);

    let surge_effects = "features = [\"audio-effect\"]\naudio-inputs = [2, 2]\naudio-outputs = [2]";
    let plugin = cached_plugin(surge_effects);
    assert_eq!((plugin.main_audio_input, plugin.main_audio_output), (0, 0));
    assert_eq!(plugin.effect_refusal(), None);
    let old = PluginCache::from_toml(&cache_with(surge_effects)).expect("parses");
    let written = old.to_toml().expect("serializes");
    assert!(!written.contains("main-audio"), "{written}");
    assert_eq!(PluginCache::from_toml(&written).expect("parses"), old);

    let flagged = "audio-inputs = [6, 2]\naudio-outputs = [2, 2]\nmain-audio-input = 1\nmain-audio-output = 1";
    let cache = PluginCache::from_toml(&cache_with(flagged)).expect("parses");
    let plugin = cache.plugins().next().expect("one plugin");
    assert_eq!((plugin.main_audio_input, plugin.main_audio_output), (1, 1));
    let written = cache.to_toml().expect("serializes");
    assert!(written.contains("main-audio-input = 1"), "{written}");
    assert!(written.contains("main-audio-output = 1"), "{written}");
    assert_eq!(PluginCache::from_toml(&written).expect("parses"), cache);
}

/// The refusal rules look at the main ports only. An effect with a
/// sidechain or several buses, and an instrument with extra outputs, are
/// offered in their role when the main port has one or two channels; the
/// layouts here are real ones (Surge XT, LSP, a drum machine). A plugin with
/// no usable main port is refused as *unsupported* (a reason from the rule),
/// not as *failed*.
#[test]
fn the_refusal_rules_read_the_main_ports_only() {
    // Every refusal below is a rule's, so unsupported: the reason it gives.
    let unsupported = |refusal: Option<Refusal>| {
        refusal.map(|why| match why {
            Refusal::Unsupported(reason) => reason,
            Refusal::Failed(reason) => panic!("a port rule's refusal is unsupported, not failed: {reason}"),
        })
    };
    let effect = |ins: &str, outs: &str, more: &str| {
        unsupported(
            cached_plugin(&format!("features = [\"audio-effect\"]\naudio-inputs = {ins}\naudio-outputs = {outs}\n{more}"))
                .effect_refusal(),
        )
    };
    let instrument = |ins: &str, outs: &str, more: &str| {
        unsupported(
            cached_plugin(&format!(
                "features = [\"instrument\"]\naudio-inputs = {ins}\naudio-outputs = {outs}\nnote-inputs = 1\n{more}"
            ))
            .source_refusal(),
        )
    };

    // Effects with a sidechain (Surge XT Effects; LSP's sidechain dynamics)
    // and multi-bus tools (LSP's x2 and x4 variants).
    for (ins, outs) in [
        ("[2, 2]", "[2]"),
        ("[1, 1]", "[1]"),
        ("[2, 1, 1]", "[2]"),
        ("[2, 2, 2, 2]", "[2, 2, 2, 2]"),
    ] {
        assert_eq!(effect(ins, outs, ""), None, "{ins} -> {outs}");
    }
    // Instruments with extra outputs (Surge XT's scenes, a drum machine),
    // and with inputs of any width: a source's inputs are all fed silence.
    let drum_machine = format!("[{}]", ["2"; 13].join(", "));
    for (ins, outs) in [("[]", "[2, 2, 2]"), ("[]", drum_machine.as_str()), ("[2, 2]", "[1, 2]"), ("[6]", "[2]")] {
        assert_eq!(instrument(ins, outs, ""), None, "{ins} -> {outs}");
    }

    // The flagged main port is the one judged, wherever it sits.
    assert_eq!(effect("[6, 2]", "[2]", "main-audio-input = 1"), None);
    assert_eq!(
        effect("[2, 6]", "[2]", "main-audio-input = 1").as_deref(),
        Some("its main input has 6 channels; 1 or 2 are hosted")
    );
    assert_eq!(instrument("[]", "[8, 2]", "main-audio-output = 1"), None);
    assert_eq!(
        instrument("[]", "[2, 8]", "main-audio-output = 1").as_deref(),
        Some("its main output has 8 channels; 1 or 2 are hosted")
    );

    // No usable main port: still refused, each with its reason.
    assert_eq!(effect("[6, 2]", "[2]", "").as_deref(), Some("its main input has 6 channels; 1 or 2 are hosted"));
    assert_eq!(effect("[2]", "[6, 2]", "").as_deref(), Some("its main output has 6 channels; 1 or 2 are hosted"));
    assert_eq!(effect("[]", "[2]", "").as_deref(), Some("it has no audio input"));
    assert_eq!(effect("[2]", "[]", "").as_deref(), Some("it has no audio output"));
    assert_eq!(instrument("[]", "[]", "").as_deref(), Some("it has no audio output"));
    assert_eq!(instrument("[]", "[6]", "").as_deref(), Some("its main output has 6 channels; 1 or 2 are hosted"));
    // A main index past the ports (a hand-edited or stale cache) is no port.
    assert_eq!(effect("[2]", "[2]", "main-audio-input = 3").as_deref(), Some("it has no audio input"));

    // Unsupported, not failed: the plugin was created, and says why it fits
    // nowhere from its ports, never "could not be created".
    let unwired = cached_plugin("features = [\"audio-effect\"]\naudio-inputs = [6]\naudio-outputs = [2]");
    assert!(unwired.is_usable());
    assert!(matches!(unwired.effect_refusal(), Some(Refusal::Unsupported(why)) if !why.contains("could not be created")));

    // A plugin that declares no role: an input makes it an effect, and it
    // is judged by its main input whatever else it has.
    let unmarked = cached_plugin("audio-inputs = [2, 2]\naudio-outputs = [2]");
    assert_eq!((unmarked.effect_refusal(), unmarked.source_refusal().is_some()), (None, true));

    // The same rule on bare flags and channel counts, for a live plugin.
    assert_eq!(scan::main_port([false, true, true]), 1);
    assert_eq!(scan::main_port([false, false]), 0);
    assert_eq!(scan::main_port([false; 0]), 0);
    assert_eq!(scan::main_channels(&[6, 2], 1), Some(2));
    assert_eq!(scan::main_channels(&[], 0), None);
    let features = ["audio-effect".to_owned()];
    assert_eq!(scan::main_port_effect_refusal(&features, Some(1), Some(2)), None);
    let features = ["instrument".to_owned()];
    assert_eq!(scan::main_port_source_refusal(&features, Some(2), 1), None);
}
