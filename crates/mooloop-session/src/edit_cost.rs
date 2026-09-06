//! What one project edit costs the UI thread, as a number.
//!
//! `#[ignore]`d, like the engine's `block_cost`: these measure wall time, so
//! they say nothing useful in a debug build and nothing stable enough to
//! assert on. Run them deliberately, in release, when changing the edit path:
//!
//! ```sh
//! cargo test -p mooloop-session --release edit_cost -- --ignored --nocapture
//! ```
//!
//! The figure that matters is milliseconds against a pointer frame. A drag
//! reports an edit on every move, and the history takes a `before` and an
//! `after` snapshot for each one, so an edit costing more than a frame is a
//! drag that cannot keep up with the mouse.

use std::time::Instant;

use mooloop_core::{AutomationLane, AutomationPoint, NoteEvent, ParamAddr, Project, ProjectChannel};

use crate::session::Session;

/// A song of the size someone would actually be dragging notes around in:
/// `channels` channels over four patterns, `notes` notes in each pattern of
/// each channel, and an automation lane on the first two channels.
fn song(channels: usize, notes: usize, lanes: usize) -> Project {
    let mut project = Project::default();
    project.pattern_lengths = vec![64; 4];
    project.channels.clear();
    for index in 0..channels {
        let mut channel = ProjectChannel::mlp8(index, 4);
        for pattern in 0..4 {
            for note in 0..notes {
                channel.notes[pattern].push(NoteEvent::new(
                    (pattern * notes + note) as u32 + 1,
                    (note as u32 % 64) * 24,
                    24,
                    36 + (note as u8 % 36),
                    100,
                ));
            }
        }
        for lane_index in 0..lanes.min(4) {
            let mut lane = AutomationLane::new(ParamAddr::strip(
                mooloop_core::EffectTarget::Channel(index as u8),
                mooloop_core::STRIP_PARAM_VOLUME,
            ));
            for point in 0..16 {
                lane.upsert(AutomationPoint::new(
                    point as u32 + 1,
                    point as u32 * 24,
                    point as f32 / 16.0,
                ));
            }
            channel.automation[lane_index].push(lane);
        }
        project.channels.push(channel);
    }
    project
}

fn median(mut samples: Vec<u128>) -> u128 {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// One `project_snapshot`, which is what the history takes twice per edit.
///
/// It is a rebuild rather than a clone: every channel's source state, notes
/// and automation are reconstructed from the session's own structures each
/// time it is asked.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn snapshot_cost() {
    println!();
    println!("  channels  notes/pattern    ns/snapshot   two (one edit)");
    for channels in [4usize, 16, 32] {
        for notes in [16usize, 64, 256] {
            let project = song(channels, notes, 1);
            let mut session = Session::default();
            session.replace_project(&project, &[]);

            for _ in 0..8 {
                let _ = session.project_snapshot(120, 50);
            }
            let mut samples = Vec::with_capacity(200);
            for _ in 0..200 {
                let started = Instant::now();
                let snapshot = session.project_snapshot(120, 50);
                samples.push(started.elapsed().as_nanos());
                std::hint::black_box(&snapshot);
            }
            let one = median(samples);
            println!(
                "  {channels:>8}  {notes:>13}  {one:>13}  {:>13.3} ms",
                (one * 2) as f64 / 1.0e6
            );
        }
    }
}

/// Bytes currently held by the allocator, counted rather than sampled.
///
/// Resident set size was tried first and is the wrong instrument: it moves a
/// page at a time and the allocator hands nothing back to the OS, so warming
/// it up made several sizes read as costing exactly zero. Counting the
/// allocations themselves answers the question asked.
fn allocated_bytes() -> usize {
    crate::COUNTING.live()
}

/// What one undo entry costs in memory.
///
/// `History` has no cap: `entries` is a plain `Vec` and every edit pushes a
/// `before` and an `after`, each a whole project. A long session therefore
/// grows without bound, and the question this answers is how fast.
#[test]
#[ignore = "measures resident memory; run deliberately in release"]
fn undo_entry_memory() {
    println!();
    println!("  channels  notes/pattern   KB per undo entry   after 500 edits");
    for channels in [4usize, 16, 32] {
        for notes in [16usize, 64, 256] {
            let project = song(channels, notes, 1);
            let mut session = Session::default();
            session.replace_project(&project, &[]);

            // Warm the allocator so the first entries are not paying for
            // arena growth the rest do not.
            let mut warm: Vec<(Project, Project)> = (0..32)
                .map(|_| {
                    (
                        session.project_snapshot(120, 50),
                        session.project_snapshot(120, 50),
                    )
                })
                .collect();
            warm.clear();
            warm.shrink_to_fit();

            let count = 200;
            let before = allocated_bytes();
            let entries: Vec<(Project, Project)> = (0..count)
                .map(|_| {
                    (
                        session.project_snapshot(120, 50),
                        session.project_snapshot(120, 50),
                    )
                })
                .collect();
            let after = allocated_bytes();
            std::hint::black_box(&entries);
            let per_entry = after.saturating_sub(before) as f64 / count as f64;
            println!(
                "  {channels:>8}  {notes:>13}  {:>17.1}  {:>14.1} MB",
                per_entry / 1024.0,
                per_entry * 500.0 / (1024.0 * 1024.0)
            );
            drop(entries);
        }
    }
}
