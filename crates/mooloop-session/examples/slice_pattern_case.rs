//! A sliced break played back from a pattern, MOO-46's listening case.
//!
//! A two-bar synthetic break at 120 BPM is sliced by transient detection,
//! and `slice_pattern` writes one note per slice where it falls. The sampler
//! then plays the pattern in Slice mode, and the song is saved, reopened
//! with `load_bundle` (which must repair nothing), and rendered offline. If
//! the pattern is right, the render is the break. The example compares
//! their energy envelopes and how far each note lands from its slice. Then
//! it renders the same
//! pattern with the slices reordered (every other pair swapped), which is
//! the point of having them as notes.
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/slice-pattern \
//!     cargo run -p mooloop-session --example slice_pattern_case -- target/slice-pattern
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mooloop_core::{PlayMode, Project, ProjectChannel, SampleReference, SliceMap};
use mooloop_dsp::sample_analysis::{detect_onsets, OnsetSettings};
use mooloop_dsp::SampleData;
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};
use mooloop_session::sampler::slice_pattern;

const SAMPLE_RATE: u32 = 48_000;
const SIXTEENTH: usize = 6_000; // at 120 BPM

struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

fn break_frames() -> Vec<[f32; 2]> {
    let len = 32 * SIXTEENTH;
    let sr = SAMPLE_RATE as f32;
    let mut out = vec![0.0f32; len];
    for step in 0..32 {
        let at = step * SIXTEENTH;
        let (kind, gain) = match step % 16 {
            0 | 6 | 10 => (0, 0.8),
            4 | 12 => (1, 0.6),
            s if s % 2 == 0 => (2, 0.3),
            _ => continue,
        };
        let mut noise = Lcg(step as u32 + 1);
        let mut phase = 0.0f32;
        let mut previous = 0.0f32;
        let length = SIXTEENTH * 2;
        for k in 0..length {
            let Some(sample) = out.get_mut(at + k) else { break };
            let t = k as f32 / sr;
            let tail = ((length - k) as f32 / (length as f32 * 0.2)).min(1.0);
            *sample += gain
                * tail
                * match kind {
                    0 => {
                        phase += std::f32::consts::TAU * (50.0 + 110.0 * (-t / 0.03).exp()) / sr;
                        phase.sin() * (-t / 0.1).exp()
                    }
                    1 => {
                        noise.next() * (-t / 0.05).exp()
                            + 0.5 * (std::f32::consts::TAU * 190.0 * t).sin() * (-t / 0.07).exp()
                    }
                    _ => {
                        let n = noise.next();
                        let bright = n - previous;
                        previous = n;
                        bright * (-t / 0.012).exp()
                    }
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

/// The correlation of two signals' 10 ms energy envelopes: 1 when one is
/// the other's shape at any level.
fn envelope_correlation(a: &[f32], b: &[f32]) -> f64 {
    let window = SAMPLE_RATE as usize / 100;
    let envelope = |signal: &[f32]| -> Vec<f64> {
        signal
            .chunks(window)
            .map(|chunk| chunk.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>().sqrt())
            .collect()
    };
    let (a, b) = (envelope(a), envelope(b));
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let (ma, mb) = (mean(&a), mean(&b));
    let cov: f64 = a.iter().zip(&b).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let var_a: f64 = a.iter().map(|x| (x - ma).powi(2)).sum();
    let var_b: f64 = b.iter().map(|y| (y - mb).powi(2)).sum();
    cov / (var_a * var_b).sqrt().max(1.0e-12)
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/slice-pattern".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");
    let frames = break_frames();
    let wav = dir.join("break.wav");
    write_wav(&wav, &frames);

    let found = detect_onsets(&frames, SAMPLE_RATE, 0, frames.len(), OnsetSettings::default());
    let mut slices = SliceMap::new();
    slices.merge_detected(&found.iter().map(|frame| *frame as u32).collect::<Vec<_>>(), 1);

    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().expect("a sampler");
    state.sample = SampleReference::File {
        path: wav.clone(),
        embedded: false,
    };
    state.params.play_mode = PlayMode::Slice;
    state.params.stretch_bars = 2.0;
    state.params.output_gain = 1.0;
    // One slice at a time, each played to its end: the break's own shape.
    state.params.polyphony = 1;
    state.slices = slices.clone();
    let params = state.params;
    channel.setup.channel.volume = 1.0;
    let written = slice_pattern(&params, &slices, frames.len(), None, 1);
    println!(
        "{} slices detected; the pattern holds {} notes over {} steps ({} unreachable)",
        slices.len(),
        written.notes.len(),
        written.steps,
        written.unreachable
    );
    let sample = Arc::new(SampleData {
        frames: frames.clone(),
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    });

    for (name, reorder) in [("pattern", false), ("reordered", true)] {
        let mut channel = channel.clone();
        let mut notes = written.notes.clone();
        if reorder {
            // Swap the material of each pair of neighbouring notes, keeping
            // the rhythm: the chop the pattern exists for.
            for pair in notes.chunks_mut(2) {
                if let [a, b] = pair {
                    std::mem::swap(&mut a.note, &mut b.note);
                }
            }
        }
        channel.notes[0] = notes;
        let project = Project {
            bpm: 120,
            channels: vec![channel],
            pattern_lengths: vec![written.steps as u16],
            ..Project::default()
        };
        let bundle = dir.join(format!("{name}.mooloop"));
        mooloop_project::save_song(&bundle, &project, mooloop_project::AssetMode::Referenced)
            .expect("the song saves");
        let report = mooloop_project::load_bundle(&bundle).expect("the song reopens");
        assert!(report.repairs.is_empty(), "the loader repaired it: {:?}", report.repairs);
        let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
            panic!("a song came back as something else");
        };
        let out = render(&reopened, &sample, &dir.join(format!("{name}.wav")));
        if !reorder {
            // A note starts on a whole tick (125 frames at 120 BPM), so a
            // slice can sound up to half a tick off its marker, and a
            // sample-for-sample null would measure that rather than the
            // break. So this compares their 10 ms energy envelopes, which is
            // what the ear follows in a break, and reports how far each note
            // lands from its slice.
            let n = out.len().min(frames.len());
            let original: Vec<f32> = frames[..n].iter().map(|f| f[0]).collect();
            let correlation = envelope_correlation(&original, &out[..n]);
            let frames_per_tick = mooloop_core::frames_per_bar(SAMPLE_RATE, 120.0)
                / f64::from(mooloop_core::TICKS_PER_BAR);
            let worst_ms = written
                .notes
                .iter()
                .zip(slices.markers())
                .map(|(note, marker)| {
                    (f64::from(note.start_tick) * frames_per_tick - f64::from(marker.frame)).abs()
                        / f64::from(SAMPLE_RATE)
                        * 1_000.0
                })
                .fold(0.0_f64, f64::max);
            println!(
                "the pattern's render follows the break's 10 ms envelope with correlation \
                 {correlation:.3}; the farthest note lands {worst_ms:.2} ms from its slice"
            );
        }
    }
}
