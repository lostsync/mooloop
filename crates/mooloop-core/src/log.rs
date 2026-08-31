//! Diagnostic logging: a running account of what the application did, so a
//! problem that only shows up once can still be looked at afterwards.
//!
//! Records always go to stderr, which is where a run started from a terminal
//! shows them. They additionally go to a file once [`start_file`] is called;
//! the preferences toggle that turns that on is the only reason this module
//! keeps a sink at all, because a user who hits a problem is rarely the same
//! person who thought to run the app from a terminal first.
//!
//! # Not from the audio thread
//!
//! Every entry point here formats a `String` and takes a lock, so none of them
//! may be called from the realtime callback. The audio thread reports through
//! the existing bounded event queue instead, and the UI logs what it reads off
//! that queue. There is no cheap-enough variant to add later: a log line that
//! is safe for the audio thread is a different mechanism, not a faster one.
//!
//! # Use
//!
//! ```
//! use mooloop_core::log_info;
//! log_info!("project", "song saved: {}", "/tmp/x.mooloop");
//! ```
//!
//! The first argument is the subsystem the line came from, so a log can be
//! filtered down to one area by eye. Keep them short and reuse the existing
//! ones: `app`, `audio`, `project`, `ui`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// How much detail to keep. Ordered least to most verbose, so a filter is a
/// single comparison.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Level {
    /// The operation the user asked for did not happen.
    Error = 0,
    /// It happened, but not the way it was asked for.
    Warn = 1,
    /// A thing worth being able to point at afterwards: a save, a load, a
    /// device added, the engine starting.
    Info = 2,
    /// Detail that is only interesting when something is already wrong.
    Debug = 3,
}

impl Level {
    /// The five-column tag used in a log line, padded so the messages line up.
    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
        }
    }

    /// Parses a level name, case-insensitively. Returns `None` rather than a
    /// default so a misspelled `MOOLOOP_LOG` can be reported instead of
    /// silently reading as something else.
    pub fn parse(name: &str) -> Option<Level> {
        match name.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" | "trace" => Some(Level::Debug),
            _ => None,
        }
    }
}

/// The console threshold. Read on every macro call before anything is
/// formatted, so a level that is switched off costs one relaxed load.
static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

/// The file threshold, held separately: the file is the artifact someone sends
/// back, so it stays at `Debug` while the console keeps whatever the terminal
/// is readable at.
static FILE_LEVEL: AtomicU8 = AtomicU8::new(Level::Debug as u8);

/// Whether [`sink`] holds an open file. Duplicated out of the mutex so that
/// [`enabled`] and the write path can both answer "is anyone listening?"
/// without taking a lock, which is the only thing that makes the level check
/// cheap enough to leave in place.
static FILE_OPEN: AtomicBool = AtomicBool::new(false);

struct Sink {
    file: File,
    path: PathBuf,
}

fn sink() -> &'static Mutex<Option<Sink>> {
    static SINK: OnceLock<Mutex<Option<Sink>>> = OnceLock::new();
    SINK.get_or_init(|| Mutex::new(None))
}

/// Sets the console threshold, and reports what it was.
pub fn set_level(level: Level) -> Level {
    let previous = LEVEL.swap(level as u8, Ordering::Relaxed);
    from_u8(previous)
}

/// The current console threshold.
pub fn level() -> Level {
    from_u8(LEVEL.load(Ordering::Relaxed))
}

fn from_u8(value: u8) -> Level {
    match value {
        0 => Level::Error,
        1 => Level::Warn,
        2 => Level::Info,
        _ => Level::Debug,
    }
}

/// Whether a record at `level` would reach any sink. The macros check this
/// before formatting, so an argument list with real work in it is not
/// evaluated for a line nobody will read.
///
/// The file is checked second and only when it is open, which is what keeps
/// `debug` genuinely free in the ordinary case: with no log file the file
/// threshold must not be allowed to answer for a sink that is not there.
pub fn enabled(level: Level) -> bool {
    level as u8 <= LEVEL.load(Ordering::Relaxed)
        || (FILE_OPEN.load(Ordering::Relaxed) && level as u8 <= FILE_LEVEL.load(Ordering::Relaxed))
}

/// Writes one record. Prefer the macros, which skip the formatting when the
/// level is off.
pub fn record(level: Level, target: &str, message: &str) {
    let line = format!(
        "{} {} {:<8} {}",
        timestamp(SystemTime::now()),
        level.tag(),
        target,
        message
    );
    if level as u8 <= LEVEL.load(Ordering::Relaxed) {
        // Not `eprintln!`: a closed or full stderr must not take down a run
        // that is otherwise fine, and the macro panics on write failure.
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{line}");
    }
    if FILE_OPEN.load(Ordering::Relaxed) && level as u8 <= FILE_LEVEL.load(Ordering::Relaxed) {
        if let Ok(mut guard) = sink().lock() {
            if let Some(sink) = guard.as_mut() {
                // Flushed per record rather than buffered: the runs worth
                // reading back are the ones that ended in a crash, and a
                // buffered tail is exactly the part that explains it.
                let _ = writeln!(sink.file, "{line}");
                let _ = sink.file.flush();
            }
        }
    }
}

