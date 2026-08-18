//! Offline rendering and WAV/MP3 encoding.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mooloop_core::{PlaybackMode, Project};
use mooloop_dsp::{SampleData, MAX_BLOCK_SIZE};
use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushGap, Quality};

use crate::render::RenderState;

const MP3_SAMPLE_RATE: u32 = 48_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderScope {
    Pattern { index: usize },
    Song,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavEncoding {
    Pcm24,
    Float32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Bitrate {
    Kbps192,
    Kbps256,
    Kbps320,
}

impl Mp3Bitrate {
    fn lame(self) -> Bitrate {
        match self {
            Self::Kbps192 => Bitrate::Kbps192,
            Self::Kbps256 => Bitrate::Kbps256,
            Self::Kbps320 => Bitrate::Kbps320,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Wav(WavEncoding),
    Mp3(Mp3Bitrate),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportSpec {
    pub path: PathBuf,
    pub scope: RenderScope,
    pub tail_seconds: f32,
    pub format: ExportFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderSummary {
    pub sample_rate: u32,
    pub base_frames: u64,
    pub tail_frames: u64,
    pub total_frames: u64,
}

#[derive(Debug)]
pub enum ExportError {
    Invalid(String),
    Io(std::io::Error),
    Wav(hound::Error),
    Mp3(String),
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid export: {message}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Wav(error) => write!(f, "WAV encoding failed: {error}"),
            Self::Mp3(error) => write!(f, "MP3 encoding failed: {error}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<std::io::Error> for ExportError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<hound::Error> for ExportError {
    fn from(value: hound::Error) -> Self {
        Self::Wav(value)
    }
}

pub struct OfflineRenderer;

impl OfflineRenderer {
    pub fn render(
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
        realtime_sample_rate: u32,
        spec: &ExportSpec,
    ) -> Result<RenderSummary, ExportError> {
        if !spec.tail_seconds.is_finite() || !(0.0..=30.0).contains(&spec.tail_seconds) {
            return Err(ExportError::Invalid(
                "tail duration must be between 0 and 30 seconds".into(),
            ));
        }
        let mut render_project = project.clone();
        match spec.scope {
            RenderScope::Pattern { index } => {
                if index >= render_project.pattern_lengths.len() {
                    return Err(ExportError::Invalid("pattern is out of range".into()));
                }
                render_project.playback_mode = PlaybackMode::Pattern;
                render_project.current_pattern = index as u16;
            }
            RenderScope::Song => render_project.playback_mode = PlaybackMode::Song,
        }
        let sample_rate = match spec.format {
            ExportFormat::Wav(_) => realtime_sample_rate,
            ExportFormat::Mp3(_) => MP3_SAMPLE_RATE,
        };
        if sample_rate == 0 {
            return Err(ExportError::Invalid("sample rate cannot be zero".into()));
        }

        let mut state = RenderState::from_project(sample_rate, &render_project, samples);
        let base_ticks = match spec.scope {
            RenderScope::Pattern { index } => state
                .pattern_length_ticks(index)
                .ok_or_else(|| ExportError::Invalid("pattern is out of range".into()))?,
            RenderScope::Song => state.song_length_ticks(),
        };
        let base_frames = (f64::from(base_ticks) / state.ticks_per_sample()).ceil() as u64;
        let tail_frames = (f64::from(spec.tail_seconds) * f64::from(sample_rate)).round() as u64;
        let summary = RenderSummary {
            sample_rate,
            base_frames,
            tail_frames,
            total_frames: base_frames.saturating_add(tail_frames),
        };

        let temporary = temporary_path(&spec.path);
        let result = match spec.format {
            ExportFormat::Wav(encoding) => render_wav(&temporary, &mut state, summary, encoding),
            ExportFormat::Mp3(bitrate) => render_mp3(&temporary, &mut state, summary, bitrate),
        };
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if spec.path.exists() {
            fs::remove_file(&spec.path)?;
        }
        fs::rename(temporary, &spec.path)?;
        Ok(summary)
    }
}

fn temporary_path(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mooloop-export");
    parent.join(format!(".{name}.part-{}", std::process::id()))
}

fn render_blocks(
    state: &mut RenderState,
    summary: RenderSummary,
    mut sink: impl FnMut(&[f32], &[f32]) -> Result<(), ExportError>,
) -> Result<(), ExportError> {
    state.play();
    let mut remaining = summary.base_frames;
    while remaining > 0 {
        let frames = remaining.min(MAX_BLOCK_SIZE as u64) as usize;
        state.process_once_block(frames);
        sink(&state.master().l[..frames], &state.master().r[..frames])?;
        remaining -= frames as u64;
    }

    state.pause();
    let mut remaining = summary.tail_frames;
    while remaining > 0 {
        let frames = remaining.min(MAX_BLOCK_SIZE as u64) as usize;
        state.process_once_block(frames);
        sink(&state.master().l[..frames], &state.master().r[..frames])?;
        remaining -= frames as u64;
    }
    Ok(())
}

fn render_wav(
    path: &Path,
    state: &mut RenderState,
    summary: RenderSummary,
    encoding: WavEncoding,
) -> Result<(), ExportError> {
    let spec = match encoding {
        WavEncoding::Pcm24 => hound::WavSpec {
            channels: 2,
            sample_rate: summary.sample_rate,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        },
        WavEncoding::Float32 => hound::WavSpec {
            channels: 2,
            sample_rate: summary.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    render_blocks(state, summary, |left, right| {
        for (&left, &right) in left.iter().zip(right) {
            match encoding {
                WavEncoding::Pcm24 => {
                    writer.write_sample(pcm24(left))?;
                    writer.write_sample(pcm24(right))?;
                }
                WavEncoding::Float32 => {
                    writer.write_sample(left)?;
                    writer.write_sample(right)?;
                }
            }
        }
        Ok(())
    })?;
    writer.finalize()?;
    Ok(())
}

fn pcm24(sample: f32) -> i32 {
    (sample.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32
}

fn render_mp3(
    path: &Path,
    state: &mut RenderState,
    summary: RenderSummary,
    bitrate: Mp3Bitrate,
) -> Result<(), ExportError> {
    let mut encoder = Builder::new()
        .ok_or_else(|| ExportError::Mp3("could not initialize LAME".into()))?
        .with_num_channels(2)
        .map_err(|error| ExportError::Mp3(error.to_string()))?
        .with_sample_rate(summary.sample_rate)
        .map_err(|error| ExportError::Mp3(error.to_string()))?
        .with_brate(bitrate.lame())
        .map_err(|error| ExportError::Mp3(error.to_string()))?
        .with_quality(Quality::Best)
        .map_err(|error| ExportError::Mp3(error.to_string()))?
        .build()
        .map_err(|error| ExportError::Mp3(error.to_string()))?;
    let mut encoded = Vec::new();
    render_blocks(state, summary, |left, right| {
        encoded.reserve(mp3lame_encoder::max_required_buffer_size(left.len()));
        encoder
            .encode_to_vec(DualPcm { left, right }, &mut encoded)
            .map_err(|error| ExportError::Mp3(error.to_string()))?;
        Ok(())
    })?;
    encoded.reserve(7200);
    encoder
        .flush_to_vec::<FlushGap>(&mut encoded)
        .map_err(|error| ExportError::Mp3(error.to_string()))?;
    let mut file = fs::File::create(path)?;
    file.write_all(&encoded)?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{NoteEvent, PatternPlacement};
    use tempfile::tempdir;

    fn audible_project() -> Project {
        let mut project = Project::default();
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 24, 60, 127));
        project
    }

    #[test]
    fn pattern_wav_has_one_pass_plus_exact_tail() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("pattern.wav");
        let project = audible_project();
        let summary = OfflineRenderer::render(
            &project,
            &[],
            48_000,
            &ExportSpec {
                path: path.clone(),
                scope: RenderScope::Pattern { index: 0 },
                tail_seconds: 0.5,
                format: ExportFormat::Wav(WavEncoding::Float32),
            },
        )
        .unwrap();
        assert_eq!(summary.base_frames, 96_000);
        assert_eq!(summary.tail_frames, 24_000);
        let reader = hound::WavReader::open(path).unwrap();
        assert_eq!(u64::from(reader.duration()), summary.total_frames);
    }

    #[test]
    fn song_scope_uses_derived_playlist_length() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("song.wav");
        let mut project = audible_project();
        project.playlist.push(PatternPlacement::new(0, 384));
        let summary = OfflineRenderer::render(
            &project,
            &[],
            48_000,
            &ExportSpec {
                path,
                scope: RenderScope::Song,
                tail_seconds: 0.0,
                format: ExportFormat::Wav(WavEncoding::Pcm24),
            },
        )
        .unwrap();
        assert_eq!(summary.base_frames, 192_000);
    }

    #[test]
    fn mp3_export_writes_encoded_frames() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("pattern.mp3");
        OfflineRenderer::render(
            &audible_project(),
            &[],
            48_000,
            &ExportSpec {
                path: path.clone(),
                scope: RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format: ExportFormat::Mp3(Mp3Bitrate::Kbps192),
            },
        )
        .unwrap();
        let bytes = fs::read(path).unwrap();
        assert!(bytes.len() > 1_000);
        assert!(bytes
            .windows(2)
            .any(|pair| pair[0] == 0xff && pair[1] & 0xe0 == 0xe0));
    }
}
