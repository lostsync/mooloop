//! How loud each ML-P8 channel with Unison above 1x plays in a real song
//! (MOO-244), rendered offline: the whole song, then each such channel with
//! every other channel muted. Run it on the tree before and after a change
//! to ML-P8's level and the difference is how far that channel's fader has
//! moved in effect.
//!
//! ```sh
//! scripts/antibox --no-incremental cargo run --release -p mooloop-session \
//!   --example mlp8_unison_levels -- target/mlp8-unison ~/perf-songs/*.mooloop
//! ```
//!
//! Prints RMS and peak in dBFS, and leaves the renders as float WAVs in the
//! output directory for listening.

use std::path::{Path, PathBuf};

use mooloop_core::{MlP8Unison, PlaybackMode, Project};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};
use mooloop_project::LoadedDocument;
use mooloop_session::document::resolve_document;

const SAMPLE_RATE: u32 = 48_000;

/// RMS and peak of a float WAV, both channels, in dBFS.
fn level(path: &Path) -> (f64, f64) {
    let mut reader = hound::WavReader::open(path).expect("the render reads back");
    let (mut sum, mut peak, mut count) = (0.0f64, 0.0f64, 0usize);
    for sample in reader.samples::<f32>() {
        let value = f64::from(sample.expect("a sample"));
        sum += value * value;
        peak = peak.max(value.abs());
        count += 1;
    }
    let db = |x: f64| 20.0 * x.max(1.0e-12).log10();
    (db((sum / count.max(1) as f64).sqrt()), db(peak))
}

fn render(project: &Project, samples: &[Option<std::sync::Arc<mooloop_dsp::SampleData>>], path: &Path) {
    let scope = if project.playback_mode == PlaybackMode::Song && !project.playlist.is_empty() {
        RenderScope::Song
    } else {
        RenderScope::Pattern {
            index: usize::from(project.current_pattern),
        }
    };
    OfflineRenderer::render(
        project,
        samples,
        SAMPLE_RATE,
        &ExportSpec {
            path: path.to_path_buf(),
            scope,
            tail_seconds: 0.0,
            format: ExportFormat::Wav(WavEncoding::Float32),
        },
    )
    .expect("the render");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().unwrap_or_else(|| "target/mlp8-unison".into()));
    std::fs::create_dir_all(&dir).expect("the output directory");
    for song in args.map(PathBuf::from) {
        let name = song.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let resolved = match resolve_document(&song) {
            Ok(resolved) => resolved,
            Err(problem) => {
                println!("{name}: does not open: {}", problem.one_line());
                continue;
            }
        };
        let LoadedDocument::Song(mut project) = resolved.report.document else {
            continue;
        };
        project.loop_range.enabled = false;
        let unison: Vec<(usize, String, MlP8Unison)> = project
            .channels
            .iter()
            .enumerate()
            .filter_map(|(index, channel)| {
                let params = channel.setup.source.mlp8_state()?.params;
                (params.unison != MlP8Unison::X1)
                    .then(|| (index, channel.setup.channel.name.clone(), params.unison))
            })
            .collect();
        if unison.is_empty() {
            continue;
        }
        let whole = dir.join(format!("{name}.wav"));
        render(&project, &resolved.samples, &whole);
        let (rms, peak) = level(&whole);
        println!("{name}: whole song  RMS {rms:6.2} dBFS  peak {peak:6.2} dBFS");
        for (index, channel, setting) in unison {
            let mut alone = project.clone();
            for (other, strip) in alone.channels.iter_mut().enumerate() {
                if other != index {
                    strip.setup.channel.muted = true;
                }
            }
            let path = dir.join(format!("{name}-{index}.wav"));
            render(&alone, &resolved.samples, &path);
            let (rms, peak) = level(&path);
            println!(
                "{name}: channel {index} {channel:?} ({setting:?})  RMS {rms:6.2} dBFS  peak {peak:6.2} dBFS"
            );
        }
    }
}
