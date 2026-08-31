#![allow(clippy::needless_range_loop)]
// Spectral code indexes several parallel arrays by bin number at once. The
// iterator rewrite clippy suggests needs a zip chain per loop and reads worse
// than `for k in 0..bins`, so the lint is off for this throwaway crate.

//! Comparison harness for issue #32.
//!
//! Run with `scripts/antibox cargo run --release -p mooloop-spike-time-stretch`.
//! Prints CSV sections to stdout and writes comparison renders to
//! `$STRETCH_SPIKE_OUT` (default `/tmp/stretch-spike`).

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use mooloop_spike_time_stretch::fixtures::{self, Fixture, SR};
use mooloop_spike_time_stretch::metrics;
use mooloop_spike_time_stretch::pvoc::{Pvoc, PvocConfig};
use mooloop_spike_time_stretch::wsola::{Wsola, WsolaConfig};
use mooloop_spike_time_stretch::{render_all, render_blocked, Source, Stretcher};

// ---------------------------------------------------------------------------
// Allocation detector. The realtime contract in docs/AUDIO_ARCHITECTURE.md
// forbids allocation on the audio thread, so "does render allocate" is a
// pass/fail gate, not a nice-to-have.
// ---------------------------------------------------------------------------

struct CountingAlloc;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static WATCHING: AtomicBool = AtomicBool::new(false);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if WATCHING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if WATCHING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

fn watch<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCS.store(0, Ordering::Relaxed);
    WATCHING.store(true, Ordering::Relaxed);
    let out = f();
    WATCHING.store(false, Ordering::Relaxed);
    (out, ALLOCS.load(Ordering::Relaxed))
}

// ---------------------------------------------------------------------------
// Candidates
// ---------------------------------------------------------------------------

type Factory = fn(f64) -> Box<dyn Stretcher>;

fn candidates() -> Vec<(&'static str, Factory)> {
    vec![
        ("wsola_fast", |r| Box::new(Wsola::new(WsolaConfig::FAST, r))),
        ("wsola_music", |r| {
            Box::new(Wsola::new(WsolaConfig::MUSIC, r))
        }),
        ("wsola_smooth", |r| {
            Box::new(Wsola::new(WsolaConfig::SMOOTH, r))
        }),
        ("wsola_break", |r| {
            Box::new(Wsola::new(WsolaConfig::BREAK, r))
        }),
        ("wsola_nosnap", |r| {
            Box::new(Wsola::new(WsolaConfig::NO_SNAP, r))
        }),
        ("pvoc_locked", |r| Box::new(Pvoc::new(PvocConfig::LOCKED, r))),
        ("pvoc_transient", |r| {
            Box::new(Pvoc::new(PvocConfig::LOCKED_TRANSIENT, r))
        }),
        ("pvoc_short", |r| Box::new(Pvoc::new(PvocConfig::SHORT, r))),
        ("pvoc_plain", |r| Box::new(Pvoc::new(PvocConfig::PLAIN, r))),
        ("pvoc_indep", |r| {
            Box::new(Pvoc::new(PvocConfig::INDEPENDENT, r))
        }),
    ]
}

const RATIOS: [f64; 12] = [
    0.25, 0.5, 0.667, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 2.0, 4.0,
];

fn out_dir() -> PathBuf {
    PathBuf::from(std::env::var("STRETCH_SPIKE_OUT").unwrap_or_else(|_| "/tmp/stretch-spike".into()))
}

// ---------------------------------------------------------------------------

struct Prepared {
    fixture: Fixture,
    onsets: Vec<usize>,
    src_mid: Vec<f32>,
    src_ltas: Vec<f32>,
    src_corr: f32,
    src_side_db: f32,
}

fn prepare(fixture: Fixture) -> Prepared {
    let onsets = metrics::onsets(&fixture.frames, SR);
    let src_mid = metrics::mid(&fixture.frames);
    let src_ltas = metrics::ltas(&src_mid, SR);
    let src_corr = metrics::stereo_correlation(&fixture.frames);
    let src_side_db = metrics::side_mid_db(&fixture.frames);
    Prepared {
        fixture,
        onsets,
        src_mid,
        src_ltas,
        src_corr,
        src_side_db,
    }
}

