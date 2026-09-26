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

/// Decode every key-zone file the given samplers name (MOO-14), once per
/// path. Runs on a document worker, beside the base samples' decode. A file
/// that is missing was already warned about by the load; one that fails to
/// decode is warned about here, and neither is returned.
pub fn decode_zone_files<'a>(
    samplers: impl IntoIterator<Item = (usize, &'a mooloop_core::SamplerState)>,
    warnings: &mut Vec<mooloop_project::AssetWarning>,
) -> Vec<(PathBuf, Arc<SampleData>)> {
    let mut decoded: Vec<(PathBuf, Arc<SampleData>)> = Vec::new();
    for (channel, state) in samplers {
        for zone in &state.zones {
            let mooloop_core::SampleReference::File { path, .. } = &zone.sample else {
                continue;
            };
            if !path.is_file() || decoded.iter().any(|(held, _)| held == path) {
                continue;
            }
            match audio_file::decode(path) {
                Ok(file) => decoded.push((path.clone(), file.sample)),
                Err(message) => warnings.push(mooloop_project::AssetWarning {
                    channel,
                    path: path.clone(),
                    message,
                }),
            }
        }
    }
    decoded
}

/// A sampler's zone buffers in zone order, from what
/// [`decode_zone_files`] returned: the shape an offline render takes.
pub fn zone_buffers(
    state: &mooloop_core::SamplerState,
    decoded: &[(PathBuf, Arc<SampleData>)],
) -> Vec<Option<Arc<SampleData>>> {
    state
        .zones
        .iter()
        .map(|zone| match &zone.sample {
            mooloop_core::SampleReference::File { path, .. } => decoded
                .iter()
                .find(|(held, _)| held == path)
                .map(|(_, sample)| sample.clone()),
            _ => None,
        })
        .collect()
}

pub struct LoadedSample {
    pub path: PathBuf,
    pub sample: Arc<SampleData>,
    pub can_previous: bool,
    pub can_next: bool,
}

