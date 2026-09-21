//! Takes nothing refers to any more (`audio-recording/06`).
//!
//! Every take is written to a file and kept, so an evening of retakes leaves
//! a pile of recordings the song does not use. This module answers which ones
//! those are.
//!
//! **Nothing here deletes.** A take is moved to the desktop trash, and the
//! reason is the one case this cannot tell apart from an unused take: a
//! recording belonging to a song that was never saved, left behind by a
//! crash. There is one app instance and a save moves a take into its project,
//! so the shared folder only ever holds this session's takes plus crash
//! leftovers -- and the crash case is exactly the one where deleting silently
//! would cost somebody a recording they wanted.
//!
//! What counts as referenced is broader than "some channel plays it":
//!
//! - any channel's sample in the **open project**;
//! - the same in **every undo and redo snapshot**, because undoing back to a
//!   take has to still find its file. This is why clearing the history is the
//!   moment takes become unused, and why the quit prompt is where the plan
//!   puts the offer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mooloop_core::project::{ChannelSource, SampleReference};
use mooloop_core::Project;

/// Where a discarded take goes.
///
/// A trait because the real one needs a desktop session to put a file in --
/// freedesktop's trash or Finder's -- which a test does not have and must not
/// depend on. It is also the seam that keeps "which files" testable apart
/// from "and then remove them".
pub trait Trash {
    /// Move `path` somewhere it can be recovered from. `Err` leaves it alone.
    fn discard(&self, path: &Path) -> Result<(), String>;
}

/// The desktop's own trash: freedesktop on Linux, Finder's on macOS.
#[derive(Debug, Default, Clone, Copy)]
pub struct DesktopTrash;

impl Trash for DesktopTrash {
    fn discard(&self, path: &Path) -> Result<(), String> {
        trash::delete(path).map_err(|error| format!("{}: {error}", path.display()))
    }
}

/// One take in a recordings folder that nothing points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedTake {
    pub path: PathBuf,
    pub bytes: u64,
    /// When it was last written.
    pub modified: SystemTime,
    /// Older than this run of the application.
    ///
    /// Only a crash leaves one, so it may belong to a song that was never
    /// saved. Reported separately and never swept without being asked: the
    /// quit prompt offers this session's takes and leaves these alone.
    pub from_earlier_session: bool,
}

/// Every sample file path a project still points at.
///
/// Paths are canonicalised before they are compared, here and in
/// [`unused_takes`]: a sample can be referenced by a route that reaches the
/// same file differently -- a symlinked data directory is the ordinary case --
/// and comparing the strings would call a referenced take unused.
pub fn referenced_paths<'a>(projects: impl IntoIterator<Item = &'a Project>) -> HashSet<PathBuf> {
    let mut paths = HashSet::new();
    for project in projects {
        for channel in &project.channels {
            let ChannelSource::Sampler(sampler) = &channel.setup.source else {
                continue;
            };
            if let SampleReference::File { path, .. } = &sampler.sample {
                paths.insert(canonical(path));
            }
        }
    }
    paths
}

/// Every take path the live session or anything in its history points at.
///
/// The two halves are different shapes and both are needed. The live session
/// holds a decoded channel with a `sample_path`; the history holds whole
/// project snapshots. Asking only the live session would offer a take that an
/// undo is about to want back.
pub fn referenced_by(
    session: &crate::session::Session,
    history: &crate::history::History<crate::project::ProjectSnapshot>,
) -> HashSet<PathBuf> {
    let mut paths: HashSet<PathBuf> = session
        .channels
        .iter()
        .filter_map(|channel| channel.sample_path.as_deref().map(canonical))
        .collect();
    for entry in history.entries() {
        paths.extend(referenced_paths([&entry.before.project, &entry.after.project]));
    }
    paths
}