fn quality_matrix(prepared: &[Prepared]) {
    println!("## quality");
    println!(
        "fixture,candidate,ratio,out_frames,dur_err_frames,onset_matched,onset_missed,\
onset_spurious,onset_mean_err_ms,onset_max_err_ms,rise_ratio,crest_delta_db,ltas_dist_db,\
pitch_err_cents,tonal_purity_db,corr_delta,side_delta_db"
    );
    for p in prepared {
        let src = Source::whole(&p.fixture.frames, &p.onsets);
        // Source-side attack references, measured once.
        let src_rise: Vec<Option<f32>> = p
            .fixture
            .true_onsets
            .iter()
            .map(|&o| metrics::attack_rise_ms(&p.src_mid, o, SR))
            .collect();
        let src_crest: Vec<Option<f32>> = p
            .fixture
            .true_onsets
            .iter()
            .map(|&o| metrics::crest_db(&p.src_mid, o, SR, 40.0))
            .collect();

        for (name, factory) in candidates() {
            for ratio in RATIOS {
                let out_len = (p.fixture.frames.len() as f64 * ratio).round() as usize;
                let mut st = factory(ratio);
                let out = render_all(st.as_mut(), &src, 0, out_len);
                let out_mid = metrics::mid(&out);

                let detected = metrics::onsets(&out, SR);
                let score = metrics::score_onsets(
                    &p.fixture.true_onsets,
                    &detected,
                    ratio,
                    SR,
                    // Tolerance scales a little with the window sizes in play.
                    35.0,
                );

                let mut rise_num = 0.0f32;
                let mut rise_den = 0.0f32;
                let mut crest_sum = 0.0f32;
                let mut crest_n = 0usize;
                for (i, &o) in p.fixture.true_onsets.iter().enumerate() {
                    let at = (o as f64 * ratio).round() as usize;
                    if let (Some(sr_), Some(or_)) =
                        (src_rise[i], metrics::attack_rise_ms(&out_mid, at, SR))
                    {
                        if sr_ > 0.05 {
                            rise_num += or_;
                            rise_den += sr_;
                        }
                    }
                    if let (Some(sc), Some(oc)) = (
                        src_crest[i],
                        metrics::crest_db(&out_mid, at, SR, 40.0),
                    ) {
                        crest_sum += oc - sc;
                        crest_n += 1;
                    }
                }
                let rise_ratio = if rise_den > 0.0 {
                    rise_num / rise_den
                } else {
                    f32::NAN
                };
                let crest_delta = if crest_n > 0 {
                    crest_sum / crest_n as f32
                } else {
                    f32::NAN
                };

                let ltas_dist = metrics::ltas_distance_db(&p.src_ltas, &metrics::ltas(&out_mid, SR));
                let pitch_err = match p.fixture.f0 {
                    Some(f0) => metrics::dominant_hz(&out_mid, SR, 30.0)
                        .map(|hz| metrics::cents(hz, f0))
                        .unwrap_or(f32::NAN),
                    None => f32::NAN,
                };
                let purity = match p.fixture.f0 {
                    Some(f0) => metrics::tonal_purity_db(&out_mid, SR, f0).unwrap_or(f32::NAN),
                    None => f32::NAN,
                };
                let corr_delta = metrics::stereo_correlation(&out) - p.src_corr;
                let side_delta = metrics::side_mid_db(&out) - p.src_side_db;

                println!(
                    "{},{},{:.3},{},{},{},{},{},{:.2},{:.2},{:.3},{:.2},{:.2},{:.1},{:.2},{:.3},{:.2}",
                    p.fixture.name,
                    name,
                    ratio,
                    out.len(),
                    out.len() as i64 - out_len as i64,
                    score.matched,
                    score.missed,
                    score.spurious,
                    score.mean_abs_err_ms,
                    score.max_abs_err_ms,
                    rise_ratio,
                    crest_delta,
                    ltas_dist,
                    pitch_err,
                    purity,
                    corr_delta,
                    side_delta,
                );
            }
        }
    }
}

