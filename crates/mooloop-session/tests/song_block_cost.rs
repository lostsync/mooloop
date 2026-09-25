//! What real songs cost, callback by callback, and where.
//!
//! MOO-195 cost 40% of a block for two bars of one song and nothing anywhere
//! else. Nothing allocated, so the soak tests were green, and `block_cost.rs`
//! times synthetic projects nobody had drawn a lane in. It was found by
//! playing a real song through the executor, timing every callback, and
//! lining the expensive ones up against the bar they fell in. This is that,
//! kept.
//!
//! `#[ignore]`d, because it measures wall time and reads songs from outside
//! the tree. Run it deliberately, in release, on a quiet machine:
//!
//! ```sh
//! SONGS=path/to/songs cargo test -p mooloop-session --release \
//!   --test song_block_cost -- --ignored --nocapture
//! ```
//!
//! `SONGS` is a directory of `.mooloop` files or a `:`-separated list of
//! them; a song's samples are decoded the way opening it decodes them, so a
//! sampler plays what it plays live. `REPS` (default 5) is how many times
//! each song is played: each callback's cost is the fastest of them, which
//! strips out whatever else the machine was doing. `BLOCK` (default 128) is
//! the callback size. `SONG_COST_OUT`, if set, is a directory to write one
//! TSV per song into -- frame, bar, nanoseconds, allocator calls -- for
//! looking at more closely. `ATTRIBUTE`, a `,`-separated list of song
//! names, re-plays each of those with every channel muted in turn and with
//! the bus inserts removed, to say which channel a spike belongs to.
//! `ATTRIBUTE_BY` picks `channels`, `kinds` (every device of one kind
//! bypassed in turn) or both, the default. `DISPLAYS=off` plays every song
//! with its saved displays switched off.
//!
//! **Read it as a map, not a verdict.** A bar that costs more than the bars
//! around it is a place to look: something the song does there is expensive,
//! and whether it should be is the question. Any allocator call inside a
//! callback is a bug outright.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::{Path, PathBuf};

use mooloop_core::{PlaybackMode, TICKS_PER_BAR};
use mooloop_engine::live_check::time_through_executor;
use mooloop_project::LoadedDocument;
use mooloop_session::document::resolve_document;

/// Allocator calls on this thread -- allocations, frees and reallocations
/// alike, because the callback's contract is that it makes none of any.
struct Counting;

thread_local! {
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

fn count() {
    let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count();
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn allocator_calls() -> usize {
    CALLS.try_with(Cell::get).unwrap_or(0)
}

const SAMPLE_RATE: u32 = 48_000;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn songs() -> Vec<PathBuf> {
    let Ok(spec) = std::env::var("SONGS") else {
        return Vec::new();
    };
    let mut songs: Vec<PathBuf> = spec
        .split(':')
        .map(PathBuf::from)
        .flat_map(|path| {
            if path.is_dir() {
                std::fs::read_dir(&path)
                    .expect("SONGS names a readable directory")
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| path.extension().is_some_and(|ext| ext == "mooloop"))
                    .collect()
            } else {
                vec![path]
            }
        })
        .collect();
    songs.sort();
    songs
}

fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    sorted[((sorted.len() - 1) as f64 * fraction).round() as usize]
}

