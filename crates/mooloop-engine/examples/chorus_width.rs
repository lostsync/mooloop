//! The Modulation chorus's Width and Mix, MOO-245's listening case.
//!
//! MOO-245 gave the Modulation device a Width: a mid/side scale on the wet
//! signal, from both sides' voices folded to the centre (0) to as wide as
//! the mode makes them (1, today's sound). Its Mix is the slot's own
//! wet/dry, now labelled so on the face.
//!
//! A sustained ML-P8 saw chord through a chorus at full Spread and half
//! Mix, two bars each at Width 100%, 50% and 0%, then the chorus at 100%
//! Mix and full width, then dry. Authored with the real `mooloop_core`
//! types, saved with `mooloop_project::save_song`, reopened with
//! `load_bundle` (which must repair nothing), and rendered offline to a
//! float WAV:
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/chorus-width \
//!     cargo run -p mooloop-engine --example chorus_width -- target/chorus-width
//! ```
//!
//! Then play `target/chorus-width/chorus-width.wav`. Each section's side
//! level against its mid is printed; the width stage is pinned in
//! `mooloop-dsp`'s `zero_width_makes_the_wet_mono` and
//! `full_width_passes_the_wet_pair_through_to_the_bit`. This render is for
//! the ear (`docs/FOCUS.md`, "Listening is a step"). An agent did not listen
//! to it.

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

/// A section's chorus Width and slot Mix, or none for dry.
type Setting = Option<(f32, f32)>;

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
    if let Some((width, mix)) = effect {
        let mut slot = EffectSlotState::new(EffectParams::Modulation(ModulationParams {
            mode: ModulationMode::Chorus,
            rate_hz: 0.6,
            depth: 0.6,
            color: 0.5,
            feedback: 0.2,
            spread: 1.0,
            tone: 0.9,
            width,
            ..ModulationParams::default()
        }));
        slot.wet_dry = mix;
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

/// Side over mid, in dB, of an interleaved stereo render.
fn side_over_mid_db(samples: &[f32]) -> f32 {
    let (mut mid, mut side) = (0.0f64, 0.0f64);
    for &[l, r] in samples.as_chunks::<2>().0 {
        let (l, r) = (f64::from(l), f64::from(r));
        mid += (0.5 * (l + r)).powi(2);
        side += (0.5 * (l - r)).powi(2);
    }
    db(((side / mid.max(1.0e-30)).sqrt()) as f32)
}

fn main() {
    let dir =
        PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/chorus-width".into()));
    std::fs::create_dir_all(&dir).expect("the output directory");

    let sections: [(&str, &str, Setting); 5] = [
        ("width 100%", "width-100", Some((1.0, 0.5))),
        ("width 50%", "width-50", Some((0.5, 0.5))),
        ("width 0%", "width-0", Some((0.0, 0.5))),
        ("mix 100%", "mix-100", Some((1.0, 1.0))),
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
            "{label:>10}: peak {:6.2} dBFS, rms {:6.2} dBFS, side/mid {:7.2} dB, all finite: {finite}",
            db(peak),
            db(rms),
            side_over_mid_db(&samples)
        );
        joined.extend_from_slice(&samples);
    }
    println!("saved and reopened five songs with nothing repaired");
    write(&dir.join("chorus-width.wav"), &joined);
    println!(
        "wrote chorus-width.wav: two bars each of width 100%, 50%, 0% at half mix, \
         full mix, dry"
    );
}
