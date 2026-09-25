//! A two-zone sampler, MOO-14's listening case.
//!
//! Two sine tones are written to WAVs: the base sample, C4 (261.6 Hz) at
//! half scale, and a zone's sample, C5 (523.3 Hz) at quarter scale. One
//! sampler plays the base up to B3 (root C4) and the zone from C4 up (root
//! C5). The song is saved with its assets embedded, reopened through the
//! app's own loader (`document::resolve_document`, which must repair
//! nothing and decode both files), and exported through the app's own export
//! path (`document::run_export`) as a float WAV.
//!
//! The pattern plays four notes a half-bar apart: C3 and B3 below the
//! split, then C4 and C6 above it. Each should be its own zone's sample,
//! pitched from its own zone's root:
//!
//! | note | zone | expected pitch | file level |
//! | --- | --- | --- | --- |
//! | C3 (48) | base | 130.8 Hz | 0.5 |
//! | B3 (59) | base | 246.9 Hz | 0.5 |
//! | C4 (60) | zone | 261.6 Hz | 0.25 |
//! | C6 (84) | zone | 1046.5 Hz | 0.25 |
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/sampler-key-zones \
//!     cargo run -p mooloop-session --example sampler_key_zones -- target/sampler-key-zones
//! ```
//!
//! It measures; it does not listen (`docs/FOCUS.md`, "Listening is a step").

use std::path::{Path, PathBuf};

use mooloop_core::{KeyRange, NoteEvent, Project, ProjectChannel, SampleReference, SampleZone};
use mooloop_engine::{ExportFormat, ExportProgress, ExportSpec, RenderJob, RenderScope, WavEncoding};
use mooloop_project::LoadedDocument;
use mooloop_session::document::{resolve_document, run_export, DocumentResult, ExportRequest};
use mooloop_session::session::Session;

const SAMPLE_RATE: u32 = 48_000;
/// Half a bar at 120 BPM, in frames.
const HALF_BAR: usize = 48_000;
const STEP: u32 = mooloop_core::TICKS_PER_STEP;
const NOTES: [(u8, f64, f32); 4] = [(48, 130.81, 0.5), (59, 246.94, 0.5), (60, 261.63, 0.25), (84, 1046.5, 0.25)];

fn write_tone(path: &Path, hz: f64, level: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("the tone writes");
    for n in 0..(2 * SAMPLE_RATE as usize) {
        let value =
            level * (std::f64::consts::TAU * hz * n as f64 / f64::from(SAMPLE_RATE)).sin() as f32;
        writer.write_sample(value).expect("a sample");
        writer.write_sample(value).expect("a sample");
    }
    writer.finalize().expect("the tone finalizes");
}

fn song(base: &Path, zone: &Path) -> Project {
    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().expect("a sampler");
    state.sample = SampleReference::File {
        path: base.to_path_buf(),
        embedded: false,
    };
    state.params.root_note = 60;
    state.params.attack = 0.0;
    state.params.decay = 8.0;
    state.params.sustain = 1.0;
    state.params.release = 0.005;
    state.params.output_gain = 1.0;
    state.keys = KeyRange::new(0, 59);
    state.zones = vec![SampleZone {
        keys: KeyRange::new(60, 127),
        root_note: 72,
        sample: SampleReference::File {
            path: zone.to_path_buf(),
            embedded: false,
        },
        ..SampleZone::default()
    }];
    channel.setup.channel.volume = 1.0;
    for (index, (note, _, _)) in NOTES.iter().enumerate() {
        channel.notes[0].push(NoteEvent::new(index as u32 + 1, index as u32 * 8 * STEP, 6 * STEP, *note, 127));
    }
    Project {
        bpm: 120,
        channels: vec![channel],
        pattern_lengths: vec![32],
        ..Project::default()
    }
}

