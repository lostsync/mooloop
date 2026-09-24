//! The master bus compressor's acceptance render (MOO-13,
//! `docs/plans/archive/master-bus-compressor/03-the-master-section-runs.md`).
//!
//! Builds a small mix with the real `mooloop_core` types -- a kick, a bass
//! line and chords, summed a few decibels hot at the master -- saves it with
//! `mooloop_project::save_song`, checks `load_bundle` repairs nothing, and
//! renders it offline five times: the section out, each voicing in, and Grip
//! again with the safety limiter looking ahead 3 ms. Each is a float WAV, and
//! each is measured against the one with the section out, which is the same
//! mix bit for bit before the compressor.
//!
//! ```sh
//! scripts/antibox --pull target/master-bus-comp \
//!     cargo run -p mooloop-engine --example master_bus_comp -- target/master-bus-comp
//! ```
//!
//! It measures; it does not listen. "Turning it on should feel special" is
//! Adam's to judge, from these files (`docs/FOCUS.md`, "Listening is a
//! step").

use std::path::{Path, PathBuf};

use mooloop_core::strip::{BusCompVoicing, MasterSectionParams};
use mooloop_core::{NoteEvent, Project, ProjectChannel, MASTER_BUS};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
/// A sixteenth, in ticks.
const STEP: u32 = 24;
const BARS: u32 = 4;

fn song() -> Project {
    let steps = (BARS * 16) as u16;
    let mut kick = ProjectChannel::drum_synth(0, 1);
    let mut bass = ProjectChannel::mono_synth(1, 1);
    let mut chords = ProjectChannel::poly_synth(2, 1);
    let mut id = 1u32;
    let mut note = |start: u32, length: u32, pitch: u8, velocity: u8| {
        id += 1;
        NoteEvent::new(id, start * STEP, length * STEP, pitch, velocity)
    };
    for bar in 0..BARS {
        let at = bar * 16;
        for beat in 0..4 {
            kick.notes[0].push(note(at + beat * 4, 2, 36, 127));
        }
        for (offset, pitch) in [(0, 33), (3, 33), (6, 36), (8, 31), (11, 31), (14, 38)] {
            bass.notes[0].push(note(at + offset, 2, pitch, 112));
        }
        let root = if bar % 2 == 0 { 57 } else { 55 };
        for pitch in [root, root + 3, root + 7, root + 10] {
            chords.notes[0].push(note(at, 14, pitch, 96));
        }
    }
    // Unity on each: three calibrated sources sum to a few decibels under
    // full scale at the master, which is where a bus compressor lives -- and
    // under the safety limiter, so the section-out render is the mix itself
    // rather than the limiter's idea of it.
    for channel in [&mut kick, &mut bass, &mut chords] {
        channel.setup.channel.volume = 1.0;
    }
    Project {
        bpm: 110,
        channels: vec![kick, bass, chords],
        pattern_lengths: vec![steps],
        ..Project::default()
    }
}

fn with_section(mut project: Project, section: MasterSectionParams) -> Project {
    project.buses[MASTER_BUS as usize].bus.strip.master = section;
    project
}

fn render(project: &Project, path: &Path) -> Vec<[f32; 2]> {
    OfflineRenderer::render(
        project,
        &[],
        SAMPLE_RATE,
        &ExportSpec {
            path: path.to_path_buf(),
            scope: RenderScope::Pattern { index: 0 },
            tail_seconds: 2.0,
            format: ExportFormat::Wav(WavEncoding::Float32),
        },
    )
    .expect("the render");
    let mut reader = hound::WavReader::open(path).expect("the render reads back");
    let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.expect("a sample")).collect();
    samples.chunks(2).map(|pair| [pair[0], pair[1]]).collect()
}

/// Gain reduction per 10 ms window, in dB, where the dry mix is loud enough
/// to say anything.
fn reduction_trace(dry: &[[f32; 2]], wet: &[[f32; 2]]) -> Vec<f32> {
    let window = SAMPLE_RATE as usize / 100;
    dry.chunks(window)
        .zip(wet.chunks(window))
        .filter_map(|(d, w)| {
            let power = |frames: &[[f32; 2]]| {
                frames.iter().map(|f| f[0] * f[0] + f[1] * f[1]).sum::<f32>() / frames.len() as f32
            };
            let (pd, pw) = (power(d), power(w));
            (pd > 1e-6).then(|| -10.0 * (pw / pd).log10())
        })
        .collect()
}

fn peak_db(frames: &[[f32; 2]]) -> f32 {
    let peak = frames.iter().fold(0.0f32, |p, f| p.max(f[0].abs()).max(f[1].abs()));
    20.0 * peak.max(1e-9).log10()
}

fn rms_db(frames: &[[f32; 2]]) -> f32 {
    let power = frames.iter().map(|f| f[0] * f[0] + f[1] * f[1]).sum::<f32>()
        / (2 * frames.len().max(1)) as f32;
    10.0 * power.max(1e-18).log10()
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/master-bus-comp".into()));
    std::fs::create_dir_all(&dir).expect("the output directory");

    let base = song();
    let bundle = dir.join("master-bus-comp.mooloop");
    let section = |voicing| MasterSectionParams {
        comp_in: true,
        voicing,
        threshold_db: -18.0,
        ..MasterSectionParams::default()
    };
    let saved = with_section(base.clone(), section(BusCompVoicing::Grip));
    mooloop_project::save_song(&bundle, &saved, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let report = mooloop_project::load_bundle(&bundle).expect("the song reopens");
    assert!(report.repairs.is_empty(), "the loader repaired it: {:?}", report.repairs);
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        panic!("a song came back as something else");
    };
    assert_eq!(
        reopened.buses[MASTER_BUS as usize].bus.strip.master,
        saved.buses[MASTER_BUS as usize].bus.strip.master,
        "the section did not survive the round trip"
    );
    println!("saved and reopened {} with nothing repaired", bundle.display());

    let dry = render(&base, &dir.join("out.wav"));
    println!(
        "out     peak {:6.2} dBFS  rms {:6.2} dBFS  {} frames",
        peak_db(&dry),
        rms_db(&dry),
        dry.len()
    );
    let mut grip_at_zero = Vec::new();
    for (name, voicing) in [
        ("grip", BusCompVoicing::Grip),
        ("punch", BusCompVoicing::Punch),
        ("tube", BusCompVoicing::Tube),
    ] {
        let project = with_section(base.clone(), section(voicing));
        let wet = render(&project, &dir.join(format!("{name}.wav")));
        let trace = reduction_trace(&dry, &wet);
        let mean = trace.iter().sum::<f32>() / trace.len().max(1) as f32;
        let most = trace.iter().fold(0.0f32, |m, &g| m.max(g));
        println!(
            "{name:<7} peak {:6.2} dBFS  rms {:6.2} dBFS  reduction mean {mean:5.2} dB, most {most:5.2} dB",
            peak_db(&wet),
            rms_db(&wet),
        );
        if voicing == BusCompVoicing::Grip {
            grip_at_zero = wet;
        }
    }
    let ahead = with_section(
        base.clone(),
        MasterSectionParams {
            lookahead_ms: 3.0,
            ..section(BusCompVoicing::Grip)
        },
    );
    let wet = render(&ahead, &dir.join("grip-lookahead-3ms.wav"));
    println!(
        "grip, 3 ms lookahead: {} frames against {} at 0 ms, {}",
        wet.len(),
        grip_at_zero.len(),
        if wet == grip_at_zero {
            "sample for sample the same file"
        } else {
            "and the files differ"
        }
    );
}
