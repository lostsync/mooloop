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
    // A stream still collecting -- a knob on a desk, a take of notes -- has
    // a `before` an undo will reach as surely as any recorded entry's.
    if let Some(before) = history.open_before() {
        paths.extend(referenced_paths([&before.project]));
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

/// What the clean-up dialog lists (`audio-recording/06`, MOO-38).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanUp {
    /// **Not used by this song**, ticked by default: this session's takes in
    /// the shared folder that nothing reaches, and takes in the song's own
    /// `recordings/` that neither the open song, its history nor the song as
    /// saved on disk reaches.
    pub not_used: Vec<UnusedTake>,
    /// **Left from earlier sessions**, unticked by default: shared-folder
    /// takes older than this run. See [`EARLIER_SESSIONS_NOTE`].
    pub earlier: Vec<UnusedTake>,
}

/// Why the earlier-session list starts unticked, in the dialog's words.
///
/// Two things leave a take there: a crash, and a quit while only the undo
/// history still reached it (MOO-38 question 2, decided conservatively --
/// see `docs/plans/audio-recording/00-status.md`). Either way it may be the
/// only copy of a take from a song that was never saved.
pub const EARLIER_SESSIONS_NOTE: &str = "Older than this session. A crash, or a quit while \
     only the undo history still used a take, leaves one here, and it may be the only copy \
     of a take from a song that was never saved. These start unticked.";

/// Build the clean-up dialog's two lists.
///
/// `shared` is the shared recordings folder. `song` is the open song's own
/// `recordings/` folder and what the song **as saved on disk** refers to --
/// the third reference source the plan names, because the song in memory
/// may not be saved yet, and a file the saved song plays is not unused just
/// because the unsaved edit stopped playing it. `None` when the song has
/// never been saved, or its saved file could not be read: then its folder is
/// not offered at all, which loses nothing.
pub fn clean_up(
    shared: &Path,
    song: Option<(&Path, &HashSet<PathBuf>)>,
    referenced: &HashSet<PathBuf>,
    session_start: SystemTime,
) -> CleanUp {
    let mut lists = CleanUp::default();
    for take in unused_takes(shared, referenced, session_start) {
        if take.from_earlier_session {
            lists.earlier.push(take);
        } else {
            lists.not_used.push(take);
        }
    }
    if let Some((folder, saved)) = song {
        let both: HashSet<PathBuf> = referenced.union(saved).cloned().collect();
        // A song's own folder has no "earlier session": everything in it was
        // put there by a save of this song, so nothing else can be using it.
        for mut take in unused_takes(folder, &both, SystemTime::UNIX_EPOCH) {
            take.from_earlier_session = false;
            lists.not_used.push(take);
        }
    }
    lists
}

/// Every sample file the song saved at `song` refers to, read from disk.
/// `None` when it cannot be read, which the caller treats as "offer nothing
/// from its folder".
pub fn saved_song_references(song: &Path) -> Option<HashSet<PathBuf>> {
    let loaded = mooloop_project::load_bundle(song).ok()?;
    let mooloop_project::LoadedDocument::Song(project) = loaded.document else {
        return None;
    };
    Some(referenced_paths([&project]))
}

/// A take's length, from its WAV header: `"0:12.4"`. Empty when the header
/// cannot be read.
pub fn length_text(path: &Path) -> String {
    let Ok(reader) = hound::WavReader::open(path) else {
        return String::new();
    };
    let rate = reader.spec().sample_rate.max(1);
    let seconds = f64::from(reader.duration()) / f64::from(rate);
    let minutes = (seconds / 60.0).floor();
    format!("{}:{:04.1}", minutes as u64, seconds - minutes * 60.0)
}

/// When a take was last written, in UTC and labelled so: `"2026-09-23 01:12 UTC"`.
pub fn date_text(when: SystemTime) -> String {
    crate::take::utc_minutes(when)
}

/// The size, as [`summary`] prints it.
pub fn size_text(bytes: u64) -> String {
    human_bytes(bytes)
}