fn profile(path: &Path, reps: usize, block: usize) {
    let name = path.file_stem().unwrap_or_default().to_string_lossy();
    let resolved = match resolve_document(path) {
        Ok(resolved) => resolved,
        Err(problem) => {
            println!("{name}: does not open: {}", problem.one_line());
            return;
        }
    };
    let LoadedDocument::Song(mut project) = resolved.report.document else {
        println!("{name}: not a song");
        return;
    };
    // The whole arrangement, once, whatever the song was left looping.
    project.loop_range.enabled = false;
    if std::env::var("DISPLAYS").is_ok_and(|displays| displays == "off") {
        project = displays_off(&project);
    }
    let song_mode = project.playback_mode == PlaybackMode::Song && !project.playlist.is_empty();
    let length_ticks = if song_mode {
        let end = project
            .playlist
            .iter()
            .filter_map(|placement| {
                let steps = *project.pattern_lengths.get(usize::from(placement.pattern))?;
                Some(placement.start_tick + u32::from(steps) * mooloop_core::TICKS_PER_STEP)
            })
            .max()
            .unwrap_or(TICKS_PER_BAR);
        end.div_ceil(TICKS_PER_BAR) * TICKS_PER_BAR
    } else {
        // A pattern-mode song is its current pattern, played four times.
        project.playback_mode = PlaybackMode::Pattern;
        let steps = project
            .pattern_lengths
            .get(usize::from(project.current_pattern))
            .copied()
            .unwrap_or(16);
        4 * u32::from(steps) * mooloop_core::TICKS_PER_STEP
    };
    let ticks_per_frame =
        f64::from(project.bpm) / 60.0 * f64::from(project.ppq) / f64::from(SAMPLE_RATE);
    let frames = (f64::from(length_ticks) / ticks_per_frame).ceil() as usize;

    let Measured {
        best,
        allocating,
        starts,
    } = measure(&project, &resolved.samples, frames, reps, block);

    let bar_of = |frame: usize| frame as f64 * ticks_per_frame / f64::from(TICKS_PER_BAR);
    let budget = block as f64 / f64::from(SAMPLE_RATE) * 1e9;
    let mut sorted = best.clone();
    sorted.sort_unstable();
    let mean = best.iter().sum::<u64>() as f64 / best.len() as f64;
    let max = *sorted.last().unwrap_or(&0);
    let allocating_blocks = allocating.iter().filter(|calls| **calls > 0).count();
    println!(
        "\n== {name}: {} bars, {} callbacks of {block}{}",
        length_ticks / TICKS_PER_BAR,
        best.len(),
        if song_mode { ", song mode" } else { ", pattern mode x4" }
    );
    println!(
        "   mean {:.0} us ({:.1}%)  p50 {} us  p99 {} us  max {} us ({:.1}% of budget)  \
         callbacks calling the allocator: {allocating_blocks}",
        mean / 1e3,
        mean / budget * 100.0,
        percentile(&sorted, 0.5) / 1000,
        percentile(&sorted, 0.99) / 1000,
        max / 1000,
        max as f64 / budget * 100.0,
    );

    // One row per bar: mean and worst, and a mark on bars that stand out from
    // the song's own typical bar.
    let bars = (length_ticks / TICKS_PER_BAR) as usize;
    let mut sum = vec![0u64; bars];
    let mut worst = vec![0u64; bars];
    let mut count = vec![0u64; bars];
    for (index, nanos) in best.iter().enumerate() {
        let bar = (bar_of(starts[index]) as usize).min(bars.saturating_sub(1));
        sum[bar] += nanos;
        worst[bar] = worst[bar].max(*nanos);
        count[bar] += 1;
    }
    let bar_means: Vec<u64> = (0..bars).map(|bar| sum[bar] / count[bar].max(1)).collect();
    let mut typical = bar_means.clone();
    typical.sort_unstable();
    let median = typical.get(typical.len() / 2).copied().unwrap_or(0).max(1);
    let row: Vec<String> = (0..bars)
        .map(|bar| {
            let mark = if bar_means[bar] * 4 > median * 5 { "*" } else { " " };
            format!("{}:{}/{}{mark}", bar + 1, bar_means[bar] / 1000, worst[bar] / 1000)
        })
        .collect();
    for chunk in row.chunks(8) {
        println!("   {}", chunk.join("  "));
    }
    let mut order: Vec<usize> = (0..best.len()).collect();
    order.sort_by_key(|&index| std::cmp::Reverse(best[index]));
    let top: Vec<String> = order
        .iter()
        .take(6)
        .map(|&index| format!("bar {:.2}: {} us", bar_of(starts[index]) + 1.0, best[index] / 1000))
        .collect();
    println!("   costliest: {}", top.join(", "));
    let first_allocating: Vec<String> = (0..best.len())
        .filter(|&index| allocating[index] > 0)
        .take(6)
        .map(|index| format!("bar {:.2}: {} calls", bar_of(starts[index]) + 1.0, allocating[index]))
        .collect();
    if !first_allocating.is_empty() {
        println!("   allocating: {}", first_allocating.join(", "));
    }

    let attributed = std::env::var("ATTRIBUTE")
        .is_ok_and(|names| names.split(',').any(|wanted| wanted == name));
    if attributed {
        attribute(&project, &resolved.samples, frames, reps, block);
    }

    if let Ok(out) = std::env::var("SONG_COST_OUT") {
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out).expect("SONG_COST_OUT can be created");
        let mut tsv = String::from("frame\tbar\tnanos\tallocator_calls\n");
        for index in 0..best.len() {
            tsv.push_str(&format!(
                "{}\t{:.4}\t{}\t{}\n",
                starts[index],
                bar_of(starts[index]) + 1.0,
                best[index],
                allocating[index]
            ));
        }
        std::fs::write(out.join(format!("{name}.tsv")), tsv).expect("the TSV is written");
    }
}

