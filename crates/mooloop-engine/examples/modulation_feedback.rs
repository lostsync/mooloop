//! The Modulation device at high feedback, MOO-200's listening case.
//!
//! MOO-200 trims the Modulation device's wet output past Feedback 75%, so a
//! feedback resonance peaks no higher than +12 dB (it reached +22 dB at the
//! knob's end). Below 75% nothing changed. Above it the wet is turned down
//! by up to 9.9 dB, at 92%, while the loop itself is untouched, so the
//! resonance still sharpens and rings as long as it did. The claim is that a
//! flanger at full feedback still sounds like a jet, only not 22 dB louder.
//!
//! A sustained ML-P8 saw chord, two bars through each of: a Flanger at 50%,
//! 75%, 92% and -92% feedback, a Phaser at 92%, and dry, all at half wet.
//! Authored with the real `mooloop_core` types, saved with
//! `mooloop_project::save_song`, reopened with `load_bundle` (which must
//! repair nothing), and rendered offline to a float WAV:
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/modulation-feedback \
//!     cargo run -p mooloop-engine --example modulation_feedback -- target/modulation-feedback
//! ```
//!
//! Then play `target/modulation-feedback/modulation-feedback.wav`. The gain
//! across the spectrum is measured in `mooloop-dsp`'s
//! `every_insert_kind_stays_under_its_gain_bound` and the ring in
//! `a_flanger_at_full_feedback_still_rings`; this render is for the ear
//! (`docs/FOCUS.md`, "Listening is a step"). An agent did not listen to it.

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

/// The mode and feedback of a section's Modulation device, or none for dry.
type Setting = Option<(ModulationMode, f32)>;

fn song(effect: Setting) -> Project {
    let mut params = MlP8Params::default();
    params.osc[0].wave = OscWave::Saw;
    params.attack = 0.005;
    params.sustain = 1.0;
    params.release = 0.2;
    // Low enough that no section reaches the safety limiter, so every
    // level printed is the device's own.
    params.master_volume = 0.2;
    let mut pad = ProjectChannel::mlp8_with_params(0, 1, params);
    for (n, pitch) in CHORD.into_iter().enumerate() {
        pad.notes[0].push(NoteEvent::new(n as u32 + 1, 0, BARS * 16 * STEP, pitch, 110));
    }
    pad.setup.channel.volume = 1.0;
    if let Some((mode, feedback)) = effect {
        let mut slot = EffectSlotState::new(EffectParams::Modulation(ModulationParams {
            mode,
            rate_hz: 0.25,
            depth: 0.8,
            color: 0.5,
            feedback,
            spread: 0.5,
            tone: 1.0,
            stages: 8,
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
    let dir = PathBuf::from(
        std::env::args().nth(1).unwrap_or_else(|| "target/modulation-feedback".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");

    let sections: [(&str, &str, Setting); 6] = [
        ("flanger 50%", "flange-50", Some((ModulationMode::Flange, 0.5))),
        ("flanger 75%", "flange-75", Some((ModulationMode::Flange, 0.75))),
        ("flanger 92%", "flange-92", Some((ModulationMode::Flange, 0.92))),
        ("flanger -92%", "flange-neg-92", Some((ModulationMode::Flange, -0.92))),
        ("phaser 92%", "phaser-92", Some((ModulationMode::Phaser, 0.92))),
        ("dry", "dry", None),
    ];
    let mut joined = Vec::new();
    for (label, stem, effect) in sections {
        let project = save_and_reopen(&song(effect), &dir.join(format!("{stem}.mooloop")));
        let part = dir.join(format!("{stem}-part.wav"));
        let samples = render(&project, &part);
        let _ = std::fs::remove_file(&part);
        let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
        let finite = samples.iter().all(|s| s.is_finite());
        println!(
            "{label:>12}: peak {:6.2} dBFS, rms {:6.2} dBFS, all finite: {finite}",
            db(peak),
            db(rms)
        );
        joined.extend_from_slice(&samples);
    }
    println!("saved and reopened six songs with nothing repaired");
    write(&dir.join("modulation-feedback.wav"), &joined);
    println!(
        "wrote modulation-feedback.wav: two bars each of flanger 50%, 75%, 92%, -92%, \
         phaser 92%, dry"
    );
}
