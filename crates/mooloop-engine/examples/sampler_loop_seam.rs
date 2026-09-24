//! The sampler's loop seam with and without a crossfade, MOO-43's listening
//! case.
//!
//! A one-bar synthetic break at 120 BPM: a kick on the downbeat and the
//! "and" of 3, a snare on 2 and 4, and a low bass tone under all of it that
//! does not fit a whole number of cycles into the bar. So the loop's end
//! cuts the tone mid-cycle and the seam steps, the click a loop point makes
//! when its ends cannot meet. The break is written to a WAV, loaded into a
//! sampler that loops it, and the song is saved, reopened with
//! `mooloop_project::load_bundle` (which must repair nothing), and rendered
//! offline for four bars at each Loop fade.
//!
//! There are two loops, because they are two different sounds:
//!
//! - **From the top**: the loop is the whole sample and starts on the kick.
//!   There is no material before it, so the loop's end fades out and its
//!   first millisecond fades back in. The first version blended the end
//!   into the loop's own opening played backwards instead, and this render
//!   measured it putting a reversed kick at full level (+1.3 dB over the
//!   downbeat at 10 ms) in front of the downbeat. That's why it went.
//! - **With a lead-in**: the sample has a quarter note before the downbeat,
//!   and the loop starts at the downbeat, so the fade blends into the real
//!   material that led into it.
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/sampler-loop-seam \
//!     cargo run -p mooloop-engine --example sampler_loop_seam -- target/sampler-loop-seam
//! ```
//!
//! It measures; it does not listen (`docs/FOCUS.md`, "Listening is a step").
//!
//! Measured 2026-09-24: the step across the seam drops from 0.18 to 0.0004
//! from the top, and from 0.16 to 0.012 after the lead-in, which is the
//! kick's own attack. From the top the fade adds no level anywhere. The
//! 0 ms renders hash the same as the same song rendered before the fade
//! existed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mooloop_core::{LoopMode, NoteEvent, Project, ProjectChannel, SampleReference};
use mooloop_dsp::SampleData;
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
const BAR: usize = 96_000; // one bar at 120 BPM
const LEAD_IN: usize = BAR / 4;
const BARS: u32 = 4;
/// A sixteenth, in ticks.
const STEP: u32 = 24;
const FADES_MS: [f32; 4] = [0.0, 2.0, 10.0, 30.0];

/// A deterministic noise source, so the break is the same on every run.
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

/// One bar of break, after `lead` frames of the same break's last beat.
fn break_frames(lead: usize) -> Vec<[f32; 2]> {
    let len = lead + BAR;
    let mut out = vec![0.0_f32; len];
    let sr = SAMPLE_RATE as f32;
    // The bass tone, 73.4 Hz, all the way through: 96,000 frames is not a
    // whole number of its cycles, which is the point.
    for (n, sample) in out.iter_mut().enumerate() {
        let t = n as f32 / sr;
        *sample += 0.25 * (std::f32::consts::TAU * 73.4 * t).sin();
    }
    let hit = |at: isize, kick: bool, out: &mut [f32]| {
        let mut noise = Lcg(at as u32 ^ 0x5eed);
        let mut phase = 0.0_f32;
        for k in 0..(sr * 0.3) as usize {
            let n = at + k as isize;
            if n < 0 || n as usize >= out.len() {
                continue;
            }
            let t = k as f32 / sr;
            let value = if kick {
                let freq = 50.0 + 120.0 * (-t / 0.03).exp();
                phase += std::f32::consts::TAU * freq / sr;
                0.8 * phase.sin() * (-t / 0.12).exp()
            } else {
                0.5 * noise.next() * (-t / 0.05).exp()
                    + 0.3 * (std::f32::consts::TAU * 180.0 * t).sin() * (-t / 0.08).exp()
            };
            out[n as usize] += value;
        }
    };
    let lead = lead as isize;
    let beat = (BAR / 4) as isize;
    // The lead-in is the bar's own last beat, so the material before the
    // downbeat is what really comes before it when the break repeats.
    for bar_start in [lead - BAR as isize, lead] {
        hit(bar_start, true, &mut out);
        hit(bar_start + beat, false, &mut out);
        hit(bar_start + 5 * beat / 2, true, &mut out);
        hit(bar_start + 3 * beat, false, &mut out);
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

fn song(sample: &Path, frames: usize, loop_start: usize, fade_ms: f32) -> Project {
    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().expect("a sampler");
    state.sample = SampleReference::File {
        path: sample.to_path_buf(),
        embedded: false,
    };
    state.params.root_note = 60;
    state.params.loop_mode = LoopMode::Forward;
    state.params.loop_start = loop_start as f32 / frames as f32;
    state.params.loop_end = 1.0;
    state.params.loop_crossfade_ms = fade_ms;
    state.params.output_gain = 1.0;
    channel.setup.channel.volume = 1.0;
    channel.notes[0].push(NoteEvent::new(1, 0, BARS * 16 * STEP, 60, 127));
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

/// The left channel of a float render.
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
    let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.expect("a sample")).collect();
    samples.chunks(2).map(|pair| pair[0]).collect()
}

fn db(ratio: f32) -> f32 {
    20.0 * ratio.max(1.0e-12).log10()
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0_f32, |p, s| p.max(s.abs()))
}

