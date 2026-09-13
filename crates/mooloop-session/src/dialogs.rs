//! File and confirmation dialogs.
//!
//! These shell out to a dialog program rather than using a toolkit dialog,
//! which is why the session layer can own them outright: `zenity` on Linux,
//! and on macOS `osascript`, whose standard additions draw the system's own
//! open, save and alert panels. Each call blocks its thread until the user
//! answers, so callers run them off the UI thread.
//!
//! A cancel comes back as `None` or `false`. So does a dialog program that
//! would not start, which is why that case is logged: without the log, "Save
//! did nothing" has no explanation anywhere. It is how File > Save first
//! behaved on a Mac, where there is no zenity to run.

use crate::audio_file;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run `command` with `run`, logging a program that would not start, which
/// the caller cannot tell apart from a cancel.
fn spawn<T>(
    command: &mut Command,
    run: impl FnOnce(&mut Command) -> std::io::Result<T>,
) -> Option<T> {
    match run(command) {
        Ok(result) => Some(result),
        Err(error) => {
            mooloop_core::log_warn!(
                "dialog",
                "could not open a dialog: {} would not start ({error}){}",
                command.get_program().to_string_lossy(),
                backend::MISSING_HINT
            );
            None
        }
    }
}

/// The path a dialog program printed, or `None` for a cancel, an empty
/// answer, or a program that would not start.
fn picked_path(mut command: Command) -> Option<PathBuf> {
    let output = spawn(&mut command, Command::output)?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

pub fn pick_bundle_dialog(title: &str) -> Option<PathBuf> {
    picked_path(backend::choose_directory(title))
}

pub fn pick_song_dialog(title: &str) -> Option<PathBuf> {
    picked_path(backend::choose_file(
        title,
        "Mooloop songs | *.mooloop manifest.toml",
    ))
    .map(normalize_song_selection)
}

fn normalize_song_selection(path: PathBuf) -> PathBuf {
    let is_legacy_manifest = path
        .file_name()
        .is_some_and(|name| name == mooloop_project::MANIFEST_FILE)
        && path
            .parent()
            .and_then(Path::extension)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mooloop"));
    if is_legacy_manifest {
        path.parent()
            .expect("manifest selection has a parent")
            .into()
    } else {
        path
    }
}

pub fn pick_save_dialog(title: &str, suggested: &str) -> Option<PathBuf> {
    picked_path(backend::choose_save_path(title, suggested, None))
}

pub fn pick_export_dialog(extension: &str) -> Option<PathBuf> {
    let mut path = picked_path(backend::choose_save_path(
        "Export audio",
        &format!("mooloop-export.{extension}"),
        Some(&format!("*.{extension}")),
    ))?;
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case(extension))
    {
        path.set_extension(extension);
    }
    Some(path)
}

pub fn confirm_dialog(question: &str) -> bool {
    spawn(&mut backend::confirm(question), Command::status).is_some_and(|status| status.success())
}

/// Pick a supported audio file. Returns `None` if cancelled or unavailable.
pub fn pick_sample_dialog() -> Option<PathBuf> {
    let patterns = audio_file::SUPPORTED_EXTENSIONS
        .iter()
        .flat_map(|extension| {
            [
                format!("*.{extension}"),
                format!("*.{}", extension.to_uppercase()),
            ]
        })
        .collect::<Vec<_>>()
        .join(" ");
    picked_path(backend::choose_file(
        "Load sample",
        &format!("Audio samples | {patterns}"),
    ))
}

/// zenity. Filters use its `--file-filter` syntax, `Name | *.a *.b`.
#[cfg(not(target_os = "macos"))]
mod backend {
    use std::process::Command;

    pub(super) const MISSING_HINT: &str = "; install zenity for file dialogs";

    fn file_selection(title: &str) -> Command {
        let mut command = Command::new("zenity");
        command
            .arg("--file-selection")
            .arg(format!("--title={title}"));
        command
    }

    pub(super) fn choose_directory(title: &str) -> Command {
        let mut command = file_selection(title);
        command.arg("--directory");
        command
    }

    pub(super) fn choose_file(title: &str, filter: &str) -> Command {
        let mut command = file_selection(title);
        command.arg(format!("--file-filter={filter}"));
        command
    }

