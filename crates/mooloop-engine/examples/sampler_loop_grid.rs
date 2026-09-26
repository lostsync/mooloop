//! A lane sweeping a sampler's Loop start, with the loop grid off and on,
//! MOO-47's listening case.
//!
//! A one-bar synthetic break loops for four bars while an automation lane
//! sweeps Loop start from the top of the bar to its last quarter. Off, the
//! loop's start slides through every frame, which is noise. On a sixteenth
//! grid it steps through the break's sixteenths, which is a rhythm: the loop
//! gets shorter a sixteenth at a time, from the back of the bar.
//!
//! Authored with the real `mooloop_core` types, saved, reopened with
//! `load_bundle` (which must repair nothing), and rendered offline. What it
//! prints is the resolver's own answer: every loop start the lane's values
//! resolve to at the control rate, using the function the voice calls. It
//! counts them, off and on.
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/sampler-loop-grid \
//!     cargo run -p mooloop-engine --example sampler_loop_grid -- target/sampler-loop-grid
//! ```
//!
//! Then play `off.wav` and `sixteenth.wav` (`docs/FOCUS.md`, "Listening is
//! a step").

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mooloop_core::generator::SAMPLER_PARAM_LOOP_START;
use mooloop_core::sampler::LoopQuantize;
use mooloop_core::{
    AutomationLane, AutomationPoint, EffectTarget, LoopMode, NoteEvent, ParamAddr, ParamOwner,
    Project, ProjectChannel, SampleReference,
};
use mooloop_dsp::{SampleData, Sampler};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
const BAR: usize = 96_000; // one bar at 120 BPM
const BARS: u32 = 4;
/// A sixteenth, in ticks.
const STEP: u32 = 24;

/// One bar of hits, a kick or a hat on every sixteenth, so a loop that
/// starts on the grid starts on a hit.
fn break_frames() -> Vec<[f32; 2]> {
    let sr = SAMPLE_RATE as f32;
    let mut out = vec![0.0_f32; BAR];
    for sixteenth in 0..16 {
        let at = sixteenth * BAR / 16;
        let kick = sixteenth % 4 == 0;
        let mut phase = 0.0_f32;
        let mut seed = sixteenth as u32 * 7_919 + 1;
        for k in 0..(sr * 0.1) as usize {
            let Some(sample) = out.get_mut(at + k) else { break };
            let t = k as f32 / sr;
            *sample += if kick {
                phase += std::f32::consts::TAU * (50.0 + 100.0 * (-t / 0.02).exp()) / sr;
                0.8 * phase.sin() * (-t / 0.06).exp()
            } else {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0;
                0.25 * noise * (-t / 0.01).exp()
            };
        }
    }
    out.into_iter().map(|s| [s, s]).collect()
}

fn write_wav(path: &Path, frames: &[[f32; 2]]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("the break writes");
    for frame in frames {
        writer.write_sample(frame[0]).expect("a sample");
        writer.write_sample(frame[1]).expect("a sample");
    }
    writer.finalize().expect("the break finalizes");
}

/// The lane: Loop start from 0 to 0.75 across the four bars.
fn lane() -> AutomationLane {
    let mut lane = AutomationLane::new(ParamAddr {
        scope: EffectTarget::Channel(0),
        owner: ParamOwner::source(mooloop_core::DeviceKind::Sampler),
        param: SAMPLER_PARAM_LOOP_START,
    });
    lane.upsert(AutomationPoint::new(1, 0, 0.0));
    lane.upsert(AutomationPoint::new(2, BARS * 16 * STEP - 1, 0.75));
    lane
}

fn song(sample: &Path, grid: LoopQuantize) -> Project {
    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().expect("a sampler");
    state.sample = SampleReference::File {
        path: sample.to_path_buf(),
        embedded: false,
    };
    state.params.loop_mode = LoopMode::Forward;
    state.params.loop_start = 0.0;
    state.params.loop_end = 1.0;
    state.params.loop_quantize = grid;
    state.params.stretch_bars = 1.0;
    state.params.output_gain = 1.0;
    channel.setup.channel.volume = 1.0;
    channel.notes[0].push(NoteEvent::new(1, 0, BARS * 16 * STEP, 60, 127));
    channel.automation[0].push(lane());
    Project {
        bpm: 120,
        channels: vec![channel],
        pattern_lengths: vec![(BARS * 16) as u16],
        ..Project::default()
    }
}

fn reopen(project: &Project, path: &Path) -> Project {
    mooloop_project::save_song(path, project, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let report = mooloop_project::load_bundle(path).expect("the song reopens");
    assert!(report.repairs.is_empty(), "the loader repaired it: {:?}", report.repairs);
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        panic!("a song came back as something else");
    };
    reopened
}

fn render(project: &Project, sample: &Arc<SampleData>, path: &Path) -> Vec<f32> {
    OfflineRenderer::render(
        project,
        &[Some(sample.clone())],
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

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/sampler-loop-grid".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");
    let frames = break_frames();
    let wav = dir.join("break.wav");
    write_wav(&wav, &frames);
    let sample = Arc::new(SampleData {
        frames,
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    });

    let mut renders = Vec::new();
    for (name, grid) in [("off", LoopQuantize::Off), ("sixteenth", LoopQuantize::Sixteenth)] {
        let project = reopen(&song(&wav, grid), &dir.join(format!("{name}.mooloop")));
        let params = project.channels[0]
            .setup
            .sampler_state()
            .expect("a sampler")
            .params;
        // The lane's value at every control tick (32 frames), resolved the
        // way the voice resolves it.
        let ticks = BARS as usize * BAR / 32;
        let mut starts: Vec<f64> = (0..ticks)
            .map(|tick| {
                let value = 0.75 * tick as f32 / ticks as f32;
                let params = mooloop_core::SamplerParams {
                    loop_start: value,
                    ..params
                };
                Sampler::resolve_loop_bounds(params, BAR, None, None).0
            })
            .collect();
        starts.dedup();
        let out = render(&project, &sample, &dir.join(format!("{name}.wav")));
        let peak = out.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
        println!(
            "{name:>9}: the lane's {ticks} control ticks resolve to {} loop starts; peak {:.2} dBFS",
            starts.len(),
            20.0 * peak.max(1.0e-9).log10()
        );
        renders.push(out);
    }
    let differ = renders[0] != renders[1];
    println!("the two renders {}", if differ { "differ" } else { "are the same file" });
}
