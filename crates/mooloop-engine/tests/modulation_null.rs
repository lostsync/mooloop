//! A null test for the engine's modulation (song modulation step 02): every
//! release song, given a busy 0.1.6-style rack on every channel, renders
//! offline to a file, and a later tree's render of the same songs is held
//! against it sample by sample.
//!
//! Ignored, and driven by the environment, because the reference is the
//! tree before a change:
//!
//! ```sh
//! MODULATION_NULL_DIR=/some/dir MODULATION_NULL_MODE=write \
//!   cargo test -p mooloop-engine --release --test modulation_null -- --ignored --nocapture
//! # change the engine, then
//! MODULATION_NULL_DIR=/some/dir MODULATION_NULL_MODE=compare \
//!   cargo test -p mooloop-engine --release --test modulation_null -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};

use mooloop_core::effect::ParamCurve;
use mooloop_core::modulation::{STRIP_PARAM_PAN, STRIP_PARAM_VOLUME};
use mooloop_core::{
    EffectTarget, ModEnvelopeParams, ModLfoParams, ModLfoWaveform, ModMathOp, ModMathParams,
    ModPolarity, ModRack, ModRandomParams, ModRandomTrigger, ModRoute, ModStepParams,
    ModStepTrigger, ModulatorParams, ParamAddr, Project, PublishesOutlets,
};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};
use mooloop_project::{load_bundle, LoadedDocument};

const SAMPLE_RATE: u32 = 48_000;

fn corpus() -> Vec<PathBuf> {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../mooloop-project/tests/fixtures/songs");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("the release corpus")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    versions.sort();
    versions
        .into_iter()
        .map(|version| version.join("starter.mooloop"))
        .filter(|path| path.exists())
        .collect()
}

/// Eight modules of every kind and sixteen routes onto the channel's
/// generator, its strip and its first effect, with an envelope gated by the
/// next channel, Math reading a lower and a higher slot, synced and free
/// LFOs, a clocked Step, clocked and note-triggered Randoms, and a route
/// from the generator's first outlet.
fn busy_rack(project: &Project, seat: usize) -> ModRack {
    let channel = &project.channels[seat];
    let kind = channel.setup.source.kind();
    let scope = EffectTarget::Channel(seat as u8);
    let next = (seat + 1) % project.channels.len();
    let mut rack = ModRack::default();
    let install = |rack: &mut ModRack, slot: usize, params: ModulatorParams| {
        rack.install(slot, params).expect("a free slot");
    };
    install(
        &mut rack,
        0,
        ModulatorParams::Lfo(ModLfoParams {
            rate_hz: 2.3 + seat as f32 * 0.37,
            ..ModLfoParams::default()
        }),
    );
    install(
        &mut rack,
        1,
        ModulatorParams::Envelope(ModEnvelopeParams {
            input_channel: next as u8,
            input_channel_id: project.channels[next].id,
            attack_seconds: 0.01,
            decay_seconds: 0.2,
            sustain: 0.4,
            release_seconds: 0.3,
            ..ModEnvelopeParams::default()
        }),
    );
    install(
        &mut rack,
        2,
        ModulatorParams::Step(ModStepParams {
            trigger: ModStepTrigger::Clock,
            glide: 0.3,
            steps: core::array::from_fn(|step| ((step * 7 + seat) % 11) as f32 / 5.0 - 1.0),
            ..ModStepParams::default()
        }),
    );
    install(
        &mut rack,
        3,
        ModulatorParams::Random(ModRandomParams {
            trigger: ModRandomTrigger::Clock,
            tempo_sync: true,
            ..ModRandomParams::default()
        }),
    );
    install(
        &mut rack,
        4,
        ModulatorParams::Math(ModMathParams {
            input_slot: 0,
            op: ModMathOp::Multiply,
            operand: 0.5,
            ..ModMathParams::default()
        }),
    );
    install(
        &mut rack,
        5,
        ModulatorParams::Math(ModMathParams {
            input_slot: 6,
            op: ModMathOp::Add,
            operand: 0.25,
            ..ModMathParams::default()
        }),
    );
    install(
        &mut rack,
        6,
        ModulatorParams::Lfo(ModLfoParams {
            tempo_sync: true,
            waveform: ModLfoWaveform::Random,
            ..ModLfoParams::default()
        }),
    );
    install(
        &mut rack,
        7,
        ModulatorParams::Random(ModRandomParams {
            trigger: ModRandomTrigger::NoteTrigger,
            ..ModRandomParams::default()
        }),
    );

    let mut destinations: Vec<ParamAddr> = kind
        .descriptors()
        .iter()
        .filter(|descriptor| !matches!(descriptor.curve, ParamCurve::Stepped(_)))
        .take(6)
        .map(|descriptor| ParamAddr::source(scope, kind, descriptor.id))
        .collect();
    destinations.push(ParamAddr::strip(scope, STRIP_PARAM_VOLUME));
    destinations.push(ParamAddr::strip(scope, STRIP_PARAM_PAN));
    if let Some(effect) = channel.setup.effects.first() {
        destinations.extend(
            effect
                .kind()
                .descriptors()
                .iter()
                .filter(|descriptor| !matches!(descriptor.curve, ParamCurve::Stepped(_)))
                .take(2)
                .map(|descriptor| ParamAddr::effect(scope, effect.id, descriptor.id)),
        );
    }
    let mut routes = 0;
    if let Some(outlet) = kind.control_outlets().first() {
        rack.add_route(ModRoute::from_outlet(
            channel.id,
            outlet.id,
            destinations[0],
            0.2,
            ModPolarity::Bipolar,
        ))
        .expect("room");
        routes += 1;
    }
    'fill: for (index, destination) in destinations.iter().enumerate() {
        for offset in 0..2 {
            if routes == mooloop_core::modulation::MAX_MOD_ROUTES_PER_CHANNEL {
                break 'fill;
            }
            let slot = ((index * 3 + offset * 5) % 8) as u8;
            let depth = 0.15 + 0.05 * offset as f32;
            let polarity = if (index + offset) % 2 == 0 {
                ModPolarity::Bipolar
            } else {
                ModPolarity::Unipolar
            };
            if rack
                .add_route(ModRoute::to_slot(slot, *destination, depth, polarity))
                .is_some()
            {
                routes += 1;
            }
        }
    }
    rack
}

