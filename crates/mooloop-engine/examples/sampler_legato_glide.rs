//! The sampler's mono glide and legato, MOO-45's listening case.
//!
//! A long sine sampled at A3 is played as a one-voice line whose first
//! three notes overlap (A3, E4, A4, E4) and whose last starts after a gap
//! (A3). It is rendered three ways:
//!
//! - `legato.wav`: Glide 80 ms, Env trig Legato. The overlapping notes slide
//!   into each other with no new attack; the note after the gap starts
//!   fresh, at its own pitch.
//! - `retrig.wav`: Glide 80 ms, Env trig Retrig. The same slides, but every
//!   note restarts the sample and the envelope.
//! - `none.wav`: no glide, Retrig: the sampler as every song saved before
//!   MOO-45 plays it.
//!
//! Authored with the real `mooloop_core` types, saved with
//! `mooloop_project::save_song`, reopened with `load_bundle` (which must
//! repair nothing), and rendered offline to float WAVs. Then measured,
//! because an agent cannot listen: the pitch in 5 ms windows across the
//! first overlap (how long the slide takes), and the level across it
//! (whether an attack restarted).
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/sampler-legato-glide \
//!     cargo run -p mooloop-engine --example sampler_legato_glide -- target/sampler-legato-glide
//! ```
//!
//! Then play the three (`docs/FOCUS.md`, "Listening is a step").

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mooloop_core::mlm1::{EnvTrigger, GlideMode};
use mooloop_core::{
    NoteEvent, Project, ProjectChannel, SampleReference, SamplerParams, VoiceMode,
};
use mooloop_dsp::SampleData;
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
/// A sixteenth, in ticks.
const STEP: u32 = 24;
/// A sixteenth at 120 BPM, in frames.
const STEP_FRAMES: usize = SAMPLE_RATE as usize / 8;
const ROOT: u8 = 57;
const ROOT_HZ: f32 = 220.0;
/// Where the first overlap's note starts, in frames: the fourth sixteenth.
const FIRST_OVERLAP: usize = 4 * STEP_FRAMES;

