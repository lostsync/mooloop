//! Fit to tempo, and what turning SYNC off keeps: MOO-39's listening case.
//!
//! A 1.5 s loop of hits every quarter second, fitted to one bar. It's
//! rendered three ways over four bars:
//!
//! - **synced at 120 BPM**, where a bar is 2 s;
//! - **synced at 90 BPM**, where the fit follows the tempo to 2.67 s;
//! - **frozen, then played at 90 BPM**: SYNC was turned off at 120 BPM,
//!   which writes the ratio it was running into the Speed knob, so the loop
//!   stays 2 s long at the new tempo.
//!
//! Each song is saved, reopened with `load_bundle` (which must repair
//! nothing), and rendered offline. The loop's period is measured from the
//! render itself, by autocorrelating its 10 ms energy envelope, and printed
//! beside the bar length.
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/sampler-fit-freeze \
//!     cargo run -p mooloop-engine --example sampler_fit_freeze -- target/sampler-fit-freeze
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mooloop_core::{LoopMode, NoteEvent, Project, ProjectChannel, SampleReference, SamplerParams};
use mooloop_dsp::{SampleData, Sampler};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
const BARS: u32 = 4;
/// A sixteenth, in ticks.
const STEP: u32 = 24;

fn loop_frames() -> Vec<[f32; 2]> {
    let len = (SAMPLE_RATE as f32 * 1.5) as usize;
    (0..len)
        .map(|n| {
            let t = n as f32 / SAMPLE_RATE as f32;
            // A louder hit at the top, so the loop has one period, not six.
            let accent = if t < 0.25 { 1.0 } else { 0.4 };
            let value = accent
                * 0.6
                * (std::f32::consts::TAU * 180.0 * t).sin()
                * (-(t % 0.25) / 0.05).exp();
            [value, value]
        })
        .collect()
}

fn write_wav(path: &Path, frames: &[[f32; 2]]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("the loop writes");
    for frame in frames {
        writer.write_sample(frame[0]).expect("a sample");
        writer.write_sample(frame[1]).expect("a sample");
    }
    writer.finalize().expect("the loop finalizes");
}

fn song(sample: &Path, bpm: u16, params: SamplerParams) -> Project {
    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().expect("a sampler");
    state.sample = SampleReference::File {
        path: sample.to_path_buf(),
        embedded: false,
    };
    state.params = params;
    channel.setup.channel.volume = 1.0;
    channel.notes[0].push(NoteEvent::new(1, 0, BARS * 16 * STEP, 60, 127));
    Project {
        bpm,
        channels: vec![channel],
        pattern_lengths: vec![(BARS * 16) as u16],
        ..Project::default()
    }
}

fn reopen(project: &Project, path: &Path) -> Project {
    mooloop_project::save_song(path, project, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let report = mooloop_project::load_bundle(path).expect("the song reopens");
    assert!(report.repairs.is_empty(), "the loader repaired it: {:?}", report.repairs);
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        panic!("a song came back as something else");
    };
    reopened
}

fn render(project: &Project, sample: &Arc<SampleData>, path: &Path) -> Vec<f32> {
    OfflineRenderer::render(
        project,
        &[Some(sample.clone())],
        SAMPLE_RATE,
        &ExportSpec {
            path: path.to_path_buf(),
            scope: RenderScope::Pattern { index: 0 },
            tail_seconds: 0.0,
            format: ExportFormat::Wav(WavEncoding::Float32),
        },
    )
    .expect("the render");
    let mut reader = hound::WavReader::open(path).expect("the render reads back");
    let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.expect("a sample")).collect();
    samples.chunks(2).map(|pair| pair[0]).collect()
}

/// The loop's period in seconds: the lag between 1 s and 3.5 s at which
/// the 10 ms energy envelope best matches itself.
fn period_seconds(signal: &[f32]) -> f64 {
    let window = SAMPLE_RATE as usize / 100;
    let envelope: Vec<f64> = signal
        .chunks(window)
        .map(|chunk| chunk.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>())
        .collect();
    let mean = envelope.iter().sum::<f64>() / envelope.len() as f64;
    let centred: Vec<f64> = envelope.iter().map(|e| e - mean).collect();
    let (mut best, mut best_lag) = (f64::MIN, 0);
    for lag in 100..350.min(centred.len() / 2) {
        let score: f64 = centred.iter().zip(&centred[lag..]).map(|(a, b)| a * b).sum::<f64>()
            / (centred.len() - lag) as f64;
        if score > best {
            best = score;
            best_lag = lag;
        }
    }
    best_lag as f64 / 100.0
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/sampler-fit-freeze".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");
    let frames = loop_frames();
    let wav = dir.join("loop.wav");
    write_wav(&wav, &frames);
    let sample = Arc::new(SampleData {
        frames,
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    });
    let synced = SamplerParams {
        loop_mode: LoopMode::Forward,
        loop_start: 0.0,
        loop_end: 1.0,
        output_gain: 1.0,
        stretch_enabled: true,
        stretch_sync: true,
        stretch_bars: 1.0,
        ..SamplerParams::default()
    };
    // What turning SYNC off at 120 BPM leaves in the knob: the session's
    // `set_stretch_sync` writes exactly this.
    let frozen = SamplerParams {
        stretch_sync: false,
        stretch_ratio: Sampler::synced_ratio(synced, &sample, 120.0, None),
        ..synced
    };
    for (name, bpm, params) in [
        ("synced-120", 120u16, synced),
        ("synced-90", 90, synced),
        ("frozen-at-120-played-at-90", 90, frozen),
    ] {
        let project = reopen(&song(&wav, bpm, params), &dir.join(format!("{name}.mooloop")));
        let out = render(&project, &sample, &dir.join(format!("{name}.wav")));
        let bar = mooloop_core::frames_per_bar(SAMPLE_RATE, f64::from(bpm)) / f64::from(SAMPLE_RATE);
        println!(
            "{name:>27}: the loop repeats every {:.2} s; a bar at {bpm} BPM is {bar:.2} s",
            period_seconds(&out)
        );
    }
    println!(
        "the knob SYNC left behind: {:.4}x",
        frozen.stretch_ratio
    );
}
