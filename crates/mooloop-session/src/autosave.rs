//! Autosave and crash recovery (MOO-103).
//!
//! A crash, a kill or a power cut used to lose everything since the last
//! save. Now, once a minute while the song has unsaved changes and no gesture
//! is open, the document is written the way a song that could not be saved
//! is set aside (`quarantine_song`): Referenced mode, so no sample is copied.
//!
//! **Where.** Each running mooloop owns one folder under the autosave root
//! (the caller passes `<state dir>/autosave`), and holds an exclusive lock on
//! a file inside it for as long as it runs. The operating system drops the
//! lock when the process ends however it ends, so a folder whose lock can be
//! taken belongs to a mooloop that is gone, and what it holds is what that
//! mooloop had not saved. A lock rather than a pid: a pid is reused, and a
//! lock cannot outlive its process.
//!
//! **When it is removed.** When the song becomes clean (saved, or replaced by
//! New or Open), its autosave goes. A quit through the unsaved-changes
//! question has been answered, so it goes too; any other ending -- a signal,
//! a crash -- leaves it for the next launch to offer.
//!
//! The writes happen on one worker thread, latest wins: a write that is still
//! waiting when a newer one arrives is dropped, so a slow disk never queues
//! songs.

use crate::document::{resolve_document, DocumentProblem, ResolvedDocument};
use mooloop_core::{log_error, log_info, log_warn, Project, SampleReference};
use mooloop_project::LoadedDocument;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often an unsaved song is written out.
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(60);

const LOCK: &str = "lock";
const SONG: &str = "song.mooloop";
const ABOUT: &str = "about.txt";

/// What the pump knows about the document each time it asks.
#[derive(Clone, Debug)]
pub struct DocumentState<'a> {
    pub revision: u64,
    pub dirty: bool,
    pub gesture_open: bool,
    /// The file the song was opened from or last saved to, if any.
    pub original: Option<&'a Path>,
    /// The window's Embed assets flag, restored with the song.
    pub embed: bool,
}

enum Job {
    Write {
        project: Box<Project>,
        about: String,
        adopted: Vec<PathBuf>,
    },
    Clear {
        adopted: Vec<PathBuf>,
    },
}

/// This process's autosave: its folder, its lock and its writer.
pub struct Autosave {
    dir: PathBuf,
    lock: Option<File>,
    interval: Duration,
    last_write: Instant,
    /// The revision last handed to the writer, `None` when nothing of this
    /// document is on disk.
    written: Option<u64>,
    /// Folders of a dead mooloop whose song this one recovered. Removed once
    /// this one's own autosave has replaced them, or the song is clean.
    adopted: Vec<PathBuf>,
    jobs: Option<Sender<Job>>,
    worker: Option<JoinHandle<()>>,
}

impl Autosave {
    /// Takes a folder under `root` and locks it.
    pub fn start(root: &Path) -> io::Result<Self> {
        Self::start_with_interval(root, AUTOSAVE_INTERVAL)
    }

