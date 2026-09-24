//! The Bus Comp insert's acceptance render (MOO-216): the master's bus
//! compressor as a device on a drum bus, against the master's own section
//! at the same settings.
//!
//! Builds a two-bar drum loop (kick, snare, hats) routed to a drum bus with
//! the real `mooloop_core` types, saves it with a Bus Comp on that bus,
//! checks `load_bundle` repairs nothing, and renders offline, per voicing:
//!
//! - `dry.wav`: nothing compressing;
//! - `insert-<voicing>.wav`: a Bus Comp on the drum bus;
//! - `master-<voicing>.wav`: no insert, the master's section in at the same
//!   settings.
//!
//! Each is measured against the dry render, and the insert against the
//! section: the drum bus is all the master hears, so the two should reduce
//! the same and come out the same file to within float rounding.
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/bus-comp-insert \
//!     cargo run -p mooloop-engine --example bus_comp_insert -- target/bus-comp-insert
//! ```
//!
//! It measures; it does not listen (`docs/FOCUS.md`, "Listening is a step").

use std::path::{Path, PathBuf};

use mooloop_core::strip::{BusCompVoicing, MasterSectionParams};
use mooloop_core::{
    BusCompParams, DrumMode, DrumSynthParams, EffectParams, EffectSlotState, NoteEvent, Project,
    ProjectChannel, MASTER_BUS,
};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
const DRUMS: usize = 1;

fn drum_bus() -> Project {
    let mut channels = Vec::new();
    for (mode, steps) in [
        (DrumMode::Kick, vec![0u32, 6, 10, 16, 22, 26]),
        (DrumMode::Snare, vec![4, 12, 20, 28]),
        (DrumMode::Hat, (0..32).step_by(2).collect::<Vec<u32>>()),
    ] {
        let mut channel = ProjectChannel::drum_synth_with_params(
            channels.len(),
            1,
            DrumSynthParams {
                mode,
                ..DrumSynthParams::default()
            },
        );
        channel.setup.channel.bus = DRUMS as u8;
        channel.setup.channel.volume = 1.0;
        for (n, step) in steps.into_iter().enumerate() {
            let tick = step * mooloop_core::TICKS_PER_STEP;
            channel.notes[0].push(NoteEvent::new((n + 1) as _, tick, 12, 60, 120));
        }
        channels.push(channel);
    }
    let mut project = Project {
        channels,
        pattern_lengths: vec![32],
        ..Project::default()
    };
    project.ensure_tracks(DRUMS + 1);
    project
}

/// No makeup, so what is measured against the dry render is the reduction
/// alone.
fn settings(voicing: BusCompVoicing) -> BusCompParams {
    BusCompParams {
        voicing,
        threshold_db: -24.0,
        makeup_db: 0.0,
        ..BusCompParams::default()
    }
}

fn with_insert(mut project: Project, params: BusCompParams) -> Project {
    let bus = &mut project.buses[DRUMS];
    bus.effects.push(EffectSlotState::bus_comp(params));
    mooloop_core::assign_device_ids(&mut bus.effects, &mut bus.next_device_id);
    project
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
            tail_seconds: 1.0,
            format: ExportFormat::Wav(WavEncoding::Float32),
        },
    )
    .expect("the render");
    let mut reader = hound::WavReader::open(path).expect("the render reads back");
    let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.expect("a sample")).collect();
    samples.chunks(2).map(|pair| [pair[0], pair[1]]).collect()
}

/// Gain reduction per 10 ms window, in dB, where the dry render is loud
/// enough to say anything.
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

fn summary(trace: &[f32]) -> (f32, f32) {
    let mean = trace.iter().sum::<f32>() / trace.len().max(1) as f32;
    let most = trace.iter().fold(0.0f32, |m, &g| m.max(g));
    (mean, most)
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/bus-comp-insert".into()));
    std::fs::create_dir_all(&dir).expect("the output directory");

    let saved = with_insert(drum_bus(), settings(BusCompVoicing::Grip));
    let bundle = dir.join("bus-comp-insert.mooloop");
    mooloop_project::save_song(&bundle, &saved, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let report = mooloop_project::load_bundle(&bundle).expect("the song reopens");
    assert!(report.repairs.is_empty(), "the loader repaired it: {:?}", report.repairs);
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        panic!("a song came back as something else");
    };
    assert_eq!(
        reopened.buses[DRUMS].effects.last().map(|effect| effect.params),
        Some(EffectParams::BusComp(settings(BusCompVoicing::Grip))),
        "the insert did not survive the round trip"
    );
    println!("saved and reopened {} with nothing repaired", bundle.display());

    let dry = render(&drum_bus(), &dir.join("dry.wav"));
    println!("dry            peak {:6.2} dBFS, {} frames", peak_db(&dry), dry.len());
    for (name, voicing) in [
        ("grip", BusCompVoicing::Grip),
        ("punch", BusCompVoicing::Punch),
        ("tube", BusCompVoicing::Tube),
    ] {
        let params = settings(voicing);
        let insert = render(&with_insert(drum_bus(), params), &dir.join(format!("insert-{name}.wav")));
        let section = render(
            &with_section(drum_bus(), params.section()),
            &dir.join(format!("master-{name}.wav")),
        );
        let (insert_mean, insert_most) = summary(&reduction_trace(&dry, &insert));
        let (section_mean, section_most) = summary(&reduction_trace(&dry, &section));
        let worst = insert
            .iter()
            .zip(&section)
            .fold(0.0f32, |w, (a, b)| w.max((a[0] - b[0]).abs()).max((a[1] - b[1]).abs()));
        println!(
            "{name:<6} insert reduction mean {insert_mean:5.2} dB, most {insert_most:5.2} dB | \
             master section mean {section_mean:5.2} dB, most {section_most:5.2} dB | \
             worst sample difference {worst:e}"
        );
    }
}
