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
use mooloop_core::{
    EffectKind, EffectSlotState, MlP8Params, NoteEvent, Project, ProjectChannel,
};

const SAMPLE_RATE: u32 = 48_000;

/// `count` ML-P8 channels, each holding one note from the downbeat. ML-P8 is
/// the widest device in the program, so it is the one whose descriptor list
/// the per-block control pass walks at full length.
///
/// The note is half a second long and the measured window is not, so read the
/// playing table with that in mind: at 64 frames the four hundred blocks fall
/// mostly inside the note and the figure is synthesis, while at 512 they run
/// well past it and most of what is being timed is the channel resting. Both
/// are real numbers about a real arrangement; neither is "the cost of a
/// voice". [`idle_block_cost`] is where a device's cost at rest is read
/// deliberately rather than by accident.
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

/// `count` sampler channels with no sample loaded and nothing scheduled.
///
/// The comparison that separates the two halves of an idle block: a sampler
/// takes the default `skip_block`, which does nothing at all, so whatever an
/// idle sampler channel costs is the engine's own per-channel overhead and
/// whatever an idle ML-P8 costs on top of it is the device declining to stay
/// still.
fn idle_sampler_project(count: usize) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    for index in 0..count {
        project.channels.push(ProjectChannel::sampler(index, 1));
    }
    project
}

/// `count` sampler channels, each carrying one `kind` and nothing to play.
///
/// The effect half of the same question `idle_sampler_project` asks about
/// generators. A resting channel still runs `skip_block` on every occupied
/// slot, because a node with a clock in it has to keep time whether or not
/// anything is passing through -- so this is what an arrangement's effect
/// racks cost between the parts, and the sampler underneath contributes the
/// nothing it is measured contributing elsewhere.
fn resting_effect_project(count: usize, kind: EffectKind) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    for index in 0..count {
        let mut channel = ProjectChannel::sampler(index, 1);
        channel.setup.push_effect(EffectSlotState::of_kind(kind));
        project.channels.push(channel);
    }
    project
}

/// Median nanoseconds per `process_block`, after a warm-up that lets every
/// voice reach steady state.
fn per_block_nanos(project: &Project, frames: usize, blocks: usize) -> u128 {
    timed(project, frames, blocks).0
}

/// The median block, and what share of the channel-blocks in the measured
/// stretch were skipped. The second number is what tells a cheap `skip_block`
/// apart from a channel that never reached one.
fn timed(project: &Project, frames: usize, blocks: usize) -> (u128, f64) {
    timed_after(project, frames, blocks, 64 * frames)
}

/// The same, with the warm-up given in frames rather than blocks.
///
/// Anything measuring a *resting* arrangement has to say how long it waited,
/// because a tail is a declared length and some of them are long: the
/// reverb's is three times its decay plus a quarter second, which at the
/// default 2.4 s decay is 7.45 s. Warm up for less than that and the figure
/// is the cost of a reverb still running, correctly, with nothing to show it.
fn timed_after(
    project: &Project,
    frames: usize,
    blocks: usize,
    warmup_frames: usize,
) -> (u128, f64) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut warmed = 0usize;
    while warmed < warmup_frames {
        render.process_block(frames);
        warmed += frames;
    }
    let before = render.slept_strip_blocks();
    let mut samples = Vec::with_capacity(blocks);
    for _ in 0..blocks {
        let started = Instant::now();
        render.process_block(frames);
        samples.push(started.elapsed().as_nanos());
    }
    let slept = render.slept_strip_blocks() - before;
    let possible = (blocks * project.channels.len()) as f64;
    samples.sort_unstable();
    (samples[samples.len() / 2], slept as f64 / possible * 100.0)
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
///
/// Two device kinds side by side, because "idle" is two costs and they have
/// nothing to do with each other. The sampler column is what the engine
/// spends on a channel it has decided not to render; the ML-P8 column is that
/// plus whatever the device does in `skip_block` to keep its clock-driven
/// state honest. A gap that grows with the frame count is per-sample work
/// happening on a block that renders nothing.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn idle_block_cost() {
    println!();
    println!("  frames  channels   sampler ns   ml-p8 ns    ml-p8 - sampler");
    for frames in [64usize, 128, 256, 512] {
        for channels in [1usize, 16, 32] {
            let mut mlp8 = loaded_project(channels);
            for channel in &mut mlp8.channels {
                channel.notes[0].clear();
            }
            let sampler = per_block_nanos(&idle_sampler_project(channels), frames, 400);
            let heavy = per_block_nanos(&mlp8, frames, 400);
            println!(
                "  {frames:>6}  {channels:>8}  {sampler:>11}  {heavy:>9}  {:>17}",
                heavy as i128 - sampler as i128
            );
        }
    }
}

