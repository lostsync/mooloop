//! No plugin format's types leave `crates/mooloop-plugin-host`.
//!
//! `docs/plans/plugin-hosting/00-status.md`, "The rule that makes VST3 and
//! AU cheap later": core, project, session, engine and UI see only the
//! neutral types in `mooloop_core::plugin`. The cheapest place that rule can
//! break is a manifest -- one crate adding `clack-host` to reach for a type
//! -- so this reads what Cargo resolved rather than what a manifest spells,
//! which also sees a dependency renamed with `package =`.

use std::path::Path;

/// The crates allowed to depend on a plugin format directly.
const ALLOWED: [&str; 2] = ["mooloop-plugin-host", "mooloop-test-plugin"];

/// A dependency that is a plugin format's own crate.
fn is_format_crate(name: &str) -> bool {
    name.starts_with("clack") || name.starts_with("clap-sys") || name.starts_with("vst3")
}

#[test]
fn only_the_plugin_host_names_a_plugin_format() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let text = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|error| panic!("{}: {error}", lock_path.display()));
    let lock: toml::Table = toml::from_str(&text).expect("Cargo.lock is TOML");
    let packages = lock["package"].as_array().expect("Cargo.lock lists packages");

    let mut local = 0;
    let mut offenders = Vec::new();
    for package in packages {
        // A package with no `source` is one of this workspace's own.
        if package.get("source").is_some() {
            continue;
        }
        local += 1;
        let name = package["name"].as_str().expect("a package has a name");
        if ALLOWED.contains(&name) {
            continue;
        }
        let dependencies = package
            .get("dependencies")
            .and_then(|deps| deps.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default();
        for dependency in dependencies {
            // "name" or "name version" when two versions are locked.
            let dependency = dependency.as_str().unwrap_or_default();
            let dependency = dependency.split(' ').next().unwrap_or_default();
            if is_format_crate(dependency) {
                offenders.push(format!("{name} -> {dependency}"));
            }
        }
    }
    assert!(local >= 9, "read {local} workspace packages; the lock was not the workspace's");
    assert!(
        offenders.is_empty(),
        "only {ALLOWED:?} may depend on a plugin format's crates; the rest see \
         mooloop_core::plugin's neutral types:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_check_would_see_a_format_crate() {
    for name in ["clack-host", "clack-plugin", "clack-extensions", "clap-sys", "vst3", "vst3-sys"] {
        assert!(is_format_crate(name), "{name}");
    }
    for name in ["mooloop-core", "serde", "claxon", "base64"] {
        assert!(!is_format_crate(name), "{name}");
    }
}
