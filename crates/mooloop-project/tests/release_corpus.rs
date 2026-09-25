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
