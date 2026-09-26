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
//! **The allocation tests need `--test-threads=1`, and one filter at a time.**
//! `COUNTING` is a global allocator and `live()` is a process-wide figure, so
//! two of them running at once measure each other. The failure is silent and
//! does not look like a failure -- it looks like a number:
//! `RenderState::from_project(0ch)` read 40.33 MB and the one-channel row read
//! 0.00 MB on a run where three of these shared a thread pool.
//!
//! ```sh
//! cargo test -p mooloop-engine --release prepared_project_memory \
//!   -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The figure that matters is nanoseconds per block against the block's own
//! real-time budget: a 128-frame block at 48 kHz must finish inside 2.67 ms,
//! and everything the engine spends before a single sample is generated comes
//! out of that.

use std::time::Instant;

use crate::render::RenderState;
use mooloop_core::{
    EffectKind, EffectSlotState, MlP8Params, ModulationMode, ModulationParams, NoteEvent,
    Project, ProjectChannel,
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

/// [`loaded_project`], with the filter actually engaged.
///
/// `MlP8Params::default`'s cutoff is `1.0` -- wide open, unresonant, nothing
/// routed to it -- which is `Prepared::filter_open`'s bypass condition, so
/// `loaded_project`'s baseline never enters `Voice::shape`'s filter half at
/// all. This is the before/after figure for `reports/fable-2026-09-22.md`
/// finding 2, Plan B: a closed, resonant filter is what pays for
/// `SvfCoeffs::for_cutoff` (the `tan()`) every sample, and what
/// `Voice::cached_hz_from_knob` (step 4's hoist) takes `hz_from_normalized`'s
/// `powf` out of.
fn filtered_project(count: usize) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    let params = MlP8Params {
        filter_cutoff: 0.3,
        filter_resonance: 0.4,
        ..MlP8Params::default()
    };
    for index in 0..count {
        let mut channel = ProjectChannel::mlp8_with_params(index, 1, params);
        channel.notes[0].push(NoteEvent::new(index as u32 + 1, 0, 96, 48 + index as u8, 100));
        project.channels.push(channel);
    }
    project
}

/// `count` sampler channels, each holding a *full* pattern: one note every
/// sixty-fourth across the whole `MAX_PATTERN_STEPS` capacity, 1,024 notes
/// per channel, built with `NoteEvent::new` in a loop the way a note-dense
/// pattern is actually authored. `loaded_project`'s one note per channel is
/// the figure `pattern-bank-floor/00-status.md` reads as "the engine is not
/// a problem"; this is the scheduling walk that number has never included
/// (`reports/fable-2026-09-22.md`, finding 1, Plan A).
fn scheduling_project(count: usize) -> Project {
    use mooloop_core::{MAX_PATTERN_STEPS, TICKS_PER_64TH};
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths = vec![MAX_PATTERN_STEPS];
    for index in 0..count {
        let mut channel = ProjectChannel::sampler(index, 1);
        channel.notes[0] = (0..1024u32)
            .map(|id| NoteEvent::new(id + 1, id * TICKS_PER_64TH, TICKS_PER_64TH, 60, 100))
            .collect();
        project.channels.push(channel);
    }
    project
}

