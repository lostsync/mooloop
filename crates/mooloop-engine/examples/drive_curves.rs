//! The Drive after MOO-250 and MOO-251, their listening case.
//!
//! The Drive's 2x oversampler became a 31-tap half-band, with a rational
//! `tanh` inside it. The claim is that it sounds as it did: 64-70 dB from
//! the old path on the smooth curves, and on Hard Clip a difference that is
//! the two kernels' aliasing (45 dB down, with the new aliasing no louder).
//!
//! A sustained ML-P8 saw chord, one bar each through the default Drive (Soft
//! at 2), Tape Warmth and Hard Clip, then dry. Authored with the real
//! `mooloop_core` types, saved with `mooloop_project::save_song`, reopened
//! with `load_bundle` (which must repair nothing), and rendered offline:
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/drive-curves \
//!     cargo run -p mooloop-engine --example drive_curves -- target/drive-curves
//! ```
//!
//! Then play `target/drive-curves/drive.wav` (`docs/FOCUS.md`, "Listening is
//! a step"). An agent did not listen to it.

use std::path::{Path, PathBuf};

use mooloop_core::{
    DriveCurve, DriveParams, EffectParams, EffectSlotState, MlP8Params, NoteEvent, OscWave,
    Project, ProjectChannel,
};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
/// A sixteenth, in ticks.
const STEP: u32 = 24;
const CHORD: [u8; 4] = [45, 52, 57, 64];

fn song(drive: Option<DriveParams>) -> Project {
    let mut params = MlP8Params::default();
    params.osc[0].wave = OscWave::Saw;
    params.attack = 0.005;
    params.sustain = 1.0;
    params.release = 0.2;
    params.master_volume = 0.5;
    let mut pad = ProjectChannel::mlp8_with_params(0, 1, params);
    for (n, pitch) in CHORD.into_iter().enumerate() {
        pad.notes[0].push(NoteEvent::new(n as u32 + 1, 0, 16 * STEP, pitch, 110));
    }
    pad.setup.channel.volume = 1.0;
    if let Some(drive) = drive {
        pad.setup.effects.push(EffectSlotState::new(EffectParams::Drive(drive)));
        mooloop_core::assign_device_ids(&mut pad.setup.effects, &mut pad.setup.next_device_id);
    }
    Project {
        bpm: 120,
        channels: vec![pad],
        pattern_lengths: vec![16],
        ..Project::default()
    }
}

fn save_and_reopen(project: &Project, path: &Path) -> Project {
    mooloop_project::save_song(path, project, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let report = mooloop_project::load_bundle(path).expect("the song reopens");
    assert!(report.repairs.is_empty(), "the loader repaired it: {:?}", report.repairs);
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        panic!("a song came back as something else");
    };
    reopened
}

/// Both channels of a float render, interleaved.
fn render(project: &Project, path: &Path) -> Vec<f32> {
    OfflineRenderer::render(
        project,
        &[],
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
    let samples = reader.samples::<f32>().map(|s| s.expect("a sample")).collect();
    let _ = std::fs::remove_file(path);
    samples
}

fn db(value: f32) -> f32 {
    20.0 * value.max(1.0e-12).log10()
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/drive-curves".into()));
    std::fs::create_dir_all(&dir).expect("the output directory");
    let with = |curve, drive, tone, mix| DriveParams {
        curve,
        drive,
        tone,
        mix,
        ..DriveParams::default()
    };
    let sections = [
        ("default (Soft 2)", Some(DriveParams::default())),
        ("Tape Warmth", Some(with(DriveCurve::Tape, 3.0, -0.2, 0.7))),
        ("Hard Clip", Some(with(DriveCurve::Hard, 12.0, 0.0, 1.0))),
        ("dry", None),
    ];
    let mut joined = Vec::new();
    for (n, (label, drive)) in sections.into_iter().enumerate() {
        let project = save_and_reopen(&song(drive), &dir.join(format!("drive-{n}.mooloop")));
        let samples = render(&project, &dir.join(format!("part-{n}.wav")));
        let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        let finite = samples.iter().all(|s| s.is_finite());
        println!("{label:>16}: peak {:6.2} dBFS, all finite: {finite}", db(peak));
        joined.extend(samples);
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(dir.join("drive.wav"), spec).expect("drive.wav");
    for sample in joined {
        writer.write_sample(sample).expect("a sample");
    }
    writer.finalize().expect("drive.wav closes");
    println!("saved and reopened four songs with nothing repaired; wrote drive.wav");
}
