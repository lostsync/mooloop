//! Seeding the shipped factory bank onto disk.
//!
//! Presets are bundle directories under the user's config dir, and until now
//! the only way one appeared was the user saving it. A shipped bank needs a
//! way in, and there are two shapes it could take: merge a read-only factory
//! directory into every scan, or write the bank out once and let it be
//! ordinary user content from then on.
//!
//! This is the second. It is a smaller change — nothing in the browser, the
//! loader, or the on-disk format learns about a second class of preset — and
//! it leaves the patches editable, which for a factory bank is the point:
//! they are starting places, and a starting place you cannot re-save is a
//! demo. The cost is that the bank is not self-healing, which the marker file
//! below makes deliberate rather than accidental.

use std::fs;
use std::path::Path;

use mooloop_core::mlm1_factory::{self, FactoryPatch};
use mooloop_core::modulation::ParamOwner;
use mooloop_core::{ChannelSetup, EffectTarget};

use crate::{sanitize_preset_name, save_channel_preset, AssetMode, Error, PresetInfo};

/// Written once the bank has been seeded. Its presence — not the presence of
/// the patches themselves — is what suppresses seeding, so deleting a patch
/// you do not want keeps it deleted instead of resurrecting it next launch.
///
/// Versioned in the name so a later bank can be added without a second
/// mechanism, and without re-writing patches the user has since edited.
// Frozen at the device's old ML-1 spelling: this file already exists in
// users' preset directories, and renaming it would re-seed a bank they
// may have deliberately pruned.
const MARKER_FILE: &str = ".ml1-factory-v1";

/// Bundle extension for a whole-channel preset, matching what the save
/// dialog writes.
const CHANNEL_BUNDLE_EXTENSION: &str = "mooloop-channel";

/// Writes the ML-M1 factory bank into `dir` unless it has been seeded before.
///
/// Returns how many patches were written: `0` when the marker is already
/// there, which is the normal case on every launch after the first.
///
/// The bank is channel-scoped rather than generator-scoped because a
/// generator preset is a bare [`mooloop_core::ChannelSource`] with nowhere to
/// put a [`mooloop_core::ModRack`], and Sequence Bleep is nothing without
/// one. Splitting the bank across both menus to avoid that would trade a
/// coherent bank for a tidier category.
pub fn seed_mlm1_bank(dir: &Path) -> Result<usize, Error> {
    let marker = dir.join(MARKER_FILE);
    if marker.exists() {
        return Ok(0);
    }
    fs::create_dir_all(dir)?;

    let mut written = 0;
    for patch in mlm1_factory::patches() {
        let stem = sanitize_preset_name(patch.name);
        let path = dir.join(format!("{stem}.{CHANNEL_BUNDLE_EXTENSION}"));
        // A name collision means the user already has something under that
        // name. Theirs wins: a first launch after an upgrade should never
        // silently replace a patch someone saved.
        if path.exists() {
            continue;
        }
        save_channel_preset(
            &path,
            &channel_setup(&patch),
            preset_info(&patch),
            AssetMode::Embedded,
        )?;
        written += 1;
    }

    // Last, so a failure part-way through leaves the bank incomplete rather
    // than marked complete. Re-running then finishes the job, and the
    // collision check above keeps that from duplicating work.
    fs::write(&marker, b"")?;
    Ok(written)
}

fn channel_setup(patch: &FactoryPatch) -> ChannelSetup {
    let mut setup = ChannelSetup::mlm1_with_params(patch.name, patch.params);
    setup.modulation = patch.modulation;
    setup
}

fn preset_info(patch: &FactoryPatch) -> PresetInfo {
    PresetInfo {
        name: patch.name.to_string(),
        category: patch.category.to_string(),
        tags: patch.tags.iter().map(|tag| (*tag).to_string()).collect(),
    }
}

