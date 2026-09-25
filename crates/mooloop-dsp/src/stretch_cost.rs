//! What stretching costs, in voices that fit in real time.
//!
//! `#[ignore]`d like the rest of the measurement tests; run deliberately, in
//! release:
//!
//! ```sh
//! cargo test -p mooloop-dsp --release stretch_cost -- --ignored --nocapture
//! ```
//!
//! The number that matters is the realtime factor. A `StretchReader` runs on
//! the audio thread while a stretched voice sounds, and `StretchPool` builds
//! one for every one of the sixteen sampler voices — so a factor of `N` means
//! `N` voices saturate a core before anything else in the block has run.

use std::time::Instant;

use crate::interpolate::Region;
use crate::stretch::{render_stretched, StretchReader};
use mooloop_core::StretchMode;

const SR: u32 = 48_000;

fn source(seconds: f32) -> Vec<[f32; 2]> {
    let frames = (seconds * SR as f32) as usize;
    (0..frames)
        .map(|index| {
            let t = index as f32 / SR as f32;
            // Something with content across the spectrum, so the correlation
            // search has a real signal to work on rather than a pure tone it
            // could match anywhere.
            let value = (t * 220.0 * std::f32::consts::TAU).sin() * 0.4
                + (t * 587.0 * std::f32::consts::TAU).sin() * 0.3
                + (t * 1490.0 * std::f32::consts::TAU).sin() * 0.2;
            [value, value * 0.8]
        })
        .collect()
}

/// The realtime reader, which is what the audio thread runs per voice.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn stretch_reader_cost() {
    let frames = source(4.0);
    let region = Region::whole(frames.len());
    println!();
    println!("  mode     ratio   ns/frame   realtime x   voices in real time");
    for mode in [StretchMode::Music, StretchMode::Drums, StretchMode::Grain] {
        for ratio in [1.0f64, 2.0, 8.0] {
            let mut reader = StretchReader::new(mode, SR);
            reader.stretcher_mut().set_ratio(ratio);
            reader.reset(0.0);
            // Warm the window before timing.
            for _ in 0..SR as usize / 10 {
                reader.read(&frames, region, 1.0);
            }
            let count = SR as usize * 2;
            let started = Instant::now();
            for _ in 0..count {
                std::hint::black_box(reader.read(&frames, region, 1.0));
            }
            let elapsed = started.elapsed();
            let per_frame = elapsed.as_nanos() as f64 / count as f64;
            let realtime = 1.0e9 / SR as f64 / per_frame;
            println!(
                "  {:<8} {ratio:>5.1}  {per_frame:>9.1}  {realtime:>10.1}  {realtime:>19.1}",
                format!("{mode:?}")
            );
        }
    }
}

/// The offline render, which is what a commit pays on the UI thread.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn stretch_render_cost() {
    println!();
    println!("  mode      seconds   ratio    ms      ms per source second");
    for mode in [StretchMode::Music, StretchMode::Drums, StretchMode::Grain] {
        for seconds in [1.0f32, 4.0] {
            let frames = source(seconds);
            let region = Region::whole(frames.len());
            let started = Instant::now();
            let render = render_stretched(&frames, region, mode, 40, 1.5, SR);
            let elapsed = started.elapsed();
            std::hint::black_box(&render);
            println!(
                "  {:<8} {seconds:>8.0}  {:>6.1}  {:>7.1}  {:>21.1}",
                format!("{mode:?}"),
                1.5,
                elapsed.as_secs_f64() * 1000.0,
                elapsed.as_secs_f64() * 1000.0 / seconds as f64
            );
        }
    }
}

/// What a stretching sampler channel reserves before it plays a note.
///
/// `StretchPool::new` builds a reader for every voice, and each reader holds
/// its own accumulator, ready buffer, search buffer and resampling scratch.
/// `docs/CAPACITY_POLICY.md` is about exactly this distinction — a ceiling
/// costs nothing, dimensioning by one costs a great deal — so this prints the
/// product rather than leaving it to be multiplied out at some later
/// surprise.
#[test]
#[ignore = "prints a footprint; run deliberately"]
fn stretch_pool_footprint() {
    use crate::stretch::StretchPool;
    println!();
    println!("  mode      per reader   16 voices   4 voices");
    for mode in [StretchMode::Music, StretchMode::Drums, StretchMode::Grain] {
        let one = StretchReader::new(mode, SR).state_bytes();
        let sixteen = StretchPool::new(mode, SR, 16).state_bytes();
        let four = StretchPool::new(mode, SR, 4).state_bytes();
        println!(
            "  {:<8}  {:>9}  {:>10}  {:>9}",
            format!("{mode:?}"),
            one,
            sixteen,
            four
        );
    }
}

/// What a stretching chord costs **per 128-frame block**, mean and worst
/// (MOO-248). The realtime factor above hides the thing that drops out: a
/// hop's splice search used to land whole in one block, and voices started
/// together searched in the same block.
///
/// Every row is played once per pass, `STRETCH_BLOCK_REPS` passes (default
/// 7), round-robin; each block's cost is its fastest over the passes, so a
/// burst of another process's work on a shared machine is stripped out and
/// every row is compared across the same stretch of time.
///
/// ```sh
/// cargo test -p mooloop-dsp --release stretch_block_cost -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn stretch_block_cost() {
    const BLOCK: usize = 128;
    const BLOCKS: usize = 750; // 2 s at 48 kHz
    // A chord's playback rates, as eight sampler voices on different keys.
    const RATES: [f64; 8] = [1.0, 1.2599, 1.4983, 2.0, 0.7492, 0.8909, 1.1225, 1.6818];
    let frames = source(2.0);
    let region = Region {
        start: 0.0,
        end: frames.len() as f64,
        edge: crate::interpolate::RegionEdge::Wrap,
    };
    let rows: Vec<(StretchMode, f64, usize)> = vec![
        (StretchMode::Music, 2.0, 1),
        (StretchMode::Music, 2.0, 8),
        (StretchMode::Music, 0.5, 8),
        (StretchMode::Drums, 2.0, 8),
        (StretchMode::Grain, 2.0, 8),
    ];
    let reps: usize = std::env::var("STRETCH_BLOCK_REPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(7);
    let mut best = vec![vec![u128::MAX; BLOCKS]; rows.len()];
    for _ in 0..reps {
        for (row, &(mode, ratio, voices)) in rows.iter().enumerate() {
            let mut readers: Vec<StretchReader> =
                (0..voices).map(|_| StretchReader::new(mode, SR)).collect();
            for reader in readers.iter_mut() {
                reader.stretcher_mut().set_ratio(ratio);
                reader.reset(0.0);
            }
            for block in 0..BLOCKS {
                let started = Instant::now();
                for (voice, reader) in readers.iter_mut().enumerate() {
                    for _ in 0..BLOCK {
                        std::hint::black_box(reader.read(&frames, region, RATES[voice]));
                    }
                }
                let nanos = started.elapsed().as_nanos();
                best[row][block] = best[row][block].min(nanos);
            }
        }
    }
    println!();
    println!("  row                        mean µs   worst µs   worst/mean");
    for (row, &(mode, ratio, voices)) in rows.iter().enumerate() {
        let mean = best[row].iter().sum::<u128>() as f64 / BLOCKS as f64 / 1000.0;
        let worst = *best[row].iter().max().unwrap() as f64 / 1000.0;
        println!(
            "  {:<26} {mean:>8.1}  {worst:>9.1}  {:>10.2}",
            format!("{mode:?} x{ratio}, {voices} voices"),
            worst / mean
        );
    }
}