/// Move the shared recordings folder from `old` to `new`, once (MOO-75).
///
/// Takes are data, not configuration, and lived under `~/.config` until
/// 2026-09-23. They move to the data directory, and **the old path is left
/// as a symbolic link to the new one**, because a song saved with referenced
/// (not embedded) samples names its takes by absolute path: without the
/// link, every such song would open with its takes missing.
///
/// Returns whether anything moved. Nothing to do -- the same folder, no old
/// folder, or an old one that is already the link -- is `Ok(false)`. A file
/// whose name the new folder already has is left where it is, and so is the
/// old folder then, with no link: both stay readable, and the error says
/// which file.
pub fn migrate_folder(old: &Path, new: &Path) -> Result<bool, String> {
    if old == new {
        return Ok(false);
    }
    let Ok(meta) = std::fs::symlink_metadata(old) else {
        return Ok(false);
    };
    if !meta.is_dir() {
        // Already the link, or something this does not own.
        return Ok(false);
    }
    let fail = |what: &str, path: &Path, error: std::io::Error| {
        format!("could not {what} {}: {error}", path.display())
    };
    if let Some(parent) = new.parent() {
        std::fs::create_dir_all(parent).map_err(|error| fail("create", parent, error))?;
    }
    // The cheap case: one rename, when the new folder is not there yet and
    // both are on one file system.
    let whole = !new.exists() && std::fs::rename(old, new).is_ok();
    if !whole {
        std::fs::create_dir_all(new).map_err(|error| fail("create", new, error))?;
        let entries = std::fs::read_dir(old).map_err(|error| fail("read", old, error))?;
        let mut kept = Vec::new();
        for entry in entries.flatten() {
            let from = entry.path();
            let to = new.join(entry.file_name());
            if to.exists() {
                kept.push(from);
                continue;
            }
            if std::fs::rename(&from, &to).is_err() {
                // Another file system: copy, and remove the original only
                // once the copy is whole.
                std::fs::copy(&from, &to).map_err(|error| fail("copy", &from, error))?;
                std::fs::remove_file(&from).map_err(|error| fail("remove", &from, error))?;
            }
        }
        if let Some(first) = kept.first() {
            return Err(format!(
                "{} file(s) left in {} because {} already has one of that name, the first {}",
                kept.len(),
                old.display(),
                new.display(),
                first.display()
            ));
        }
        std::fs::remove_dir(old).map_err(|error| fail("remove", old, error))?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(new, old).map_err(|error| fail("link", old, error))?;
    Ok(true)
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

    /// **The dialog's two lists** (MOO-38). This session's unused shared
    /// takes and the song's own unused takes are "not used by this song";
    /// older shared takes are "left from earlier sessions". A take in the
    /// song's folder that only the song **as saved on disk** plays is not
    /// offered: the unsaved edit that stopped playing it may never be saved.
    #[test]
    fn the_clean_up_lists_split_by_session_and_keep_what_the_saved_song_plays() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("recordings");
        let song = dir.path().join("song.mooloop-assets").join("recordings");
        let old = write_take(&shared, "20260919-120000-old.wav");
        let session_start = SystemTime::now();
        std::thread::sleep(Duration::from_millis(20));
        let fresh = write_take(&shared, "20260923-120000-fresh.wav");
        let replaced = write_take(&song, "20260922-120000-replaced.wav");
        let saved_only = write_take(&song, "20260922-130000-saved.wav");
        let playing = write_take(&song, "20260922-140000-playing.wav");
        // `old` predates the session however fast the disk is.
        let an_hour_ago = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(an_hour_ago)
            .unwrap();

        let referenced = referenced_paths([&project_playing(&playing)]);
        let saved = referenced_paths([&project_playing(&saved_only)]);
        let lists = clean_up(&shared, Some((&song, &saved)), &referenced, session_start);

        let names = |takes: &[UnusedTake]| -> Vec<PathBuf> {
            takes.iter().map(|take| take.path.clone()).collect()
        };
        assert_eq!(names(&lists.not_used), [fresh, replaced]);
        assert_eq!(names(&lists.earlier), [old]);
        assert!(lists.not_used.iter().all(|take| !take.from_earlier_session));

        // A song never saved, or one whose file will not read: its folder is
        // not offered at all.
        let unsaved = clean_up(&shared, None, &referenced, session_start);
        assert_eq!(unsaved.not_used.len(), 1, "{unsaved:?}");
    }

    #[test]
    fn a_takes_length_and_date_read_as_a_person_would_say_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("take.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 1000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..(2 * 72_500) {
            writer.write_sample(0.0f32).unwrap();
        }
        writer.finalize().unwrap();
        assert_eq!(length_text(&path), "1:12.5");
        assert_eq!(length_text(&dir.path().join("missing.wav")), "");
        assert_eq!(
            date_text(SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_125_920)),
            "2026-09-23 01:12 UTC"
        );
    }

    /// **The recordings folder moves out of the config directory once, and
    /// the old path keeps working** (MOO-75): a song saved with referenced
    /// samples names its takes by absolute path, so the old folder becomes a
    /// link to the new one.
    #[cfg(unix)]
    #[test]
    fn the_recordings_folder_moves_once_and_leaves_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("config/mooloop/recordings");
        let new = dir.path().join("data/mooloop/recordings");
        let take = write_take(&old, "20260920-120000-take.wav");

        assert_eq!(migrate_folder(&old, &new), Ok(true));

        assert!(new.join("20260920-120000-take.wav").is_file());
        assert!(std::fs::symlink_metadata(&old).unwrap().file_type().is_symlink());
        assert!(take.is_file(), "a reference to the old path still finds the take");
        assert_eq!(migrate_folder(&old, &new), Ok(false), "only once");
        assert_eq!(migrate_folder(&new, &new), Ok(false));
        assert_eq!(
            migrate_folder(&dir.path().join("nothing"), &new),
            Ok(false),
            "no old folder is nothing to do"
        );
    }

    /// Into a new folder that already has takes -- this build ran once with
    /// a fresh data directory -- each file moves across, and one whose name
    /// is taken stays where it was, with the old folder and no link.
    #[cfg(unix)]
    #[test]
    fn a_name_the_new_folder_already_has_is_left_where_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        write_take(&old, "a.wav");
        write_take(&old, "b.wav");
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("a.wav"), b"a different take").unwrap();

        let error = migrate_folder(&old, &new).unwrap_err();

        assert!(error.contains("a.wav"), "{error}");
        assert!(new.join("b.wav").is_file());
        assert_eq!(std::fs::read(new.join("a.wav")).unwrap(), b"a different take");
        assert!(old.join("a.wav").is_file(), "nothing is overwritten or lost");
        assert!(!std::fs::symlink_metadata(&old).unwrap().file_type().is_symlink());
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
