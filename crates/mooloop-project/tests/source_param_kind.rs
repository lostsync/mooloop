//! A generator address names the kind of device it was made on (MOO-135),
//! and every song saved before that loads with each such address intact and
//! given its channel's kind.
//!
//! A song from before the change is this build's own song with every
//! `source_kind` key taken out. That is exactly what 0.1.5 wrote --
//! `mooloop-core`'s `a_reader_that_predates_the_kind_still_reads_the_address`
//! pins the two spellings against each other -- and no release in the corpus
//! (`release_corpus.rs`) holds a lane, route or binding on a generator, so
//! the legacy song is made here rather than taken from there.

use mooloop_core::control::{ControlBinding, ControlSource, ControlTarget};
use mooloop_core::{
    AutomationLane, AutomationPoint, ChannelId, DeviceKind, EffectTarget, MidiChannelFilter,
    MidiPortFilter, ModPolarity, ModRoute, ModulatorKind, ParamAddr, ParamOwner, Project,
    ProjectChannel,
};
use mooloop_project::{load_bundle, save_song, AssetMode, LoadedDocument};
use std::path::Path;

/// A sampler on channel 0 and a DS-01 on channel 1, each with a lane, an LFO
/// route and a CC binding on one of its own controls.
fn song() -> Project {
    let mut project = Project {
        channels: vec![
            ProjectChannel::sampler(0, 1).with_id(ChannelId(0)),
            ProjectChannel::ds01(1, 1).with_id(ChannelId(1)),
        ],
        next_channel_id: 2,
        ..Project::default()
    };
    for (index, channel) in project.channels.iter_mut().enumerate() {
        let kind = channel.setup.source.kind();
        let here = EffectTarget::Channel(index as u8);
        let mut ids = kind.descriptors().iter().map(|descriptor| descriptor.id);
        let (lane_id, route_id) = (ids.next().unwrap(), ids.next().unwrap());
        let mut lane = AutomationLane::new(ParamAddr::source(here, kind, lane_id));
        assert!(lane.upsert(AutomationPoint::new(1, 0, 0.25)));
        channel.automation[0].push(lane);
        let rack = &mut channel.setup.modulation;
        rack.install(0, ModulatorKind::Lfo.default_params()).unwrap();
        rack.add_route(ModRoute::to_slot(
            0,
            ParamAddr::source(here, kind, route_id),
            0.5,
            ModPolarity::Bipolar,
        ))
        .unwrap();
    }
    for index in 0..2u8 {
        let kind = project.channels[usize::from(index)].setup.source.kind();
        let address = ParamAddr::source(EffectTarget::Channel(index), kind, kind.descriptors()[0].id);
        let key = project.param_key(address).unwrap();
        project.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 20 + index,
            },
            ControlTarget::Param(key),
        ));
    }
    project
}

/// Every generator address in `project`, with the kind of the channel it is
/// on: lanes, routes and bindings.
fn source_owners(project: &Project) -> Vec<(ParamOwner, DeviceKind)> {
    let mut found = Vec::new();
    for channel in &project.channels {
        let kind = channel.setup.source.kind();
        for lane in channel.automation.iter().flatten() {
            found.push((lane.target.owner, kind));
        }
        for route in channel.setup.modulation.routes.iter().flatten() {
            found.push((route.destination.owner, kind));
        }
    }
    for binding in &project.control_map.bindings {
        let ControlTarget::Param(key) = binding.target else { continue };
        let mooloop_core::ChainKey::Channel(id) = key.scope else { continue };
        let channel = project.channel_index(id).unwrap();
        found.push((key.owner, project.channels[channel].setup.source.kind()));
    }
    found.retain(|(owner, _)| matches!(owner, ParamOwner::Source { .. }));
    found
}

fn load(path: &Path) -> (Project, usize) {
    let report = load_bundle(path).unwrap_or_else(|error| panic!("{error}"));
    let repairs = report.repairs.len();
    let LoadedDocument::Song(project) = report.document else {
        panic!("a song")
    };
    (project, repairs)
}

