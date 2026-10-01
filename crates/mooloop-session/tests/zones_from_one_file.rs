//! Many key zones from one file (MOO-464; Adam, 2026-09-30): twelve hits in
//! one wav become twelve playable zones in one action, the file is shared
//! rather than copied or decoded again, and a song holding them saves,
//! embeds and reopens with the file once. Slices and zones never both
//! apply, and a song saved since 0.1.5 with both plays as it did.

use std::path::Path;
use std::sync::Arc;

use mooloop_core::{
    KeyRange, NoteEvent, PlayMode, Project, ProjectChannel, SampleReference, SampleZone,
    SamplerParams, SliceMap, TICKS_PER_STEP,
};
use mooloop_dsp::SampleData;
use mooloop_project::{AssetMode, LoadedDocument, PresetInfo};
use mooloop_session::sampler::channel_audio;
use mooloop_session::session::Session;

const RATE: u32 = 48_000;
/// A quarter second per hit.
const HIT: usize = RATE as usize / 4;
const HITS: usize = 12;

/// Twelve hits, each a quarter second held at its own level, so where a
/// voice reads is what it plays.
fn levels() -> Vec<f32> {
    (0..HITS).map(|hit| 0.05 + 0.07 * hit as f32).collect()
}

fn hits() -> Vec<[f32; 2]> {
    levels()
        .iter()
        .flat_map(|level| std::iter::repeat_n([*level, *level], HIT))
        .collect()
}

fn write_wav(path: &Path, frames: &[[f32; 2]]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for [left, right] in frames {
        writer.write_sample(*left).unwrap();
        writer.write_sample(*right).unwrap();
    }
    writer.finalize().unwrap();
}

fn decode(path: &Path) -> Arc<SampleData> {
    mooloop_session::audio_file::decode(path).unwrap().sample
}

/// A sampler on `file`, in Slice mode with a marker on every hit, playing
/// `notes` an eighth of a second each, half a second apart.
fn sliced_song(file: &Path, notes: impl IntoIterator<Item = u8>) -> Project {
    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().unwrap();
    state.sample = SampleReference::File {
        path: file.to_path_buf(),
        embedded: false,
    };
    state.params = SamplerParams {
        attack: 0.0,
        decay: 8.0,
        sustain: 1.0,
        release: 0.005,
        play_mode: PlayMode::Slice,
        ..state.params
    };
    let mut slices = SliceMap::new();
    for hit in 0..HITS {
        slices.add((hit * HIT) as u32);
    }
    state.slices = slices;
    channel.notes[0] = notes
        .into_iter()
        .enumerate()
        .map(|(index, note)| {
            NoteEvent::new(index as u32 + 1, index as u32 * 4 * TICKS_PER_STEP, 2 * TICKS_PER_STEP, note, 127)
        })
        .collect();
    Project {
        bpm: 120,
        channels: vec![channel],
        pattern_lengths: vec![64],
        ..Project::default()
    }
}

/// The level each struck note plays, a sixteenth into it, from a render of
/// notes half a second apart; asserts each holds one level.
fn note_levels(render: &[f32], count: usize) -> Vec<f32> {
    (0..count)
        .map(|note| {
            let start = note * RATE as usize / 2 + RATE as usize / 32;
            let window = &render[start..start + RATE as usize / 32];
            let (low, high) = window
                .iter()
                .fold((f32::MAX, f32::MIN), |(low, high), value| (low.min(*value), high.max(*value)));
            assert!(high - low < 1e-4, "note {note} reads more than one hit: {low}..{high}");
            high
        })
        .collect()
}

fn play(project: &Project, session: &Session) -> Vec<f32> {
    mooloop_engine::live_check::play_audio_through_executor(
        project,
        vec![channel_audio(&session.channels[0])],
        RATE,
        HITS * RATE as usize / 2,
        256,
    )
    .into_iter()
    .step_by(2)
    .collect()
}

/// **Done-when 1 and 3.** Twelve hits in one wav, sliced, become twelve
/// zones in one action: one key each from the key chosen, each rooted on its
/// own key so it plays its hit as recorded -- the same levels Slice mode
/// played -- every zone on the one file and the one buffer, the file
/// untouched, the patch out of Slice mode. Restoring the snapshot taken
/// before it (one undo step) brings the sliced patch back.
#[test]
fn twelve_hits_in_one_file_become_twelve_zones_in_one_action() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("xylophone.wav");
    write_wav(&file, &hits());
    let bytes = std::fs::read(&file).unwrap();
    let audio = decode(&file);

    let song = sliced_song(&file, 36..36 + HITS as u8);
    let mut session = Session::default();
    session.replace_project(&song, &[Some(audio.clone())]);
    session.selected = 0;
    let sliced = note_levels(&play(&song, &session), HITS);
    let unit = sliced[0] / levels()[0];
    for (played, level) in sliced.iter().zip(levels()) {
        assert!((played / unit - level).abs() < 1e-3, "Slice mode played {played} for {level}");
    }

    let before = session.project_snapshot(120, 0);
    let params = session.zones_from_slices(0, 60).expect("zones from the slices");
    assert_eq!(params.play_mode, PlayMode::Pitched);
    let channel = &session.channels[0];
    assert_eq!(channel.zones.len() + 1, HITS, "zone 1 and eleven more");
    assert_eq!(channel.keys, KeyRange::new(60, 60));
    assert_eq!(params.root_note, 60);
    for (index, zone) in channel.zones.iter().enumerate() {
        let key = 61 + index as u8;
        assert_eq!(zone.zone.keys, KeyRange::new(key, key));
        assert_eq!(zone.zone.root_note, key);
        assert_eq!(zone.path(), Some(file.as_path()));
        assert!(Arc::ptr_eq(zone.sample.as_ref().unwrap(), &audio), "a second buffer");
    }
    assert!(Arc::ptr_eq(session.zone_audio.get(&file).unwrap(), &audio));
    assert_eq!(std::fs::read(&file).unwrap(), bytes, "the file was edited");

    let mut zoned = session.project_snapshot(120, 0);
    zoned.channels[0].notes[0] = sliced_song(&file, 60..60 + HITS as u8).channels[0].notes[0].clone();
    let played = note_levels(&play(&zoned, &session), HITS);
    for (zone, slice) in played.iter().zip(&sliced) {
        assert!((zone - slice).abs() < 1e-4, "a zone played {zone} where its slice played {slice}");
    }

    // Undo restores the snapshot before the edit, which is the sliced patch.
    session.replace_project(&before, &[Some(audio)]);
    assert!(session.channels[0].zones.is_empty());
    assert_eq!(session.channels[0].sampler_params().play_mode, PlayMode::Slice);
}

