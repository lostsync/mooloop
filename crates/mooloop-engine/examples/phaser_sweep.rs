//! The Modulation phaser at its fastest rate and full depth, MOO-235's
//! listening case.
//!
//! MOO-235 moved the phaser's all-pass coefficients from an `exp2` and a
//! `tan` per stage per sample to a control rate (every 16 samples, a
//! straight line between). The claim is that it sounds the same. The hardest
//! case for that claim is the fastest sweep the knobs allow: Rate 12 Hz,
//! Depth 100%, 12 stages, heavy feedback, on something broadband.
//!
//! A sustained ML-P8 saw chord, with a Modulation device in Phaser mode at
//! half wet, two bars at 12 Hz and then two at 0.5 Hz so the sweep can be
//! heard as a sweep. Authored with the real `mooloop_core` types, saved with
//! `mooloop_project::save_song`, reopened with `load_bundle` (which must
//! repair nothing), and rendered offline to a float WAV:
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/phaser-sweep \
//!     cargo run -p mooloop-engine --example phaser_sweep -- target/phaser-sweep
//! ```
//!
//! Then play `target/phaser-sweep/phaser.wav` against `dry.wav`. The
//! difference from the old per-sample formula is measured in
//! `mooloop-dsp`'s `control_rate_phaser_matches_the_per_sample_formula`; this
//! render is for the ear (`docs/FOCUS.md`, "Listening is a step"). An agent
//! did not listen to it.

use std::path::{Path, PathBuf};

use mooloop_core::{
    EffectParams, EffectSlotState, MlP8Params, ModulationMode, ModulationParams, NoteEvent,
    OscWave, Project, ProjectChannel,
};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
/// A sixteenth, in ticks.
const STEP: u32 = 24;
const BARS: u32 = 2;
const CHORD: [u8; 4] = [45, 52, 57, 64];

fn song(rate_hz: f32, phaser: bool) -> Project {
    let mut params = MlP8Params::default();
    params.osc[0].wave = OscWave::Saw;
    params.attack = 0.005;
    params.sustain = 1.0;
    params.release = 0.2;
    params.master_volume = 0.5;
    let mut pad = ProjectChannel::mlp8_with_params(0, 1, params);
    for (n, pitch) in CHORD.into_iter().enumerate() {
        pad.notes[0].push(NoteEvent::new(n as u32 + 1, 0, BARS * 16 * STEP, pitch, 110));
    }
    pad.setup.channel.volume = 1.0;
    if phaser {
        let mut slot = EffectSlotState::new(EffectParams::Modulation(ModulationParams {
            mode: ModulationMode::Phaser,
            rate_hz,
            depth: 1.0,
            color: 0.5,
            feedback: 0.7,
            spread: 0.5,
            tone: 1.0,
            stages: 12,
            ..ModulationParams::default()
        }));
        slot.wet_dry = 0.5;
        pad.setup.effects.push(slot);
        mooloop_core::assign_device_ids(&mut pad.setup.effects, &mut pad.setup.next_device_id);
    }
    Project {
        bpm: 120,
        channels: vec![pad],
        pattern_lengths: vec![(BARS * 16) as u16],
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
    reader.samples::<f32>().map(|s| s.expect("a sample")).collect()
}

fn write(path: &Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("the joined file");
    for &sample in samples {
        writer.write_sample(sample).expect("a sample");
    }
    writer.finalize().expect("the joined file closes");
}

fn db(value: f32) -> f32 {
    20.0 * value.max(1.0e-12).log10()
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/phaser-sweep".into()));
    std::fs::create_dir_all(&dir).expect("the output directory");

    let fast = save_and_reopen(&song(12.0, true), &dir.join("phaser-fastest.mooloop"));
    let slow = save_and_reopen(&song(0.5, true), &dir.join("phaser-slow.mooloop"));
    let dry = save_and_reopen(&song(12.0, false), &dir.join("phaser-dry.mooloop"));
    println!("saved and reopened three songs with nothing repaired");

    let fast = render(&fast, &dir.join("fast.wav"));
    let slow = render(&slow, &dir.join("slow.wav"));
    let dry = render(&dry, &dir.join("dry-part.wav"));
    let _ = std::fs::remove_file(dir.join("dry-part.wav"));
    let joined: Vec<f32> = fast.iter().chain(&slow).copied().collect();
    write(&dir.join("phaser.wav"), &joined);
    let dry_twice: Vec<f32> = dry.iter().chain(&dry).copied().collect();
    write(&dir.join("dry.wav"), &dry_twice);

    let stats = |samples: &[f32]| {
        let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
        (db(peak), db(rms))
    };
    for (label, samples) in [("12 Hz", &fast), ("0.5 Hz", &slow), ("dry", &dry)] {
        let (peak, rms) = stats(samples);
        let finite = samples.iter().all(|s| s.is_finite());
        println!("{label:>6}: peak {peak:6.2} dBFS, rms {rms:6.2} dBFS, all finite: {finite}");
    }
    println!("wrote phaser.wav (two bars at 12 Hz, then two at 0.5 Hz) and dry.wav");
}