/// `canonicalize` when the file is there, the path as written when it is not.
///
/// A reference to a file that has already gone cannot match anything in the
/// folder listing anyway, and a missing file must not make the whole scan
/// fail -- the answer to "which takes are unused" would become "all of them".
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Every take in `dir` that `referenced` does not name.
///
/// `session_start` is when this run of the application began; anything older
/// is a crash leftover and is marked rather than filtered, so the caller
/// decides what to do about it.
///
/// A folder that cannot be read is not an error: the recordings folder is
/// created on first use, so its absence means no takes rather than a failure.
pub fn unused_takes(
    dir: &Path,
    referenced: &HashSet<PathBuf>,
    session_start: SystemTime,
) -> Vec<UnusedTake> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut unused: Vec<UnusedTake> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            // Only what this app writes. A folder the user has put something
            // else in is not this feature's business to tidy.
            if path.extension().and_then(|extension| extension.to_str()) != Some("wav") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            if !meta.is_file() || referenced.contains(&canonical(&path)) {
                return None;
            }
            let modified = meta.modified().ok()?;
            Some(UnusedTake {
                path,
                bytes: meta.len(),
                modified,
                from_earlier_session: modified < session_start,
            })
        })
        .collect();
    // Stable order, so a prompt lists the same thing twice running.
    unused.sort_by(|left, right| left.path.cmp(&right.path));
    unused
}

/// Move every take in `takes` to `trash`, and report what happened.
///
/// One failure does not stop the rest: a take whose file has gone since the
/// scan is not a reason to keep the others.
pub fn discard_all(trash: &dyn Trash, takes: &[UnusedTake]) -> (usize, Vec<String>) {
    let mut moved = 0;
    let mut failures = Vec::new();
    for take in takes {
        match trash.discard(&take.path) {
            Ok(()) => moved += 1,
            Err(error) => failures.push(error),
        }
    }
    (moved, failures)
}

/// `"3 takes (12.4 MB)"`, for a prompt with one line to say it in.
pub fn summary(takes: &[UnusedTake]) -> String {
    let bytes: u64 = takes.iter().map(|take| take.bytes).sum();
    let count = takes.len();
    let plural = if count == 1 { "take" } else { "takes" };
    format!("{count} {plural} ({})", human_bytes(bytes))
}