fn block_agreement(prepared: &Prepared) {
    println!();
    println!("## block_size_agreement (max abs sample difference vs one-shot render)");
    println!("candidate,ratio,block,max_abs_diff");
    let src = Source::whole(&prepared.fixture.frames, &prepared.onsets);
    for (name, factory) in candidates() {
        for ratio in [0.75f64, 1.0, 1.37, 2.0] {
            let out_len = (prepared.fixture.frames.len() as f64 * ratio).round() as usize;
            let mut reference_st = factory(ratio);
            let reference = render_all(reference_st.as_mut(), &src, 0, out_len);
            for block in [32usize, 64, 128, 256, 480, 512, 1024] {
                let mut st = factory(ratio);
                let got = render_blocked(st.as_mut(), &src, 0, out_len, block);
                let diff = reference
                    .iter()
                    .zip(got.iter())
                    .map(|(a, b)| (a[0] - b[0]).abs().max((a[1] - b[1]).abs()))
                    .fold(0.0f32, f32::max);
                println!("{name},{ratio:.2},{block},{diff:e}");
            }
        }
    }
}

fn allocation_gate(prepared: &Prepared) {
    println!();
    println!("## realtime_allocation_gate");
    println!("candidate,allocs_during_render,allocs_during_reset,allocs_during_set_ratio");
    let src = Source::whole(&prepared.fixture.frames, &prepared.onsets);
    for (name, factory) in candidates() {
        let mut st = factory(1.25);
        let mut buf = vec![[0.0f32; 2]; 256];
        // Warm the instance outside the watch window.
        st.render(&src, &mut buf);
        let (_, render_allocs) = watch(|| {
            for _ in 0..200 {
                st.render(&src, &mut buf);
            }
        });
        let (_, reset_allocs) = watch(|| st.reset(0));
        let (_, ratio_allocs) = watch(|| st.set_ratio(0.8));
        println!("{name},{render_allocs},{reset_allocs},{ratio_allocs}");
    }
}

fn cpu_budget(prepared: &Prepared) {
    println!();
    println!("## cpu (one voice, 48 kHz stereo, release build)");
    println!(
        "candidate,ratio,block,realtime_factor,mean_block_us,worst_block_us,\
budget_us,worst_pct_of_budget,state_bytes"
    );
    let src = Source::looped(&prepared.fixture.frames, &prepared.onsets);
    let seconds = 10.0f64;
    for (name, factory) in candidates() {
        for ratio in [0.8f64, 1.25] {
            for block in [128usize, 256] {
                let mut st = factory(ratio);
                st.reset(0);
                let mut buf = vec![[0.0f32; 2]; block];
                // Warmup.
                for _ in 0..200 {
                    st.render(&src, &mut buf);
                }
                let blocks = (seconds * SR as f64 / block as f64) as usize;
                let mut worst = 0u128;
                let start = Instant::now();
                for _ in 0..blocks {
                    let t = Instant::now();
                    st.render(&src, &mut buf);
                    let e = t.elapsed().as_nanos();
                    if e > worst {
                        worst = e;
                    }
                }
                let total = start.elapsed().as_secs_f64();
                let produced = (blocks * block) as f64 / SR as f64;
                let budget_us = block as f64 / SR as f64 * 1.0e6;
                let mean_us = total / blocks as f64 * 1.0e6;
                let worst_us = worst as f64 / 1000.0;
                println!(
                    "{},{:.2},{},{:.1},{:.2},{:.2},{:.1},{:.1},{}",
                    name,
                    ratio,
                    block,
                    produced / total,
                    mean_us,
                    worst_us,
                    budget_us,
                    worst_us / budget_us * 100.0,
                    st.state_bytes(),
                );
            }
        }
    }
}

