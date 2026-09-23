//! A real panic, through the real hook, leaves a crash report with a
//! backtrace in it (P5 in `reports/teams-2026-09-22.md`).
//!
//! Its own test binary because the hook is process-wide: installed in the
//! library's unit tests it would write up every other test's panic too.

use std::path::PathBuf;

#[test]
fn a_forced_panic_leaves_a_crash_report_with_a_backtrace() {
    let dir: PathBuf = std::env::temp_dir().join(format!("mooloop-crash-hook-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    mooloop_core::log::install_panic_hook(dir.clone(), "0.0.0 (test build)".to_owned());

    let joined = std::thread::Builder::new()
        .name("forced-panic".to_owned())
        .spawn(|| panic!("forced for the crash report test"))
        .unwrap()
        .join();
    assert!(joined.is_err(), "the thread was meant to panic");

    let reports: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the hook made the crash directory")
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = std::fs::read_to_string(&reports[0]).unwrap();
    for expected in [
        "0.0.0 (test build)",
        "thread: forced-panic",
        "forced for the crash report test",
        "backtrace:",
    ] {
        assert!(report.contains(expected), "{expected:?} missing from {report}");
    }
    // A captured backtrace, not the placeholder an uncaptured one prints:
    // frames, one of them in this test.
    let frames = report
        .lines()
        .skip_while(|line| *line != "backtrace:")
        .filter(|line| line.trim_start().split(':').next().is_some_and(|n| n.parse::<u32>().is_ok()))
        .count();
    assert!(frames > 3, "{report}");
    assert!(report.contains("crash_report"), "{report}");
    let _ = std::fs::remove_dir_all(&dir);
}
