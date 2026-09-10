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

/// A bare project is a history unit in its own right in a few places -- the
/// effect-chain tests undo against one directly -- and it already knows what
/// it occupies.
impl crate::history::Retained for Project {
    fn retained_bytes(&self) -> usize {
        self.heap_bytes()
    }
}

impl crate::history::Retained for ProjectSnapshot {
    /// The project's own heap, plus the sample table's.
    ///
    /// The `Arc`s in `samples` are counted as pointers rather than as the
    /// audio they point at, and that is the honest number for a budget: the
    /// decoded samples are shared with the engine and every other snapshot,
    /// so charging one history entry for all of them would say the history
    /// costs hundreds of megabytes it does not own. What retaining the `Arc`
    /// *does* cost is keeping a replaced sample alive after the user loaded
    /// another one, which is a real cost but a bounded and separate one.
    fn retained_bytes(&self) -> usize {
        self.project.heap_bytes()
            + std::mem::size_of::<Self>()
            + self.samples.capacity()
                * std::mem::size_of::<Option<std::sync::Arc<SampleData>>>()
    }
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
    /// The structural channel edit that produced `project`, when there was
    /// one.
    ///
    /// `Project::rescope_after` has already renumbered everything the *song*
    /// holds by the time this is sent. This carries the same edit forward so
    /// the pump can run `Session::rescope_after` over the things the session
    /// holds -- the selected device, the open lane, the preset labels -- none
    /// of which are in the snapshot and all of which are keyed by a channel
    /// index.
    ///
    /// `None` for an undo or a redo, which restore a whole document rather
    /// than applying an edit to one. The session labels do not survive undo,
    /// which `Session::effect_preset_name` already says in as many words.
    pub channel_edit: Option<mooloop_core::ChannelEdit>,
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