fn voice_scaling(prepared: &Prepared) {
    println!();
    println!("## polyphony_budget (block 128 = 2667 us, ratio 1.25, one core)");
    println!("# Real instances rendering into the same block, so this includes");
    println!("# the cache pressure a per-voice state footprint actually causes.");
    println!("candidate,voices,mean_block_us,worst_block_us,mean_pct,worst_pct,total_state_kib");
    let src = Source::looped(&prepared.fixture.frames, &prepared.onsets);
    let block = 128usize;
    let budget_us = block as f64 / SR as f64 * 1.0e6;
    for (name, factory) in candidates() {
        for voices in [1usize, 4, 8, 16] {
            let mut pool: Vec<Box<dyn Stretcher>> = (0..voices).map(|_| factory(1.25)).collect();
            // Stagger the start positions so the voices are not producing their
            // internal frames on the same block, which is the realistic case.
            for (i, st) in pool.iter_mut().enumerate() {
                st.reset(i * 977);
            }
            let mut buf = vec![[0.0f32; 2]; block];
            for _ in 0..100 {
                for st in pool.iter_mut() {
                    st.render(&src, &mut buf);
                }
            }
            let blocks = 3000usize;
            let mut worst = 0u128;
            let start = Instant::now();
            for _ in 0..blocks {
                let t = Instant::now();
                for st in pool.iter_mut() {
                    st.render(&src, &mut buf);
                }
                let e = t.elapsed().as_nanos();
                if e > worst {
                    worst = e;
                }
            }
            let mean_us = start.elapsed().as_secs_f64() / blocks as f64 * 1.0e6;
            let worst_us = worst as f64 / 1000.0;
            let total_state: usize = pool.iter().map(|s| s.state_bytes()).sum();
            println!(
                "{},{},{:.1},{:.1},{:.1},{:.1},{}",
                name,
                voices,
                mean_us,
                worst_us,
                mean_us / budget_us * 100.0,
                worst_us / budget_us * 100.0,
                total_state / 1024,
            );
        }
    }
}

fn ratio_change() {
    println!();
    println!("## live_ratio_change");
    println!("# Measured on a looped 440 Hz tone, where the only thing that can");
    println!("# produce curvature at the change point is the algorithm itself.");
    println!("# `excess_db` is the glitch at the change minus the same statistic");
    println!("# at an untouched control point. Roughly: >6 dB is an audible click.");
    println!("candidate,from,to,seam_db,control_db,excess_db");
    let tone = fixtures::sine_tone();
    let onsets: Vec<usize> = Vec::new();
    let src = Source::looped(&tone.frames, &onsets);
    for (name, factory) in candidates() {
        for (from, to) in [(1.0f64, 1.5f64), (1.0, 0.75), (1.25, 1.26), (0.5, 2.0)] {
            let mut st = factory(from);
            st.reset(0);
            let half = SR as usize * 2;
            let mut out = vec![[0.0f32; 2]; half * 2];
            let block = 128usize;
            let mut pos = 0usize;
            while pos < out.len() {
                if pos == half {
                    st.set_ratio(to);
                }
                let n = block.min(out.len() - pos);
                st.render(&src, &mut out[pos..pos + n]);
                pos += n;
            }
            let m = metrics::mid(&out);
            let radius = 2048usize;
            let seam = metrics::glitch_db(&m, half, radius);
            let control = metrics::glitch_db(&m, half / 2, radius);
            println!("{name},{from:.2},{to:.2},{seam:.1},{control:.1},{:.1}", seam - control);
        }
    }
}

fn loop_seam() {
    println!();
    println!("## loop_seam");
    println!("# Stationary loop material, so any curvature spike at the wrap is");
    println!("# the algorithm's join and nothing else. Same excess_db reading.");
    println!("candidate,material,ratio,seam_db,control_db,excess_db");
    let materials = [fixtures::sine_tone(), fixtures::stereo_wide()];
    for material in &materials {
        let onsets: Vec<usize> = Vec::new();
        let src = Source::looped(&material.frames, &onsets);
        let loop_len = material.frames.len();
        for (name, factory) in candidates() {
            for ratio in [0.75f64, 1.0, 1.5] {
                let mut st = factory(ratio);
                let out_len = (loop_len as f64 * ratio * 2.6) as usize;
                let out = render_all(st.as_mut(), &src, 0, out_len);
                let m = metrics::mid(&out);
                let radius = 2048usize;
                let mut worst = f32::NEG_INFINITY;
                for k in 1..=2 {
                    let at = (loop_len as f64 * ratio * k as f64).round() as usize;
                    if at + radius < m.len() {
                        worst = worst.max(metrics::glitch_db(&m, at, radius));
                    }
                }
                // Control point deliberately half a loop away from any wrap.
                let control_at = (loop_len as f64 * ratio * 1.5).round() as usize;
                let control = if control_at + radius < m.len() {
                    metrics::glitch_db(&m, control_at, radius)
                } else {
                    0.0
                };
                println!(
                    "{},{},{:.2},{:.1},{:.1},{:.1}",
                    name,
                    material.name,
                    ratio,
                    worst,
                    control,
                    worst - control
                );
            }
        }
    }
}

