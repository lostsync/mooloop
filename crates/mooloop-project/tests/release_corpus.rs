//! Songs saved by tagged releases (MOO-121): every one still opens, as the
//! song it was, and survives being saved again by this build.
//!
//! Each `tests/fixtures/songs/<tag>/` holds what that release's own
//! `save_song` wrote for one fixed document: that release's
//! `Project::starter_kit(7)` (a seeded four-piece v1 drum synth kit on Drums,
//! Bass and Reverb tracks) at 131 BPM and 58% swing, its channels renamed
//! `corpus 0`, `corpus 1`, ... They were written by building the release's
//! `mooloop-project` on the build box, never by this tree, so a format change
//! that forgets an old song fails here. Add a directory when a release is
//! tagged; from 0.1.5 the starter kit takes no seed (`Project::starter_kit()`,
//! four DS-01 channels on one Drums track), and the files already here stay
//! as their releases wrote them.

use mooloop_project::{load_bundle, save_song, AssetMode, LoadedDocument};
use std::path::{Path, PathBuf};

fn corpus() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/songs");
    let mut songs = Vec::new();
    for release in std::fs::read_dir(&root).expect("the corpus directory exists") {
        let release = release.unwrap().path();
        for entry in std::fs::read_dir(&release).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|extension| extension == "mooloop") {
                songs.push(path);
            }
        }
    }
    songs.sort();
    songs
}

#[test]
fn every_release_song_opens_as_the_song_it_was() {
    let songs = corpus();
    assert!(!songs.is_empty(), "the corpus is empty");
    for path in songs {
        let report = load_bundle(&path).unwrap_or_else(|error| {
            panic!("{} no longer opens: {error}", path.display())
        });
        // Opened as it was, not mended into something else.
        assert!(
            report.repairs.is_empty(),
            "{} needed repairs: {:?}",
            path.display(),
            report.repairs
        );
        let LoadedDocument::Song(project) = report.document else {
            panic!("{} opened as something other than a song", path.display());
        };
        assert_eq!(project.bpm, 131, "{}", path.display());
        assert_eq!(project.swing_percent, 58, "{}", path.display());
        let names: Vec<&str> = project
            .channels
            .iter()
            .map(|channel| channel.setup.channel.name.as_str())
            .collect();
        let expected: Vec<String> = (0..names.len()).map(|index| format!("corpus {index}")).collect();
        assert_eq!(names, expected, "{}", path.display());
        assert!(!names.is_empty(), "{} lost its channels", path.display());

        // Saved again by this build, it reads back as what was opened.
        let temp = tempfile::tempdir().unwrap();
        let again = temp.path().join("again.mooloop");
        save_song(&again, &project, AssetMode::Referenced).unwrap();
        let LoadedDocument::Song(resaved) = load_bundle(&again).unwrap().document else {
            panic!("a song");
        };
        assert_eq!(resaved, project, "{} changed on a second save", path.display());
    }
}

/// A song with no modulation saves with no `modulation` table at all, in
/// the song or on a channel (song-modulation step 01): a 0.1.6 build opening
/// it finds nothing it does not know.
#[test]
fn a_release_song_without_modulation_saves_no_modulation_table() {
    for path in corpus() {
        let LoadedDocument::Song(project) = load_bundle(&path).unwrap().document else {
            panic!("a song");
        };
        assert!(project.modulation.is_empty(), "{}", path.display());
        let temp = tempfile::tempdir().unwrap();
        let again = temp.path().join("again.mooloop");
        save_song(&again, &project, AssetMode::Referenced).unwrap();
        let text = std::fs::read_to_string(&again).unwrap();
        // `modulation = ...` is also a chorus control, so look for tables.
        assert!(
            !text.contains("[document.modulation") && !text.contains("setup.modulation"),
            "{} wrote a modulation table",
            path.display()
        );
    }
}