/// What each effect kind costs on a channel that is resting.
///
/// Sixteen channels at 512 frames, which is a large arrangement and a large
/// buffer on purpose: per-sample work in a `skip_block` shows up as a figure
/// that scales with both, and anything that does not is a fixed cost worth
/// far less attention. The bare sampler row is the floor to read the rest
/// against.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn resting_effect_cost() {
    let channels = 16;
    let frames = 512;
    // Twelve seconds of silence before the clock starts: longer than the
    // longest declared tail in the program, so what is being timed is an
    // arrangement that has finished resting rather than one still doing it.
    let warmup = SAMPLE_RATE as usize * 12;
    println!();
    println!("  {channels} channels, {frames}-frame block, rested 12 s");
    println!("  effect         ns/block   over bare   channel-blocks slept");
    let (bare, bare_slept) = timed_after(&idle_sampler_project(channels), frames, 400, warmup);
    println!("  {:<12}  {bare:>9}  {:>10}  {bare_slept:>18.0}%", "(none)", "");
    for kind in [
        EffectKind::Eq,
        EffectKind::Modulation,
        EffectKind::Filter,
        EffectKind::Drive,
        EffectKind::Bitcrush,
        EffectKind::Delay,
        EffectKind::Reverb,
        EffectKind::Plate,
        EffectKind::Gate,
        EffectKind::Compressor,
        EffectKind::Limiter,
    ] {
        let (nanos, slept) =
            timed_after(&resting_effect_project(channels, kind), frames, 400, warmup);
        println!(
            "  {:<12}  {nanos:>9}  {:>10}  {slept:>18.0}%",
            format!("{kind:?}"),
            nanos as i128 - bare as i128
        );
    }
}

/// Drive is the only effect in the program that declares latency, and the
/// only one whose cost at rest has no per-sample loop to point at. This says
/// which of the two shapes that cost has: work per frame scales with the
/// block, work per block does not.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn resting_drive_shape() {
    let warmup = SAMPLE_RATE as usize * 12;
    println!();
    println!("  channels  frames   bare ns   drive ns   over bare   per frame");
    for channels in [1usize, 16] {
        for frames in [64usize, 128, 256, 512] {
            let (bare, _) = timed_after(&idle_sampler_project(channels), frames, 400, warmup);
            let (drive, slept) = timed_after(
                &resting_effect_project(channels, EffectKind::Drive),
                frames,
                400,
                warmup,
            );
            let over = drive as i128 - bare as i128;
            println!(
                "  {channels:>8}  {frames:>6}  {bare:>8}  {drive:>9}  {over:>10}  {:>10.2}  ({slept:.0}% slept)",
                over as f64 / frames as f64
            );
        }
    }
}

/// The same effects with audio actually going through them.
///
/// `resting_effect_cost` says what a rack costs between the parts, which is
/// the question the rest-and-tail mechanism is about. This is the other half:
/// the delay, chorus, reverb and plate read their rings once or many times a
/// sample while they are working, so anything done to that arithmetic shows
/// up here rather than there.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn playing_effect_cost() {
    let channels = 8;
    let frames = 512;
    println!();
    println!("  {channels} ML-P8 channels holding a note, {frames}-frame block");
    println!("  effect         ns/block   over bare");
    let bare = per_block_nanos(&loaded_project(channels), frames, 400);
    println!("  {:<12}  {bare:>9}", "(none)");
    for kind in [
        EffectKind::Modulation,
        EffectKind::Delay,
        EffectKind::Reverb,
        EffectKind::Plate,
        EffectKind::Drive,
    ] {
        let mut project = loaded_project(channels);
        for channel in &mut project.channels {
            channel.setup.push_effect(EffectSlotState::of_kind(kind));
        }
        let nanos = per_block_nanos(&project, frames, 400);
        println!(
            "  {:<12}  {nanos:>9}  {:>10}",
            format!("{kind:?}"),
            nanos as i128 - bare as i128
        );
    }
}