    /// [`Self::start`] with another interval, for tests.
    pub fn start_with_interval(root: &Path, interval: Duration) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        // The sequence number keeps two starts in one second of one process
        // apart; only tests do that, but a name collision fails the rename.
        static STARTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let name = format!(
            "{}-{}-{}",
            mooloop_core::log::file_stamp(),
            std::process::id(),
            STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        // Locked under a hidden name and only then renamed into view, so a
        // scan by another mooloop never sees this folder unlocked and takes
        // it for a dead one.
        let staging = root.join(format!(".{name}"));
        fs::create_dir_all(&staging)?;
        let lock = File::create(staging.join(LOCK))?;
        // `File::try_lock` is std since Rust 1.89. The pinned toolchain is
        // newer, but Adam's laptop builds with Fedora's own rustc, which has
        // to be 1.89 or later too.
        lock.try_lock().map_err(io::Error::other)?;
        let dir = root.join(&name);
        fs::rename(&staging, &dir)?;
        let (jobs, received) = mpsc::channel();
        let worker_dir = dir.clone();
        let worker = std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || write_jobs(&worker_dir, received))?;
        log_info!("project", "autosaving to {}", dir.display());
        Ok(Self {
            dir,
            lock: Some(lock),
            interval,
            last_write: Instant::now(),
            written: None,
            adopted: Vec::new(),
            jobs: Some(jobs),
            worker: Some(worker),
        })
    }

    /// This process's folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Called from the pump, as often as it likes. Writes the song when it
    /// has changed since the last write, the interval has passed, and no
    /// gesture is open; clears the autosave when the song is clean.
    /// `snapshot` is called only when a write is due.
    pub fn update(&mut self, now: Instant, state: DocumentState<'_>, snapshot: impl FnOnce() -> Project) {
        if !state.dirty {
            if self.written.is_some() || !self.adopted.is_empty() {
                self.written = None;
                let adopted = std::mem::take(&mut self.adopted);
                self.send(Job::Clear { adopted });
            }
            return;
        }
        if state.gesture_open
            || self.written == Some(state.revision)
            || now.duration_since(self.last_write) < self.interval
        {
            return;
        }
        self.last_write = now;
        self.written = Some(state.revision);
        let (project, owned) = reference_in_place(snapshot());
        let about = about_text(SystemTime::now(), state.original, state.embed, &owned);
        let adopted = std::mem::take(&mut self.adopted);
        self.send(Job::Write {
            project: Box::new(project),
            about,
            adopted,
        });
    }

    /// Takes over the folder of a recovered song: it is removed once this
    /// process has written its own autosave, or the song is clean. Until
    /// then a second crash still finds it.
    pub fn adopt(&mut self, recovered: &Recoverable) {
        self.adopted.push(recovered.dir.clone());
    }

    /// Stops the writer, waiting for a write in progress. `keep` leaves what
    /// was written for the next launch to offer; otherwise the folder goes.
    pub fn finish(mut self, keep: bool) {
        if !keep {
            let adopted = std::mem::take(&mut self.adopted);
            self.send(Job::Clear { adopted });
        }
        self.stop();
        // The lock goes before the folder: Windows will not delete a file
        // that is open.
        self.lock = None;
        if !keep {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }

    fn stop(&mut self) {
        self.jobs = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Autosave {
    /// An `Autosave` dropped without [`Autosave::finish`] -- an unwinding
    /// panic -- keeps what it wrote: that is the case it exists for.
    fn drop(&mut self) {
        self.stop();
    }
}

fn write_jobs(dir: &Path, jobs: Receiver<Job>) {
    while let Ok(first) = jobs.recv() {
        // Latest wins; the folders a dropped job would have removed are
        // carried over to the one that replaces it.
        let mut job = first;
        let mut adopted = Vec::new();
        while let Ok(next) = jobs.try_recv() {
            adopted.extend(take_adopted(&mut job));
            job = next;
        }
        adopted.extend(take_adopted(&mut job));
        let done = match job {
            Job::Write { project, about, .. } => write_song(dir, &project, &about),
            Job::Clear { .. } => clear(dir),
        };
        match done {
            Ok(()) => {
                for old in adopted {
                    let _ = fs::remove_dir_all(old);
                }
            }
            Err(error) => {
                log_error!("project", "autosave to {} failed: {error}", dir.display());
            }
        }
    }
}

fn take_adopted(job: &mut Job) -> Vec<PathBuf> {
    match job {
        Job::Write { adopted, .. } | Job::Clear { adopted } => std::mem::take(adopted),
    }
}

fn write_song(dir: &Path, project: &Project, about: &str) -> io::Result<()> {
    // The song file is replaced by a rename inside `save_song`, so a crash
    // mid-write leaves the previous autosave whole.
    mooloop_project::save_song(&dir.join(SONG), project, mooloop_project::AssetMode::Referenced)
        .map_err(io::Error::other)?;
    let staging = dir.join(format!(".{ABOUT}"));
    fs::write(&staging, about)?;
    fs::rename(staging, dir.join(ABOUT))
}

fn clear(dir: &Path) -> io::Result<()> {
    for name in [ABOUT, SONG] {
        match fs::remove_file(dir.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// The song with every sample it owns referenced where it already is, and
/// the paths of those samples.
///
/// A Referenced save still copies a sample the song owns into the new
/// bundle's sidecar (`prepare_song_asset`: an owned sample cannot be
/// un-embedded), so an autosave of an embedded song would copy every sample
/// once a minute. It points at the file in the song's own sidecar, or the
/// recordings folder, instead, and remembers which were owned so a recovery
/// gives them back to the song.
fn reference_in_place(mut project: Project) -> (Project, Vec<PathBuf>) {
    let mut owned = Vec::new();
    for channel in &mut project.channels {
        if let Some(sampler) = channel.setup.source.sampler_state_mut() {
            if let SampleReference::File { path, embedded } = &mut sampler.sample {
                if *embedded {
                    owned.push(path.clone());
                    *embedded = false;
                }
            }
        }
    }
    (project, owned)
}

/// When the autosave was written, and the song it stands for, one fact a
/// line: `saved <unix seconds>`, `original <path>` (absent for a song never
/// saved), `embed <0|1>`, and `owned <path>` for each sample the song owns.
fn about_text(now: SystemTime, original: Option<&Path>, embed: bool, owned: &[PathBuf]) -> String {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let mut text = format!("saved {seconds}\nembed {}\n", u8::from(embed));
    if let Some(original) = original {
        text.push_str(&format!("original {}\n", original.display()));
    }
    for path in owned {
        text.push_str(&format!("owned {}\n", path.display()));
    }
    text
}

/// A song a mooloop that is no longer running had not saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recoverable {
    /// The dead process's folder.
    pub dir: PathBuf,
    /// The autosaved song, to open as a song.
    pub song: PathBuf,
    /// Where the song was opened from or saved to, if anywhere. The
    /// recovered song takes this path, so Save goes where the user expects,
    /// never into the autosave folder.
    pub original: Option<PathBuf>,
    pub saved_at: SystemTime,
    /// The Embed assets flag the song had.
    pub embed: bool,
    /// The samples the song owned, referenced in place by the autosave.
    pub owned: Vec<PathBuf>,
}

impl Recoverable {
    /// The question's title: "Recover unsaved changes to Groove?".
    pub fn question(&self) -> String {
        match self.original.as_deref().and_then(song_label) {
            Some(name) => format!("Recover unsaved changes to {name}?"),
            None => "Recover an unsaved song?".to_owned(),
        }
    }

    /// The question's detail: how long ago, in words that need no clock.
    /// mooloop has no timezone database (see `mooloop_core::log`), so a
    /// wall-clock time would be UTC and wrong for most users.
    pub fn detail(&self, now: SystemTime) -> String {
        let ago = now.duration_since(self.saved_at).unwrap_or_default().as_secs();
        let when = match ago {
            0..=89 => "a minute ago".to_owned(),
            90..=3599 => format!("{} minutes ago", (ago + 30) / 60),
            3600..=5399 => "an hour ago".to_owned(),
            5400..=86_399 => format!("{} hours ago", (ago + 1800) / 3600),
            _ => format!("{} days ago", (ago + 43_200) / 86_400),
        };
        format!(
            "mooloop closed before these changes were saved. They were autosaved {when}. \
             Discard deletes them."
        )
    }
}

fn song_label(path: &Path) -> Option<String> {
    path.file_stem().map(|stem| stem.to_string_lossy().into_owned())
}

/// Every song a dead mooloop left unsaved under `root`, newest first. A
/// folder that is still locked belongs to a running mooloop -- this one
/// included -- and is left alone. A dead folder with nothing in it is
/// removed on the way.
pub fn find_recoverable(root: &Path) -> Vec<Recoverable> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let dir = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if hidden || !dir.is_dir() || still_running(&dir) {
            continue;
        }
        match read_recoverable(&dir) {
            Some(recoverable) => found.push(recoverable),
            None => {
                log_info!("project", "removing an empty autosave folder {}", dir.display());
                let _ = fs::remove_dir_all(&dir);
            }
        }
    }
    found.sort_by_key(|recoverable| std::cmp::Reverse(recoverable.saved_at));
    found
}

/// Whether a live process holds `dir`'s lock.
///
/// Asked a few times over a tenth of a second before it says yes: a child
/// process forked by the holder (a file chooser, `ffmpeg`) keeps a copy of
/// the lock's descriptor until it execs, so a lock can outlive its process
/// by that moment. The session's own tests fork like this in parallel, and
/// found it.
fn still_running(dir: &Path) -> bool {
    let Ok(lock) = File::options().read(true).write(true).open(dir.join(LOCK)) else {
        // No lock file at all: not a folder a live mooloop could be holding.
        return false;
    };
    for attempt in 0..5 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(20));
        }
        if lock.try_lock().is_ok() {
            let _ = lock.unlock();
            return false;
        }
    }
    true
}