/// Each callback's fastest time over `reps` plays, the most allocator calls
/// any play made in it, and the frame it started on.
struct Measured {
    best: Vec<u64>,
    allocating: Vec<usize>,
    starts: Vec<usize>,
}

fn measure(
    project: &mooloop_core::Project,
    samples: &[Option<std::sync::Arc<mooloop_dsp::SampleData>>],
    frames: usize,
    reps: usize,
    block: usize,
) -> Measured {
    let mut best: Vec<u64> = Vec::new();
    let mut allocating: Vec<usize> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    for _ in 0..reps {
        let costs = time_through_executor(
            project,
            samples,
            SAMPLE_RATE,
            frames,
            block,
            allocator_calls,
        );
        if best.is_empty() {
            best = vec![u64::MAX; costs.len()];
            allocating = vec![0; costs.len()];
            starts = costs.iter().map(|cost| cost.frame).collect();
        }
        for (index, cost) in costs.iter().enumerate() {
            best[index] = best[index].min(cost.nanos);
            allocating[index] = allocating[index].max(cost.allocations);
        }
    }

    Measured {
        best,
        allocating,
        starts,
    }
}

/// Median, 99th percentile and worst, in microseconds, and the spike: how
/// far the costliest 1% of callbacks sit above the median.
fn shape(best: &[u64]) -> (u64, u64, u64, u64) {
    let mut sorted = best.to_vec();
    sorted.sort_unstable();
    let median = percentile(&sorted, 0.5);
    let top = &sorted[sorted.len() - (sorted.len() / 100).max(1)..];
    let spike = (top.iter().sum::<u64>() / top.len() as u64).saturating_sub(median);
    (
        median / 1000,
        percentile(&sorted, 0.99) / 1000,
        sorted[sorted.len() - 1] / 1000,
        spike / 1000,
    )
}

fn all_effects(project: &mooloop_core::Project) -> impl Iterator<Item = &mooloop_core::EffectSlotState> {
    project
        .channels
        .iter()
        .flat_map(|channel| channel.setup.effects.iter())
        .chain(project.buses.iter().flat_map(|bus| bus.effects.iter()))
}

fn all_effects_mut(
    project: &mut mooloop_core::Project,
) -> impl Iterator<Item = &mut mooloop_core::EffectSlotState> {
    project
        .channels
        .iter_mut()
        .flat_map(|channel| channel.setup.effects.iter_mut())
        .chain(project.buses.iter_mut().flat_map(|bus| bus.effects.iter_mut()))
}

/// `project` with every saved display switched off: the Preamp's band display
/// and the EQ's analyzer. These are view settings saved with a song, and each
/// runs on the audio thread for as long as it is on, looked at or not.
fn displays_off(project: &mooloop_core::Project) -> mooloop_core::Project {
    let mut unwatched = project.clone();
    for effect in all_effects_mut(&mut unwatched) {
        match &mut effect.params {
            mooloop_core::EffectParams::Preamp(params) => params.display_enabled = false,
            mooloop_core::EffectParams::Eq(params) => params.analyzer_enabled = false,
            _ => {}
        }
    }
    unwatched
}