/// Largest log kept before the previous run's file is rolled aside. One
/// generation of history is enough to cover "it did it again just now" while
/// staying small enough to attach to a bug report.
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// Starts writing records to `path`, creating its directory. Appends, so a
/// problem that only appears every few runs still has its history; once the
/// file passes [`MAX_LOG_BYTES`] the existing one is moved to `<path>.1` and a
/// fresh one starts.
///
/// Replaces any previously open file. `header` is written first, and should
/// say what build produced the run.
pub fn start_file(path: &Path, header: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) {
        // Best effort: a rename that fails leaves a large file that keeps
        // growing, which is still better than refusing to log at all.
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "\n=== {} {header}", timestamp(SystemTime::now()))?;
    file.flush()?;
    let mut guard = sink().lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some(Sink {
        file,
        path: path.to_path_buf(),
    });
    // Published last, while the lock is still held: a writer that sees this
    // set is guaranteed to find the sink behind it.
    FILE_OPEN.store(true, Ordering::Relaxed);
    Ok(())
}

/// Stops writing to the file. The file is left in place; this only closes it.
pub fn stop_file() {
    let mut guard = sink().lock().unwrap_or_else(|e| e.into_inner());
    FILE_OPEN.store(false, Ordering::Relaxed);
    *guard = None;
}

/// Where records are currently being written, if anywhere.
pub fn file_path() -> Option<PathBuf> {
    let guard = sink().lock().ok()?;
    guard.as_ref().map(|sink| sink.path.clone())
}

/// The same instant as [`timestamp`], compacted for use in a file name:
/// `20260831-142203`. Sorts chronologically as text, which is what makes a
/// directory of these readable without opening any of them.
pub fn file_stamp() -> String {
    let stamp = timestamp(SystemTime::now());
    stamp
        .trim_end_matches('Z')
        .replace(['-', ':'], "")
        .replace(' ', "-")
}

/// UTC, as `2026-08-31 14:22:03Z`. Deliberately not local time: mooloop has no
/// timezone database and guessing from `TZ` would produce a stamp that is
/// wrong in exactly the situations logs get compared across machines.
fn timestamp(now: SystemTime) -> String {
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let time = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}Z",
        time / 3600,
        (time % 3600) / 60,
        time % 60
    )
}

/// The calendar date `days` after the Unix epoch (Howard Hinnant's
/// `civil_from_days`). Counts from March so February's variable length falls
/// at the end of the year and needs no special case; the year is shifted back
/// at the end.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_position + 2) / 5 + 1) as u32;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    } as u32;
    let year = year_of_era + era * 400;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Records a failure the user asked about. See the [module docs](self).
#[macro_export]
macro_rules! log_error {
    ($target:expr, $($arg:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Error) {
            $crate::log::record(
                $crate::log::Level::Error, $target, &::std::format!($($arg)*));
        }
    };
}

/// Records something that worked, but not as asked. See the [module docs](self).
#[macro_export]
macro_rules! log_warn {
    ($target:expr, $($arg:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Warn) {
            $crate::log::record(
                $crate::log::Level::Warn, $target, &::std::format!($($arg)*));
        }
    };
}

/// Records a milestone worth pointing at later. See the [module docs](self).
#[macro_export]
macro_rules! log_info {
    ($target:expr, $($arg:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Info) {
            $crate::log::record(
                $crate::log::Level::Info, $target, &::std::format!($($arg)*));
        }
    };
}

/// Records detail that only matters once something is wrong. See the
/// [module docs](self).
#[macro_export]
macro_rules! log_debug {
    ($target:expr, $($arg:tt)*) => {
        if $crate::log::enabled($crate::log::Level::Debug) {
            $crate::log::record(
                $crate::log::Level::Debug, $target, &::std::format!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(seconds: u64) -> String {
        timestamp(UNIX_EPOCH + Duration::from_secs(seconds))
    }

    #[test]
    fn the_stamp_reads_as_a_date_and_a_time() {
        assert_eq!(at(0), "1970-01-01 00:00:00Z");
        assert_eq!(at(1_756_648_923), "2025-08-31 14:02:03Z");
    }

    #[test]
    fn leap_days_land_on_the_right_date() {
        // 2024 is a leap year, 2100 is not, 2000 is: the three cases the
        // 400-year cycle has to get right.
        assert!(at(1_709_164_800).starts_with("2024-02-29"));
        assert!(at(951_782_400).starts_with("2000-02-29"));
        assert!(at(4_107_542_400).starts_with("2100-03-01"));
    }

    #[test]
    fn a_level_name_round_trips_and_a_typo_is_refused() {
        for (name, level) in [
            ("error", Level::Error),
            ("WARN", Level::Warn),
            (" Info ", Level::Info),
            ("debug", Level::Debug),
        ] {
            assert_eq!(Level::parse(name), Some(level), "{name}");
        }
        assert_eq!(Level::parse("verbose"), None);
    }

    #[test]
    fn a_file_sink_collects_the_records_the_console_threshold_would_drop() {
        let dir = std::env::temp_dir().join(format!("mooloop-log-{}", std::process::id()));
        let path = dir.join("mooloop.log");
        let _ = std::fs::remove_file(&path);
        start_file(&path, "test build").expect("open the log");
        // Console at Error, file still at Debug: the point of the toggle is
        // that the file keeps what the terminal is too quiet to show.
        let previous = set_level(Level::Error);
        record(Level::Debug, "test", "a quiet detail");
        set_level(previous);
        assert_eq!(file_path().as_deref(), Some(path.as_path()));
        stop_file();

        let written = std::fs::read_to_string(&path).expect("read the log back");
        assert!(written.contains("test build"), "{written}");
        assert!(
            written.contains("DEBUG test     a quiet detail"),
            "{written}"
        );
        assert!(file_path().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
