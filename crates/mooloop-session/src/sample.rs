//! Sample loading, measurement, and description.
//!
//! Everything here works on decoded audio and filesystem paths, so it is
//! shared by the browser, the sampler editor, and the document loader
//! without any of them needing a view layer.

use crate::audio_file;
use crate::browser::browser_display_name;
use mooloop_core::SamplerParams;
use mooloop_dsp::SampleData;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct LoadedSample {
    pub path: PathBuf,
    pub sample: Arc<SampleData>,
    pub can_previous: bool,
    pub can_next: bool,
}

pub fn sample_files_in_directory(path: &Path) -> Result<Vec<PathBuf>, String> {
    let directory = path
        .parent()
        .ok_or_else(|| "sample path has no parent directory".to_string())?;
    let mut files = std::fs::read_dir(directory)
        .map_err(|e| format!("could not read sample directory: {e}"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|entry| entry.is_file() && audio_file::is_supported_extension(entry))
        .collect::<Vec<_>>();
    files.sort_by_cached_key(|entry| {
        entry
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    Ok(files)
}

pub fn sample_index(path: &Path, files: &[PathBuf]) -> Option<usize> {
    files
        .iter()
        .position(|candidate| candidate == path)
        .or_else(|| {
            let name = path.file_name()?;
            files
                .iter()
                .position(|candidate| candidate.file_name() == Some(name))
        })
}

pub fn adjacent_sample(path: &Path, direction: isize) -> Result<Option<PathBuf>, String> {
    let files = sample_files_in_directory(path)?;
    let Some(index) = sample_index(path, &files) else {
        return Ok(None);
    };
    let next = index as isize + direction;
    Ok((next >= 0)
        .then(|| files.get(next as usize).cloned())
        .flatten())
}

pub fn load_sample_at_path(path: &Path) -> Result<LoadedSample, String> {
    let files = sample_files_in_directory(path)?;
    let index = sample_index(path, &files);
    let sample = audio_file::decode(path)?.sample;
    Ok(LoadedSample {
        path: path.to_path_buf(),
        sample,
        can_previous: index.is_some_and(|index| index > 0),
        can_next: index.is_some_and(|index| index + 1 < files.len()),
    })
}

pub fn waveform_peaks(sample: &SampleData, max_bins: usize) -> Vec<f32> {
    peaks_from_frames(&sample.frames, max_bins)
}

/// Like `waveform_peaks`, but bins only the frames in `[start_frame,
/// end_frame)`. Used to re-derive real detail for whatever range the
/// waveform view is zoomed/scrolled to, rather than just stretching the
/// full-sample overview's fixed bins.
pub fn waveform_peaks_windowed(
    sample: &SampleData,
    max_bins: usize,
    start_frame: usize,
    end_frame: usize,
) -> Vec<f32> {
    let len = sample.frames.len();
    let start = start_frame.min(len);
    let end = end_frame.clamp(start, len);
    peaks_from_frames(&sample.frames[start..end], max_bins)
}

fn peaks_from_frames(frames: &[[f32; 2]], max_bins: usize) -> Vec<f32> {
    if frames.is_empty() || max_bins == 0 {
        return Vec::new();
    }
    let bins = max_bins.min(frames.len());
    let mut peaks = (0..bins)
        .map(|bin| {
            let start = bin * frames.len() / bins;
            let end = ((bin + 1) * frames.len() / bins).max(start + 1);
            frames[start..end]
                .iter()
                .map(|frame| frame[0].abs().max(frame[1].abs()))
                .fold(0.0f32, f32::max)
        })
        .collect::<Vec<_>>();
    let peak = peaks.iter().copied().fold(0.0f32, f32::max);
    if peak > 0.0 {
        for value in &mut peaks {
            *value /= peak;
        }
    }
    peaks
}

pub fn sample_description(sample: &SampleData) -> String {
    let seconds = f64::from(sample_duration(sample));
    format!("{seconds:.3} s  |  {} Hz  |  stereo", sample.sample_rate)
}

pub fn sample_duration(sample: &SampleData) -> f32 {
    sample.len() as f32 / sample.sample_rate.max(1) as f32
}

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Nearest note name for a (possibly fractional) MIDI note number. A4 = 69.
fn midi_to_note_name(midi: f64) -> String {
    let rounded = midi.round().clamp(0.0, 127.0) as i64;
    let name = NOTE_NAMES[rounded.rem_euclid(12) as usize];
    let octave = rounded / 12 - 1;
    format!("{name}{octave}")
}

fn midi_to_frequency_hz(midi: f64) -> f32 {
    (440.0 * 2f64.powf((midi - 69.0) / 12.0)) as f32
}

/// The note name and frequency the sampler's root note actually plays at
/// once coarse/fine tuning are applied — the musically meaningful readout
/// for the Coarse/Fine knob pair, since "+3 st / +40 ct" alone doesn't say
/// what pitch that is.
pub fn tune_label(params: SamplerParams) -> String {
    let midi = f64::from(params.root_note)
        + f64::from(params.tune_semitones)
        + f64::from(params.tune_cents) / 100.0;
    format!(
        "{} · {:.1} Hz",
        midi_to_note_name(midi),
        midi_to_frequency_hz(midi)
    )
}

/// Bins for the info pane's waveform. The pane is a couple of inches wide;
/// more bins would be sub-pixel detail.
const BROWSER_INFO_BINS: usize = 128;

/// Everything the sidebar's info pane shows about one inspected file, plus
/// the decoded sample itself for the preview voice.
pub struct SampleInspection {
    pub name: String,
    pub stats: String,
    pub peaks: Vec<f32>,
    pub sample: Arc<SampleData>,
}

/// Decodes a sample once for the pane's waveform, source stats, and preview
/// voice.
pub fn inspect_sample(path: &Path) -> Result<SampleInspection, String> {
    let decoded = audio_file::decode(path)?;
    let sample = decoded.sample;
    let channels = match decoded.source_channels {
        1 => "mono".to_owned(),
        2 => "stereo".to_owned(),
        other => format!("{other}ch"),
    };
    let format = decoded.bits_per_sample.map_or_else(
        || decoded.codec_name.to_uppercase(),
        |bits| format!("{bits}-bit {}", decoded.codec_name),
    );
    let stats = format!(
        "{} Hz · {} · {}\n{} frames · {:.2} s",
        sample.sample_rate,
        format,
        channels,
        sample.frames.len(),
        sample_duration(&sample)
    );
    Ok(SampleInspection {
        name: browser_display_name(path),
        stats,
        peaks: waveform_peaks(&sample, BROWSER_INFO_BINS),
        sample,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspect_sample_reports_header_stats_and_peaks() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("tone.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for index in 0..4_410 {
            writer.write_sample(((index % 100) * 200) as i16).unwrap();
        }
        writer.finalize().unwrap();

        let inspection = inspect_sample(&path).unwrap();
        assert_eq!(inspection.name, "tone.wav");
        assert!(
            inspection.stats.contains("44100 Hz"),
            "{}",
            inspection.stats
        );
        assert!(
            inspection.stats.contains("16-bit"),
            "{}",
            inspection.stats
        );
        assert!(inspection.stats.contains("mono"), "{}", inspection.stats);
        assert_eq!(inspection.sample.frames.len(), 4_410);
        assert_eq!(inspection.peaks.len(), BROWSER_INFO_BINS);
        assert!(inspection.peaks.iter().any(|peak| *peak > 0.0));

        // Anything the decoder cannot open never reaches the pane.
        let text = temp.path().join("noise.wav");
        std::fs::write(&text, b"definitely not RIFF").unwrap();
        assert!(inspect_sample(&text).is_err());
    }

    #[test]
    fn decodes_16bit_stereo_wav() {
        let path = std::env::temp_dir().join("mooloop_decode_test_16bit.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..1000i32 {
            let v = ((i % 100) * 300 - 15_000) as i16;
            writer.write_sample(v).unwrap();
            writer.write_sample(v).unwrap();
        }
        writer.finalize().unwrap();

        let data = audio_file::decode(&path).unwrap().sample;
        assert_eq!(data.sample_rate, 44_100);
        assert_eq!(data.len(), 1000);
        assert!(data.frames.iter().any(|f| f[0] != 0.0));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_garbage_file() {
        let path = std::env::temp_dir().join("mooloop_decode_test_garbage.wav");
        std::fs::write(&path, b"not a wav at all").unwrap();
        assert!(audio_file::decode(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn waveform_peaks_are_normalized_and_bounded() {
        let sample = SampleData {
            frames: vec![[0.0, 0.0], [0.25, -0.5], [1.0, -0.75], [0.1, 0.2]],
            sample_rate: 48_000,
            root_note: 60,
        };

        let peaks = waveform_peaks(&sample, 2);

        assert_eq!(peaks, vec![0.5, 1.0]);
    }

    #[test]
    fn adjacent_sample_walks_mixed_formats_without_wrapping() {
        let directory = std::env::temp_dir().join(format!(
            "mooloop_sample_browser_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let a = directory.join("a-kick.wav");
        let b = directory.join("B-snare.FLAC");
        let c = directory.join("c-hat.mp3");
        for path in [&a, &b, &c] {
            std::fs::write(path, []).unwrap();
        }
        std::fs::write(directory.join("ignore.txt"), []).unwrap();

        assert_eq!(adjacent_sample(&a, -1).unwrap(), None);
        assert_eq!(adjacent_sample(&a, 1).unwrap(), Some(b.clone()));
        assert_eq!(adjacent_sample(&b, 1).unwrap(), Some(c.clone()));
        assert_eq!(adjacent_sample(&c, 1).unwrap(), None);

        std::fs::remove_dir_all(directory).unwrap();
    }
}