fn note_on_ramp() {
    println!();
    println!("## note_on_ramp (time to reach 90% level on a steady tone at ratio 1)");
    println!("candidate,ramp_ms");
    let tone = fixtures::sine_tone();
    let onsets: Vec<usize> = Vec::new();
    let src = Source::whole(&tone.frames, &onsets);
    for (name, factory) in candidates() {
        let mut st = factory(1.0);
        let out = render_all(st.as_mut(), &src, 0, tone.frames.len());
        let m = metrics::mid(&out);
        println!("{},{:.2}", name, metrics::start_ramp_ms(&m, SR));
    }
}

fn prepare_cost() {
    println!();
    println!("## control_side_prepare_cost");
    println!("stage,input_seconds,ms,notes");
    let brk = fixtures::drum_break();
    let secs = brk.frames.len() as f64 / SR as f64;
    let t = Instant::now();
    let onsets = metrics::onsets(&brk.frames, SR);
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "onset_table,{:.2},{:.2},{} onsets detected vs {} placed",
        secs,
        ms,
        onsets.len(),
        brk.true_onsets.len()
    );

    // Cost of the prepared/cached architecture: render the whole sample once.
    for (name, factory) in candidates() {
        let src = Source::whole(&brk.frames, &onsets);
        let ratio = 1.25f64;
        let out_len = (brk.frames.len() as f64 * ratio) as usize;
        let mut st = factory(ratio);
        let t = Instant::now();
        let out = render_all(st.as_mut(), &src, 0, out_len);
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "prepared_render[{}],{:.2},{:.2},{} KiB cached per (sample x ratio)",
            name,
            secs,
            ms,
            out.len() * 8 / 1024
        );
    }
}

fn write_renders(prepared: &[Prepared]) {
    let dir = out_dir();
    println!();
    println!("## renders written to {}", dir.display());
    let listen = ["drum_break", "percussive_oneshot", "mixed_loop", "bass_line"];
    let listen_ratios = [0.75f64, 1.25, 2.0];
    let listen_candidates = [
        "wsola_music",
        "wsola_nosnap",
        "wsola_fast",
        "pvoc_locked",
        "pvoc_transient",
    ];
    for p in prepared {
        if !listen.contains(&p.fixture.name) {
            continue;
        }
        metrics::write_wav(
            &dir.join(format!("{}__source.wav", p.fixture.name)),
            &p.fixture.frames,
            SR,
        );
        let src = Source::whole(&p.fixture.frames, &p.onsets);
        for (name, factory) in candidates() {
            if !listen_candidates.contains(&name) {
                continue;
            }
            for ratio in listen_ratios {
                let out_len = (p.fixture.frames.len() as f64 * ratio).round() as usize;
                let mut st = factory(ratio);
                let out = render_all(st.as_mut(), &src, 0, out_len);
                metrics::write_wav(
                    &dir.join(format!(
                        "{}__{}__r{:.2}.wav",
                        p.fixture.name,
                        name,
                        ratio
                    )),
                    &out,
                    SR,
                );
            }
        }
    }
    println!("done");
}

fn main() {
    let prepared: Vec<Prepared> = fixtures::all().into_iter().map(prepare).collect();

    println!("# mooloop time-stretch spike (issue #32)");
    println!("sample_rate={SR}");
    println!();
    println!("## fixtures");
    println!("name,frames,seconds,placed_onsets,detected_onsets");
    for p in &prepared {
        println!(
            "{},{},{:.3},{},{}",
            p.fixture.name,
            p.fixture.frames.len(),
            p.fixture.frames.len() as f64 / SR as f64,
            p.fixture.true_onsets.len(),
            p.onsets.len()
        );
    }
    println!();

    quality_matrix(&prepared);

    let brk = prepared
        .iter()
        .find(|p| p.fixture.name == "drum_break")
        .expect("drum_break fixture");
    block_agreement(brk);
    allocation_gate(brk);
    cpu_budget(brk);
    voice_scaling(brk);
    ratio_change();
    loop_seam();
    note_on_ramp();
    prepare_cost();
    write_renders(&prepared);
}
