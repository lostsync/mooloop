//! What the engine's test modules need to render a project and read the result.
//!
//! Seven of them each declared their own. `SAMPLE_RATE` was written out eight
//! times, `render_blocks` four, `render_master` three, `peak_of` three, and the
//! three `render_master` copies differed only in whether they spelled the
//! block size `1024` or `1_024`. None of that could diverge in a way that
//! mattered -- they are test helpers, and a drift would show up as a failing
//! test rather than as shipped behaviour -- but the *cost* was real: a change
//! to `RenderState::from_project`'s signature is one edit here and was seven
//! before.

use crate::render::RenderState;
use mooloop_core::Project;

pub(crate) const SAMPLE_RATE: u32 = 48_000;

/// The block size these tests render at unless the block size *is* what is
/// under test, as it is in `console_tests`.
pub(crate) const BLOCK: usize = 1_024;

/// Render `seconds` of `project` from the top of the transport, in blocks of
/// `block`, and return the master's two channels.
///
/// The block loop is here rather than a single `process_once_block` call
/// because several of these tests exist to show that a result does *not* depend
/// on where the block boundaries fall.
pub(crate) fn render_master_in_blocks(
    project: &Project,
    seconds: f32,
    block: usize,
) -> (Vec<f32>, Vec<f32>) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut left = Vec::with_capacity(remaining);
    let mut right = Vec::with_capacity(remaining);
    while remaining > 0 {
        let frames = remaining.min(block);
        render.process_once_block(frames);
        let master = render.master();
        left.extend_from_slice(&master.l[..frames]);
        right.extend_from_slice(&master.r[..frames]);
        remaining -= frames;
    }
    (left, right)
}

/// [`render_master_in_blocks`] at the ordinary block size.
pub(crate) fn render_master(project: &Project, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    render_master_in_blocks(project, seconds, BLOCK)
}

/// The master's left channel alone, at a chosen block size. What a test that
/// is checking level or silence rather than stereo placement wants.
pub(crate) fn render_blocks(project: &Project, seconds: f32, block: usize) -> Vec<f32> {
    render_master_in_blocks(project, seconds, block).0
}

pub(crate) fn peak_of(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
}

/// The largest absolute difference between two renders, for the tests that
/// assert two paths agree sample for sample.
///
/// `idle_skip_tests` has one of these that also reports *where* the worst
/// difference was, which is a different function and is named as one there.
pub(crate) fn worst_difference(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .fold(0.0f32, |worst, (x, y)| worst.max((x - y).abs()))
}
