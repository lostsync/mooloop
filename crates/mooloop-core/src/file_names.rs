//! Choosing a file name nothing has yet, and putting a file there without
//! replacing one (MOO-241, MOO-188).
//!
//! Three writers need the same promise, that a file already on disk is never
//! written over by one that happened to want its name: a recorded take
//! (`mooloop-session`'s `take.rs`), an asset copied into a song's folder
//! (`mooloop-project`), and an export (`RenderSettings::job` and the
//! engine's finishing step). Checking first and writing second leaves a gap
//! another writer can land in, so the last step is always one that refuses:
//! a file opened with `create_new`, or [`rename_no_replace`].
//!
//! Two spellings, because the two kinds of name are read differently. A take
//! or an asset is a name nobody typed, and a clash is rare, so it counts on
//! from the name itself: `kick.wav`, `kick-2.wav`, `kick-3.wav`
//! ([`candidates`]). An export is a name somebody typed, often exported
//! again and again, so it is numbered in a column that sorts:
//! `song-001.mp3`, `song-002.mp3` ([`numbered`]).

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// `wanted`, then `wanted` with `-2`, `-3`, ... before its extension, without
/// end.
pub fn candidates(wanted: &str) -> impl Iterator<Item = String> + '_ {
    let (stem, extension) = match wanted.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (wanted, None),
    };
    std::iter::once(wanted.to_string()).chain((2u64..).map(move |n| match extension {
        Some(extension) => format!("{stem}-{n}.{extension}"),
        None => format!("{stem}-{n}"),
    }))
}