fn sine(seconds: f32) -> Vec<[f32; 2]> {
    let len = (SAMPLE_RATE as f32 * seconds) as usize;
    (0..len)
        .map(|n| {
            let value = 0.5 * (std::f32::consts::TAU * ROOT_HZ * n as f32 / SAMPLE_RATE as f32).sin();
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
    let mut writer = hound::WavWriter::create(path, spec).expect("the sample writes");
    for frame in frames {
        writer.write_sample(frame[0]).expect("a sample");
        writer.write_sample(frame[1]).expect("a sample");
    }
    writer.finalize().expect("the sample finalizes");
}

fn song(sample: &Path, glide: f32, env_trigger: EnvTrigger) -> Project {
    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().expect("a sampler");
    state.sample = SampleReference::File {
        path: sample.to_path_buf(),
        embedded: false,
    };
    state.params = SamplerParams {
        root_note: ROOT,
        attack: 0.005,
        decay: 1.0,
        sustain: 1.0,
        release: 0.05,
        output_gain: 1.0,
        voice_mode: VoiceMode::Gate,
        polyphony: 1,
        glide,
        glide_mode: GlideMode::Legato,
        env_trigger,
        ..SamplerParams::default()
    };
    channel.setup.channel.volume = 1.0;
    // (start step, length in steps, note). The first four overlap by two
    // steps each; the last starts a step after the fourth ends.
    let line = [(0, 6, 57), (4, 6, 64), (8, 6, 69), (12, 4, 64), (17, 7, 57)];
    for (id, (start, length, pitch)) in line.into_iter().enumerate() {
        channel.notes[0].push(NoteEvent::new(
            id as u32 + 1,
            start * STEP,
            length * STEP,
            pitch,
            110,
        ));
    }
    Project {
        bpm: 120,
        channels: vec![channel],
        pattern_lengths: vec![32],
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

/// The pitch in Hz over `window` frames from `at`, from the time between
/// its first and last upward zero crossings.
fn pitch(signal: &[f32], at: usize, window: usize) -> f32 {
    let span = &signal[at..(at + window).min(signal.len())];
    let crossings: Vec<f32> = span
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] <= 0.0 && pair[1] > 0.0)
        .map(|(index, pair)| index as f32 + pair[0] / (pair[0] - pair[1]))
        .collect();
    match (crossings.first(), crossings.last()) {
        (Some(first), Some(last)) if crossings.len() > 1 => {
            (crossings.len() - 1) as f32 * SAMPLE_RATE as f32 / (last - first)
        }
        _ => 0.0,
    }
}

fn rms(signal: &[f32], at: usize, window: usize) -> f32 {
    let span = &signal[at..(at + window).min(signal.len())];
    (span.iter().map(|s| s * s).sum::<f32>() / span.len().max(1) as f32).sqrt()
}

fn db(ratio: f32) -> f32 {
    20.0 * ratio.max(1.0e-9).log10()
}

/// How long the first overlap's slide takes, in ms: from the last 20 ms
/// window still within 2% of A3 to the first within 2% of E4, centre to
/// centre, stepping 2.5 ms. A window straddling an instant jump reads as
/// neither, so an unglided note change measures about one window (20 ms):
/// that is this measurement's floor, not a slide.
fn slide_ms(signal: &[f32]) -> f32 {
    let window = SAMPLE_RATE as usize / 50;
    let hop = SAMPLE_RATE as usize / 400;
    let from = ROOT_HZ;
    let to = ROOT_HZ * 2.0_f32.powf(7.0 / 12.0);
    let mut left = None;
    for index in 0..120 {
        let at = FIRST_OVERLAP - window + index * hop;
        let hz = pitch(signal, at, window);
        if (hz - from).abs() < from * 0.02 {
            left = Some(at);
        }
        if (hz - to).abs() < to * 0.02 {
            return left.map_or(f32::NAN, |start| {
                at.saturating_sub(start) as f32 * 1000.0 / SAMPLE_RATE as f32
            });
        }
    }
    f32::NAN
}

/// The deepest dip in level across the first overlap, in dB under the
/// level before it: a restarted attack shows as a dip. 10 ms windows (two
/// periods of A3, so a steady tone reads steady) stepped a millisecond at a
/// time through the 30 ms around the note change.
fn dip_db(signal: &[f32]) -> f32 {
    let ms = SAMPLE_RATE as usize / 1000;
    let before = rms(signal, FIRST_OVERLAP - 40 * ms, 20 * ms);
    let lowest = (0..30)
        .map(|index| rms(signal, FIRST_OVERLAP - 15 * ms + index * ms, 10 * ms))
        .fold(f32::MAX, f32::min);
    db(lowest / before)
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/sampler-legato-glide".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");
    let frames = sine(16.0);
    let wav = dir.join("sine-a3.wav");
    write_wav(&wav, &frames);
    let sample = Arc::new(SampleData {
        frames,
        sample_rate: SAMPLE_RATE,
        root_note: ROOT,
    });
    for (name, glide, trigger) in [
        ("legato", 0.08, EnvTrigger::Legato),
        ("retrig", 0.08, EnvTrigger::Retrig),
        ("none", 0.0, EnvTrigger::Retrig),
    ] {
        let project = reopen(&song(&wav, glide, trigger), &dir.join(format!("{name}.mooloop")));
        let signal = render(&project, &sample, &dir.join(format!("{name}.wav")));
        let fresh = pitch(&signal, 17 * STEP_FRAMES + SAMPLE_RATE as usize / 100, 1_920);
        println!(
            "{name:>6}: the first overlap slides in {:5.1} ms, its level dips {:6.1} dB, \
             and the note after the gap starts at {fresh:5.1} Hz (A3 is {ROOT_HZ} Hz)",
            slide_ms(&signal),
            dip_db(&signal),
        );
    }
    println!("saved, reopened with nothing repaired, and rendered three ways");
}