    pub(super) fn choose_save_path(title: &str, suggested: &str, filter: Option<&str>) -> Command {
        let mut command = file_selection(title);
        command
            .arg("--save")
            .arg("--confirm-overwrite")
            .arg(format!("--filename={suggested}"));
        if let Some(filter) = filter {
            command.arg(format!("--file-filter={filter}"));
        }
        command
    }

    pub(super) fn confirm(question: &str) -> Command {
        let mut command = Command::new("zenity");
        command
            .arg("--question")
            .arg(format!("--text={question}"))
            .arg("--ok-label=Continue")
            .arg("--cancel-label=Cancel");
        command
    }
}

/// `osascript`, which every Mac has. The panels show every file: a filter is
/// zenity syntax and is ignored here, and every caller already refuses what it
/// cannot use -- a song that will not load, a sample that will not decode, an
/// export whose extension it corrects.
#[cfg(target_os = "macos")]
mod backend {
    use std::process::Command;

    pub(super) const MISSING_HINT: &str = "";

    /// One `on run` handler around `body`. `activate` brings osascript, and so
    /// its panel, in front of mooloop's window. Titles and questions arrive
    /// through `argv` rather than being spliced into the script, so nothing in
    /// them can be read as AppleScript.
    fn osascript(body: &str, args: &[&str]) -> Command {
        let mut command = Command::new("osascript");
        command
            .args(["-e", "on run argv", "-e", "activate", "-e", body, "-e", "end run"])
            .args(args);
        command
    }

    pub(super) fn choose_directory(title: &str) -> Command {
        osascript(
            "POSIX path of (choose folder with prompt (item 1 of argv))",
            &[title],
        )
    }

    pub(super) fn choose_file(title: &str, _filter: &str) -> Command {
        osascript(
            "POSIX path of (choose file with prompt (item 1 of argv))",
            &[title],
        )
    }

    /// The save panel asks before replacing a file itself, as zenity's
    /// `--confirm-overwrite` does.
    pub(super) fn choose_save_path(title: &str, suggested: &str, _filter: Option<&str>) -> Command {
        osascript(
            "POSIX path of (choose file name with prompt (item 1 of argv) default name (item 2 of argv))",
            &[title, suggested],
        )
    }

    /// Cancel is AppleScript error -128, which osascript turns into a failing
    /// exit status, as zenity does for its Cancel button.
    pub(super) fn confirm(question: &str) -> Command {
        osascript(
            r#"display dialog (item 1 of argv) buttons {"Cancel", "Continue"} default button "Continue" cancel button "Cancel" with icon caution"#,
            &[question],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn song_selection_accepts_new_files_and_legacy_manifests() {
        let file = PathBuf::from("/songs/beat.mooloop");
        assert_eq!(normalize_song_selection(file.clone()), file);

        let legacy_manifest = PathBuf::from("/songs/old.mooloop/manifest.toml");
        assert_eq!(
            normalize_song_selection(legacy_manifest),
            PathBuf::from("/songs/old.mooloop")
        );
    }

    /// The zenity command lines are the ones Linux has always run; splitting
    /// out a macOS backend was not a reason for them to change.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn zenity_is_asked_what_it_always_was() {
        assert_eq!(
            args(&backend::choose_save_path("Save mooloop song", "Untitled.mooloop", None)),
            [
                "--file-selection",
                "--title=Save mooloop song",
                "--save",
                "--confirm-overwrite",
                "--filename=Untitled.mooloop",
            ]
        );
        assert_eq!(
            args(&backend::confirm("Discard?")),
            ["--question", "--text=Discard?", "--ok-label=Continue", "--cancel-label=Cancel"]
        );
    }

    /// A title is data. Quotes in it must not reach the script's source, where
    /// they would end a string literal and turn the rest into code.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_title_reaches_applescript_as_an_argument() {
        let title = r#"Save "quoted" song" & (do shell script "true")"#;
        let command = backend::choose_save_path(title, "Untitled.mooloop", None);
        let args = args(&command);
        assert_eq!(&args[args.len() - 2..], [title, "Untitled.mooloop"]);
        assert!(args[..args.len() - 2].iter().all(|arg| !arg.contains("quoted")));
    }
}