/// A new, empty file in `directory`, named `wanted` or the first of its
/// [`candidates`] that nothing has, and its path. It never opens a file that
/// is already there, however close together two callers ask.
pub fn create_unclaimed(directory: &Path, wanted: &str) -> io::Result<(PathBuf, File)> {
    for name in candidates(wanted) {
        let path = directory.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!("the candidates never run out")
}

/// `path` with `-NNN` before its extension: three digits, zero-padded, and
/// more after 999. Number 0 is `path` itself. A name that already ends in a
/// number is not read: `song-001` numbered 1 is `song-001-001`.
pub fn numbered(path: &Path, number: u32) -> PathBuf {
    if number == 0 {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = match path.extension() {
        Some(extension) => format!("{stem}-{number:03}.{}", extension.to_string_lossy()),
        None => format!("{stem}-{number:03}"),
    };
    path.with_file_name(name)
}

/// The lowest number at which none of `paths`, [`numbered`], is `taken`.
/// Files written together share it, so a job's files keep one number even
/// when only one of them clashed.
pub fn free_number(paths: &[PathBuf], taken: impl Fn(&Path) -> bool) -> u32 {
    (0..)
        .find(|&number| paths.iter().all(|path| !taken(&numbered(path, number))))
        .expect("an unbounded search finds a free number")
}

/// Whether anything is at `path`, a dangling link included: a link is a
/// name, and writing to it would follow it.
pub fn is_taken(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Move `from` to `to`, and fail with [`io::ErrorKind::AlreadyExists`] rather
/// than replace anything at `to`.
///
/// A hard link then an unlink, which the OS refuses atomically when `to` is
/// taken, on Linux, macOS and Windows alike. A file system with no hard links
/// (FAT and exFAT, some network shares; a USB stick is a likely place for an
/// export) claims `to` with `create_new` and copies into it instead, which
/// refuses just as atomically. It never falls back to a plain rename, which
/// would replace.
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    match fs::hard_link(from, to) {
        Ok(()) => {
            // The file is in place under both names. Failing to drop the old
            // one leaves a stray name behind, not a lost file.
            let _ = fs::remove_file(from);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(error),
        Err(_) => copy_no_replace(from, to),
    }
}

/// [`rename_no_replace`] where there are no hard links: claim `to`, copy,
/// then drop `from`. A copy that fails removes what it claimed.
fn copy_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    let mut target = OpenOptions::new().write(true).create_new(true).open(to)?;
    let copied = File::open(from)
        .and_then(|mut source| io::copy(&mut source, &mut target))
        .and_then(|_| target.sync_all());
    if let Err(error) = copied {
        drop(target);
        let _ = fs::remove_file(to);
        return Err(error);
    }
    let _ = fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh folder of this test's own under the system's temporary one.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "mooloop-file-names-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn candidates_count_on_before_the_extension() {
        let first: Vec<String> = candidates("kick.wav").take(3).collect();
        assert_eq!(first, ["kick.wav", "kick-2.wav", "kick-3.wav"]);
        let bare: Vec<String> = candidates("notes").take(2).collect();
        assert_eq!(bare, ["notes", "notes-2"]);
        let hidden: Vec<String> = candidates(".wav").take(2).collect();
        assert_eq!(hidden, [".wav", ".wav-2"]);
    }

    #[test]
    fn a_new_file_never_opens_one_already_there() {
        let dir = Scratch::new();
        fs::write(dir.0.join("take.wav"), b"first").unwrap();
        let (path, _file) = create_unclaimed(&dir.0, "take.wav").unwrap();
        assert_eq!(path, dir.0.join("take-2.wav"));
        let (path, _file) = create_unclaimed(&dir.0, "take.wav").unwrap();
        assert_eq!(path, dir.0.join("take-3.wav"));
        assert_eq!(fs::read(dir.0.join("take.wav")).unwrap(), b"first");
    }

    /// MOO-188's rules for the number.
    #[test]
    fn a_number_is_three_digits_before_the_extension() {
        let song = Path::new("/music/song.mp3");
        assert_eq!(numbered(song, 0), song);
        assert_eq!(numbered(song, 1), Path::new("/music/song-001.mp3"));
        assert_eq!(numbered(song, 1000), Path::new("/music/song-1000.mp3"));
        assert_eq!(
            numbered(Path::new("/music/song-001.wav"), 1),
            Path::new("/music/song-001-001.wav"),
            "a typed number is not read"
        );
        assert_eq!(numbered(Path::new("/music/song"), 2), Path::new("/music/song-002"));
    }

    #[test]
    fn a_job_takes_the_lowest_number_free_for_all_of_its_files() {
        let taken = |names: &'static [&'static str]| {
            move |path: &Path| names.iter().any(|name| path == Path::new(name))
        };
        let one = [PathBuf::from("song.wav")];
        assert_eq!(free_number(&one, taken(&[])), 0, "nothing there: as typed");
        assert_eq!(free_number(&one, taken(&["song.wav"])), 1);
        assert_eq!(
            free_number(&one, taken(&["song.wav", "song-001.wav", "song-002.wav"])),
            3
        );
        let stems = [
            PathBuf::from("drums.wav"),
            PathBuf::from("bass.wav"),
            PathBuf::from("keys.wav"),
        ];
        assert_eq!(free_number(&stems, taken(&["bass.wav"])), 1, "one clash numbers all three");
        assert_eq!(free_number(&stems, taken(&["bass.wav", "keys-001.wav"])), 2);
    }

    #[test]
    fn a_rename_that_must_not_replace_refuses() {
        let dir = Scratch::new();
        let (from, to) = (dir.0.join(".part"), dir.0.join("song.wav"));
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"old").unwrap();
        let refused = rename_no_replace(&from, &to).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"old");
        assert_eq!(fs::read(&from).unwrap(), b"new", "the new file is still there to place");

        fs::remove_file(&to).unwrap();
        rename_no_replace(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"new");
        assert!(!is_taken(&from), "moved, not copied");
    }

    /// The route a file system without hard links takes refuses as well.
    #[test]
    fn the_copy_fallback_refuses_too() {
        let dir = Scratch::new();
        let (from, to) = (dir.0.join(".part"), dir.0.join("song.wav"));
        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"old").unwrap();
        let refused = copy_no_replace(&from, &to).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"old");

        fs::remove_file(&to).unwrap();
        copy_no_replace(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"new");
        assert!(!is_taken(&from));
    }
}
