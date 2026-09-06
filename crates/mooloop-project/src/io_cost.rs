//! What opening and saving a song costs.
//!
//! `#[ignore]`d, like the engine's `block_cost` and the session's
//! `edit_cost`: these measure wall time and say nothing useful in a debug
//! build. Run them deliberately, in release:
//!
//! ```sh
//! cargo test -p mooloop-project --release io_cost -- --ignored --nocapture
//! ```
//!
//! The figure that matters is how long a user waits between choosing a file
//! and having it open, and whether that time grows sensibly with the song or
//! faster than it.

use std::time::Instant;

use mooloop_core::{
    AutomationLane, AutomationPoint, EffectSlotState, NoteEvent, ParamAddr, Project,
    ProjectChannel,
};
use tempfile::tempdir;

use crate::{load_bundle, save_song, AssetMode};

/// A song with `channels` channels over four patterns, `notes` notes in each
/// pattern of each channel, an automation lane per channel, and three effects
/// on every channel -- which is what a mixed arrangement looks like rather
/// than a bare one.
fn song(channels: usize, notes: usize) -> Project {
    let mut project = Project {
        pattern_lengths: vec![64; 4],
        ..Project::default()
    };
    project.channels.clear();
    for index in 0..channels {
        let mut channel = ProjectChannel::mlp8(index, 4);
        for kind in [
            mooloop_core::EffectKind::Eq,
            mooloop_core::EffectKind::Delay,
            mooloop_core::EffectKind::Reverb,
        ] {
            channel.setup.effects.push(EffectSlotState::of_kind(kind));
        }
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
            channel.automation[pattern].push(lane);
        }
        project.channels.push(channel);
    }
    project
}

/// Save and open, across the sizes a song actually reaches.
///
/// Printed with the note count beside it so the two columns can be read
/// against each other: work proportional to the song is expected, work that
/// climbs faster than the song is the thing to find.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn save_and_load_cost() {
    println!();
    println!("      notes  channels    save ms    load ms   bytes");
    for channels in [4usize, 16, 32] {
        for notes in [16usize, 64, 256] {
            let project = song(channels, notes);
            let total_notes = channels * 4 * notes;
            let dir = tempdir().expect("tempdir");
            let path = dir.path().join("song.mlsong");

            let started = Instant::now();
            save_song(&path, &project, AssetMode::Referenced).expect("save");
            let save = started.elapsed();

            let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

            let started = Instant::now();
            let report = load_bundle(&path).expect("load");
            let load = started.elapsed();
            std::hint::black_box(&report);

            println!(
                "  {total_notes:>9}  {channels:>8}  {:>9.2}  {:>9.2}  {bytes:>7}",
                save.as_secs_f64() * 1000.0,
                load.as_secs_f64() * 1000.0
            );
        }
    }
}