/// The song as 0.1.5 would have written it: no `source_kind` anywhere,
/// whether the address was written as a table or inline.
fn without_kinds(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with("source_kind = ") {
            continue;
        }
        let mut line = line.to_owned();
        while let Some(start) = line.find(", source_kind = \"") {
            let rest = &line[start + ", source_kind = \"".len()..];
            let end = start + ", source_kind = \"".len() + rest.find('"').unwrap() + 1;
            line.replace_range(start..end, "");
        }
        out.push_str(&line);
        out.push('\n');
    }
    assert!(!out.contains("source_kind"), "the strip missed one:\n{out}");
    out
}

#[test]
fn a_song_writes_each_generator_address_with_its_kind_and_reads_it_back() {
    let project = song();
    assert_eq!(source_owners(&project).len(), 6);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.mooloop");
    save_song(&path, &project, AssetMode::Referenced).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("source_kind = \"sampler\"").count(), 3, "{text}");
    assert_eq!(text.matches("source_kind = \"ds01\"").count(), 3, "{text}");

    let (loaded, repairs) = load(&path);
    assert_eq!(repairs, 0);
    assert_eq!(source_owners(&loaded), source_owners(&project));
}

/// The issue's second "done when": a song saved before the change loads with
/// every `Source` address intact, each given the kind its channel runs, no
/// repair reported -- and saves back with the kind written.
#[test]
fn a_song_saved_before_kinds_loads_with_every_source_address_and_its_channels_kind() {
    let project = song();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.mooloop");
    save_song(&path, &project, AssetMode::Referenced).unwrap();
    let legacy = dir.path().join("legacy.mooloop");
    std::fs::write(&legacy, without_kinds(&std::fs::read_to_string(&path).unwrap())).unwrap();

    let (loaded, repairs) = load(&legacy);
    assert_eq!(repairs, 0, "an old song needs no repair");
    let owners = source_owners(&loaded);
    assert_eq!(owners.len(), 6, "a generator address was lost");
    for (owner, kind) in &owners {
        assert_eq!(*owner, ParamOwner::source(*kind));
    }
    assert!(!loaded.has_unidentified_source_kinds());
    assert_eq!(owners, source_owners(&project));

    let again = dir.path().join("again.mooloop");
    save_song(&again, &loaded, AssetMode::Referenced).unwrap();
    let text = std::fs::read_to_string(&again).unwrap();
    assert_eq!(text.matches("source_kind = ").count(), 6, "{text}");
    let (reloaded, repairs) = load(&again);
    assert_eq!(repairs, 0);
    assert_eq!(reloaded, loaded);
}

/// A kind this version cannot read -- a later version's device, or a hand
/// edit -- does not fail the song. It is read as absent and given the
/// channel's kind, which is what any version before 0.1.6 did with it.
#[test]
fn an_unreadable_kind_does_not_fail_the_song() {
    let project = song();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.mooloop");
    save_song(&path, &project, AssetMode::Referenced).unwrap();
    let text = std::fs::read_to_string(&path)
        .unwrap()
        .replace("source_kind = \"ds01\"", "source_kind = \"NoSuchKind\"");
    assert!(text.contains("NoSuchKind"));
    let odd = dir.path().join("odd.mooloop");
    std::fs::write(&odd, text).unwrap();

    let (loaded, _) = load(&odd);
    assert_eq!(source_owners(&loaded), source_owners(&project));
    assert!(!loaded.has_unidentified_source_kinds());
}

/// A lane made on another device is kept through a save and a load, byte for
/// byte, and not reported: it is inert on this channel, not broken.
#[test]
fn another_devices_lane_survives_a_save_unchanged() {
    let mut project = song();
    let drum_id = DeviceKind::DrumSynth.descriptors()[3].id;
    let mut lane = AutomationLane::new(ParamAddr::source(
        EffectTarget::Channel(0),
        DeviceKind::DrumSynth,
        drum_id,
    ));
    assert!(lane.upsert(AutomationPoint::new(1, 48, 0.5)));
    project.channels[0].automation[0].push(lane.clone());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.mooloop");
    save_song(&path, &project, AssetMode::Referenced).unwrap();
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("source_kind = \"drum_synth\""));

    let (loaded, repairs) = load(&path);
    assert_eq!(repairs, 0);
    assert!(loaded.channels[0].automation[0].contains(&lane));
}
