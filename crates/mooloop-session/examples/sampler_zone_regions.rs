//! Render-identity check for per-zone regions (MOO-463).
//!
//! Writes three songs and their samples, reopens each through the app's own
//! loader and exports it through the app's own export path, then prints a
//! digest of each render's samples:
//!
//! - `v1-lanes`: a sampler with no zones, looping with a fade, stretched,
//!   with lanes on Start and Tune cents.
//! - `v1-reverse`: a sampler with no zones, reversed and ping-ponging.
//! - `zoned-0.1.5`: a sampler with two extra zones written the way 0.1.5
//!   wrote them, with no `region`, under a lane on Loop end.
//!
//! Nothing here is compared to a stored value. To check that a change
//! renders these songs as an earlier tree did, run it on both trees and
//! compare the printed digests:
//!
//! ```sh
//! scripts/antibox --no-incremental cargo run -p mooloop-session --example sampler_zone_regions -- target/zone-regions
//! ```
//!
//! It builds only on APIs both trees have, so it can be dropped into an
//! older checkout unchanged.

use std::path::{Path, PathBuf};

use mooloop_core::{
    AutomationLane, AutomationPoint, DeviceKind, EffectTarget, KeyRange, LoopMode, NoteEvent,
    ParamAddr, ParamOwner, Project, ProjectChannel, SampleReference, SampleZone,
};
use mooloop_engine::{ExportFormat, ExportProgress, ExportSpec, RenderJob, RenderScope, WavEncoding};
use mooloop_project::LoadedDocument;
use mooloop_session::document::{resolve_document, run_export, DocumentResult, ExportRequest};
use mooloop_session::session::Session;

const SAMPLE_RATE: u32 = 48_000;
const STEP: u32 = mooloop_core::TICKS_PER_STEP;

/// Two seconds of a tone with some upper partials, so a moved read head
/// changes the render.
fn write_tone(path: &Path, hz: f64) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("the tone writes");
    for n in 0..(2 * SAMPLE_RATE as usize) {
        let t = n as f64 / f64::from(SAMPLE_RATE);
        let decay = (-1.5 * t).exp();
        let value = (decay
            * (0.5 * (std::f64::consts::TAU * hz * t).sin()
                + 0.2 * (std::f64::consts::TAU * 3.0 * hz * t).sin()))
            as f32;
        writer.write_sample(value).expect("a sample");
        writer.write_sample(-value).expect("a sample");
    }
    writer.finalize().expect("the tone finalizes");
}

fn file(path: &Path) -> SampleReference {
    SampleReference::File {
        path: path.to_path_buf(),
        embedded: false,
    }
}

fn lane(param: u32, points: &[(u32, f32)]) -> AutomationLane {
    let mut lane = AutomationLane::new(ParamAddr {
        scope: EffectTarget::Channel(0),
        owner: ParamOwner::source(DeviceKind::Sampler),
        param,
    });
    for (tick, value) in points {
        let id = lane.allocate_id();
        lane.upsert(AutomationPoint::new(id, *tick, *value));
    }
    lane
}

fn song(base: &Path, notes: &[u8]) -> (Project, ProjectChannel) {
    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().expect("a sampler");
    state.sample = file(base);
    state.params.attack = 0.002;
    state.params.decay = 4.0;
    state.params.sustain = 0.8;
    state.params.release = 0.05;
    state.params.polyphony = 4;
    for (index, note) in notes.iter().enumerate() {
        channel.notes[0].push(NoteEvent::new(index as u32 + 1, index as u32 * 6 * STEP, 5 * STEP, *note, 110));
    }
    let project = Project {
        bpm: 120,
        pattern_lengths: vec![32],
        ..Project::default()
    };
    (project, channel)
}

fn v1_lanes(dir: &Path) -> Project {
    let (mut project, mut channel) = song(&dir.join("a.wav"), &[48, 55, 60, 67, 72]);
    let state = channel.setup.sampler_state_mut().unwrap();
    state.params.loop_mode = LoopMode::Forward;
    state.params.loop_start = 0.3;
    state.params.loop_end = 0.7;
    state.params.loop_crossfade_ms = 5.0;
    state.params.tune_cents = 7.0;
    state.params.stretch_enabled = true;
    state.params.stretch_ratio = 1.25;
    channel.automation[0] = vec![
        lane(mooloop_core::generator::SAMPLER_PARAM_START, &[(0, 0.0), (16 * STEP, 0.2), (32 * STEP, 0.05)]),
        lane(mooloop_core::generator::SAMPLER_PARAM_TUNE_CENTS, &[(0, 0.5), (32 * STEP, 0.7)]),
    ];
    project.channels = vec![channel];
    project
}

