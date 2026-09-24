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
    render_in_blocks(project, seconds, block, true)
}

/// The master's two channels with the output guard's limiter taken off: the
/// *mix*, for a test measuring summing above 0 dBFS, where the safety
/// limiter would otherwise be what it measured (MOO-93).
pub(crate) fn render_mix(project: &Project, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    render_in_blocks(project, seconds, BLOCK, false)
}

fn render_in_blocks(
    project: &Project,
    seconds: f32,
    block: usize,
    limited: bool,
) -> (Vec<f32>, Vec<f32>) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    if !limited {
        render.unlimit_output();
    }
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

/// The largest sample-to-sample step in either channel of a stereo render.
///
/// A click is a step: whatever else a control change does to the sound, the
/// part a listener hears as a tick is the output moving further in one sample
/// than the material ever moves by itself. So this is the one number the
/// "control changes are continuous" family (MOO-104) reads, and each case
/// compares it against the same measurement taken on the material alone.
pub(crate) fn largest_step(left: &[f32], right: &[f32]) -> f32 {
    let (left, right) = (
        mooloop_dsp::testkit::max_step(left),
        mooloop_dsp::testkit::max_step(right),
    );
    // NaN-aware, like the kit's measure: a poisoned side is never "no step".
    if left.is_nan() || right.is_nan() {
        return f32::NAN;
    }
    left.max(right)
}

/// What [`step_across`] measured on one side of a control change and across it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Transition {
    /// The largest step the material took by itself, over a window as long
    /// as the one measured after the change and ending where it begins.
    pub before: f32,
    /// The largest step from the last frame before the change to the end of
    /// the window after it. Includes the step *onto* the first frame after
    /// the change, which is where a block-boundary switch lands.
    pub across: f32,
    /// The largest step the material takes once the change has settled:
    /// over the second half of the window after it, long past any ramp.
    ///
    /// Needed because a change can make the material itself step further.
    /// A pan to one side puts the whole of a sine on that side, which is
    /// forty percent louder there than at centre, and its own steps are
    /// forty percent larger with it -- the first draft of this family failed
    /// a perfectly smooth pan for exactly that.
    pub after: f32,
    /// The loudest sample in the window before the change, so a bound can be
    /// stated as a fraction of the signal rather than as a bare number.
    pub peak: f32,
}

impl Transition {
    /// The family's step bound: a change may add no more than **one percent
    /// of the signal's peak** to the largest step the material takes on its
    /// own, either side of the change.
    ///
    /// Why that figure. A hard switch on a sustained tone steps by up to the
    /// whole peak -- a hundred times this. A 5 ms one-pole ramp, which is what
    /// the sends and the output stages use, moves a full-scale gain change by
    /// 1/240 of it in its first sample at 48 kHz, and a polarity flip, which is
    /// a change of two, by 1/120: both land under one percent. So the bound
    /// passes any real ramp and fails any switch, with room on both sides.
    pub fn is_continuous(&self) -> bool {
        self.across <= self.bound()
    }

    /// The largest step [`Self::is_continuous`] allows.
    pub fn bound(&self) -> f32 {
        self.before.max(self.after) + 0.01 * self.peak
    }
}

/// Render `render` for `lead_frames`, apply `change`, render `tail_frames`
/// more, and measure the master's largest step before and across the change.
///
/// `change` gets the renderer itself rather than a command, so the same
/// helper serves an `apply_command`, an install or anything else a test can
/// do between two blocks. The change lands exactly at `lead_frames`: the lead
/// is rendered in [`BLOCK`]-sized blocks and a shorter last one, so the
/// transition falls where the test says it does rather than at the next
/// boundary.
///
/// The renderer should already be playing, and the material should be
/// sustained across the whole window -- a note starting or ending inside it
/// would be measured as the change's step.
pub(crate) fn step_across(
    render: &mut RenderState,
    lead_frames: usize,
    change: impl FnOnce(&mut RenderState),
    tail_frames: usize,
) -> Transition {
    let (lead_l, lead_r) = render_frames(render, lead_frames);
    change(render);
    let (tail_l, tail_r) = render_frames(render, tail_frames);
    Transition::measure((&lead_l, &lead_r), (&tail_l, &tail_r))
}

impl Transition {
    /// Measure a change that landed between `lead` and `tail`, each the
    /// master's `(left, right)`: what [`step_across`] does with a renderer,
    /// for a test that has to drive something else -- the executor, when
    /// the change is one it holds back (MOO-213).
    pub(crate) fn measure(lead: (&[f32], &[f32]), tail: (&[f32], &[f32])) -> Self {
        let ((lead_l, lead_r), (tail_l, tail_r)) = (lead, tail);
        let window = tail_l.len().min(lead_l.len());
        let from = lead_l.len() - window;
        let before = largest_step(&lead_l[from..], &lead_r[from..]);
        let peak = peak_of(&lead_l[from..]).max(peak_of(&lead_r[from..]));
        // The last frame before the change leads each side, so the step onto
        // the first frame after it is counted.
        let across_l: Vec<f32> = lead_l.last().into_iter().chain(tail_l).copied().collect();
        let across_r: Vec<f32> = lead_r.last().into_iter().chain(tail_r).copied().collect();
        let settled = tail_l.len() / 2;
        Self {
            before,
            across: largest_step(&across_l, &across_r),
            after: largest_step(&tail_l[settled..], &tail_r[settled..]),
            peak,
        }
    }
}

/// Render `frames` more of `render` in [`BLOCK`]-sized blocks, returning the
/// master's two channels.
pub(crate) fn render_frames(render: &mut RenderState, frames: usize) -> (Vec<f32>, Vec<f32>) {
    let mut remaining = frames;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    while remaining > 0 {
        let block = remaining.min(BLOCK);
        render.process_once_block(block);
        let master = render.master();
        left.extend_from_slice(&master.l[..block]);
        right.extend_from_slice(&master.r[..block]);
        remaining -= block;
    }
    (left, right)
}
