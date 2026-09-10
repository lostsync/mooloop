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
#[ignore = "measures live allocation; run alone, in release, with --test-threads=1"]
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
    #[cfg(unix)]
    fn request_realtime() -> bool {
        let param = libc::sched_param { sched_priority: 55 };
        // SAFETY: `param` outlives the call and `sched_setscheduler` reads it
        // without retaining it. Pid 0 is the calling thread.
        unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) == 0 }
    }
    #[cfg(not(unix))]
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
