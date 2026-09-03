//! Whole-project values: the undo unit, the structural edit that carries it,
//! and the pattern-bank invariant both depend on.

use crate::history::Entry as HistoryEntry;
use mooloop_core::Project;
use mooloop_dsp::SampleData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A complete, UI-owned project snapshot. Samples stay beside the serializable
/// project because restoring an edit must never decode audio on the UI thread.
#[derive(Clone)]
pub struct ProjectSnapshot {
    pub project: Project,
    pub samples: Vec<Option<Arc<SampleData>>>,
}

/// Keep every channel's pattern-indexed banks parallel to the project's
/// pattern list. A clipboard can outlive pattern edits, and old projects may
/// legitimately arrive without the automation banks introduced later.
pub fn normalize_project_pattern_banks(project: &mut Project) {
    let pattern_count = project.pattern_lengths.len();
    for channel in &mut project.channels {
        channel.notes.resize_with(pattern_count, Vec::new);
        channel.automation.resize_with(pattern_count, Vec::new);
    }
}

pub fn fresh_starter_seed() -> u64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    clock
        ^ SEQUENCE
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

#[derive(Clone, Copy)]
pub enum HistoryMove {
    Record,
    Undo,
    Redo,
}

/// Structural channel edits are prepared by UI callbacks and installed by the
/// pump, which exclusively owns the engine handle. A complete project swap
/// keeps insertion/removal/reordering atomically visible to the audio thread.
pub struct ProjectEdit {
    pub project: Project,
    pub samples: Vec<Option<Arc<SampleData>>>,
    pub status: String,
    pub history: Option<(HistoryMove, HistoryEntry<ProjectSnapshot>)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::NoteEvent;

    #[test]
    fn project_pattern_banks_are_normalized_for_stale_clipboard_channels() {
        let mut project = Project {
            pattern_lengths: vec![16, 32, 8],
            ..Default::default()
        };
        project.channels[0].notes = vec![vec![NoteEvent::new(1, 0, 6, 60, 100)]];
        project.channels[0].automation = vec![Vec::new(), Vec::new(), Vec::new(), Vec::new()];

        normalize_project_pattern_banks(&mut project);

        assert_eq!(project.channels[0].notes.len(), 3);
        assert_eq!(project.channels[0].automation.len(), 3);
        assert_eq!(project.channels[0].notes[0].len(), 1);
        assert!(project.channels[0].notes[1].is_empty());
        assert!(project.channels[0].notes[2].is_empty());
    }
}