/// What the mixer costs when nothing is routed through it.
///
/// Every project carries all `MAX_BUSES` of them, whether or not a channel
/// names one: `default_buses` builds the full bank and the render loop clears,
/// peaks, chains, balances and meters each one every block. This truncates the
/// bank instead, which is not a thing the UI can do -- it is here to say what
/// share of an empty block the sixteen unused buses are.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn idle_bus_cost() {
    println!();
    println!("  frames   buses    ns/block");
    for frames in [64usize, 128, 256, 512] {
        for buses in [1usize, 2, 5, 17] {
            let mut project = idle_sampler_project(1);
            project.buses.truncate(buses);
            let nanos = per_block_nanos(&project, frames, 400);
            println!("  {frames:>6}  {buses:>6}  {nanos:>10}");
        }
    }
}

/// What a prepared project *holds*, as opposed to what it costs to run.
///
/// The song is almost irrelevant to the answer and that is the finding: a
/// one-channel project with no effects already allocates about a gigabyte,
/// and fifteen channels with a full chain each add a few tens of megabytes on
/// top of it. The floor is the number.
///
/// It is a floor because [`ChannelStrip`] holds *every* generator at once --
/// sampler, drum synth, mono, poly, ML-M1, ML-P8, DS-01 and aux in -- with
/// `active_source` choosing which one runs, and because `RenderState`
/// preallocates `MAX_CHANNELS` of them whether or not the song has that many
/// channels. Both halves are deliberate: switching a channel's generator, or
/// adding a channel, then allocates nothing on the audio thread. The price is
/// two hundred and fifty-six unused strips holding eight unused generators
/// each, which is most of this figure.
///
/// Recorded here rather than argued about, so that any later decision to make
/// strips or generators arrive on demand has a before to point at.
#[test]
#[ignore = "measures live allocation; run deliberately in release"]
fn prepared_project_memory() {
    println!();
    println!(
        "  one ChannelStrip is {:.1} KB inline and holds all {} generator kinds",
        std::mem::size_of::<crate::render::ChannelStrip>() as f64 / 1024.0,
        8,
    );
    println!();
    println!("  channels   effects each   live MB after RenderState::from_project");
    for (channels, effects) in [(1usize, 0usize), (15, 3), (15, 10), (32, 10), (64, 10)] {
        let mut project = idle_sampler_project(channels);
        for channel in &mut project.channels {
            for _ in 0..effects {
                channel
                    .setup
                    .push_effect(EffectSlotState::of_kind(EffectKind::Reverb));
            }
        }
        let before = crate::COUNTING.live();
        let render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        let after = crate::COUNTING.live();
        println!(
            "  {channels:>8}   {effects:>12}   {:>8.1} MB",
            after.saturating_sub(before) as f64 / (1024.0 * 1024.0)
        );
        drop(render);
    }
}

/// What installing a project costs the thread that does it.
///
/// Every `PendingEngineMessage::ProjectEdit` reaches
/// `EngineHandle::install_project`, which builds a *complete* new
/// `RenderState` and hands the displaced one back through the reclaim ring to
/// be dropped by `poll` -- both on the UI thread. `prepared_project_memory`
/// says that state is about a gigabyte; this says what allocating and freeing
/// it costs.
///
/// The budget to read it against is a pointer frame. A drag reports an edit on
/// every move, so an install costing more than about 8 ms is a drag the
/// interface cannot keep up with, and one costing a large fraction of a core
/// is a UI thread that stays busy for as long as the mouse is moving.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn project_install_cost() {
    println!();
    println!("  channels   effects each   ms per install   installs per core-second");
    for (channels, effects) in [(1usize, 0usize), (15, 3), (15, 10), (32, 10)] {
        let mut project = idle_sampler_project(channels);
        for channel in &mut project.channels {
            for _ in 0..effects {
                channel
                    .setup
                    .push_effect(EffectSlotState::of_kind(EffectKind::Reverb));
            }
        }
        // Warm the allocator so the first install is not paying for arena
        // growth the rest do not.
        for _ in 0..4 {
            drop(RenderState::from_project(SAMPLE_RATE, &project, &[]));
        }
        let mut timings = Vec::with_capacity(40);
        for _ in 0..40 {
            let started = Instant::now();
            let render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
            // The drop is the other half of the cost and lands on the same
            // thread, one poll later.
            drop(render);
            timings.push(started.elapsed().as_nanos());
        }
        timings.sort_unstable();
        let median = timings[timings.len() / 2];
        println!(
            "  {channels:>8}   {effects:>12}   {:>14.2}   {:>24.0}",
            median as f64 / 1.0e6,
            1.0e9 / median as f64
        );
    }
}