fn read_recoverable(dir: &Path) -> Option<Recoverable> {
    let song = dir.join(SONG);
    if !song.is_file() {
        return None;
    }
    let about = fs::read_to_string(dir.join(ABOUT)).unwrap_or_default();
    let mut saved_at = None;
    let mut original = None;
    let mut embed = false;
    let mut owned = Vec::new();
    for line in about.lines() {
        if let Some(seconds) = line.strip_prefix("saved ") {
            saved_at = seconds
                .trim()
                .parse::<u64>()
                .ok()
                .map(|seconds| UNIX_EPOCH + Duration::from_secs(seconds));
        } else if let Some(path) = line.strip_prefix("original ") {
            original = Some(PathBuf::from(path));
        } else if let Some(flag) = line.strip_prefix("embed ") {
            embed = flag.trim() == "1";
        } else if let Some(path) = line.strip_prefix("owned ") {
            owned.push(PathBuf::from(path));
        }
    }
    let saved_at = saved_at
        .or_else(|| fs::metadata(&song).and_then(|meta| meta.modified()).ok())
        .unwrap_or(UNIX_EPOCH);
    Some(Recoverable {
        dir: dir.to_path_buf(),
        song,
        original,
        saved_at,
        embed,
        owned,
    })
}

/// Reads a recoverable song the way File > Open reads one, and gives back to
/// it the samples it owned before the autosave referenced them in place.
pub fn recover(recoverable: &Recoverable) -> Result<ResolvedDocument, DocumentProblem> {
    let mut document = resolve_document(&recoverable.song)?;
    let same = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let owned: Vec<PathBuf> = recoverable.owned.iter().map(|path| same(path)).collect();
    if let LoadedDocument::Song(project) = &mut document.report.document {
        for channel in &mut project.channels {
            if let Some(sampler) = channel.setup.source.sampler_state_mut() {
                if let SampleReference::File { path, embedded } = &mut sampler.sample {
                    if owned.contains(&same(path)) {
                        *embedded = true;
                    }
                }
            }
        }
    }
    Ok(document)
}

