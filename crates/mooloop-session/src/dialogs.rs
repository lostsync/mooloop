//! File and confirmation dialogs.
//!
//! These shell out to `zenity` rather than using a toolkit dialog, which is
//! why the session layer can own them outright.

use crate::audio_file;
use std::path::{Path, PathBuf};

fn zenity_path(mut command: std::process::Command) -> Option<PathBuf> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

pub fn pick_bundle_via_zenity(title: &str) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg("--directory")
        .arg(format!("--title={title}"));
    zenity_path(command)
}

pub fn pick_song_via_zenity(title: &str) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg(format!("--title={title}"))
        .arg("--file-filter=Mooloop songs | *.mooloop manifest.toml");
    zenity_path(command).map(normalize_song_selection)
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

pub fn pick_save_via_zenity(title: &str, suggested: &str) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg("--save")
        .arg("--confirm-overwrite")
        .arg(format!("--title={title}"))
        .arg(format!("--filename={suggested}"));
    zenity_path(command)
}

pub fn pick_export_via_zenity(extension: &str) -> Option<PathBuf> {
    let mut command = std::process::Command::new("zenity");
    command
        .arg("--file-selection")
        .arg("--save")
        .arg("--confirm-overwrite")
        .arg("--title=Export audio")
        .arg(format!("--filename=mooloop-export.{extension}"))
        .arg(format!("--file-filter=*.{extension}"));
    let mut path = zenity_path(command)?;
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case(extension))
    {
        path.set_extension(extension);
    }
    Some(path)
}

pub fn confirm_via_zenity(question: &str) -> bool {
    std::process::Command::new("zenity")
        .arg("--question")
        .arg(format!("--text={question}"))
        .arg("--ok-label=Continue")
        .arg("--cancel-label=Cancel")
        .status()
        .is_ok_and(|status| status.success())
}

/// Spawn zenity to pick a supported audio file. Returns `None` if cancelled
/// or unavailable.
pub fn pick_sample_via_zenity() -> Option<PathBuf> {
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
    let out = std::process::Command::new("zenity")
        .arg("--file-selection")
        .arg(format!("--file-filter=Audio samples | {patterns}"))
        .arg("--title=Load sample")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
