//! Whole-project values: the undo unit, the structural edit that carries it,
//! and the pattern-bank invariant both depend on.

use crate::history::Entry as HistoryEntry;
use mooloop_core::{ChannelId, Project};
use mooloop_dsp::SampleData;
use std::collections::HashMap;
use std::sync::Arc;

/// A complete, UI-owned project snapshot. Samples stay beside the serializable
/// project because restoring an edit must never decode audio on the UI thread.
#[derive(Clone)]
pub struct ProjectSnapshot {
    pub project: Project,
    /// The decoded audio, **keyed by the channel that owns it**.
    ///
    /// This was a `Vec` parallel to `project.channels`, kept in step by hand
    /// in three places in `ui/src/lib.rs` -- a paste inserted into it, a
    /// delete removed from it, and a move had to rotate it or every sampler
    /// between the two seats played the wrong file. That is this codebase's
    /// characteristic fault written out in three functions: a second list
    /// that has to be told, every time, what the first one just did.
    ///
    /// Keyed by identity there is nothing to tell. A channel edit does not
    /// touch this map at all, and [`Self::seated`] derives the positional
    /// list the install wants from the project it is being installed with.
    pub samples: HashMap<ChannelId, Arc<SampleData>>,
}

impl ProjectSnapshot {
    /// The audio in seat order, for the install.
    ///
    /// The engine addresses channels by position, so the conversion happens
    /// here -- once, against the project these samples are being installed
    /// with, rather than in a list that has to be maintained alongside.
    pub fn seated(&self) -> Vec<Option<Arc<SampleData>>> {
        self.project
            .channels
            .iter()
            .map(|channel| self.samples.get(&channel.id).cloned())
            .collect()
    }
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
                * std::mem::size_of::<(ChannelId, std::sync::Arc<SampleData>)>()
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
    /// The structural edit to the channel or track list that produced
    /// `project`, when there was one.
    ///
    /// `Project::rescope_after` and `rescope_tracks_after` have already
    /// renumbered everything the *song* holds by the time this is sent. This
    /// carries the same edit forward so the pump can run
    /// `Session::rescope_after` or `rescope_after_track` over the things the
    /// session holds -- the selected device, the open lane, the preset labels
    /// -- none of which are in the snapshot and all of which are keyed by a
    /// seat in one list or the other.
    ///
    /// `None` for an undo or a redo, which restore a whole document rather
    /// than applying an edit to one. The session labels do not survive undo,
    /// which `Session::effect_preset_name` already says in as many words.
    pub edit: Option<mooloop_core::ListEdit>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::NoteEvent;

    /// **The budget's own guard.** The application's history is
    /// `History<ProjectSnapshot>` (`CommandState::history`), and every test
    /// of the ceiling builds a `History<Project>`, a `History<Heavy>`, or a
    /// `History<i32>` -- so if *this* impl returned zero, all four budget
    /// tests would stay green while the history kept as much as it liked.
    /// `edit_cost.rs` names that exact hazard: "an estimate that reads low
    /// would let the history keep more than it is allowed to, which is the
    /// bug it exists to prevent". This is `AGENTS.md`'s question -- does
    /// anything read the copy the test checks? -- answered for the one impl
    /// that ships.
    #[test]
    fn a_snapshots_retained_bytes_counts_the_project_and_the_sample_table() {
        use crate::history::Retained;

        let mut project = Project::default();
        for _ in 0..8 {
            project.channels.push(mooloop_core::ProjectChannel::sampler(0, 1));
        }
        for note in 0..256u32 {
            project.channels[0].notes[0].push(NoteEvent::new(note + 1, note, 24, 60, 100));
        }

        let empty = ProjectSnapshot {
            project: Project::default(),
            samples: HashMap::new(),
        };
        let loaded = ProjectSnapshot {
            project: project.clone(),
            samples: HashMap::new(),
        };

        // Against the snapshot's *own* project, not the one it was cloned
        // from: `heap_bytes` counts capacity, and `Vec::clone` allocates
        // exactly `len`, so a clone of a pushed-into project legitimately
        // reads smaller than its source.
        assert!(
            loaded.retained_bytes() >= loaded.project.heap_bytes(),
            "a snapshot has to charge at least the project it holds: {} against {}",
            loaded.retained_bytes(),
            loaded.project.heap_bytes()
        );
        assert!(
            loaded.retained_bytes() > empty.retained_bytes(),
            "and has to grow with what it holds, or the ceiling is not a ceiling"
        );

        // The sample table is charged by capacity, as pointers -- the reason
        // is in `retained_bytes`'s own doc, and this pins that it is charged
        // at all. Capacity rather than entries, so this needs no audio: the
        // claim is about what the table itself occupies.
        let with_table = ProjectSnapshot {
            project: Project::default(),
            samples: HashMap::with_capacity(64),
        };
        assert!(
            with_table.retained_bytes() > empty.retained_bytes(),
            "the sample table's own capacity is part of what a snapshot keeps"
        );
    }

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
