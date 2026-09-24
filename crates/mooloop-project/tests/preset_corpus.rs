//! One effect preset per insert kind, as a build saved it (MOO-197): every one
//! still opens as the device it was.
//!
//! `release_corpus.rs` holds songs; this holds the other thing a user keeps
//! on disk and expects a later build to read. Each
//! `tests/fixtures/presets/<version>/<kind>.mooloop-effect` is what that
//! version's `save_effect_preset` wrote for [`every_kind_moved_off_its_defaults`]:
//! each insert kind with every one of its parameters moved off its default,
//! so a parameter that stops loading, or loads as something else, shows up
//! here as a difference rather than passing as a default.
//!
//! The files were written on the build box by
//! `write_the_preset_corpus` (ignored; run it with `--ignored` and pull the
//! directory back) and are never rewritten by an ordinary run. Add a
//! directory when a release changes what a preset holds.

use mooloop_core::{EffectKind, EffectSlotState};
use mooloop_project::{load_bundle, save_effect_preset, AssetMode, LoadedDocument, PresetInfo};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/presets")
}

/// The file name a kind's preset is kept under.
fn file_name(kind: EffectKind) -> String {
    format!("{}.mooloop-effect", kind.label().to_lowercase().replace(' ', "_"))
}

/// Each insert kind, every parameter moved off its default by the same
/// fraction of its range. What the corpus was written from.
fn every_kind_moved_off_its_defaults() -> Vec<EffectSlotState> {
    EffectKind::ALL
        .iter()
        .map(|kind| {
            let mut slot = EffectSlotState::of_kind(*kind);
            for descriptor in kind.descriptors() {
                let value = descriptor.min + 0.37 * (descriptor.max - descriptor.min);
                slot.params.set(descriptor.id, value);
            }
            slot.wet_dry = 0.63;
            slot
        })
        .collect()
}

#[test]
#[ignore = "writes the corpus; run once per format change, on the box, and pull it back"]
fn write_the_preset_corpus() {
    let directory = root().join(format!("v{}", env!("CARGO_PKG_VERSION")));
    std::fs::create_dir_all(&directory).unwrap();
    for slot in every_kind_moved_off_its_defaults() {
        let path = directory.join(file_name(slot.kind()));
        let info = PresetInfo {
            name: format!("corpus {}", slot.kind().label()),
            category: String::new(),
            tags: Vec::new(),
        };
        save_effect_preset(&path, &slot, info, AssetMode::Embedded).unwrap();
    }
}

#[test]
fn every_kind_has_a_preset_in_the_corpus() {
    let mut versions: Vec<PathBuf> = std::fs::read_dir(root())
        .expect("the preset corpus exists")
        .map(|entry| entry.unwrap().path())
        .collect();
    versions.sort();
    let newest = versions.last().expect("at least one version");
    for kind in EffectKind::ALL {
        assert!(
            newest.join(file_name(kind)).exists(),
            "{kind:?} has no preset in {}; run write_the_preset_corpus",
            newest.display()
        );
    }
}

/// Parameters a corpus version was written without, because they were added
/// after it: that version's file cannot hold them, so they open at their
/// defaults, and that is the claim to check rather than an exception.
///
/// A container's Level arrived with `containers/09` (2026-09-23). Its Mute
/// and Solo arrived with it too, but land on "off" at 37% of a two-position
/// range either way, so they need no row.
const ADDED_AFTER: [(&str, EffectKind, u32); 2] = [
    ("v0.1.4", EffectKind::Chain, mooloop_core::CONTAINER_PARAM_LEVEL),
    ("v0.1.4", EffectKind::Layer, mooloop_core::CONTAINER_PARAM_LEVEL),
];

/// What `version`'s files should open as: every parameter moved, except
/// those added after it was written, at their defaults.
fn expected_from(version: &Path) -> Vec<EffectSlotState> {
    let name = version.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    every_kind_moved_off_its_defaults()
        .into_iter()
        .map(|mut slot| {
            for (added_after, kind, id) in ADDED_AFTER {
                if added_after == name && slot.kind() == kind {
                    let descriptor = kind.descriptor(id).expect("a described parameter");
                    slot.params.set(id, descriptor.default);
                }
            }
            slot
        })
        .collect()
}

#[test]
fn every_preset_in_the_corpus_opens_as_the_device_it_was() {
    let mut opened = 0;
    for version in std::fs::read_dir(root()).expect("the preset corpus exists") {
        let version = version.unwrap().path();
        let expected = expected_from(&version);
        for slot in &expected {
            let path = version.join(file_name(slot.kind()));
            if !path.exists() {
                continue;
            }
            let report = load_bundle(&path)
                .unwrap_or_else(|error| panic!("{} no longer opens: {error}", path.display()));
            let LoadedDocument::Effect(loaded) = report.document else {
                panic!("{} opened as something other than an effect", path.display());
            };
            assert_eq!(*loaded, *slot, "{} opened as another device", path.display());
            opened += 1;
        }
    }
    assert!(opened >= EffectKind::ALL.len(), "the corpus is missing presets");
}