/// Sizes as a person reads them. Takes are megabytes, so this stops at GB.
fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes / KB)
    } else {
        format!("{bytes:.0} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    /// A trash that only remembers, so the test needs no desktop session.
    #[derive(Default)]
    struct Remembering {
        taken: Mutex<Vec<PathBuf>>,
    }

    impl Trash for Remembering {
        fn discard(&self, path: &Path) -> Result<(), String> {
            self.taken.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }

    /// A project whose one sampler channel plays `path`.
    fn project_playing(path: &Path) -> Project {
        let mut project = Project::default();
        let channel = project.channels.first_mut().expect("a default channel");
        channel.setup.source = ChannelSource::Sampler(mooloop_core::project::SamplerState {
            sample: SampleReference::File {
                path: path.to_path_buf(),
                embedded: true,
            },
            ..Default::default()
        });
        project
    }

    fn write_take(dir: &Path, name: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, vec![0u8; 2048]).unwrap();
        path
    }

    /// **A take the undo history still reaches is not offered for deletion**,
    /// and becomes unused the moment the history is cleared. That is the
    /// whole reason the offer lives on the quit prompt: closing a project is
    /// when history-only takes stop being reachable.
    #[test]
    fn a_take_only_the_history_refers_to_is_kept_until_the_history_goes() {
        let dir = tempfile::tempdir().unwrap();
        let take = write_take(dir.path(), "20260920-120000-Sampler_1.wav");
        let long_ago = SystemTime::now() - Duration::from_secs(60);

        // The live project has moved on; only an undo snapshot still plays it.
        let live = Project::default();
        let undone = project_playing(&take);
        let referenced = referenced_paths([&live, &undone]);
        assert!(
            unused_takes(dir.path(), &referenced, long_ago).is_empty(),
            "an undo would have nothing to go back to"
        );

        // History cleared: nothing reaches it now.
        let referenced = referenced_paths([&live]);
        let unused = unused_takes(dir.path(), &referenced, long_ago);
        assert_eq!(unused.len(), 1);
        assert_eq!(unused[0].path, take);
        assert_eq!(unused[0].bytes, 2048);
        assert!(!unused[0].from_earlier_session);
    }

    /// A take the open project plays is never offered, however many other
    /// takes are lying beside it.
    #[test]
    fn a_take_the_song_plays_is_never_offered() {
        let dir = tempfile::tempdir().unwrap();
        let played = write_take(dir.path(), "20260920-120000-kept.wav");
        let spare = write_take(dir.path(), "20260920-130000-spare.wav");
        let referenced = referenced_paths([&project_playing(&played)]);

        let unused = unused_takes(dir.path(), &referenced, SystemTime::UNIX_EPOCH);

        assert_eq!(unused.len(), 1, "{unused:?}");
        assert_eq!(unused[0].path, spare);
    }

    /// **A file older than this run is a crash leftover**, and is marked so
    /// the quit prompt can leave it alone: it may be the only copy of a take
    /// from a song that was never saved.
    #[test]
    fn a_file_from_an_earlier_session_is_marked_not_swept() {
        let dir = tempfile::tempdir().unwrap();
        write_take(dir.path(), "20260919-120000-old.wav");
        // Every file on disk predates a session that starts now.
        let session_start = SystemTime::now() + Duration::from_secs(1);

        let unused = unused_takes(dir.path(), &HashSet::new(), session_start);

        assert_eq!(unused.len(), 1);
        assert!(
            unused[0].from_earlier_session,
            "a crash leftover must be told apart from this session's takes"
        );
    }

    /// Only this app's own files, and only real ones.
    #[test]
    fn the_scan_ignores_anything_that_is_not_a_take() {
        let dir = tempfile::tempdir().unwrap();
        write_take(dir.path(), "20260920-120000-take.wav");
        std::fs::write(dir.path().join("notes.txt"), b"not a take").unwrap();
        std::fs::create_dir(dir.path().join("a-folder.wav")).unwrap();

        let unused = unused_takes(dir.path(), &HashSet::new(), SystemTime::UNIX_EPOCH);

        assert_eq!(unused.len(), 1, "{unused:?}");
        assert!(unused[0].path.ends_with("20260920-120000-take.wav"));
    }

    /// A missing folder is no takes, not a failure. The recordings folder is
    /// created on first use, so this is the ordinary state before one.
    #[test]
    fn a_folder_that_is_not_there_yet_has_no_takes() {
        let dir = tempfile::tempdir().unwrap();
        let unused = unused_takes(
            &dir.path().join("recordings"),
            &HashSet::new(),
            SystemTime::UNIX_EPOCH,
        );
        assert!(unused.is_empty());
    }

    /// Discarding goes through the trash and reports the count; the file is
    /// never removed by this crate itself.
    #[test]
    fn discarding_hands_every_take_to_the_trash() {
        let dir = tempfile::tempdir().unwrap();
        write_take(dir.path(), "20260920-120000-one.wav");
        write_take(dir.path(), "20260920-130000-two.wav");
        let unused = unused_takes(dir.path(), &HashSet::new(), SystemTime::UNIX_EPOCH);
        let trash = Remembering::default();

        let (moved, failures) = discard_all(&trash, &unused);

        assert_eq!(moved, 2);
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(trash.taken.lock().unwrap().len(), 2);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            2,
            "discarding is the trash's job, not this crate's"
        );
    }

    #[test]
    fn a_summary_reads_as_a_person_would_say_it() {
        let dir = tempfile::tempdir().unwrap();
        write_take(dir.path(), "20260920-120000-one.wav");
        let one = unused_takes(dir.path(), &HashSet::new(), SystemTime::UNIX_EPOCH);
        assert_eq!(summary(&one), "1 take (2 KB)");
        assert_eq!(summary(&[]), "0 takes (0 bytes)");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