/// Every sample file an autosave under `root` refers to, a running
/// mooloop's or a dead one's, resolved as `recordings::referenced_by`
/// resolves them.
///
/// An autosave references samples where they are, and an untitled song's
/// recorded take is in the shared recordings folder, where File > Clean Up
/// Takes lists a take no open song uses -- one from before a crash among
/// them. That list has to hold these back, or the recovery would find its
/// take in the trash.
pub fn referenced_samples(root: &Path) -> std::collections::HashSet<PathBuf> {
    let mut paths = std::collections::HashSet::new();
    let Ok(entries) = fs::read_dir(root) else {
        return paths;
    };
    for entry in entries.flatten() {
        let song = entry.path().join(SONG);
        if song.is_file() {
            if let Some(found) = crate::recordings::saved_song_references(&song) {
                paths.extend(found);
            }
        }
    }
    paths
}

/// Deletes a recoverable song the user chose not to recover.
pub fn discard(recoverable: &Recoverable) {
    if let Err(error) = fs::remove_dir_all(&recoverable.dir) {
        log_warn!(
            "project",
            "could not remove the autosave {}: {error}",
            recoverable.dir.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_project::{load_bundle, LoadedDocument};

    fn state(revision: u64, dirty: bool, original: Option<&Path>) -> DocumentState<'_> {
        DocumentState {
            revision,
            dirty,
            gesture_open: false,
            original,
            embed: false,
        }
    }

    fn edited_song() -> Project {
        let mut project = Project::starter_kit(7);
        project.bpm = 97;
        project
    }

    fn load_song(path: &Path) -> Project {
        match load_bundle(path).expect("the autosave loads").document {
            LoadedDocument::Song(project) => project,
            _ => panic!("the autosave is a song"),
        }
    }

    /// **After an edit, a kill and a relaunch, the song is offered back with
    /// the edit in it** (MOO-103). `finish(keep: true)` joins the writer and
    /// drops the lock, which is what the operating system does for a process
    /// that is killed.
    #[test]
    fn an_edit_survives_a_kill_and_is_offered_on_relaunch() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("Groove.mooloop");
        let mut running = Autosave::start_with_interval(root.path(), Duration::ZERO).unwrap();

        assert!(
            find_recoverable(root.path()).is_empty(),
            "a running mooloop's autosave is not offered to another"
        );
        running.update(Instant::now(), state(3, true, Some(&original)), edited_song);
        running.finish(true);

        let found = find_recoverable(root.path());
        assert_eq!(found.len(), 1, "{found:?}");
        let recoverable = &found[0];
        assert_eq!(recoverable.original.as_deref(), Some(original.as_path()));
        assert_eq!(recoverable.question(), "Recover unsaved changes to Groove?");
        assert_eq!(load_song(&recoverable.song).bpm, 97, "the edit came back");

        // The relaunched mooloop recovers it, adopts the folder, and once its
        // own autosave has been written the dead one's is gone.
        let mut relaunched = Autosave::start_with_interval(root.path(), Duration::ZERO).unwrap();
        relaunched.adopt(recoverable);
        relaunched.update(Instant::now(), state(1, true, Some(&original)), edited_song);
        let own = relaunched.dir().to_path_buf();
        relaunched.finish(true);
        assert!(!recoverable.dir.exists(), "the adopted folder was removed");
        let found = find_recoverable(root.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].dir, own);
    }

    /// A song saved (clean) has nothing to recover, and neither does a quit
    /// that answered the unsaved-changes question.
    #[test]
    fn a_clean_song_or_an_answered_quit_leaves_nothing() {
        let root = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::start_with_interval(root.path(), Duration::ZERO).unwrap();
        autosave.update(Instant::now(), state(1, true, None), edited_song);
        autosave.update(Instant::now(), state(2, false, None), || unreachable!());
        autosave.finish(true);
        assert!(find_recoverable(root.path()).is_empty(), "saved, so clean");
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            0,
            "and the empty dead folder was removed by the scan"
        );

        let mut autosave = Autosave::start_with_interval(root.path(), Duration::ZERO).unwrap();
        autosave.update(Instant::now(), state(1, true, None), edited_song);
        autosave.finish(false);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    /// Nothing is written inside a gesture, before the interval, or twice for
    /// one revision.
    #[test]
    fn it_writes_only_when_due() {
        let root = tempfile::tempdir().unwrap();
        let mut autosave = Autosave::start_with_interval(root.path(), Duration::from_secs(60)).unwrap();
        let start = Instant::now();
        let mut snapshots = 0;
        let mut ask = |at: Instant, revision: u64, gesture_open: bool| {
            let state = DocumentState {
                revision,
                dirty: true,
                gesture_open,
                original: None,
                embed: false,
            };
            autosave.update(at, state, || {
                snapshots += 1;
                edited_song()
            });
        };
        ask(start, 1, false);
        ask(start + Duration::from_secs(61), 1, true);
        ask(start + Duration::from_secs(62), 1, false);
        ask(start + Duration::from_secs(63), 1, false);
        ask(start + Duration::from_secs(90), 2, false);
        ask(start + Duration::from_secs(121), 2, false);
        assert_eq!(snapshots, 1, "one write: the first due tick outside a gesture");
        autosave.finish(false);
    }

    /// **An embedded song's samples are not copied into the autosave**, and a
    /// recovery gives them back to the song. A Referenced save copies a
    /// sample the song owns, so an autosave of an embedded song would have
    /// copied every sample once a minute.
    #[test]
    fn an_embedded_sample_is_referenced_in_place_and_owned_again_on_recovery() {
        let root = tempfile::tempdir().unwrap();
        let sidecar = root.path().join("Groove.mooloop-assets").join("samples");
        fs::create_dir_all(&sidecar).unwrap();
        let kick = sidecar.join("00-kick.wav");
        write_wav(&kick);

        let mut project = edited_song();
        project.channels[0].setup = mooloop_core::ChannelSetup::sampler("Kick");
        project.channels[0]
            .setup
            .source
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::File {
            path: kick.clone(),
            embedded: true,
        };

        let autosaves = root.path().join("autosave");
        let mut autosave = Autosave::start_with_interval(&autosaves, Duration::ZERO).unwrap();
        let state = DocumentState {
            embed: true,
            ..state(1, true, None)
        };
        autosave.update(Instant::now(), state, move || project);
        autosave.finish(true);

        let wavs = walk(&autosaves)
            .into_iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "wav"))
            .count();
        assert_eq!(wavs, 0, "no sample was copied into the autosave");

        let found = find_recoverable(&autosaves);
        assert!(found[0].embed);
        let Ok(document) = recover(&found[0]) else {
            panic!("the autosave recovers");
        };
        assert!(document.samples[0].is_some(), "the sample decoded from where it was");
        let LoadedDocument::Song(recovered) = document.report.document else {
            panic!("a song");
        };
        match &recovered.channels[0].setup.source.sampler_state().unwrap().sample {
            SampleReference::File { path, embedded } => {
                assert!(*embedded, "the song owns its sample again");
                assert_eq!(path.canonicalize().unwrap(), kick.canonicalize().unwrap());
            }
            other => panic!("{other:?}"),
        }
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                found.extend(walk(&path));
            } else {
                found.push(path);
            }
        }
        found
    }

    /// **An untitled song's recorded take survives a crash** (MOO-103, the
    /// orchestrator's first condition). The take is in the shared recordings
    /// folder, owned by a song that was never saved. After the crash, Clean
    /// Up Takes must not offer it, and the recovery must play it and own it.
    #[test]
    fn an_untitled_song_with_a_take_is_recovered_after_a_crash() {
        let root = tempfile::tempdir().unwrap();
        let recordings = root.path().join("recordings");
        fs::create_dir_all(&recordings).unwrap();
        let take = recordings.join("take-20260923-101500.wav");
        write_wav(&take);

        let mut project = edited_song();
        project.channels[0].setup = mooloop_core::ChannelSetup::sampler("Take");
        project.channels[0]
            .setup
            .source
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::File {
            path: take.clone(),
            embedded: true,
        };
        let autosaves = root.path().join("autosave");
        let mut autosave = Autosave::start_with_interval(&autosaves, Duration::ZERO).unwrap();
        autosave.update(Instant::now(), state(1, true, None), move || project);
        autosave.finish(true);

        // The relaunch, before anyone answers the recovery question: the
        // take is from an earlier session and nothing open uses it.
        let unprotected = crate::recordings::clean_up(
            &recordings,
            None,
            &Default::default(),
            SystemTime::now(),
        );
        assert_eq!(unprotected.earlier.len(), 1, "without the autosave it would be offered");
        let protected = referenced_samples(&autosaves);
        let lists = crate::recordings::clean_up(&recordings, None, &protected, SystemTime::now());
        assert!(
            lists.not_used.is_empty() && lists.earlier.is_empty(),
            "the autosave's take was offered for the trash: {:?}",
            lists.earlier
        );

        let found = find_recoverable(&autosaves);
        assert_eq!(found[0].question(), "Recover an unsaved song?");
        let Ok(document) = recover(&found[0]) else {
            panic!("the autosave recovers");
        };
        assert!(document.samples[0].is_some(), "the take plays");
        let LoadedDocument::Song(recovered) = document.report.document else {
            panic!("a song");
        };
        assert!(matches!(
            &recovered.channels[0].setup.source.sampler_state().unwrap().sample,
            SampleReference::File { embedded: true, .. }
        ));
    }

    fn write_wav(path: &Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut wav = hound::WavWriter::create(path, spec).unwrap();
        for n in 0..480 {
            wav.write_sample((n * 50) as i16).unwrap();
        }
        wav.finalize().unwrap();
    }

    #[test]
    fn the_detail_says_how_long_ago_without_a_clock() {
        let recoverable = Recoverable {
            dir: PathBuf::new(),
            song: PathBuf::new(),
            original: None,
            saved_at: UNIX_EPOCH + Duration::from_secs(1_000_000),
            embed: false,
            owned: Vec::new(),
        };
        let at = |seconds: u64| UNIX_EPOCH + Duration::from_secs(1_000_000 + seconds);
        assert!(recoverable.detail(at(20)).contains("a minute ago"));
        assert!(recoverable.detail(at(14 * 60)).contains("14 minutes ago"));
        assert!(recoverable.detail(at(3 * 3600)).contains("3 hours ago"));
        assert_eq!(recoverable.question(), "Recover an unsaved song?");
    }
}