/// **Done-when 2.** A song with twelve zones on one file saves with its
/// assets embedded holding that file once, and so does a channel preset;
/// each reopens with all twelve zones on the one embedded file, and the load
/// decodes it once: every zone shares zone 1's buffer.
#[test]
fn twelve_zones_on_one_file_save_and_reopen_with_the_file_once() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("xylophone.wav");
    write_wav(&file, &hits());
    let song = sliced_song(&file, []);
    let mut session = Session::default();
    session.replace_project(&song, &[Some(decode(&file))]);
    session.zones_from_slices(0, 48).unwrap();
    let snapshot = session.project_snapshot(120, 0);

    let saved = temp.path().join("song.mooloop");
    mooloop_project::save_song(&saved, &snapshot, AssetMode::Embedded).unwrap();
    let preset = temp.path().join("xylophone.mooloop-channel");
    mooloop_project::save_channel_preset(
        &preset,
        &snapshot.channels[0].setup,
        PresetInfo {
            name: "xylophone".into(),
            category: String::new(),
            tags: Vec::new(),
        },
        AssetMode::Embedded,
    )
    .unwrap();
    std::fs::remove_file(&file).unwrap();

    for bundle in [&saved, &preset] {
        let resolved = mooloop_session::document::resolve_document(bundle)
            .unwrap_or_else(|problem| panic!("{}: {}", bundle.display(), problem.message));
        let setup = match &resolved.report.document {
            LoadedDocument::Song(song) => song.channels[0].setup.clone(),
            LoadedDocument::Channel(setup) => (**setup).clone(),
            other => panic!("{other:?}"),
        };
        let state = setup.sampler_state().unwrap();
        assert_eq!(state.zones.len() + 1, HITS, "{}", bundle.display());
        let SampleReference::File { path, embedded } = &state.sample else {
            panic!("zone 1 lost its file");
        };
        assert!(*embedded && path.is_file());
        assert!(state.zones.iter().all(|zone: &SampleZone| zone.sample == state.sample));
        let folder = path.parent().unwrap();
        let files = std::fs::read_dir(folder).unwrap().count();
        assert_eq!(files, 1, "{} holds {files} files", folder.display());
        let base = resolved.samples[0].as_ref().expect("zone 1 decoded");
        assert_eq!(resolved.zone_audio.len(), 1);
        assert!(Arc::ptr_eq(&resolved.zone_audio[0].1, base), "the file was decoded twice");
    }
}

/// **A song saved since 0.1.5 with slices and zones plays as it did**:
/// slices, its zones unheard. Saved, reopened and rendered in-process, it is
/// identical to the same song without its zones.
#[test]
fn a_song_saved_with_slices_and_zones_plays_its_slices_as_it_did() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("xylophone.wav");
    write_wav(&file, &hits());
    let other = temp.path().join("other.wav");
    write_wav(&other, &vec![[0.9, 0.9]; RATE as usize]);
    let mut song = sliced_song(&file, 36..36 + HITS as u8);
    let state = song.channels[0].setup.sampler_state_mut().unwrap();
    state.keys = KeyRange::new(0, 59);
    // As 0.1.5 saved a zone: no region of its own.
    state.zones = vec![SampleZone {
        keys: KeyRange::new(36, 127),
        root_note: 60,
        sample: SampleReference::File {
            path: other.clone(),
            embedded: false,
        },
        region: None,
        ..SampleZone::default()
    }];
    let saved = temp.path().join("both.mooloop");
    mooloop_project::save_song(&saved, &song, AssetMode::Referenced).unwrap();

    let resolved = mooloop_session::document::resolve_document(&saved).unwrap_or_else(|p| panic!("{}", p.message));
    assert!(resolved.report.repairs.is_empty());
    let LoadedDocument::Song(loaded) = &resolved.report.document else {
        panic!("not a song");
    };
    let mut session = Session::default();
    session.admit_zone_audio(resolved.zone_audio.clone(), true);
    session.replace_project(loaded, &resolved.samples);
    assert_eq!(session.channels[0].zones.len(), 1);
    let both = play(loaded, &session);

    let mut unzoned = loaded.clone();
    let state = unzoned.channels[0].setup.sampler_state_mut().unwrap();
    state.zones.clear();
    state.keys = KeyRange::FULL;
    let mut plain = Session::default();
    plain.replace_project(&unzoned, &resolved.samples);
    let slices_only = play(&unzoned, &plain);
    assert!(both.iter().any(|value| *value != 0.0));
    assert_eq!(both, slices_only, "the zones were heard");
}
