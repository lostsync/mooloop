//! What one `process_block` costs, as a number rather than an impression.
//!
//! These are `#[ignore]`d: they measure wall time, so they say nothing useful
//! in a debug build and nothing stable enough to assert on in CI. Run them
//! deliberately, in release, when changing anything on the block path:
//!
//! ```sh
//! cargo test -p mooloop-engine --release block_cost -- --ignored --nocapture
//! ```
//!
//! The figure that matters is nanoseconds per block against the block's own
//! real-time budget: a 128-frame block at 48 kHz must finish inside 2.67 ms,
//! and everything the engine spends before a single sample is generated comes
//! out of that.

use std::time::Instant;

use crate::render::RenderState;
use mooloop_core::{MlP8Params, NoteEvent, Project, ProjectChannel};

const SAMPLE_RATE: u32 = 48_000;

/// `count` ML-P8 channels, each holding one note from the downbeat. ML-P8 is
/// the widest device in the program, so it is the one whose descriptor list
/// the per-block control pass walks at full length.
fn loaded_project(count: usize) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    for index in 0..count {
        let mut channel = ProjectChannel::mlp8_with_params(index, 1, MlP8Params::default());
        channel.notes[0].push(NoteEvent::new(index as u32 + 1, 0, 96, 48 + index as u8, 100));
        project.channels.push(channel);
    }
    project
}

/// Median nanoseconds per `process_block`, after a warm-up that lets every
/// voice reach steady state.
fn per_block_nanos(project: &Project, frames: usize, blocks: usize) -> u128 {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    for _ in 0..64 {
        render.process_block(frames);
    }
    let mut samples = Vec::with_capacity(blocks);
    for _ in 0..blocks {
        let started = Instant::now();
        render.process_block(frames);
        samples.push(started.elapsed().as_nanos());
    }
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// The block path across the channel counts and buffer sizes a host actually
/// asks for. Printed, not asserted: the point is the shape of the table.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn block_cost_by_channels_and_buffer() {
    println!();
    println!("  frames  channels    ns/block   % of budget");
    for frames in [64usize, 128, 256, 512] {
        let budget_nanos = frames as f64 / SAMPLE_RATE as f64 * 1e9;
        for channels in [1usize, 8, 16, 32] {
            let project = loaded_project(channels);
            let nanos = per_block_nanos(&project, frames, 400);
            println!(
                "  {frames:>6}  {channels:>8}  {nanos:>10}   {:>6.2}%",
                nanos as f64 / budget_nanos * 100.0
            );
        }
    }
}

/// The same measurement with every channel silent and idle, which is what an
/// arrangement looks like between its parts. Fixed per-block overhead that
/// does not depend on anything playing shows up here and nowhere else.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn idle_block_cost() {
    println!();
    println!("  frames  channels    ns/block (idle)");
    for frames in [64usize, 128, 256, 512] {
        for channels in [1usize, 16, 32] {
            let mut project = loaded_project(channels);
            for channel in &mut project.channels {
                channel.notes[0].clear();
            }
            let nanos = per_block_nanos(&project, frames, 400);
            println!("  {frames:>6}  {channels:>8}  {nanos:>10}");
        }
    }
}