/// Which channel carries a song's cost: the song again with each channel
/// muted in turn, and once with every bus's inserts removed. A channel whose
/// muting takes the spike away is where to look; mute is used rather than
/// removal so every other channel keeps its seat and its routing.
///
/// **The variants are played round-robin**: every variant once per pass,
/// `reps` passes, each callback's cost the fastest of its passes. Played one
/// variant at a time, a burst of someone else's work on a shared machine
/// lands on every pass of one variant and reads as that variant's cost --
/// on 2026-09-25 "displays off" and "mute Kick" both read 60-100 us *dearer*
/// than the song as saved that way. Interleaved, a burst lands on one pass
/// of many variants and the minimum removes it. `saves` is the as-saved
/// mean less the variant's.
fn attribute(
    project: &mooloop_core::Project,
    samples: &[Option<std::sync::Arc<mooloop_dsp::SampleData>>],
    frames: usize,
    reps: usize,
    block: usize,
) {
    let mut variants: Vec<(String, mooloop_core::Project)> = vec![
        ("(as saved)".into(), project.clone()),
        ("displays off".into(), displays_off(project)),
    ];
    let by = std::env::var("ATTRIBUTE_BY").unwrap_or_else(|_| "channels,kinds".into());
    if by.split(',').any(|by| by == "kinds") {
        // Every device of one kind bypassed, song-wide. Bypass rather than
        // removal so a container's span still names the rows it did; and a
        // bypassed device should cost nothing once it has faded out, so a
        // kind whose bypass saves nothing is itself worth a look.
        let mut kinds: Vec<mooloop_core::EffectKind> = Vec::new();
        for effect in all_effects(project) {
            if !kinds.contains(&effect.params.kind()) {
                kinds.push(effect.params.kind());
            }
        }
        for kind in kinds {
            let mut bypassed = project.clone();
            let mut count = 0;
            for effect in all_effects_mut(&mut bypassed) {
                if effect.params.kind() == kind && !effect.bypassed {
                    effect.bypassed = true;
                    count += 1;
                }
            }
            variants.push((format!("bypass {count} {kind:?}"), bypassed));
        }
    }
    let channels = by.split(',').any(|by| by == "channels");
    for (index, channel) in project.channels.iter().enumerate().filter(|_| channels) {
        let mut muted = project.clone();
        muted.channels[index].setup.channel.muted = true;
        let effects: Vec<String> = channel
            .setup
            .effects
            .iter()
            .map(|effect| format!("{:?}", effect.params.kind()))
            .collect();
        let label = format!(
            "mute {} {} [{:?}{}{}]",
            index,
            channel.setup.channel.name,
            channel.setup.channel.kind,
            if effects.is_empty() { "" } else { ": " },
            effects.join(",")
        );
        variants.push((label, muted));
    }
    if channels {
        for (index, bus) in project.buses.iter().enumerate() {
            if bus.effects.is_empty() {
                continue;
            }
            let mut bare = project.clone();
            bare.buses[index].effects.clear();
            let effects: Vec<String> = bus
                .effects
                .iter()
                .map(|effect| format!("{:?}", effect.params.kind()))
                .collect();
            variants.push((
                format!("no inserts on bus {index} [{}]", effects.join(",")),
                bare,
            ));
        }
        let mut bare = project.clone();
        for bus in &mut bare.buses {
            bus.effects.clear();
        }
        variants.push(("no bus inserts".into(), bare));
    }

    let mut best: Vec<Vec<u64>> = vec![Vec::new(); variants.len()];
    for _ in 0..reps {
        for ((_, variant), best) in variants.iter().zip(best.iter_mut()) {
            let measured = measure(variant, samples, frames, 1, block);
            if best.is_empty() {
                *best = measured.best;
            } else {
                for (kept, nanos) in best.iter_mut().zip(measured.best) {
                    *kept = (*kept).min(nanos);
                }
            }
        }
    }
    let mean_of = |best: &[u64]| best.iter().sum::<u64>() / best.len().max(1) as u64 / 1000;
    let saved = mean_of(&best[0]);
    for ((label, _), best) in variants.iter().zip(&best) {
        let (median, p99, max, spike) = shape(best);
        let mean = mean_of(best);
        println!(
            "   {label:<34} mean {mean:>5}  p50 {median:>5}  p99 {p99:>5}  max {max:>5}  \
             spike {spike:>5}  saves {:>5} us",
            saved as i64 - mean as i64
        );
    }
}

#[test]
#[ignore]
fn song_block_cost() {
    let songs = songs();
    if songs.is_empty() {
        println!("SONGS is unset or empty: nothing to profile");
        return;
    }
    let reps = env_usize("REPS", 5).max(1);
    let block = env_usize("BLOCK", 128).max(1);
    for song in &songs {
        profile(song, reps, block);
    }
}