/// Result of a background sample load, delivered to the pump.
pub struct LoadResult {
    pub channel: usize,
    pub source_revision: u64,
    /// Which *request* this completion answers.
    ///
    /// `source_revision` is a property of the project, not of a request, so
    /// two in-flight loads for one channel both match it and the last one to
    /// finish wins. Completion order is decode time, which is file size and
    /// page cache: load a long file, change your mind and load a short one,
    /// and the short one lands first and is overwritten. The user clicks
    /// sample B and hears sample A.
    ///
    /// Zero for a `new_channel` load, which is not keyed by channel because
    /// the channel does not exist yet; see `Session::next_sample_request`.
    pub request: u64,
    /// Load into a sampler channel created on arrival (`channel` is then the
    /// index the new channel will take) rather than an existing one.
    pub new_channel: bool,
    /// Add the file as a new key zone of `channel` (MOO-14) rather than
    /// replace its sample.
    pub zone: bool,
    /// `None` = dialog cancelled; `Some(Err)` = decode failed.
    pub result: Option<Result<LoadedSample, String>>,
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

/// Decode the file at `path` for a sampler.
///
/// The error names the file, because it is shown to the user
/// (`status_bar::notify` in `mooloop-ui`), and a decode failure that arrives
/// after a click on a different file has to say which one it was.
pub fn load_sample_at_path(path: &Path) -> Result<LoadedSample, String> {
    let named = |error: String| format!("{}: {error}", browser_display_name(path));
    let files = sample_files_in_directory(path).map_err(named)?;
    let index = sample_index(path, &files);
    let sample = audio_file::decode(path).map_err(named)?.sample;
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

/// How long an instrument preset's audition runs, in sixteenths at 120 BPM:
/// one bar of phrase, then the render's tail.
const AUDITION_STEPS: u32 = 16;
/// The audition's tail after the bar, so a long release is heard ending.
const AUDITION_TAIL_S: f32 = 1.0;

/// What a click on an instrument preset in the browser plays (MOO-227): the
/// preset rendered offline on a throwaway one-channel song, as a sample the
/// browser's preview voice plays like any other file.
///
/// Rendered rather than hosted live because the preview voice already plays
/// a buffer, bypassing the song's chains and mute, and a render needs no new
/// realtime path: nothing reaches the channel's own chain, nothing is
/// recorded, and the song is untouched. The phrase depends on the kind: a
/// bar of hits for the drum devices, a rising arpeggio into a held chord
/// for everything pitched, and the root and its fifth for a sampler.
///
/// A channel preset is auditioned with its inserts, which are part of what
/// it is. An effect preset has no audition yet: what it should sound like is
/// Adam's question on MOO-227.
pub fn audition_preset(path: &Path, sample_rate: u32) -> Result<SampleInspection, String> {
    use mooloop_core::{ChannelSource, DeviceKind, ProjectChannel};
    use mooloop_project::LoadedDocument;

    let report = mooloop_project::load_bundle(path).map_err(|error| error.to_string())?;
    let mut channel = ProjectChannel::sampler(0, 1);
    match report.document {
        LoadedDocument::Generator(source) => channel.setup.source = *source,
        LoadedDocument::Channel(setup) => channel.setup = *setup,
        _ => return Err("Only instrument and channel presets can be auditioned".into()),
    }
    let kind = channel.setup.source.kind();
    // A zoned sampler preset is auditioned with its zones (MOO-14): the
    // render refuses a song whose zone audio it was not given.
    let zones = match &channel.setup.source {
        ChannelSource::Sampler(state) => {
            let decoded = decode_zone_files([(0, state)], &mut Vec::new());
            vec![zone_buffers(state, &decoded)]
        }
        _ => Vec::new(),
    };
    let sample = match &channel.setup.source {
        ChannelSource::Sampler(state) => match &state.sample {
            mooloop_core::SampleReference::File { path, .. } => {
                Some(audio_file::decode(path)?.sample)
            }
            _ => return Err("This sampler preset has no sample to play".into()),
        },
        ChannelSource::AuxIn(_) | ChannelSource::Plugin(_) => {
            return Err(format!("A {} preset has nothing to audition", kind.label()));
        }
        _ => None,
    };
    // (start sixteenth, length in sixteenths, note, velocity)
    let phrase: &[(u32, u32, u8, u8)] = match kind {
        DeviceKind::DrumSynth | DeviceKind::Ds01 => {
            &[(0, 2, 60, 120), (4, 2, 60, 90), (8, 2, 60, 120), (12, 1, 60, 70), (14, 2, 60, 100)]
        }
        DeviceKind::Sampler => {
            let root = match &channel.setup.source {
                ChannelSource::Sampler(state) => state.params.root_note.min(120),
                _ => 60,
            };
            return render_audition(
                path,
                kind,
                channel,
                &[(0, 6, root, 110), (8, 8, root + 7, 110)],
                sample,
                zones,
                sample_rate,
            );
        }
        _ => &[
            (0, 2, 48, 110),
            (2, 2, 52, 100),
            (4, 2, 55, 100),
            (6, 2, 60, 110),
            (8, 8, 48, 100),
            (8, 8, 52, 100),
            (8, 8, 55, 100),
            (8, 8, 60, 100),
        ],
    };
    render_audition(path, kind, channel, phrase, sample, zones, sample_rate)
}

/// Render `phrase` on `channel` alone and describe the result for the
/// browser's info pane. [`audition_preset`]'s second half.
fn render_audition(
    path: &Path,
    kind: mooloop_core::DeviceKind,
    mut channel: mooloop_core::ProjectChannel,
    phrase: &[(u32, u32, u8, u8)],
    sample: Option<Arc<SampleData>>,
    zones: Vec<Vec<Option<Arc<SampleData>>>>,
    sample_rate: u32,
) -> Result<SampleInspection, String> {
        use mooloop_core::{NoteEvent, Project};
        use mooloop_engine::{
            ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderJob, RenderScope,
            WavEncoding,
        };
        const STEP: u32 = mooloop_core::TICKS_PER_STEP;
        for (id, &(start, length, note, velocity)) in phrase.iter().enumerate() {
            channel.notes[0].push(NoteEvent::new(
                id as u32 + 1,
                start * STEP,
                length * STEP,
                note,
                velocity,
            ));
        }
        let project = Project {
            bpm: 120,
            channels: vec![channel],
            pattern_lengths: vec![AUDITION_STEPS as u16],
            ..Project::default()
        };
        // A render writes a file, so the audition goes through one in the
        // temporary directory and is read straight back.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let temp = std::env::temp_dir().join(format!(
            "mooloop-audition-{}-{}.wav",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        OfflineRenderer::render_job_with_plugins(
            &project,
            &[sample],
            &zones,
            sample_rate,
            &RenderJob::single(&ExportSpec {
                path: temp.clone(),
                scope: RenderScope::Pattern { index: 0 },
                tail_seconds: AUDITION_TAIL_S,
                format: ExportFormat::Wav(WavEncoding::Float32),
            }),
            &ExportProgress::new(),
            &mut |_| std::collections::BTreeMap::new(),
        )
        .map_err(|failure| format!("The audition did not render: {failure}"))?;
        let decoded = audio_file::decode(&temp);
        let _ = std::fs::remove_file(&temp);
        let rendered = decoded?.sample;
        Ok(SampleInspection {
            name: browser_display_name(path),
            // The full name: the preset browser has the room for it (Adam,
            // 2026-09-26, both where there's room, the model number where
            // it's tight), and the 260px pane fits "Polyneight ML-P8 preset
            // · audition" at 9px.
            stats: format!(
                "{} preset · audition\n{:.2} s",
                kind.title(),
                sample_duration(&rendered)
            ),
            peaks: waveform_peaks(&rendered, BROWSER_INFO_BINS),
            sample: rendered,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click on an instrument preset plays it (MOO-227): the preset is
    /// rendered offline to a sample the browser's preview voice plays, a bar
    /// of phrase plus a tail, with sound in it and the song untouched. An
    /// effect preset says it has no audition rather than playing nothing.
    #[test]
    fn an_instrument_preset_auditions_as_a_rendered_phrase() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("pad.mooloop");
        let source = mooloop_core::ChannelSource::MlP8(mooloop_core::MlP8State::default());
        mooloop_project::save_generator_preset(
            &path,
            &source,
            mooloop_project::PresetInfo {
                name: "Pad".into(),
                category: String::new(),
                tags: Vec::new(),
            },
            mooloop_project::AssetMode::Referenced,
        )
        .unwrap();
        let audition = audition_preset(&path, 48_000).expect("the preset auditions");
        let seconds = sample_duration(&audition.sample);
        // The bar is 2 s at 120 BPM; the export ends its tail once the
        // render has fallen silent, so a short release adds little.
        assert!((1.9..3.5).contains(&seconds), "the audition lasted {seconds} s");
        let peak = audition
            .sample
            .frames
            .iter()
            .fold(0.0_f32, |p, f| p.max(f[0].abs()).max(f[1].abs()));
        assert!(peak > 0.01, "the audition was silent");
        assert!(
            audition.stats.starts_with("Polyneight ML-P8 preset"),
            "{}",
            audition.stats
        );

        let effect = temp.path().join("delay.mooloop");
        mooloop_project::save_effect_preset(
            &effect,
            &mooloop_core::EffectSlotState::new(mooloop_core::EffectKind::Drive.default_params()),
            mooloop_project::PresetInfo {
                name: "Delay".into(),
                category: String::new(),
                tags: Vec::new(),
            },
            mooloop_project::AssetMode::Referenced,
        )
        .unwrap();
        assert!(audition_preset(&effect, 48_000).is_err());
    }

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