fn v1_reverse(dir: &Path) -> Project {
    let (mut project, mut channel) = song(&dir.join("a.wav"), &[50, 62, 74]);
    let state = channel.setup.sampler_state_mut().unwrap();
    state.params.reverse = true;
    state.params.start = 0.1;
    state.params.end = 0.8;
    state.params.loop_mode = LoopMode::Pingpong;
    state.params.loop_start = 0.4;
    state.params.loop_end = 0.6;
    project.channels = vec![channel];
    project
}

fn zoned(dir: &Path) -> Project {
    let (mut project, mut channel) = song(&dir.join("a.wav"), &[40, 52, 64, 70, 84, 96]);
    let state = channel.setup.sampler_state_mut().unwrap();
    state.params.start = 0.05;
    state.params.end = 0.9;
    state.params.loop_mode = LoopMode::Forward;
    state.params.loop_start = 0.25;
    state.params.loop_end = 0.75;
    state.params.tune_semitones = -2.0;
    state.keys = KeyRange::new(0, 59);
    state.zones = vec![
        SampleZone {
            keys: KeyRange::new(60, 79),
            root_note: 64,
            sample: file(&dir.join("b.wav")),
            ..SampleZone::default()
        },
        SampleZone {
            keys: KeyRange::new(80, 127),
            root_note: 88,
            sample: file(&dir.join("c.wav")),
            ..SampleZone::default()
        },
    ];
    channel.automation[0] = vec![lane(
        mooloop_core::generator::SAMPLER_PARAM_LOOP_END,
        &[(0, 0.75), (32 * STEP, 0.5)],
    )];
    project.channels = vec![channel];
    project
}

/// The saved song with every zone's `region` table taken out, which is the
/// file 0.1.5 wrote. A tree with no regions writes none, so this changes
/// nothing there.
fn strip_regions(path: &Path) {
    let text = std::fs::read_to_string(path).expect("the song reads");
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            skipping = trimmed.trim_end().ends_with(".region]");
        } else if trimmed.starts_with("region =") || trimmed.starts_with("region=") {
            continue;
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(path, out).expect("the song writes");
}

/// FNV-1a over the render's sample bits.
fn digest(path: &Path) -> (u64, usize, f32) {
    let samples: Vec<f32> = hound::WavReader::open(path)
        .expect("the render reads back")
        .samples::<f32>()
        .map(|s| s.expect("a sample"))
        .collect();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for sample in &samples {
        for byte in sample.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    let peak = samples.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
    (hash, samples.len(), peak)
}

fn render(dir: &Path, name: &str, project: &Project, old_zones: bool) {
    let bundle = dir.join(format!("{name}.mooloop"));
    mooloop_project::save_song(&bundle, project, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    if old_zones {
        strip_regions(&bundle);
    }
    let resolved = resolve_document(&bundle).unwrap_or_else(|problem| panic!("{}", problem.message));
    assert!(resolved.report.repairs.is_empty(), "repaired: {:?}", resolved.report.repairs);
    let LoadedDocument::Song(project) = resolved.report.document else {
        panic!("a song came back as something else");
    };
    let mut session = Session::default();
    session.admit_zone_audio(resolved.zone_audio, true);
    session.replace_project(&project, &resolved.samples);
    assert!(session.missing_zones().is_empty());
    let out = dir.join(format!("{name}.wav"));
    let request = ExportRequest {
        project: session.project_snapshot(120, 0),
        samples: session.sample_snapshots(),
        zones: session.zone_sample_snapshots(),
        job: RenderJob::single(&ExportSpec {
            path: out.clone(),
            scope: RenderScope::Pattern { index: 0 },
            tail_seconds: 0.5,
            format: ExportFormat::Wav(WavEncoding::Float32),
        }),
    };
    match run_export(request, SAMPLE_RATE, &ExportProgress::new(), Default::default()) {
        DocumentResult::Exported { .. } => {}
        _ => panic!("the export failed"),
    }
    let (hash, len, peak) = digest(&out);
    println!("{name}: {hash:016x} ({len} samples, peak {peak:.4})");
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/zone-regions".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");
    write_tone(&dir.join("a.wav"), 220.0);
    write_tone(&dir.join("b.wav"), 330.0);
    write_tone(&dir.join("c.wav"), 495.0);
    render(&dir, "v1-lanes", &v1_lanes(&dir), false);
    render(&dir, "v1-reverse", &v1_reverse(&dir), false);
    render(&dir, "zoned-0.1.5", &zoned(&dir), true);
}
