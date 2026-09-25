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
//! looking at more closely.
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

    let mut best: Vec<u64> = Vec::new();
    let mut allocating: Vec<usize> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    for _ in 0..reps {
        let costs = time_through_executor(
            &project,
            &resolved.samples,
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
