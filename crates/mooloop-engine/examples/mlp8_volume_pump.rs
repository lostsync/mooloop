//! ML-P8's Volume under modulation, MOO-214's listening case.
//!
//! A held A-minor chord on an ML-P8 of pure sines, its Volume pumped by a
//! channel LFO at a quarter note, the way a sidechain pump is built in
//! mooloop. Before MOO-214 the Volume was read raw once per control tick
//! (32 frames), so the pump was a staircase with a step every 0.67 ms: a
//! zipper at 1.5 kHz, heard as a buzz on every duck.
//!
//! Authored with the real `mooloop_core` types, saved with
//! `mooloop_project::save_song`, reopened with `load_bundle` (which must
//! repair nothing), and rendered offline to a float WAV. Then measured,
//! because an agent cannot listen: a chord of pure sines below 400 Hz has
//! nothing of its own near 1.5 kHz, so the zipper's sidebands, at 1.5 kHz
//! plus and minus each chord tone, are measured against the tone itself.
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/mlp8-volume-pump \
//!     cargo run -p mooloop-engine --example mlp8_volume_pump -- target/mlp8-volume-pump
//! ```
//!
//! Then play `target/mlp8-volume-pump/pump.wav`. `steady.wav` is the same
//! chord with no pump, for comparison (`docs/FOCUS.md`, "Listening is a
//! step").
//!
//! Measured 2026-09-24: the loudest zipper line sits 67 dB under its tone
//! with Volume read raw, and 100 dB under it smoothed. Unpumped, the floor
//! there is about 155 dB down.

use std::path::{Path, PathBuf};

use mooloop_core::{
    EffectTarget, MlP8Params, ModLfoParams, ModLfoWaveform, ModPolarity, ModRoute, ModSourceId,
    ModSourceRef, ModulatorParams, NoteEvent, OscWave, ParamAddr, ParamOwner, Project,
    ProjectChannel,
};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
/// A sixteenth, in ticks.
const STEP: u32 = 24;
const BARS: u32 = 4;
const CHORD: [u8; 3] = [57, 60, 64];
/// The control rate a generator hears modulation at: one value per
/// `CONTROL_RATE_FRAMES` (32) frames.
const ZIPPER_HZ: f32 = SAMPLE_RATE as f32 / 32.0;

fn song(pumped: bool) -> Project {
    let mut params = MlP8Params::default();
    params.osc[0].wave = OscWave::Sine;
    params.attack = 0.005;
    params.sustain = 1.0;
    params.release = 0.2;
    params.master_volume = 0.6;
    let mut pad = ProjectChannel::mlp8_with_params(0, 1, params);
    for (n, pitch) in CHORD.into_iter().enumerate() {
        pad.notes[0].push(NoteEvent::new(n as u32 + 1, 0, BARS * 16 * STEP, pitch, 110));
    }
    pad.setup.channel.volume = 1.0;
    if pumped {
        let rack = &mut pad.setup.modulation;
        rack.install(
            0,
            ModulatorParams::Lfo(ModLfoParams {
                waveform: ModLfoWaveform::Sine,
                // A quarter note at 120 BPM.
                rate_hz: 2.0,
                depth: 1.0,
                ..ModLfoParams::default()
            }),
        )
        .expect("a fresh rack has a free slot");
        rack.add_route(ModRoute {
            source: ModSourceRef::Id(ModSourceId(0)),
            source_slot: 0,
            destination: ParamAddr {
                scope: EffectTarget::Channel(0),
                owner: ParamOwner::Source,
                param: mooloop_core::mlp8::PARAM_MASTER_VOLUME,
            },
            depth: -0.4,
            polarity: ModPolarity::Bipolar,
        })
        .expect("Volume is a legal destination");
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

/// The left channel of a float render.
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
    let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.expect("a sample")).collect();
    samples.chunks(2).map(|pair| pair[0]).collect()
}

fn db(ratio: f32) -> f32 {
    20.0 * ratio.max(1.0e-12).log10()
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/mlp8-volume-pump".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");

    let pumped = save_and_reopen(&song(true), &dir.join("mlp8-volume-pump.mooloop"));
    println!("saved and reopened the song with nothing repaired");
    let steady = save_and_reopen(&song(false), &dir.join("mlp8-steady.mooloop"));

    let pump = render(&pumped, &dir.join("pump.wav"));
    let still = render(&steady, &dir.join("steady.wav"));
    // Skip the first half second: the attack, and the LFO's first cycle.
    let settle = SAMPLE_RATE as usize / 2;
    let measure = |signal: &[f32], hz: f32| {
        mooloop_dsp::testkit::tone_amplitude(&signal[settle..], SAMPLE_RATE, hz)
    };
    // The staircase's error is a 1.5 kHz sawtooth whose size and sign follow
    // the LFO's slope, so it lands beside each sideband at odd multiples of
    // the LFO's rate rather than on it (the slope averages to zero). Every
    // line out to 30 Hz either side is read, and the loudest is the answer.
    let lines = |signal: &[f32], hz: f32| {
        let mut loudest = 0.0_f32;
        for centre in [ZIPPER_HZ - hz, ZIPPER_HZ + hz] {
            for m in -15..=15 {
                loudest = loudest.max(measure(signal, centre + 2.0 * m as f32));
            }
        }
        loudest
    };
    let mut worst = f32::NEG_INFINITY;
    for pitch in CHORD {
        let hz = 440.0 * 2.0_f32.powf((f32::from(pitch) - 69.0) / 12.0);
        let level = db(lines(&pump, hz) / measure(&pump, hz));
        let floor = db(lines(&still, hz) / measure(&still, hz));
        println!(
            "{hz:7.1} Hz tone: its loudest zipper line is {level:7.1} dB under it pumped, \
             {floor:7.1} dB unpumped"
        );
        worst = worst.max(level);
    }
    let peak = |s: &[f32]| s.iter().fold(0.0_f32, |p, x| p.max(x.abs()));
    println!(
        "pump peak {:.2} dBFS, steady peak {:.2} dBFS; the loudest zipper sideband is {worst:.1} dB \
         under its tone",
        db(peak(&pump)),
        db(peak(&still))
    );
}
