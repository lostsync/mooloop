//! Filesystem walking for the sample browser.
//!
//! This produces paths and names. Turning them into rows is the view's job.

use crate::audio_file;
use std::path::{Path, PathBuf};

/// Extensions the browser treats as playable. The decoder decides what we
/// can actually open; this predicate decides what the tree shows.
pub fn is_playable_sample(path: &Path) -> bool {
    audio_file::is_supported_extension(path)
}

/// Whether `path` contains a playable sample anywhere below it. Folders
/// without one are dead weight in the tree, so the browser hides them;
/// the recursion is bounded because symlink cycles terminate at `MAX_DEPTH`.
pub fn has_playable_descendant(path: &Path, depth: usize) -> bool {
    const MAX_DEPTH: usize = 16;
    if depth > MAX_DEPTH {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if is_playable_sample(&child) {
            return true;
        }
        if child.is_dir() && has_playable_descendant(&child, depth + 1) {
            return true;
        }
    }
    false
}

/// Entries of `path` the sample browser shows: subdirectories first, then
/// playable sample files, each case-insensitively sorted by name. Hidden
/// entries are skipped, and an unreadable directory simply lists as empty.
pub fn scan_browser_dir(path: &Path) -> Vec<(bool, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let child = entry.path();
        let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if child.is_dir() {
            dirs.push(child);
        } else if is_playable_sample(&child) {
            files.push(child);
        }
    }
    let by_lower_name = |child: &PathBuf| {
        child
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_lowercase()
    };
    dirs.sort_by_key(&by_lower_name);
    files.sort_by_key(by_lower_name);
    dirs.into_iter()
        .map(|child| (true, child))
        .chain(files.into_iter().map(|child| (false, child)))
        .collect()
}

pub fn browser_display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