/// A sine's frequency from its zero crossings: the time between the first
/// and the last, interpolated between frames, over the half-cycles between.
fn pitch(signal: &[f32]) -> f64 {
    let crossings: Vec<f64> = signal
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| (pair[0] < 0.0) != (pair[1] < 0.0))
        .map(|(n, pair)| n as f64 + f64::from(pair[0] / (pair[0] - pair[1])))
        .collect();
    let (Some(first), Some(last)) = (crossings.first(), crossings.last()) else {
        return 0.0;
    };
    let half_cycles = (crossings.len() - 1) as f64;
    half_cycles / 2.0 / ((last - first) / f64::from(SAMPLE_RATE))
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0_f32, |p, s| p.max(s.abs()))
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/sampler-key-zones".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");
    let (base, zone) = (dir.join("base-c4.wav"), dir.join("zone-c5.wav"));
    write_tone(&base, 261.63, 0.5);
    write_tone(&zone, 523.25, 0.25);

    let bundle = dir.join("key-zones.mooloop");
    mooloop_project::save_song(&bundle, &song(&base, &zone), mooloop_project::AssetMode::Embedded)
        .expect("the song saves");
    let resolved = resolve_document(&bundle).unwrap_or_else(|problem| panic!("{}", problem.message));
    assert!(resolved.report.repairs.is_empty(), "repaired: {:?}", resolved.report.repairs);
    assert!(resolved.report.warnings.is_empty(), "warned: {:?}", resolved.report.warnings);
    assert_eq!(resolved.zone_audio.len(), 1, "the zone's file was decoded");
    let LoadedDocument::Song(project) = resolved.report.document else {
        panic!("a song came back as something else");
    };

    let mut session = Session::default();
    session.admit_zone_audio(resolved.zone_audio, true);
    session.replace_project(&project, &resolved.samples);
    assert!(session.missing_zones().is_empty());

    let out = dir.join("key-zones.wav");
    let request = ExportRequest {
        project: session.project_snapshot(120, 0),
        samples: session.sample_snapshots(),
        zones: session.zone_sample_snapshots(),
        job: RenderJob::single(&ExportSpec {
            path: out.clone(),
            scope: RenderScope::Pattern { index: 0 },
            tail_seconds: 0.0,
            format: ExportFormat::Wav(WavEncoding::Float32),
        }),
    };
    match run_export(request, SAMPLE_RATE, &ExportProgress::new(), Default::default()) {
        DocumentResult::Exported { .. } => {}
        _ => panic!("the export failed"),
    }
    let left: Vec<f32> = hound::WavReader::open(&out)
        .expect("the render reads back")
        .samples::<f32>()
        .map(|s| s.expect("a sample"))
        .step_by(2)
        .collect();

    println!("wrote {}", out.display());
    // Levels are compared zone to zone rather than to the files' own: the
    // channel's centred pan takes 3 dB off both alike.
    let mut worst_cents = 0.0_f64;
    let mut peaks = Vec::new();
    for (index, (note, hz, level)) in NOTES.iter().enumerate() {
        // The middle of the note: past the attack, before the release.
        let start = index * HALF_BAR + 4_800;
        let window = &left[start..start + 24_000];
        let (measured, loudest) = (pitch(window), peak(window));
        let cents = 1200.0 * (measured / hz).log2();
        worst_cents = worst_cents.max(cents.abs());
        println!(
            "note {note}: {measured:.1} Hz (expected {hz:.1}, {cents:+.1} cents), peak {loudest:.3} (file level {level})"
        );
        assert!(cents.abs() < 10.0, "note {note} is off pitch");
        peaks.push(loudest);
    }
    let base = (peaks[0] + peaks[1]) / 2.0;
    let zone = (peaks[2] + peaks[3]) / 2.0;
    let ratio_db = 20.0 * (base / zone).log10();
    println!("base zone over upper zone: {ratio_db:+.2} dB (the files are 6.02 dB apart)");
    assert!((ratio_db - 6.02).abs() < 0.2, "a note played the other zone's file");
    println!("worst pitch error {worst_cents:.2} cents");
}
