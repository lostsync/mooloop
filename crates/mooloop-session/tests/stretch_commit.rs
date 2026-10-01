//! A stretch commit is a render that replaces the sample (MOO-375, with
//! MOO-370 and MOO-394; Adam, 2026-09-30).
//!
//! The render is written to the renders folder when it is made and becomes
//! the channel's sample; commits stack; REVERT goes back to the original in
//! one step with the markers on screen mapped onto it. These hold the
//! session to that through the real save and load.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mooloop_core::{Project, SampleCommit, SampleReference, SamplerParams, StretchMode};
use mooloop_dsp::interpolate::{Region, RegionEdge};
use mooloop_dsp::SampleData;
use mooloop_project::{AssetMode, LoadedDocument, PresetInfo};
use mooloop_session::sampler::{commit_is_stale, CommitRefusal};
use mooloop_session::session::Session;

const RATE: u32 = 48_000;
/// A frame either side of a marker is a rounding; a millisecond is a flam.
const MILLISECOND: i64 = 48;

/// One second of a decaying click every quarter second: material a slice
/// marker has a reason to sit on.
fn clicks(seconds: f32) -> Vec<[f32; 2]> {
    let frames = (seconds * RATE as f32) as usize;
    (0..frames)
        .map(|index| {
            let since = (index % (RATE as usize / 4)) as f32 / RATE as f32;
            let value = (since * 2_200.0 * std::f32::consts::TAU).sin() * (-since * 30.0).exp();
            [value * 0.8, value * 0.7]
        })
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

/// A session whose first channel plays `original`, fitted to one bar.
fn session_playing(original: &Path) -> Session {
    let mut project = Project::default();
    let state = project.channels[0].setup.sampler_state_mut().unwrap();
    state.sample = SampleReference::File {
        path: original.to_path_buf(),
        embedded: false,
    };
    state.params = SamplerParams {
        stretch_enabled: true,
        stretch_sync: true,
        stretch_bars: 1.0,
        ..state.params
    };
    let mut session = Session::default();
    session.replace_project(&project, &[Some(decode(original))]);
    session.selected = 0;
    session
}

fn marker_frames(session: &Session) -> Vec<u32> {
    session.channels[0]
        .slices
        .markers()
        .iter()
        .map(|marker| marker.frame)
        .collect()
}

fn near(actual: u32, expected: f64) -> bool {
    (i64::from(actual) - expected.round() as i64).abs() < MILLISECOND
}

struct Folders {
    _temp: tempfile::TempDir,
    original: PathBuf,
    renders: PathBuf,
    root: PathBuf,
}

fn folders() -> Folders {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let original = root.join("break.wav");
    write_wav(&original, &clicks(1.0));
    Folders {
        renders: root.join("data/renders"),
        original,
        root,
        _temp: temp,
    }
}

/// The issue's own case. Commit, add a slice, change the tempo and commit
/// again: the slice is still there, at its stretched position. Then REVERT:
/// the original is back, with that slice mapped onto it.
#[test]
fn a_slice_made_after_a_commit_survives_the_next_commit_and_the_revert() {
    let dirs = folders();
    let mut session = session_playing(&dirs.original);

    // One second as one bar at 120 BPM (two seconds) is a stretch by 2.
    let first = session.commit_stretch(120.0, &dirs.renders).expect("a commit");
    assert!((first.ratio - 2.0).abs() < 1.0e-4);
    let render = first.stored.expect("the render was written");
    assert!(render.starts_with(&dirs.renders), "{}", render.display());
    let channel = &session.channels[0];
    assert_eq!(channel.sample_path.as_deref(), Some(render.as_path()));
    assert!(channel.sample_embedded, "the song owns its render, as it owns a take");
    assert_eq!(decode(&render).frames, channel.sample_data.as_ref().unwrap().frames);
    assert_eq!(channel.sample_data.as_ref().unwrap().frames.len(), 96_000);

    // A slice on the render, at its frame 24,000: the original's 12,000.
    session.add_slice(0.25, false);
    assert_eq!(marker_frames(&session), vec![24_000]);

    let channel = &session.channels[0];
    let commit = channel.commit.as_deref().unwrap();
    assert!(!commit_is_stale(channel, commit, 120.0));
    assert!(commit_is_stale(channel, commit, 100.0), "the tempo moved");

    // At 100 BPM a bar is 2.4 s, so the committed audio stretches by 1.2.
    let second = session.commit_stretch(100.0, &dirs.renders).expect("a second commit");
    assert!((second.ratio - 1.2).abs() < 1.0e-4, "{}", second.ratio);
    assert!(second.stored.is_ok());
    let frames = marker_frames(&session);
    assert_eq!(frames.len(), 1, "the slice made after the first commit is gone");
    assert!(near(frames[0], 28_800.0), "not at its stretched position: {frames:?}");
    let commit = session.channels[0].commit.as_deref().unwrap();
    assert_eq!(commit.steps.len(), 2, "commits stack");
    assert_eq!(
        commit.original,
        Some(SampleReference::File {
            path: dirs.original.clone(),
            embedded: false
        })
    );
    assert!(
        matches!(
            session.commit_stretch(100.0, &dirs.renders),
            Err(CommitRefusal::NothingToCommit)
        ),
        "a commit that is not stale changes nothing"
    );

    let reverted = session.revert_stretch().expect("a revert");
    assert!(!reverted.original_changed);
    let channel = &session.channels[0];
    assert_eq!(channel.sample_path.as_deref(), Some(dirs.original.as_path()));
    assert!(!channel.sample_embedded);
    assert!(channel.commit.is_none());
    assert_eq!(channel.sample_data.as_ref().unwrap().frames.len(), 48_000);
    let frames = marker_frames(&session);
    assert_eq!(frames.len(), 1);
    assert!(near(frames[0], 12_000.0), "the slice did not come home: {frames:?}");
    assert!(reverted.params.stretch_enabled, "the live stretch is back");
}

/// MOO-367's missing assertion, as the new model makes it meaningful: after
/// a commit, moving a marker that a Slices loop grid snaps the loop to
/// changes the span the commit fitted, so committing again would stretch,
/// and the commit reads stale.
#[test]
fn moving_a_marker_the_loop_snaps_to_makes_the_commit_stale() {
    use mooloop_core::sampler::{LoopMode, LoopQuantize};

    let dirs = folders();
    let mut session = session_playing(&dirs.original);
    if let Some(params) = session.channels[0].sampler_params_mut() {
        params.loop_mode = LoopMode::Forward;
        params.loop_quantize = LoopQuantize::Slices;
        params.loop_start = 0.26;
        params.loop_end = 0.74;
    }
    session.channels[0].slices.add(12_000);
    session.channels[0].slices.add(36_000);
    session.commit_stretch(120.0, &dirs.renders).expect("a commit");
    let channel = &session.channels[0];
    assert!(!commit_is_stale(channel, channel.commit.as_deref().unwrap(), 120.0));

    // The loop's end snaps to the second marker; move it.
    session.move_slice(1, 0.9).expect("the marker moved");
    let channel = &session.channels[0];
    assert!(commit_is_stale(channel, channel.commit.as_deref().unwrap(), 120.0));
}

/// The commit after a stale one renders the *committed* audio, never the
/// original: a trim made after the first commit is still there after the
/// second (Adam, 2026-09-30).
#[test]
fn a_trim_made_after_a_commit_survives_the_next_one() {
    let dirs = folders();
    let mut session = session_playing(&dirs.original);
    session.commit_stretch(120.0, &dirs.renders).expect("a commit");
    if let Some(params) = session.channels[0].sampler_params_mut() {
        params.end = 0.75;
    }
    session.commit_stretch(100.0, &dirs.renders).expect("a second commit");
    let end = session.channels[0].sampler_params().end;
    assert!((end - 0.75).abs() < 1.0e-3, "the trim went back: {end}");
}

/// MOO-394: a reload plays the stored render, not a re-render -- even when
/// the original was replaced on disk since. A REVERT onto the replaced
/// original says so and still brings the markers.
#[test]
fn a_reload_plays_the_stored_render() {
    let dirs = folders();
    let mut session = session_playing(&dirs.original);
    session.commit_stretch(120.0, &dirs.renders).expect("a commit");
    session.add_slice(0.5, false);
    let baked = session.channels[0].sample_data.clone().unwrap();

    let song = dirs.root.join("song.mooloop");
    mooloop_project::save_song(&song, &session.project_snapshot(120, 50), AssetMode::Referenced)
        .unwrap();
    write_wav(&dirs.original, &clicks(0.5));

    let Ok(resolved) = mooloop_session::document::resolve_document(&song) else {
        panic!("the song did not load");
    };
    let LoadedDocument::Song(project) = &resolved.report.document else {
        panic!("a song");
    };
    let state = project.channels[0].setup.sampler_state().unwrap();
    let SampleReference::File { path, embedded } = &state.sample else {
        panic!("the render is a file");
    };
    assert!(*embedded, "the render stays owned in a referenced save");
    assert!(path.starts_with(dirs.root.join("song.mooloop-assets")), "{}", path.display());
    let played = resolved.samples[0].clone().expect("the render decoded");
    assert_eq!(played.frames, baked.frames, "the reload re-rendered");

    let mut reopened = Session::default();
    reopened.replace_project(project, &resolved.samples);
    reopened.selected = 0;
    let channel = &reopened.channels[0];
    assert!(channel.committed_sample.is_none(), "nothing re-made");
    assert_eq!(channel.published_sample().unwrap().frames, baked.frames);

    let reverted = reopened.revert_stretch().expect("a revert");
    assert!(reverted.original_changed, "the original changed on disk");
    let frames = marker_frames(&reopened);
    assert_eq!(frames.len(), 1);
    assert!(near(frames[0], 12_000.0), "halfway through the new original: {frames:?}");
}

/// The original travels with the render: a song and a channel preset saved
/// with embedded assets reopen and REVERT with every file outside them gone.
#[test]
fn a_saved_commit_reverts_after_reopening_with_nothing_else_on_disk() {
    let dirs = folders();
    let mut session = session_playing(&dirs.original);
    session.commit_stretch(120.0, &dirs.renders).expect("a commit");
    session.add_slice(0.25, false);

    let song = dirs.root.join("song.mooloop");
    let snapshot = session.project_snapshot(120, 50);
    mooloop_project::save_song(&song, &snapshot, AssetMode::Embedded).unwrap();
    let preset = dirs.root.join("break.mooloop-channel");
    mooloop_project::save_channel_preset(
        &preset,
        &snapshot.channels[0].setup,
        PresetInfo {
            name: "break".into(),
            category: String::new(),
            tags: Vec::new(),
        },
        AssetMode::Embedded,
    )
    .unwrap();
    std::fs::remove_file(&dirs.original).unwrap();
    std::fs::remove_dir_all(&dirs.renders).unwrap();

    for bundle in [&song, &preset] {
        let Ok(resolved) = mooloop_session::document::resolve_document(bundle) else {
            panic!("{} did not load", bundle.display());
        };
        let mut project = Project::default();
        match &resolved.report.document {
            LoadedDocument::Song(song) => project = song.clone(),
            LoadedDocument::Channel(setup) => project.channels[0].setup = (**setup).clone(),
            other => panic!("{other:?}"),
        }
        let commit = project.channels[0]
            .setup
            .sampler_state()
            .and_then(|state| state.commit.as_deref())
            .expect("the commit was saved");
        let Some(SampleReference::File { path, embedded }) = &commit.original else {
            panic!("the original was saved: {commit:?}");
        };
        assert!(*embedded && path.is_file(), "{}: {}", bundle.display(), path.display());

        let mut reopened = Session::default();
        reopened.replace_project(&project, &resolved.samples);
        reopened.selected = 0;
        reopened.revert_stretch().expect("a revert");
        let channel = &reopened.channels[0];
        assert_eq!(channel.sample_data.as_ref().unwrap().frames.len(), 48_000);
        let frames = marker_frames(&reopened);
        assert!(near(frames[0], 12_000.0), "{}: {frames:?}", bundle.display());
    }
}

/// A commit whose render cannot be written still happens, and says so: the
/// channel holds the render, unstored, over its original, so a save keeps
/// what re-makes it.
#[test]
fn a_render_that_cannot_be_written_is_held_unstored() {
    let dirs = folders();
    // A file where the folder should be.
    std::fs::create_dir_all(dirs.renders.parent().unwrap()).unwrap();
    std::fs::write(&dirs.renders, b"not a folder").unwrap();
    let mut session = session_playing(&dirs.original);
    let committed = session.commit_stretch(120.0, &dirs.renders).expect("a commit");
    assert!(committed.stored.is_err());
    let channel = &session.channels[0];
    assert_eq!(channel.sample_path.as_deref(), Some(dirs.original.as_path()));
    assert!(channel.commit.as_deref().unwrap().is_unstored());
    assert_eq!(channel.published_sample().unwrap().frames.len(), 96_000);

    // What a save writes re-makes the same render on install.
    let project = session.project_snapshot(120, 50);
    let mut reopened = Session::default();
    reopened.replace_project(&project, &[Some(decode(&dirs.original))]);
    assert_eq!(
        reopened.channels[0].published_sample().unwrap().frames,
        session.channels[0].published_sample().unwrap().frames
    );
}

/// A song saved by 0.1.5 with a commit loads and plays as it did: its
/// commit table is read as one unstored step over its playback region, and
/// the install re-makes the same render 0.1.5 made, compared in-process
/// against a render of that region.
#[test]
fn a_0_1_5_commit_loads_and_plays_as_it_did() {
    // The table's fields as 0.1.5 wrote them (`mooloop-core` reads the same
    // table from TOML in its own test).
    let table = r#"{
        "mode": "drums", "ratio": 1.5, "grain": 512,
        "source_start": 0.25, "source_end": 0.75,
        "source_loop_start": 0.0, "source_loop_end": 1.0,
        "source_markers": [{ "id": 1, "frame": 12000, "hand": true }]
    }"#;
    let commit: SampleCommit = serde_json::from_str(table).unwrap();
    assert!(commit.is_unstored());

    let dirs = folders();
    let source = decode(&dirs.original);
    let mut project = Project::default();
    let state = project.channels[0].setup.sampler_state_mut().unwrap();
    state.sample = SampleReference::File {
        path: dirs.original.clone(),
        embedded: false,
    };
    state.commit = Some(Box::new(commit));
    let mut session = Session::default();
    session.replace_project(&project, &[Some(source.clone())]);

    let baked = mooloop_dsp::stretch::render_stretched(
        &source.frames,
        Region {
            start: 12_000.0,
            end: 36_000.0,
            edge: RegionEdge::Silent,
        },
        StretchMode::Drums,
        512,
        1.5,
        RATE,
    );
    assert_eq!(
        session.channels[0].published_sample().unwrap().frames,
        baked.frames,
        "a 0.1.5 commit no longer plays the render it was baked as"
    );
}
