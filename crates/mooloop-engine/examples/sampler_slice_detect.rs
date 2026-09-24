//! Transient detection on a known break, MOO-44's listening case.
//!
//! A two-bar synthetic break at 120 BPM, with a kick, a snare, hats and a
//! quiet ghost snare, where every hit's frame is known. It also has a sustained
//! bass tone under it, which must not read as hits. The detector runs at
//! three sensitivities, and each marker it finds is measured against the
//! nearest known hit. Then the break is loaded into a sampler in Slice mode
//! with the default detection accepted, saved, reopened with `load_bundle`
//! (which must repair nothing), and rendered with each slice played in
//! reverse order on the sixteenths, a chop you can hear the slices in.
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/sampler-slice-detect \
//!     cargo run -p mooloop-engine --example sampler_slice_detect -- target/sampler-slice-detect
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mooloop_core::{
    NoteEvent, PlayMode, Project, ProjectChannel, SampleReference, SliceMap,
    DEFAULT_SLICE_BASE_NOTE,
};
use mooloop_dsp::sample_analysis::{detect_onsets, OnsetSettings};
use mooloop_dsp::SampleData;
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
const SIXTEENTH: usize = 6_000; // at 120 BPM
/// A sixteenth, in ticks.
const STEP: u32 = 24;

struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    }
}

/// The break, and the frame of every hit in it.
fn break_frames() -> (Vec<[f32; 2]>, Vec<usize>) {
    let len = 32 * SIXTEENTH;
    let sr = SAMPLE_RATE as f32;
    let mut out = vec![0.0f32; len];
    for (n, sample) in out.iter_mut().enumerate() {
        *sample += 0.12 * (std::f32::consts::TAU * 55.0 * n as f32 / sr).sin();
    }
    let mut hits = Vec::new();
    for step in 0..32 {
        let at = step * SIXTEENTH;
        let (kind, gain) = match step % 16 {
            0 | 10 => (0, 0.8),
            4 | 12 => (1, 0.6),
            7 => (1, 0.12), // a ghost snare
            s if s % 2 == 0 => (2, 0.3),
            _ => continue,
        };
        hits.push(at);
        let mut noise = Lcg(step as u32 + 1);
        let mut phase = 0.0f32;
        let mut previous = 0.0f32;
        for k in 0..SIXTEENTH * 2 {
            let Some(sample) = out.get_mut(at + k) else { break };
            let t = k as f32 / sr;
            // Faded out over the last fifth, so a hit's end isn't a click.
            let tail = ((SIXTEENTH * 2 - k) as f32 / (SIXTEENTH as f32 * 0.4)).min(1.0);
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
    (out.into_iter().map(|s| [s, s]).collect(), hits)
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

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/sampler-slice-detect".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");
    let (frames, hits) = break_frames();
    let wav = dir.join("break.wav");
    write_wav(&wav, &frames);

    for sensitivity in [0.25f32, 0.5, 0.9] {
        let settings = OnsetSettings {
            sensitivity,
            ..OnsetSettings::default()
        };
        let found = detect_onsets(&frames, SAMPLE_RATE, 0, frames.len(), settings);
        let mut matched = 0;
        let mut worst_ms = 0.0f32;
        for &hit in &hits {
            if let Some(nearest) = found.iter().min_by_key(|onset| onset.abs_diff(hit)) {
                let ms = nearest.abs_diff(hit) as f32 / SAMPLE_RATE as f32 * 1_000.0;
                if ms < 10.0 {
                    matched += 1;
                    worst_ms = worst_ms.max(ms);
                }
            }
        }
        let extra = found
            .iter()
            .filter(|onset| hits.iter().all(|hit| onset.abs_diff(*hit) as f32 >= SAMPLE_RATE as f32 / 100.0))
            .count();
        println!(
            "sensitivity {sensitivity:.2}: {} markers; {matched} of {} hits found within 10 ms \
             (worst {worst_ms:.1} ms early or late), {extra} markers on no hit",
            found.len(),
            hits.len()
        );
    }

    // Accept the default detection, and chop the break with it.
    let found = detect_onsets(&frames, SAMPLE_RATE, 0, frames.len(), OnsetSettings::default());
    let mut map = SliceMap::new();
    map.merge_detected(
        &found.iter().map(|frame| *frame as u32).collect::<Vec<_>>(),
        1,
    );
    let slices = map.len();
    let mut channel = ProjectChannel::sampler(0, 1);
    let state = channel.setup.sampler_state_mut().expect("a sampler");
    state.sample = SampleReference::File {
        path: wav.clone(),
        embedded: false,
    };
    state.params.play_mode = PlayMode::Slice;
    state.params.output_gain = 1.0;
    state.slices = map;
    channel.setup.channel.volume = 1.0;
    for (n, slice) in (0..slices).rev().enumerate() {
        let note = DEFAULT_SLICE_BASE_NOTE as usize + slice;
        if note > 127 || n >= 32 {
            break;
        }
        channel.notes[0].push(NoteEvent::new(n as u32 + 1, n as u32 * STEP, STEP, note as u8, 120));
    }
    let project = Project {
        bpm: 120,
        channels: vec![channel],
        pattern_lengths: vec![32],
        ..Project::default()
    };
    let bundle = dir.join("chop.mooloop");
    mooloop_project::save_song(&bundle, &project, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let report = mooloop_project::load_bundle(&bundle).expect("the song reopens");
    assert!(report.repairs.is_empty(), "the loader repaired it: {:?}", report.repairs);
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        panic!("a song came back as something else");
    };
    let sample = Arc::new(SampleData {
        frames,
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    });
    OfflineRenderer::render(
        &reopened,
        &[Some(sample)],
        SAMPLE_RATE,
        &ExportSpec {
            path: dir.join("chop.wav"),
            scope: RenderScope::Pattern { index: 0 },
            tail_seconds: 0.5,
            format: ExportFormat::Wav(WavEncoding::Float32),
        },
    )
    .expect("the render");
    println!("{slices} slices accepted; chop.wav plays them backwards on the sixteenths");
}