/// Points every modulation route in `setup` at `channel`.
///
/// A [`mooloop_core::ModRoute`] names its destination channel absolutely, so
/// a rack is only correct on the channel it was authored on. That is right
/// for a project — the scope is what will let one channel modulate another —
/// and wrong for a preset, which is a description of an instrument and has no
/// business claiming a channel number. Loading is where the two meet, so
/// loading is where the rewrite belongs.
///
/// Only `Channel`-scoped destinations move. A route aimed at a bus is aimed
/// at shared state that exists independently of which channel loaded it.
pub fn rescope_modulation(setup: &mut ChannelSetup, channel: u8) {
    for route in setup.modulation.routes.iter_mut().flatten() {
        if matches!(route.destination.scope, EffectTarget::Channel(_)) {
            route.destination.scope = EffectTarget::Channel(channel);
        }
        // A route into a modulator or effect slot is scoped twice over: the
        // slot number is already channel-local, so only the scope needs
        // moving. Spelled out because the `owner` match looks conspicuously
        // absent otherwise.
        debug_assert!(matches!(
            route.destination.owner,
            ParamOwner::Source
                | ParamOwner::Strip
                | ParamOwner::Effect { .. }
                | ParamOwner::Modulator { .. }
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{list_presets, load_bundle, LoadedDocument};
    use mooloop_core::modulation::{ModPolarity, ModRoute, ParamAddr};
    use mooloop_core::{ChannelSource, DeviceKind};
    use tempfile::tempdir;

    /// The bank has to survive the round trip it will actually make: written
    /// as bundles, found by the browser, and read back as the same patch. A
    /// bank that only existed in memory would pass every DSP test in the
    /// suite and still ship nothing.
    #[test]
    fn the_seeded_bank_lists_and_loads_back_unchanged() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("channels");

        assert_eq!(seed_mlm1_bank(&dir).unwrap(), 6);

        let listed = list_presets(&dir);
        assert_eq!(listed.len(), 6);
        for summary in &listed {
            assert_eq!(summary.category, "ML-M1");
            assert_eq!(summary.kind, DeviceKind::MlM1);
        }

        for patch in mlm1_factory::patches() {
            let summary = listed
                .iter()
                .find(|found| found.name == patch.name)
                .unwrap_or_else(|| panic!("{} is missing from the bank", patch.name));
            let report = load_bundle(&summary.path).unwrap();
            let LoadedDocument::Channel(setup) = report.document else {
                panic!("{} did not load as a channel", patch.name);
            };
            let ChannelSource::MlM1(state) = setup.source else {
                panic!("{} did not load as an ML-M1", patch.name);
            };
            assert_eq!(state.params, patch.params, "{} changed on disk", patch.name);
            assert_eq!(
                setup.modulation, patch.modulation,
                "{}'s rack changed on disk",
                patch.name
            );
        }
    }

    /// Seeding is a first-run action, not a repair. Running it again must be
    /// a no-op, or an edited patch would be silently reverted on every
    /// launch — which is the difference between a starting place and a demo.
    #[test]
    fn seeding_twice_writes_nothing_the_second_time() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("channels");
        assert_eq!(seed_mlm1_bank(&dir).unwrap(), 6);
        assert_eq!(seed_mlm1_bank(&dir).unwrap(), 0);
    }

    /// Deleting a factory patch has to stick. The marker file is what makes
    /// that true, and this is the test that says so.
    #[test]
    fn a_deleted_patch_does_not_come_back() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("channels");
        seed_mlm1_bank(&dir).unwrap();

        let doomed = list_presets(&dir)[0].path.clone();
        fs::remove_dir_all(&doomed).unwrap();

        assert_eq!(seed_mlm1_bank(&dir).unwrap(), 0);
        assert_eq!(list_presets(&dir).len(), 5);
    }

    /// The upgrade case: someone already has a preset saved under a bank
    /// name. Seeding must not overwrite it — a first launch after an update
    /// silently replacing someone's work is the worst thing this code could
    /// do, and it is the one case the marker file cannot protect against.
    #[test]
    fn seeding_leaves_an_existing_preset_of_the_same_name_alone() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("channels");
        fs::create_dir_all(&dir).unwrap();

        let mut mine = ChannelSetup::mlm1("Acid Line");
        mine.channel.volume = 0.123;
        let path = dir.join("Acid_Line.mooloop-channel");
        save_channel_preset(
            &path,
            &mine,
            PresetInfo {
                name: "Acid Line".to_string(),
                category: "Mine".to_string(),
                tags: Vec::new(),
            },
            AssetMode::Embedded,
        )
        .unwrap();

        assert_eq!(seed_mlm1_bank(&dir).unwrap(), 5);

        let LoadedDocument::Channel(reloaded) = load_bundle(&path).unwrap().document else {
            panic!("the user's preset stopped being a channel");
        };
        assert_eq!(reloaded.channel.volume, 0.123);
    }

    /// A preset describes an instrument and has no business claiming a
    /// channel number, but a route stores one. Loading is where that gets
    /// reconciled.
    #[test]
    fn loading_points_a_saved_rack_at_the_channel_it_lands_on() {
        let mut setup = ChannelSetup::mlm1("test");
        setup.modulation.routes[0] = Some(ModRoute {
            source_slot: 0,
            destination: ParamAddr {
                scope: EffectTarget::Channel(3),
                owner: ParamOwner::Source,
                param: 5,
            },
            depth: 0.5,
            polarity: ModPolarity::Bipolar,
        });
        // A bus destination is shared state that exists whatever channel
        // loaded the preset, so it must be left where it points.
        setup.modulation.routes[1] = Some(ModRoute {
            source_slot: 0,
            destination: ParamAddr {
                scope: EffectTarget::Bus(1),
                owner: ParamOwner::Strip,
                param: 0,
            },
            depth: 0.5,
            polarity: ModPolarity::Bipolar,
        });

        rescope_modulation(&mut setup, 7);

        assert_eq!(
            setup.modulation.routes[0].unwrap().destination.scope,
            EffectTarget::Channel(7)
        );
        assert_eq!(
            setup.modulation.routes[1].unwrap().destination.scope,
            EffectTarget::Bus(1)
        );
    }

    /// Sequence Bleep is the patch that motivated the rescoping, so it is the
    /// one worth checking end to end rather than only in the abstract.
    #[test]
    fn the_one_patch_with_a_rack_survives_seeding_and_reloading_onto_any_channel() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("channels");
        seed_mlm1_bank(&dir).unwrap();

        let summary = list_presets(&dir)
            .into_iter()
            .find(|found| found.name == "Sequence Bleep")
            .expect("Sequence Bleep was not seeded");
        let LoadedDocument::Channel(mut setup) = load_bundle(&summary.path).unwrap().document
        else {
            panic!("Sequence Bleep did not load as a channel");
        };

        assert_eq!(setup.modulation.slots.iter().flatten().count(), 2);
        assert_eq!(setup.modulation.routes.iter().flatten().count(), 2);

        rescope_modulation(&mut setup, 5);
        for route in setup.modulation.routes.iter().flatten() {
            assert_eq!(route.destination.scope, EffectTarget::Channel(5));
        }
    }
}