/// **A 0.1.6 song's channel racks convert into the song's set, and the set
/// survives a save** (song-modulation step 01). The corpus songs carry empty
/// racks, so the newest is given one the way 0.1.6 wrote it -- on the
/// channel's setup, under `modulation` -- with a module of three kinds, an
/// envelope gated by another channel, and a route from the channel's own
/// DS-01 outlet.
#[test]
fn a_release_songs_channel_modulation_converts_and_round_trips() {
    use mooloop_core::{
        ds01, InputSource, ModEnvelopeParams, ModLfoParams, ModPolarity, ModRack, ModRoute,
        ModSourceRef, ModStepParams, ModulatorParams, ParamAddr, ParamOwner,
    };
    let newest = corpus().pop().expect("the corpus has a song");
    let LoadedDocument::Song(project) = load_bundle(&newest).unwrap().document else {
        panic!("a song");
    };
    assert!(project.channels.len() >= 3, "the starter has a channel to gate from");
    let kind = project.channels[0].setup.source.kind();
    let cutoff = ParamAddr {
        scope: mooloop_core::EffectTarget::Channel(0),
        owner: ParamOwner::source(kind),
        param: ds01::PARAM_FILTER_CUTOFF,
    };
    let tone = ParamAddr {
        param: ds01::PARAM_TONE_LEVEL,
        ..cutoff
    };
    let mut rack = ModRack::default();
    rack.install(0, ModulatorParams::Lfo(ModLfoParams { rate_hz: 3.0, ..ModLfoParams::default() }));
    rack.install(
        1,
        ModulatorParams::Envelope(ModEnvelopeParams {
            input_channel: 2,
            input_channel_id: project.channels[2].id,
            ..ModEnvelopeParams::default()
        }),
    );
    rack.install(3, ModulatorParams::Step(ModStepParams::default()));
    rack.add_route(ModRoute::to_slot(0, cutoff, 0.5, ModPolarity::Bipolar)).unwrap();
    rack.add_route(ModRoute::to_slot(1, tone, 0.25, ModPolarity::Unipolar)).unwrap();
    rack.add_route(ModRoute::from_outlet(
        mooloop_core::ChannelId::UNASSIGNED,
        ds01::DS01_OUTLET_TRIGGER,
        tone,
        0.3,
        ModPolarity::Bipolar,
    ))
    .unwrap();
    // Written where 0.1.6 wrote it: into the first channel's rack table,
    // in the rack's own (unchanged) serialization.
    let rack_text = toml::to_string(&rack).unwrap();
    let nested: Vec<String> = rack_text
        .lines()
        .map(|line| {
            if let Some(rest) = line.strip_prefix("[[") {
                format!("[[document.channels.setup.modulation.{rest}")
            } else if let Some(rest) = line.strip_prefix('[') {
                format!("[document.channels.setup.modulation.{rest}")
            } else {
                line.to_string()
            }
        })
        .collect();
    let fixture = std::fs::read_to_string(&newest).unwrap();
    let header = "[document.channels.setup.modulation]\n";
    let at = fixture.find(header).expect("the first channel's rack table") + header.len();
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old.mooloop");
    std::fs::write(&old, format!("{}{}\n{}", &fixture[..at], nested.join("\n"), &fixture[at..])).unwrap();

    let report = load_bundle(&old).unwrap();
    assert!(report.repairs.is_empty(), "the conversion needed repairs: {:?}", report.repairs);
    let LoadedDocument::Song(converted) = report.document else {
        panic!("a song");
    };
    // Lifted: the channel carries nothing, the song has every module.
    assert!(converted.channels.iter().all(|channel| channel.setup.preset_modulation.is_none()));
    let own = converted.channels[0].id;
    let gate = converted.channels[2].id;
    let inputs: Vec<(Option<u8>, InputSource)> = converted
        .modulation
        .modules
        .iter()
        .map(|module| {
            assert_eq!(module.rack.map(|seat| seat.channel), Some(own));
            (module.rack.map(|seat| seat.slot), converted.modulation.input_of(module.id))
        })
        .collect();
    assert_eq!(
        inputs,
        [
            (Some(0), InputSource::ChannelNotes(own)),
            (Some(1), InputSource::ChannelNotes(gate)),
            (Some(3), InputSource::ChannelNotes(own)),
        ]
    );
    // As a patch: one gate tag per channel, shared, and a wire per input.
    assert_eq!(converted.modulation.tags.len(), 2);
    assert_eq!(converted.modulation.wires.len(), 3);
    assert_eq!(converted.modulation.routes.len(), 3);
    assert!(converted.modulation.routes.iter().any(|route| route.source
        == ModSourceRef::GeneratorOutlet {
            channel: own,
            outlet: ds01::DS01_OUTLET_TRIGGER,
        }));
    // The engine runs the rack it ran before.
    let held = converted.channel_rack(0);
    for slot in 0..mooloop_core::modulation::MAX_MODULATORS_PER_CHANNEL {
        assert_eq!(held.params(slot), rack.params(slot), "slot {slot}");
    }
    let routes = |rack: &ModRack| {
        let mut routes: Vec<_> = rack
            .routes
            .iter()
            .flatten()
            .map(|route| (route.source_slot, route.destination, route.depth.to_bits(), route.polarity))
            .collect();
        routes.sort_by_key(|route| (route.0, route.1.param));
        routes
    };
    assert_eq!(routes(&held), routes(&rack));

    // Saved, it is the song's table; loaded again, the same set.
    let again = temp.path().join("again.mooloop");
    save_song(&again, &converted, AssetMode::Referenced).unwrap();
    let text = std::fs::read_to_string(&again).unwrap();
    assert!(text.contains("[document.modulation]"), "the song's table is missing");
    assert!(!text.contains("setup.modulation"), "a channel still writes a rack");
    let LoadedDocument::Song(reloaded) = load_bundle(&again).unwrap().document else {
        panic!("a song");
    };
    assert_eq!(reloaded.modulation, converted.modulation);
    assert_eq!(reloaded, converted);
}