fn largest_step(signal: &[f32]) -> f32 {
    signal.windows(2).fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()))
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/sampler-loop-seam".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");

    for (name, lead) in [("top", 0usize), ("lead-in", LEAD_IN)] {
        let frames = break_frames(lead);
        let wav = dir.join(format!("break-{name}.wav"));
        write_wav(&wav, &frames);
        let sample = Arc::new(SampleData {
            frames: frames.clone(),
            sample_rate: SAMPLE_RATE,
            root_note: 60,
        });
        let loop_len = frames.len() - lead;
        // Where the head wraps: after the run-in, then once a loop.
        let seams: Vec<usize> = (1..BARS as usize).map(|k| k * loop_len + lead).collect();
        let mut hard: Option<Vec<f32>> = None;
        println!("loop {name}: {} frames, loop from frame {lead}", frames.len());
        for fade in FADES_MS {
            let project = reopen(
                &song(&wav, frames.len(), lead, fade),
                &dir.join(format!("{name}-{fade}ms.mooloop")),
            );
            let out = render(&project, &sample, &dir.join(format!("{name}-{fade}ms.wav")));
            // The seam's own step, against the largest step in the quiet
            // stretch just before the loop's last 40 ms.
            let mut worst_seam = 0.0_f32;
            let mut across = 0.0_f32;
            let mut context = 0.0_f32;
            for &seam in &seams {
                worst_seam = worst_seam.max(largest_step(&out[seam - 32..seam + 32]));
                across = across.max((out[seam] - out[seam - 1]).abs());
                let quiet = seam - SAMPLE_RATE as usize / 25;
                context = context.max(largest_step(&out[quiet - 2_000..quiet]));
            }
            // What the fade did to the bar's last 40 ms and to the downbeat
            // after each seam, against the hard seam.
            let (mut end_change, mut added, mut downbeat_change) = (0.0_f32, 0.0_f32, 0.0_f32);
            let downbeat = peak(&out[seams[0]..seams[0] + SAMPLE_RATE as usize / 20]);
            if let Some(hard) = &hard {
                for &seam in &seams {
                    let tail = seam - SAMPLE_RATE as usize / 25;
                    let diff: Vec<f32> =
                        (tail..seam).map(|n| out[n] - hard[n]).collect();
                    end_change = end_change.max(peak(&diff));
                    // Level the fade put there that the hard seam did not
                    // have: a reversed hit would show here.
                    added = out[tail..seam]
                        .iter()
                        .zip(&hard[tail..seam])
                        .fold(added, |most, (o, h)| most.max(o.abs() - h.abs()));
                    let head: Vec<f32> = (seam..seam + SAMPLE_RATE as usize / 20)
                        .map(|n| out[n] - hard[n])
                        .collect();
                    downbeat_change = downbeat_change.max(peak(&head));
                }
            }
            println!(
                "  {fade:4.0} ms: step across the seam {across:.4}, largest within 32 frames \
                 {worst_seam:.4} (the quiet bar end steps at most {context:.4}); downbeat peak \
                 {:.2} dBFS; the fade changed the bar's last 40 ms by up to {:.1} dB and added \
                 at most {:.1} dB, both against the downbeat; it moved the downbeat by {}",
                db(downbeat),
                db(end_change / downbeat),
                db(added.max(0.0) / downbeat),
                if downbeat_change == 0.0 {
                    "nothing".to_string()
                } else {
                    format!("{:.1} dB under it", db(downbeat_change / downbeat))
                }
            );
            if fade == 0.0 {
                hard = Some(out);
            }
        }
    }
}