/// The song with notes on every channel in its first pattern, and that
/// pattern placed four times on the playlist: the starters ship empty.
fn with_notes(mut project: Project) -> Project {
    use mooloop_core::{NoteEvent, PatternPlacement, TICKS_PER_STEP};
    let steps = u32::from(project.pattern_lengths[0]);
    for (seat, channel) in project.channels.iter_mut().enumerate() {
        let seat = seat as u32;
        channel.notes[0] = (0..steps)
            .filter(|step| (step + seat).is_multiple_of(3))
            .map(|step| {
                NoteEvent::new(
                    step + 1,
                    step * TICKS_PER_STEP,
                    TICKS_PER_STEP * (1 + step % 2),
                    (36 + (seat * 5 + step * 7) % 36) as u8,
                    90 + (step % 4) as u8 * 10,
                )
            })
            .collect();
        channel.next_note_id = steps + 1;
    }
    let pattern_ticks = steps * TICKS_PER_STEP;
    project.playlist = (0..4u32)
        .map(|instance| PatternPlacement {
            pattern: 0,
            start_tick: instance * pattern_ticks,
        })
        .collect();
    project
}

/// Each release song as it opens, and with a busy rack on every channel.
fn songs() -> Vec<(String, Project)> {
    let mut songs = Vec::new();
    for path in corpus() {
        let version = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .expect("a version directory")
            .to_string();
        let LoadedDocument::Song(project) = load_bundle(&path).expect("it opens").document else {
            panic!("{version} is a song");
        };
        let project = with_notes(project);
        let mut busy = project.clone();
        for seat in 0..busy.channels.len() {
            busy.channels[seat].setup.preset_modulation = Some(busy_rack(&busy, seat));
        }
        busy.lift_channel_modulation();
        songs.push((format!("{version}-plain"), project));
        songs.push((format!("{version}-busy"), busy));
    }
    songs
}

fn render(project: &Project, path: &Path) {
    OfflineRenderer::render(
        project,
        &[],
        SAMPLE_RATE,
        &ExportSpec {
            path: path.to_path_buf(),
            scope: RenderScope::Song,
            tail_seconds: 2.0,
            format: ExportFormat::Wav(WavEncoding::Float32),
        },
    )
    .expect("it renders");
}

fn samples(path: &Path) -> Vec<f32> {
    hound::WavReader::open(path)
        .expect("readable")
        .samples::<f32>()
        .map(|sample| sample.expect("a sample"))
        .collect()
}

#[test]
#[ignore = "a before/after harness; run deliberately with MODULATION_NULL_DIR set"]
fn every_release_song_renders_as_it_did() {
    let dir = PathBuf::from(std::env::var("MODULATION_NULL_DIR").expect("MODULATION_NULL_DIR"));
    let mode = std::env::var("MODULATION_NULL_MODE").unwrap_or_else(|_| "compare".into());
    std::fs::create_dir_all(&dir).expect("the directory");
    let mut failures = Vec::new();
    for (name, project) in songs() {
        let reference = dir.join(format!("{name}.wav"));
        if mode == "write" {
            render(&project, &reference);
            let rendered = samples(&reference);
            let peak = rendered
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
            println!("{name}: wrote {} samples, peak {peak:.4}", rendered.len());
            continue;
        }
        let now = dir.join(format!("{name}.after.wav"));
        render(&project, &now);
        let (was, is) = (samples(&reference), samples(&now));
        let difference = was
            .iter()
            .zip(&is)
            .fold(0.0f32, |worst, (a, b)| worst.max((a - b).abs()));
        let peak = was
            .iter()
            .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
        println!(
            "{name}: {} vs {} samples, peak {peak:.4}, largest difference {difference:e}",
            was.len(),
            is.len()
        );
        if was.len() != is.len() || difference > 0.0 {
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "these did not null: {failures:?}");
}
