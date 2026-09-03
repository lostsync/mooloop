//! Filesystem walking for the sample browser.
//!
//! This produces paths and names. Turning them into rows is the view's job.

use crate::session::Session;
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

/// Which channel a background sample load is destined for, and the source
/// revision it was started against.
///
/// The revision travels with the load so an arrival that raced a device
/// change can be discarded rather than dropped onto the wrong instrument.
pub struct LoadTarget {
    pub channel: usize,
    pub source_revision: u64,
    pub path: PathBuf,
}

impl Session {
    /// Expands or collapses a browser folder.
    ///
    /// Insert-or-remove: a path never expanded collapses to a no-op remove,
    /// so the set only ever holds folders that are open.
    pub fn toggle_browser_folder(&mut self, path: PathBuf) {
        if !self.browser_expanded.remove(&path) {
            self.browser_expanded.insert(path);
        }
    }

    /// Drops a browser location and forgets its expansion.
    ///
    /// Only top-level rows offer removal, so a path the tree hands back that
    /// is not a location is a stale no-op rather than an error.
    pub fn remove_browser_location(&mut self, path: &Path) {
        self.browser_locations.retain(|p| p != path);
        self.browser_expanded.remove(path);
    }

    /// The selected channel's current sample, as a load target.
    ///
    /// `None` when the channel has no sample to step away from.
    pub fn selected_sample_target(&self) -> Option<LoadTarget> {
        Some(LoadTarget {
            channel: self.selected,
            source_revision: self.source_revision,
            path: self.channels.get(self.selected)?.sample_path.clone()?,
        })
    }
}