/// [`scheduling_project`]'s full patterns, placed 64 times end to end on
/// the playlist -- Song mode's shape of the same question, and the one
/// `block_cost.rs` has never asked at all: nothing here built a Song-mode
/// project before this.
fn scheduling_song_project(count: usize) -> Project {
    let mut project = scheduling_project(count);
    let pattern_ticks = u32::from(project.pattern_lengths[0]) * mooloop_core::TICKS_PER_STEP;
    project.playback_mode = mooloop_core::PlaybackMode::Song;
    project.playlist = (0..64u32)
        .map(|instance| mooloop_core::PatternPlacement {
            pattern: 0,
            start_tick: instance * pattern_ticks,
        })
        .collect();
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

/// What scheduling itself costs, as opposed to what `loaded_project`
/// measures.
///
/// `loaded_project` holds one note per channel and no Song-mode case has
/// ever existed here, so the 13-16% of a quantum
/// `pattern-bank-floor/00-status.md` reads as "the engine is not a
/// problem" has never included the per-block walk over a note-dense
/// pattern or a placed-out song
/// (`reports/fable-2026-09-22.md`, finding 1). Sixteen channels, each
/// holding a full 1,024-note pattern, in Pattern mode and with the same
/// patterns placed 64 times in Song mode, at 64 and 512 frames -- printed
/// beside `loaded_project`'s one-note figure at the same channel count and
/// frame size so the two read together.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn scheduling_cost() {
    println!();
    println!("  mode       frames  ns/block   loaded_project (1 note/ch) ns/block");
    for frames in [64usize, 512] {
        let baseline = per_block_nanos(&loaded_project(16), frames, 400);

        let pattern_ns = per_block_nanos(&scheduling_project(16), frames, 400);
        println!("  {:<9}  {frames:>6}  {pattern_ns:>9}   {baseline:>9}", "Pattern");

        let song_ns = per_block_nanos(&scheduling_song_project(16), frames, 400);
        println!("  {:<9}  {frames:>6}  {song_ns:>9}   {baseline:>9}", "Song");
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
        EffectKind::Preamp,
        EffectKind::Drive,
        EffectKind::Bitcrush,
        EffectKind::Delay,
        EffectKind::Reverb,
        EffectKind::Plate,
        EffectKind::Gate,
        EffectKind::Compressor,
        EffectKind::BusComp,
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

/// **What a loop fold costs.** A fold clears every tail-holding ring in the
/// project inside one callback: a delay's two-second stereo line is 768 KB
/// at 48 kHz, and a reverb is eight lines, four input diffusers, eight in-loop
/// diffusers and a predelay.
/// The cost scales with the *song*, not the block, and a fold is every few
/// seconds in a loop-based instrument
/// (`reports/fable-2026-09-21.md`, finding 2).
///
/// Sixteen channels, each with a delay, a reverb and a plate; the same
/// arrangement measured with the loop range on and off, so the difference is
/// the fold rather than the arrangement. The loop is one bar, so at 120 bpm a
/// fold lands about every two seconds and the median block is unaffected --
/// **read the maximum, not the median**, which is why both are printed.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn loop_fold_cost() {
    fn tail_project(channels: usize, looping: bool) -> Project {
        let mut project = Project::default();
        project.channels.clear();
        for index in 0..channels {
            let mut channel = ProjectChannel::sampler(index, 1);
            for kind in [EffectKind::Delay, EffectKind::Reverb, EffectKind::Plate] {
                channel.setup.push_effect(EffectSlotState::of_kind(kind));
            }
            project.channels.push(channel);
        }
        project.playback_mode = mooloop_core::PlaybackMode::Song;
        project.playlist = vec![mooloop_core::PatternPlacement {
            pattern: 0,
            start_tick: 0,
        }];
        project.loop_range = mooloop_core::LoopRange {
            start_tick: 0,
            end_tick: mooloop_core::TICKS_PER_BAR,
            enabled: looping,
        };
        project
    }

    fn blocks(project: &Project, frames: usize, blocks: usize) -> (u128, u128) {
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
        (samples[samples.len() / 2], *samples.last().expect("blocks"))
    }

    let channels = 16;
    let frames = 512;
    // Ten seconds of blocks: five folds at 120 bpm with a one-bar loop.
    let count = SAMPLE_RATE as usize * 10 / frames;
    println!();
    println!("  {channels} channels of delay+reverb+plate, {frames}-frame block");
    println!("  loop        median ns   max ns");
    for looping in [false, true] {
        let (median, max) = blocks(&tail_project(channels, looping), frames, count);
        println!(
            "  {:<10}  {median:>9}  {max:>7}",
            if looping { "on" } else { "off" }
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
        // A closed, resonant `FilterEffect` on a static cutoff: one range
        // covers the whole block (no `ParamValue` events), so this is
        // squarely the case `reports/fable-2026-09-22.md` finding 2, Plan B
        // targets -- `SvfCoeffs` computed twice a range and lerped, instead
        // of `tan()` and a divide every sample.
        EffectKind::Filter,
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

    // The modulation row above measures its default mode (chorus); a
    // 12-stage phaser is a distinct cost shape inside the same device --
    // up to 72 transcendental calls a sample before Plan C step 1's hoist
    // (`reports/fable-2026-09-22.md` finding 2) -- so it gets its own row
    // rather than being read off the chorus figure.
    {
        let mut project = loaded_project(channels);
        for channel in &mut project.channels {
            channel.setup.push_effect(EffectSlotState::modulation(ModulationParams {
                mode: ModulationMode::Phaser,
                stages: 12,
                ..ModulationParams::default()
            }));
        }
        let nanos = per_block_nanos(&project, frames, 400);
        println!(
            "  {:<12}  {nanos:>9}  {:>10}",
            "Phaser12",
            nanos as i128 - bare as i128
        );
    }
}

/// The voice filter's own cost, as opposed to the `FilterEffect`
/// `playing_effect_cost` measures.
///
/// [`loaded_project`]'s default patch has the filter wide open
/// (`filter_cutoff: 1.0`), which is `Prepared::filter_open`'s bypass
/// condition, so that baseline never runs `Voice::shape`'s filter half at
/// all -- a closed, resonant patch is the one that does. Companion to
/// [`playing_effect_cost`] for `reports/fable-2026-09-22.md` finding 2,
/// Plan B step 4 (`Voice::cached_hz_from_knob`, the sampler's
/// `Sampler::filter_base_hz`, and the equivalent hoists in `monosynth.rs`,
/// `polysynth.rs` and `mlm1.rs`).
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn filter_engaged_cost() {
    let channels = 8;
    println!();
    println!("  {channels} ML-P8 channels holding a note, filter closed and resonant");
    println!("  frames   open ns/block   closed ns/block   over open");
    for frames in [64usize, 128, 256, 512] {
        let open = per_block_nanos(&loaded_project(channels), frames, 400);
        let closed = per_block_nanos(&filtered_project(channels), frames, 400);
        println!(
            "  {frames:>6}  {open:>13}  {closed:>15}  {:>10}",
            closed as i128 - open as i128
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
/// It was a floor because [`ChannelStrip`] held *every* generator at once,
/// with a tag choosing which one ran, so switching a channel's generator
/// allocated nothing on the audio thread. Since MOO-56 a strip holds one boxed
/// source -- a change builds the new one on the control thread and sends the
/// old one back through the reclaim ring -- so a live strip pays for the one
/// generator it plays. Strips are built only for the channels a song has
/// (`RenderState::grow_channels`), not all `MAX_CHANNELS` of them.
///
/// Recorded here rather than argued about, so that any later decision to make
/// strips or generators arrive on demand has a before to point at.
#[test]
#[ignore = "measures live allocation; run alone, in release, with --test-threads=1"]
fn prepared_project_memory() {
    println!();
    println!(
        "  one ChannelStrip is {:.1} KB inline, plus the one generator it plays",
        std::mem::size_of::<crate::render::ChannelStrip>() as f64 / 1024.0,
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

/// What a track costs, and what raising `MAX_BUSES` would cost.
///
/// `docs/CAPACITY_POLICY.md` forbids small product caps and warns, in the same
/// breath, that the expensive mistake is *dimensioning by* a ceiling rather
/// than reserving one. `MAX_BUSES` is currently seventeen, which is a small
/// product cap; raising it is the obvious fix and is exactly the move that
/// document says to measure first, because several fixed arrays multiply by
/// it and none of them looks expensive where it is defined.
///
/// So this prints both halves: what one track costs when it is made, and what
/// the ceiling costs whether or not any track exists. `MAX_BUSES` is a
/// compile-time constant and cannot be swept in one run, so the second half is
/// arithmetic against the current value -- which is the point, since it makes
/// the price of any candidate ceiling readable rather than a guess.
#[test]
#[ignore = "measures live allocation; run deliberately in release"]
fn track_memory() {
    use mooloop_core::{MAX_BUSES, MAX_CHANNELS, MAX_EFFECTS_PER_CHANNEL};
    println!();

    // --- the marginal cost of a track that exists -----------------------
    let mut project = idle_sampler_project(1);
    let before = crate::COUNTING.live();
    let one = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    let with_one_track = crate::COUNTING.live().saturating_sub(before);
    drop(one);

    project.ensure_tracks(MAX_BUSES);
    let before = crate::COUNTING.live();
    let full = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    let with_full_bank = crate::COUNTING.live().saturating_sub(before);
    drop(full);

    let marginal = with_full_bank.saturating_sub(with_one_track) as f64 / (MAX_BUSES - 1) as f64;
    println!("  a project with 1 track        {:>9.2} MB", with_one_track as f64 / 1048576.0);
    println!("  a project with {MAX_BUSES} tracks       {:>9.2} MB", with_full_bank as f64 / 1048576.0);
    println!("  marginal cost of one track    {:>9.1} KB", marginal / 1024.0);
    println!();

    // --- the cost of the ceiling itself ---------------------------------
    //
    // These are allocated from `MAX_BUSES` whether or not a track exists, so
    // they are what a larger address space would cost before anybody made
    // anything.
    let targets = MAX_CHANNELS + MAX_BUSES;
    let stages = MAX_EFFECTS_PER_CHANNEL + 1;
    // What is still dimensioned by the ceilings, now that the spectrum is a
    // pool: one `u32` per stage for the subscription, one for the buffer
    // collision counter, and six for the meters.
    let subscription = targets * stages * 4;
    let collisions = targets * stages * 4;
    let meters = targets * stages * 6 * 4;
    let pool = crate::meters::SPECTRUM_SLOTS * mooloop_dsp::SPECTRUM_BINS * 4;
    let per_bus_of_ceiling = stages * (4 + 4 + 6 * 4);
    println!("  dimensioned by MAX_BUSES = {MAX_BUSES}, whatever the song holds:");
    println!(
        "    DeviceMeters cells          {:>9.2} MB   ({targets} targets x {stages} stages x 6)",
        meters as f64 / 1048576.0,
    );
    println!(
        "    spectrum subscriptions      {:>9.2} MB",
        subscription as f64 / 1048576.0,
    );
    println!(
        "    buffer collisions           {:>9.2} MB",
        collisions as f64 / 1048576.0,
    );
    println!(
        "    the spectra themselves      {:>9.1} KB   ({} slots x {} bins) -- a pool, so it does *not* scale",
        pool as f64 / 1024.0,
        crate::meters::SPECTRUM_SLOTS,
        mooloop_dsp::SPECTRUM_BINS,
    );
    println!(
        "    ...of which per bus         {:>9.1} KB   <- multiply this by any rise",
        per_bus_of_ceiling as f64 / 1024.0,
    );
    println!(
        "    CompiledBusGraph            {:>9} B",
        std::mem::size_of::<mooloop_core::CompiledBusGraph>(),
    );
    println!(
        "    CompiledLatency             {:>9} B",
        std::mem::size_of::<mooloop_core::CompiledLatency>(),
    );
    println!();
    for candidate in [32usize, 64, 128, 256] {
        let rise = candidate.saturating_sub(MAX_BUSES);
        println!(
            "  MAX_BUSES = {candidate:>3}  adds {:>7.2} MB of fixed cost before a track exists",
            (rise * per_bus_of_ceiling) as f64 / 1048576.0,
        );
    }
}

/// What a send costs, which is the figure `docs/CAPACITY_POLICY.md` asks for
/// before anything reserves for one.
///
/// Two numbers matter and they are different in kind. The **floor** is what a
/// project pays for the feature existing while it uses none of it, and it has
/// to be zero: a `Vec` that is empty and a scratch that is not allocated. The
/// **marginal** cost is what one send costs when somebody makes one, and it is
/// a compensation ring plus a `Smoothed` plus a few bytes of routing.
///
/// The three 64 KB scratch buffers are the one lump, and they are per *engine*
/// rather than per send -- so they land on the first send a project makes and
/// never again.
#[test]
#[ignore = "measures memory; run deliberately"]
fn send_memory() {
    use mooloop_core::{AuxSend, MAX_BUSES, MAX_CHANNELS};
    println!();

    let mut project = idle_sampler_project(1);
    project.ensure_tracks(4);

    let before = crate::COUNTING.live();
    let none = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    let floor = crate::COUNTING.live().saturating_sub(before);
    drop(none);

    project.buses[1].sends.push(AuxSend::new(2));
    let before = crate::COUNTING.live();
    let one = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    let with_one = crate::COUNTING.live().saturating_sub(before);
    drop(one);

    for target in 0..8 {
        project.buses[1].sends.push(AuxSend::new(2 + (target % 2)));
    }
    let before = crate::COUNTING.live();
    let many = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    let with_nine = crate::COUNTING.live().saturating_sub(before);
    drop(many);

    println!("  a project with no sends       {:>9.2} MB", floor as f64 / 1048576.0);
    println!("  ...with one send              {:>9.2} MB", with_one as f64 / 1048576.0);
    println!("  ...with nine                  {:>9.2} MB", with_nine as f64 / 1048576.0);
    println!();
    println!(
        "  the first send costs          {:>9.1} KB   (three shared scratch buffers, once)",
        with_one.saturating_sub(floor) as f64 / 1024.0,
    );
    println!(
        "  each one after                {:>9.1} B",
        with_nine.saturating_sub(with_one) as f64 / 8.0,
    );
    println!(
        "    SendSpec                    {:>9} B",
        std::mem::size_of::<crate::SendSpec>(),
    );
    println!(
        "    the producer start table    {:>9} B   ({} slots, allocated only when a send exists)",
        (MAX_CHANNELS + MAX_BUSES + 1) * 4,
        MAX_CHANNELS + MAX_BUSES + 1,
    );
    println!();
    println!("  Nothing here is dimensioned by a maximum number of sends, because");
    println!("  there is not one. `docs/CAPACITY_POLICY.md` is why.");
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

/// Whether rebuilding the render state actually disturbs a thread that has a
/// deadline, which is the claim `CAPACITY_POLICY.md` records as a hypothesis.
///
/// `project_install_cost` establishes that an edit allocates and frees about a
/// gigabyte on the UI thread, and that a drag does it on every move frame.
/// That the *UI* thread saturates follows arithmetically. That the *audio*
/// thread suffers for it does not: it holds `SCHED_FIFO`, and scheduling
/// priority is exactly the mechanism meant to stop one thread's work from
/// delaying another's.
///
/// So this puts the two side by side. A thread wakes on a fixed period and
/// records how late each wake-up was, first against an idle machine and then
/// against a main thread installing projects at drag rate. If the second
/// column matches the first, the hypothesis is wrong and the dropouts are
/// something else; if it does not, the allocator is reaching the audio thread
/// through something priority does not defer.
///
/// Two honest limits on what this can conclude. It runs wherever the test
/// suite runs, which is not Adam's laptop, and a machine with more cores and
/// more memory will show less of the effect rather than more. And it reports
/// whether it managed to get `SCHED_FIFO` for the waking thread: without it, a
/// late wake-up might be ordinary scheduling rather than the thing being
/// looked for, so the run says which it measured.
#[test]
#[ignore = "measures wall time under load; run deliberately in release"]
fn install_churn_disturbs_a_deadline_thread() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// 1024 frames at 48 kHz: the buffer size Adam's settings ask for.
    const PERIOD: Duration = Duration::from_nanos(21_333_333);
    const WAKEUPS: usize = 120;

    /// Ask for realtime scheduling on this thread, and say whether it worked.
    /// The whole question is what happens to a thread that *has* priority.
    #[cfg(target_os = "linux")]
    fn request_realtime() -> bool {
        let param = libc::sched_param { sched_priority: 55 };
        // SAFETY: `param` outlives the call and `sched_setscheduler` reads it
        // without retaining it. Pid 0 is the calling thread.
        unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) == 0 }
    }
    #[cfg(not(target_os = "linux"))]
    fn request_realtime() -> bool {
        false
    }

    /// Wake `WAKEUPS` times on `PERIOD`, doing a little work each time, and
    /// report how many wake-ups were more than half a period late and what
    /// the worst one was.
    ///
    /// Deadlines are absolute rather than a `sleep(PERIOD)` in a loop, because
    /// a relative sleep silently absorbs the very lateness being measured.
    fn run_deadline_thread(stop: Arc<AtomicBool>) -> (bool, usize, f64) {
        let realtime = request_realtime();
        let mut scratch = vec![0.0f32; 2048];
        let start = Instant::now();
        let mut late = 0usize;
        let mut worst = 0f64;
        for tick in 1..=WAKEUPS {
            let deadline = start + PERIOD * tick as u32;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                std::thread::sleep((deadline - now).min(Duration::from_micros(500)));
            }
            let woke = Instant::now();
            let lateness = woke.saturating_duration_since(deadline).as_secs_f64();
            if lateness > PERIOD.as_secs_f64() / 2.0 {
                late += 1;
            }
            worst = worst.max(lateness);
            // A token block of work, so the thread touches memory rather than
            // only sleeping.
            for (index, sample) in scratch.iter_mut().enumerate() {
                *sample = (index as f32).sin();
            }
            std::hint::black_box(&scratch);
        }
        stop.store(true, Ordering::Relaxed);
        (realtime, late, worst * 1000.0)
    }

    let project = {
        let mut project = idle_sampler_project(15);
        for channel in &mut project.channels {
            for _ in 0..3 {
                channel
                    .setup
                    .push_effect(EffectSlotState::of_kind(EffectKind::Reverb));
            }
        }
        project
    };

    println!();
    println!("  {WAKEUPS} wake-ups on a 21.3 ms period (1024 frames at 48 kHz)");
    println!();

    // Baseline: nothing else running.
    let stop = Arc::new(AtomicBool::new(false));
    let quiet = std::thread::spawn({
        let stop = stop.clone();
        move || run_deadline_thread(stop)
    })
    .join()
    .expect("deadline thread");

    // Under churn: a main thread installing projects the way a drag does.
    let stop = Arc::new(AtomicBool::new(false));
    let handle = std::thread::spawn({
        let stop = stop.clone();
        move || run_deadline_thread(stop)
    });
    let mut installs = 0usize;
    while !stop.load(Ordering::Relaxed) {
        drop(RenderState::from_project(SAMPLE_RATE, &project, &[]));
        installs += 1;
    }
    let loaded = handle.join().expect("deadline thread");

    println!(
        "  realtime scheduling granted: {}",
        if quiet.0 { "yes (SCHED_FIFO 55)" } else { "NO -- lateness below may be ordinary scheduling" }
    );
    println!("  installs performed during the loaded run: {installs}");
    println!();
    println!("                     late wake-ups   worst lateness");
    println!("  idle machine       {:>13}   {:>11.2} ms", quiet.1, quiet.2);
    println!("  installing         {:>13}   {:>11.2} ms", loaded.1, loaded.2);
}

/// Where the floor in [`prepared_project_memory`] actually is.
///
/// That test says a one-channel project allocates about a gigabyte and does
/// not say what of. This bisects it by bracketing each piece of
/// `RenderState::new` separately, because the floor is a sum of independently
/// reasonable decisions and only the total is alarming.
#[test]
#[ignore = "measures live allocation; run alone, in release, with --test-threads=1"]
fn render_state_floor_by_component() {
    use mooloop_core::{MAX_BUSES, MAX_CHANNELS};

    fn measure<T>(label: &str, build: impl FnOnce() -> T) {
        let before = crate::COUNTING.live();
        let value = build();
        let after = crate::COUNTING.live();
        println!(
            "  {label:<34} {:>10.2} MB",
            after.saturating_sub(before) as f64 / (1024.0 * 1024.0)
        );
        drop(value);
    }

    println!();
    println!(
        "  MAX_CHANNELS {MAX_CHANNELS}, MAX_BUSES {MAX_BUSES}, \
         MAX_EFFECTS_PER_CHANNEL {}",
        mooloop_core::MAX_EFFECTS_PER_CHANNEL
    );
    println!();
    measure("BusMeters::new", crate::meters::BusMeters::new);
    measure("DeviceMeters::new", crate::meters::DeviceMeters::new);
    measure("DeviceTelemetry::new", crate::meters::DeviceTelemetry::new);
    measure("PlayheadMeters::new", crate::meters::PlayheadMeters::new);
    measure("ModulatorMeters::new", crate::meters::ModulatorMeters::new);
    measure("256x ModRack::default", || {
        (0..MAX_CHANNELS)
            .map(|_| mooloop_core::modulation::ModRack::default())
            .collect::<Vec<_>>()
    });
    measure("256x ModulatorRack::new", || {
        (0..MAX_CHANNELS)
            .map(|_| mooloop_dsp::ModulatorRack::new())
            .collect::<Vec<_>>()
    });
    measure("Sequencer::new(1,1)", || {
        crate::sequencer::Sequencer::new(1, 1, 16, mooloop_core::Ppq::DEFAULT)
    });
    println!();
    let empty = idle_sampler_project(0);
    measure("RenderState::from_project(0ch)", || {
        RenderState::from_project(SAMPLE_RATE, &empty, &[])
    });
    let one = idle_sampler_project(1);
    measure("RenderState::from_project(1ch)", || {
        RenderState::from_project(SAMPLE_RATE, &one, &[])
    });
}

/// **What each device costs while it plays**, one device at a time, in the
/// unit the song sweep (`session/tests/song_block_cost.rs`) reports: mean and
/// worst microseconds per 128-frame block.
///
/// The song sweep says which device *kinds* cost the most in real songs; this
/// says what one of them costs on its own, at its defaults and at the
/// heaviest settings a real patch uses (every factory patch of every kind),
/// so a finding can be stated as "this device, these settings, this many
/// microseconds". Each row plays a one-bar pattern twice from a fresh
/// `RenderState`: an instrument holds a chord for three quarters of every
/// bar, and an effect sits after an ML-P8 playing the same chord.
///
/// **Built for a shared, noisy machine.** The rows are played round-robin,
/// every row once per pass, `REPS` passes (default 5), and each block's cost
/// is its fastest over the passes. A burst of someone else's work lands on
/// one pass of many rows rather than on every pass of one, so the minimum
/// strips it out, and two rows are always compared across the same stretch
/// of time. The row is the mean and the worst of those minima; `over` is the
/// row less its reference (the empty song for the engine and the
/// instruments, the bare ML-P8 chord for an effect, ML-P8 + Filter for the
/// control pass).
///
/// ```sh
/// cargo test -p mooloop-engine --release device_cost -- --ignored --nocapture
/// ```
///
/// `DEVICE_COST=engine,instruments,effects,modulation` picks sections
/// (default all); `DEVICE_COST_MATCH=<text>` keeps only rows whose label
/// contains it (and their references). Added by the 2026-09-25 performance
/// survey, whose findings are in the Linear project Performance.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn device_cost() {
    use mooloop_core::automation::{AutomationLane, AutomationPoint};
    use mooloop_core::effect::{
        EffectParams, EqBandKind, ModulationMode, ModulationParams, FILTER_PARAM_CUTOFF_HZ,
    };
    use mooloop_core::mixer::EffectTarget;
    use mooloop_core::mlp8::{MlP8Chorus, MlP8Unison};
    use mooloop_core::modulation::{
        ModEnvelopeParams, ModLfoParams, ModMathParams, ModPolarity, ModRandomParams, ModRoute,
        ModStepParams, ModulatorParams, ParamAddr,
    };
    use mooloop_core::sampler::StretchMode;
    use mooloop_dsp::SampleData;
    use std::sync::Arc;

    const FRAMES: usize = 128;
    const CHORD: [u8; 8] = [48, 55, 60, 64, 67, 71, 74, 79];

    /// One line of the table: a project to play, how long to let it run
    /// first, and which earlier row its `over` column is read against.
    struct Row {
        label: String,
        project: Option<Project>,
        warmup_blocks: usize,
        reference: usize,
    }

    fn chord(channel: &mut ProjectChannel, notes: &[u8], bar: u32) {
        channel.notes[0].clear();
        for (index, note) in notes.iter().enumerate() {
            channel.notes[0].push(NoteEvent::new(index as u32 + 1, 0, bar * 3 / 4, *note, 100));
        }
    }

    fn solo(mut channel: ProjectChannel) -> Project {
        let mut project = Project::default();
        project.channels.clear();
        channel.rescope(0);
        project.channels.push(channel);
        project
    }

    let reps = std::env::var("REPS")
        .ok()
        .and_then(|reps| reps.parse().ok())
        .unwrap_or(5usize)
        .max(1);
    let sections = std::env::var("DEVICE_COST")
        .unwrap_or_else(|_| "engine,instruments,effects,modulation".into());
    let wants = |section: &str| sections.split(',').any(|wanted| wanted == section);
    let matching = std::env::var("DEVICE_COST_MATCH").ok();

    let bar = mooloop_core::playlist::TICKS_PER_BAR;
    // Two bars at the default 120 bpm.
    let blocks = (4 * SAMPLE_RATE as usize).div_ceil(FRAMES);
    let rested = 12 * SAMPLE_RATE as usize / FRAMES;

    let mut rows: Vec<Row> = Vec::new();
    let heading = |rows: &mut Vec<Row>, label: &str| {
        rows.push(Row {
            label: label.into(),
            project: None,
            warmup_blocks: 0,
            reference: 0,
        });
    };
    let add =
        |rows: &mut Vec<Row>, label: String, project: Project, warmup: usize, reference: usize| {
            rows.push(Row {
                label,
                project: Some(project),
                warmup_blocks: warmup,
                reference,
            });
            rows.len() - 1
        };

    let empty = {
        let mut project = Project::default();
        project.channels.clear();
        project
    };
    let floor = add(&mut rows, "empty song (no channels)".into(), empty, 0, 0);

    if wants("engine") {
        heading(&mut rows, "-- the engine around the devices --");
        for count in [1usize, 8, 32, 128] {
            add(
                &mut rows,
                format!("{count} idle samplers (no notes)"),
                idle_sampler_project(count),
                0,
                floor,
            );
        }
        for count in [8usize, 32] {
            let mut project = loaded_project(count);
            for channel in &mut project.channels {
                channel.notes[0].clear();
            }
            add(
                &mut rows,
                format!("{count} idle ML-P8 (no notes)"),
                project,
                0,
                floor,
            );
        }
        // An arrangement's parked channels: a rack each, nothing to play,
        // measured after twelve seconds so every declared tail is over.
        for count in [8usize, 32] {
            let mut project = idle_sampler_project(count);
            for channel in &mut project.channels {
                for kind in [
                    EffectKind::Eq,
                    EffectKind::Compressor,
                    EffectKind::Delay,
                    EffectKind::Reverb,
                ] {
                    channel
                        .setup
                        .push_effect(EffectSlotState::of_kind(kind))
                        .expect("room");
                }
            }
            add(
                &mut rows,
                format!("{count} idle samplers + EQ,Comp,Delay,Reverb, rested"),
                project,
                rested,
                floor,
            );
        }
        // Muted channels that would otherwise play: what a mute saves.
        for count in [8usize, 32] {
            let mut project = loaded_project(count);
            for channel in &mut project.channels {
                chord(channel, &CHORD[..4], bar);
                channel.setup.channel.muted = true;
            }
            add(
                &mut rows,
                format!("{count} muted ML-P8 holding chords"),
                project,
                0,
                floor,
            );
        }
    }

    if wants("instruments") {
        heading(
            &mut rows,
            "-- instruments, one channel, a chord held 3/4 of each bar --",
        );
        let instrument =
            |rows: &mut Vec<Row>, label: String, mut channel: ProjectChannel, notes: &[u8]| {
                chord(&mut channel, notes, bar);
                add(rows, label, solo(channel), 0, floor);
            };
        let sampler = |polyphony: u8| {
            let mut channel = ProjectChannel::sampler(0, 1);
            channel
                .setup
                .sampler_state_mut()
                .expect("a sampler")
                .params
                .polyphony = polyphony;
            channel
        };
        for note in [60u8, 48, 72] {
            instrument(
                &mut rows,
                format!("Sampler, 1 voice, note {note}"),
                sampler(1),
                &[note],
            );
        }
        instrument(
            &mut rows,
            "Sampler, 8 voices, 8 notes".into(),
            sampler(8),
            &CHORD,
        );
        for mode in StretchMode::all() {
            for ratio in [0.5f32, 2.0] {
                for (polyphony, notes) in [(1u8, &CHORD[..1]), (8, &CHORD[..])] {
                    let mut channel = sampler(polyphony);
                    let params = &mut channel.setup.sampler_state_mut().expect("a sampler").params;
                    params.stretch_enabled = true;
                    params.stretch_mode = mode;
                    params.stretch_ratio = ratio;
                    let label =
                        format!("Sampler stretch {mode:?} x{ratio}, {} voices", notes.len());
                    instrument(&mut rows, label, channel, notes);
                }
            }
        }
        instrument(
            &mut rows,
            "DrumSynth".into(),
            ProjectChannel::drum_synth(0, 1),
            &CHORD[..1],
        );
        instrument(
            &mut rows,
            "MonoSynth".into(),
            ProjectChannel::mono_synth(0, 1),
            &CHORD[..1],
        );
        for notes in [1usize, 8] {
            instrument(
                &mut rows,
                format!("PolySynth, {notes} notes"),
                ProjectChannel::poly_synth(0, 1),
                &CHORD[..notes],
            );
        }
        for patch in mooloop_core::ds01_factory::patches() {
            let channel = ProjectChannel::ds01_with_params(0, 1, patch.params);
            instrument(
                &mut rows,
                format!("DS-01 {}", patch.name),
                channel,
                &CHORD[..1],
            );
        }
        for patch in mooloop_core::mlm1_factory::patches() {
            let channel = ProjectChannel::mlm1_with_params(0, 1, patch.params);
            instrument(
                &mut rows,
                format!("ML-M1 {}", patch.name),
                channel,
                &CHORD[..1],
            );
        }
        for (notes, unison) in [
            (1usize, MlP8Unison::X1),
            (1, MlP8Unison::X2),
            (1, MlP8Unison::X4),
            (1, MlP8Unison::X8),
            (8, MlP8Unison::X1),
            (8, MlP8Unison::X8),
        ] {
            let params = MlP8Params {
                unison,
                ..MlP8Params::default()
            };
            let channel = ProjectChannel::mlp8_with_params(0, 1, params);
            instrument(
                &mut rows,
                format!("ML-P8 default, unison {unison:?}, {notes} notes"),
                channel,
                &CHORD[..notes],
            );
        }
        for chorus in [MlP8Chorus::One, MlP8Chorus::Two, MlP8Chorus::Ensemble] {
            let params = MlP8Params {
                chorus,
                ..MlP8Params::default()
            };
            let channel = ProjectChannel::mlp8_with_params(0, 1, params);
            instrument(
                &mut rows,
                format!("ML-P8 chorus {chorus:?}, 8 notes"),
                channel,
                &CHORD,
            );
        }
        {
            let closed = MlP8Params {
                filter_cutoff: 0.3,
                filter_resonance: 0.4,
                ..MlP8Params::default()
            };
            for unison in [MlP8Unison::X1, MlP8Unison::X8] {
                let params = MlP8Params { unison, ..closed };
                let channel = ProjectChannel::mlp8_with_params(0, 1, params);
                instrument(
                    &mut rows,
                    format!("ML-P8 filter closed, unison {unison:?}, 1 note"),
                    channel,
                    &CHORD[..1],
                );
            }
            let channel = ProjectChannel::mlp8_with_params(0, 1, closed);
            instrument(
                &mut rows,
                "ML-P8 filter closed, 8 notes".into(),
                channel,
                &CHORD,
            );
            let params = MlP8Params {
                drive: 0.5,
                ..MlP8Params::default()
            };
            let channel = ProjectChannel::mlp8_with_params(0, 1, params);
            instrument(
                &mut rows,
                "ML-P8 drive 0.5 (filter open), 8 notes".into(),
                channel,
                &CHORD,
            );
        }
        for patch in mooloop_core::mlp8_factory::patches() {
            let unison = patch.params.unison;
            for notes in [1usize, 8] {
                let channel = ProjectChannel::mlp8_with_params(0, 1, patch.params);
                instrument(
                    &mut rows,
                    format!("ML-P8 {} ({unison:?}), {notes} notes", patch.name),
                    channel,
                    &CHORD[..notes],
                );
            }
            let params = MlP8Params {
                unison: MlP8Unison::X8,
                ..patch.params
            };
            let channel = ProjectChannel::mlp8_with_params(0, 1, params);
            instrument(
                &mut rows,
                format!("ML-P8 {} unison X8, 1 note", patch.name),
                channel,
                &CHORD[..1],
            );
        }
    }

    // The reference an effect is read against: an ML-P8 holding a chord.
    let voiced = || {
        let mut channel = ProjectChannel::mlp8(0, 1);
        chord(&mut channel, &CHORD[..4], bar);
        solo(channel)
    };

    if wants("effects") {
        heading(
            &mut rows,
            "-- effects, after an ML-P8 holding a 4-note chord --",
        );
        let bare = add(&mut rows, "(ML-P8, no effect)".into(), voiced(), 0, 0);
        rows[bare].reference = bare;
        let with = |effect: EffectSlotState| {
            let mut project = voiced();
            project.channels[0].setup.push_effect(effect).expect("room");
            project
        };
        let kinds = [
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
            EffectKind::Buffer,
            EffectKind::Preamp,
            EffectKind::BusComp,
        ];
        for kind in kinds {
            let default = EffectSlotState::of_kind(kind);
            let wet = default.wet_dry;
            add(
                &mut rows,
                format!("{kind:?} default (wet {wet})"),
                with(default),
                0,
                bare,
            );
            let mut full = default;
            full.wet_dry = 1.0;
            add(
                &mut rows,
                format!("{kind:?} default, wet 1.0"),
                with(full),
                0,
                bare,
            );
            let mut half = default;
            half.wet_dry = 0.5;
            add(
                &mut rows,
                format!("{kind:?} default, wet 0.5"),
                with(half),
                0,
                bare,
            );
            full.bypassed = true;
            add(
                &mut rows,
                format!("{kind:?} default, wet 1.0, bypassed"),
                with(full),
                0,
                bare,
            );
            half.bypassed = true;
            add(
                &mut rows,
                format!("{kind:?} default, wet 0.5, bypassed"),
                with(half),
                0,
                bare,
            );
            for patch in mooloop_core::effect_factory::patches(kind) {
                let wet = patch.effect.wet_dry;
                add(
                    &mut rows,
                    format!("{kind:?} patch {} (wet {wet})", patch.name),
                    with(patch.effect),
                    0,
                    bare,
                );
            }
        }
        for mode in [
            ModulationMode::Chorus,
            ModulationMode::Flange,
            ModulationMode::Ensemble,
            ModulationMode::Adt,
        ] {
            let effect = EffectSlotState::modulation(ModulationParams {
                mode,
                ..ModulationParams::default()
            });
            add(
                &mut rows,
                format!("Modulation {mode:?}, wet 1.0"),
                with(effect),
                0,
                bare,
            );
        }
        for stages in [4u8, 8, 12] {
            let effect = EffectSlotState::modulation(ModulationParams {
                mode: ModulationMode::Phaser,
                stages,
                ..ModulationParams::default()
            });
            add(
                &mut rows,
                format!("Modulation Phaser {stages} stages, wet 1.0"),
                with(effect),
                0,
                bare,
            );
        }
        for display in [false, true] {
            let mut effect = EffectSlotState::of_kind(EffectKind::Preamp);
            if let EffectParams::Preamp(params) = &mut effect.params {
                params.display_enabled = display;
            }
            add(
                &mut rows,
                format!("Preamp, display_enabled {display}"),
                with(effect),
                0,
                bare,
            );
        }
        // The EQ by how many stages it runs, flat and not.
        for (label, bands, gain) in [
            ("EQ, no band on", 0usize, 0.0f32),
            ("EQ, 1 bell on at 0 dB", 1, 0.0),
            ("EQ, 1 bell on at -4 dB", 1, -4.0),
            ("EQ, 7 bells on at 0 dB", 7, 0.0),
            ("EQ, 7 bells on at -4 dB", 7, -4.0),
        ] {
            let mut effect = EffectSlotState::of_kind(EffectKind::Eq);
            if let EffectParams::Eq(params) = &mut effect.params {
                for (index, band) in params.bands.iter_mut().enumerate() {
                    band.enabled = index < bands;
                    band.kind = EqBandKind::Bell;
                    band.gain_db = gain;
                }
                params.high_pass.enabled = false;
                params.low_pass.enabled = false;
            }
            add(&mut rows, label.into(), with(effect), 0, bare);
        }
    }

    if wants("modulation") {
        heading(
            &mut rows,
            "-- the control pass: modulators, routes and lanes on ML-P8 + Filter --",
        );
        let filtered = || {
            let mut project = voiced();
            let device = project.channels[0]
                .setup
                .push_effect(EffectSlotState::of_kind(EffectKind::Filter))
                .expect("room");
            (project, device)
        };
        let reference = add(
            &mut rows,
            "(ML-P8 + Filter, nothing driven)".into(),
            filtered().0,
            0,
            0,
        );
        rows[reference].reference = reference;
        let target = EffectTarget::Channel(0);
        let kinds: [(&str, ModulatorParams); 5] = [
            ("LFO", ModulatorParams::Lfo(ModLfoParams::default())),
            (
                "Envelope",
                ModulatorParams::Envelope(ModEnvelopeParams::default()),
            ),
            ("Step", ModulatorParams::Step(ModStepParams::default())),
            (
                "Random",
                ModulatorParams::Random(ModRandomParams::default()),
            ),
            ("Math", ModulatorParams::Math(ModMathParams::default())),
        ];
        for (name, params) in kinds {
            let (mut project, device) = filtered();
            let rack = &mut project.channels[0].setup.modulation;
            rack.install(0, ModulatorParams::Lfo(ModLfoParams::default()));
            rack.install(1, params);
            rack.add_route(ModRoute::to_slot(
                1,
                ParamAddr::effect(target, device, FILTER_PARAM_CUTOFF_HZ),
                0.4,
                ModPolarity::Bipolar,
            ))
            .expect("room in the matrix");
            add(
                &mut rows,
                format!("LFO + 1 {name} -> filter cutoff"),
                project,
                0,
                reference,
            );
        }
        // Every modulator slot filled and routed to the Filter's parameters
        // until the matrix is full: the widest a channel's control pass gets.
        {
            let (mut project, device) = filtered();
            let rack = &mut project.channels[0].setup.modulation;
            let slots = mooloop_core::modulation::MAX_MODULATORS_PER_CHANNEL;
            for slot in 0..slots {
                rack.install(
                    slot,
                    ModulatorParams::Lfo(ModLfoParams {
                        rate_hz: 0.5 + slot as f32,
                        ..ModLfoParams::default()
                    }),
                );
            }
            let mut routes = 0;
            'fill: for descriptor in EffectKind::Filter.descriptors() {
                for slot in 0..slots {
                    let route = ModRoute::to_slot(
                        slot as u8,
                        ParamAddr::effect(target, device, descriptor.id),
                        0.1,
                        ModPolarity::Bipolar,
                    );
                    if rack.add_route(route).is_none() {
                        break 'fill;
                    }
                    routes += 1;
                }
            }
            add(
                &mut rows,
                format!("{slots} LFOs, {routes} routes -> filter"),
                project,
                0,
                reference,
            );
        }
        // Automation: one lane per Filter parameter, each moving across the
        // bar.
        for lanes in [1usize, 4] {
            let (mut project, device) = filtered();
            for (index, descriptor) in EffectKind::Filter
                .descriptors()
                .iter()
                .take(lanes)
                .enumerate()
            {
                let mut lane =
                    AutomationLane::new(ParamAddr::effect(target, device, descriptor.id));
                lane.reserve_points();
                lane.reset_points([
                    AutomationPoint::new(1, 0, 0.2),
                    AutomationPoint::new(2, bar / 2, 0.8),
                    AutomationPoint::new(3, bar - 1, 0.3 + index as f32 * 0.1),
                ]);
                project.channels[0].automation[0].push(lane);
            }
            add(
                &mut rows,
                format!("{lanes} automation lanes on the filter"),
                project,
                0,
                reference,
            );
        }
    }

    // Keep what was asked for, and whatever it is read against.
    let mut keep: Vec<bool> = rows
        .iter()
        .map(|row| {
            row.project.is_none()
                || matching
                    .as_deref()
                    .is_none_or(|text| row.label.contains(text))
        })
        .collect();
    for index in 0..rows.len() {
        if keep[index] && rows[index].project.is_some() {
            keep[rows[index].reference] = true;
        }
    }

    // The sampler's sample: two seconds with content across the spectrum, so
    // a stretcher's splice search has something real to search.
    let sample = Arc::new(SampleData {
        frames: (0..2 * SAMPLE_RATE as usize)
            .map(|index| {
                let t = index as f32 / SAMPLE_RATE as f32;
                let value = (t * 220.0 * std::f32::consts::TAU).sin() * 0.4
                    + (t * 587.0 * std::f32::consts::TAU).sin() * 0.3
                    + (t * 1490.0 * std::f32::consts::TAU).sin() * 0.2;
                [value, value * 0.8]
            })
            .collect(),
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    });
    let samples: Vec<Option<Arc<SampleData>>> = vec![Some(sample)];

    let _ftz = crate::executor::FlushToZero::enable();
    let mut best: Vec<Vec<u64>> = rows.iter().map(|_| vec![u64::MAX; blocks]).collect();
    for _ in 0..reps {
        for (index, row) in rows.iter().enumerate() {
            let (Some(project), true) = (&row.project, keep[index]) else {
                continue;
            };
            let mut render = RenderState::from_project(SAMPLE_RATE, project, &samples);
            render.play();
            for _ in 0..row.warmup_blocks {
                render.process_block(FRAMES);
            }
            for slot in best[index].iter_mut() {
                let started = Instant::now();
                render.process_block(FRAMES);
                *slot = (*slot).min(started.elapsed().as_nanos() as u64);
            }
        }
    }

    let mean = |index: usize| best[index].iter().sum::<u64>() as f64 / blocks as f64 / 1e3;
    println!();
    println!("  {FRAMES}-frame blocks, two bars, per-block min of {reps} round-robin passes, us");
    for (index, row) in rows.iter().enumerate() {
        if !keep[index] {
            continue;
        }
        if row.project.is_none() {
            println!("\n  {}", row.label);
            continue;
        }
        let worst = *best[index].iter().max().unwrap_or(&0) as f64 / 1e3;
        println!(
            "  {:<52} mean {:>7.1}  max {worst:>7.1}  over {:>7.1}",
            row.label,
            mean(index),
            mean(index) - mean(row.reference)
        );
    }
}

/// **What timing every channel and bus costs** (MOO-236): the claim is under
/// 1% of a 128-frame block with 32 channels.
///
/// Thirty-two ML-P8 channels, each holding a note for the whole measured
/// stretch, rendered with site timing off and on. Each pass builds both
/// states fresh and times them block by block, alternately, so the two see
/// the same blocks under the same load on a shared box; each block's figure
/// is its minimum over the passes, and the overhead is the mean difference
/// of those minima. Timing on also covers the costliest-three pick, which
/// the executor makes only on a block that ran long: it is run on every
/// block here, so the figure is an upper bound.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn site_timing_cost() {
    const FRAMES: usize = 128;
    const CHANNELS: usize = 32;
    // 180 blocks is 480 ms, inside the half-second note, so every channel is
    // sounding in every measured block.
    const BLOCKS: usize = 180;
    const PASSES: usize = 25;
    let project = loaded_project(CHANNELS);
    // Each block's minimum, timing off and on.
    let mut best = [[u64::MAX; 2]; BLOCKS];
    for _ in 0..PASSES {
        // Boxed: two render states side by side are a lot of stack.
        let mut states = [false, true].map(|timed| {
            let mut render = Box::new(RenderState::from_project(SAMPLE_RATE, &project, &[]));
            render.set_site_timing(timed);
            render.play();
            render
        });
        for block in &mut best {
            for (index, render) in states.iter_mut().enumerate() {
                let started = Instant::now();
                render.process_block(FRAMES);
                if index == 1 {
                    std::hint::black_box(render.costliest_sites());
                }
                block[index] = block[index].min(started.elapsed().as_nanos() as u64);
            }
        }
    }
    let mean = |index: usize| best.iter().map(|block| block[index]).sum::<u64>() as f64 / BLOCKS as f64;
    let budget = FRAMES as f64 / SAMPLE_RATE as f64 * 1e9;
    let (off, on) = (mean(0), mean(1));
    println!();
    println!("  {CHANNELS} channels, {FRAMES}-frame blocks, per-block min of {PASSES} interleaved passes");
    println!("  timing off {:>9.0} ns/block  {:>6.2}% of budget", off, off / budget * 100.0);
    println!("  timing on  {:>9.0} ns/block  {:>6.2}% of budget", on, on / budget * 100.0);
    println!(
        "  overhead   {:>9.0} ns/block  {:>6.3}% of budget",
        on - off,
        (on - off) / budget * 100.0
    );
}
