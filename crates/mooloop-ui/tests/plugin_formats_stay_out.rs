//! The window sees only the plugin host's neutral types.
//!
//! `mooloop-ui` depends on `mooloop-plugin-host` for the scanner's cache
//! (MOO-83), and that crate also re-exports the CLAP types it is built on.
//! `docs/plans/plugin-hosting/00-status.md`, "The rule that makes VST3 and
//! AU cheap later", keeps every plugin format's type inside the host crate;
//! core's `plugin_formats_stay_out.rs` holds the manifests to it, and cannot
//! see a crate that reaches a format's types *through* the host. This reads
//! every path this crate names under `mooloop_plugin_host` and allows only
//! the neutral ones.

use std::path::Path;

/// What the window may name from the host crate: the scanner's cache and
/// the format-free instance contract.
const NEUTRAL: [&str; 6] = [
    "scan",
    "HostError",
    "HostedInstance",
    "PluginCache",
    "ScannedPlugin",
    "self",
];

/// Every name after `mooloop_plugin_host::` in `text` that is not neutral,
/// with the brace group of a `use` read name by name.
fn format_paths(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in text.match_indices("mooloop_plugin_host::") {
        let rest = &text[at + "mooloop_plugin_host::".len()..];
        let names: Vec<&str> = if let Some(group) = rest.strip_prefix('{') {
            let group = group.split('}').next().unwrap_or_default();
            group.split(',').collect()
        } else {
            vec![rest]
        };
        for name in names {
            let name: String = name
                .trim()
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && !NEUTRAL.contains(&name.as_str()) {
                found.push(name);
            }
        }
    }
    // A format's crate named directly, however it got here.
    for word in ["clack_host", "clack_plugin", "clack_extensions", "clap_sys"] {
        if text.contains(word) {
            found.push(word.to_string());
        }
    }
    found
}

fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_window_names_only_neutral_plugin_types() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    sources(&root.join("tests"), &mut files);
    let this = file!().rsplit('/').next().unwrap_or_default().to_string();
    let mut offenders = Vec::new();
    let mut read = 0;
    for file in files {
        // This file spells the forbidden names to look for them.
        if file.file_name().is_some_and(|name| name.to_string_lossy() == this) {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("a source file reads");
        if text.contains("mooloop_plugin_host") {
            read += 1;
        }
        for name in format_paths(&text) {
            offenders.push(format!("{}: {name}", file.display()));
        }
    }
    assert!(read >= 1, "no file names mooloop_plugin_host: the check read the wrong tree");
    assert!(
        offenders.is_empty(),
        "the window names a plugin format's type; use the neutral ones \
         ({NEUTRAL:?}) or a session call:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_check_would_see_a_format_path() {
    assert_eq!(
        format_paths("use mooloop_plugin_host::clap::ClapOpener;"),
        ["clap"]
    );
    assert_eq!(
        format_paths("use mooloop_plugin_host::{HostError, clack_host};"),
        ["clack_host", "clack_host"]
    );
    assert!(format_paths("use mooloop_plugin_host::scan::{PluginCache, ScannedPlugin};").is_empty());
    assert!(format_paths("use mooloop_plugin_host::{HostError, HostedInstance};").is_empty());
}
