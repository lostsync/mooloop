//! Offline rendering and WAV/MP3 encoding.

use std::fmt;
use std::fs;
use std::io::Write;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use mooloop_core::{PlaybackMode, Project};
use mooloop_dsp::{SampleData, MAX_BLOCK_SIZE};
use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushGap, Quality};

use crate::render::RenderState;

/// The rate an MP3 file is written at, for a render at `rate`.
///
/// The project always renders at the session's rate, so what is encoded is
/// what was heard (MOO-125); MP3 itself stops at 48 kHz, so LAME converts a
/// faster render down to the rate of its own family -- 88.2 and 176.4 kHz to
/// 44.1, everything else above 48 kHz to 48 -- as it encodes. A rate MP3
/// already has is written as it is.
fn mp3_file_rate(rate: u32) -> u32 {
    const LEGAL: [u32; 9] = [
        8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
    ];
    if LEGAL.contains(&rate) {
        rate
    } else if rate.is_multiple_of(44_100) {
        44_100
    } else {
        48_000
    }
}

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
    /// The most tail the render may add after the last bar, 0 to 30 s. The
    /// tail stops as soon as every device has fallen silent
    /// (`RenderState::is_at_rest`), so this is a cap, not a length.
    pub tail_seconds: f32,
    pub format: ExportFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderSummary {
    /// The rate the project was rendered at: always the session's.
    pub sample_rate: u32,
    /// The rate the file holds. The session's, except for an MP3 of a
    /// session faster than MP3 allows (`mp3_file_rate`).
    pub file_sample_rate: u32,
    pub base_frames: u64,
    /// The tail actually rendered, at `sample_rate`: until the project fell
    /// silent, and never more than `ExportSpec::tail_seconds`.
    pub tail_frames: u64,
    pub total_frames: u64,
    /// Parameter events the render had no room for
    /// (`RenderState::refused_events`). Non-zero means the file is not
    /// quite what the project says, so it is also logged.
    pub refused_events: u64,
    /// Samples (left and right counted separately) that reached the master's
    /// output guard above 0 dBFS. The safety limiter brought each one down
    /// to the ceiling -- the file holds none -- but a non-zero count means
    /// the mix was over and what was written is limited, so it is logged.
    pub overs: u64,
    /// Samples the 24-bit encoder had to clamp to full scale. The limiter
    /// runs first, so this is zero unless the limiter stopped holding its
    /// ceiling; it is counted rather than assumed, because the clamp is
    /// silent and a file damaged by one says nothing on its own.
    pub clipped_samples: u64,
    /// Samples the output guard found NaN or infinite and wrote as silence:
    /// a device blew up during the render. Logged when non-zero.
    pub non_finite_samples: u64,
}

#[derive(Debug)]
pub enum ExportError {
    Invalid(String),
    Io(std::io::Error),
    Wav(hound::Error),
    Mp3(String),
    /// The render was cancelled. Nothing was written at the target, and any
    /// file already there is as it was.
    Cancelled,
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid export: {message}"),
            Self::Io(error) => write!(f, "{error}"),
            Self::Wav(error) => write!(f, "WAV encoding failed: {error}"),
            Self::Mp3(error) => write!(f, "MP3 encoding failed: {error}"),
            Self::Cancelled => write!(f, "the export was cancelled"),
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

/// How far a render is, and the way to stop it: shared between the thread
/// rendering and whoever shows the progress (MOO-125).
#[derive(Debug, Default)]
pub struct ExportProgress {
    /// Frames rendered so far.
    done: AtomicU64,
    /// The most frames the render can take: the bars plus the whole tail
    /// cap. Zero until the render has started.
    most: AtomicU64,
    cancelled: AtomicBool,
}

impl ExportProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// How far the render is, 0 to 1, or `None` before it has started. The
    /// tail is counted at its cap, so a render that falls silent early
    /// jumps to the end.
    pub fn fraction(&self) -> Option<f32> {
        let most = self.most.load(Ordering::Relaxed);
        if most == 0 {
            return None;
        }
        let done = self.done.load(Ordering::Relaxed).min(most);
        Some((done as f64 / most as f64) as f32)
    }

    /// Ask the render to stop. It stops at its next block, removes what it
    /// had written, and returns [`ExportError::Cancelled`].
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn advance(&self, frames: usize) -> Result<(), ExportError> {
        self.done.fetch_add(frames as u64, Ordering::Relaxed);
        if self.is_cancelled() {
            Err(ExportError::Cancelled)
        } else {
            Ok(())
        }
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
        Self::render_with_progress(
            project,
            samples,
            realtime_sample_rate,
            spec,
            &ExportProgress::new(),
        )
    }

    /// [`Self::render`], reporting its progress into `progress` and stopping
    /// when it is cancelled.
    pub fn render_with_progress(
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
        realtime_sample_rate: u32,
        spec: &ExportSpec,
        progress: &ExportProgress,
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
        // The session's rate for every format: an MP3 rendered at 48 kHz
        // from a 96 kHz session was not what was heard wherever a sound
        // depends on the rate (MOO-125). The encoder converts afterwards.
        let sample_rate = realtime_sample_rate;
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
        let tail_cap = (f64::from(spec.tail_seconds) * f64::from(sample_rate)).round() as u64;
        let file_sample_rate = match spec.format {
            ExportFormat::Wav(_) => sample_rate,
            ExportFormat::Mp3(_) => mp3_file_rate(sample_rate),
        };
        let mut summary = RenderSummary {
            sample_rate,
            file_sample_rate,
            base_frames,
            tail_frames: tail_cap,
            total_frames: base_frames.saturating_add(tail_cap),
            refused_events: 0,
            overs: 0,
            clipped_samples: 0,
            non_finite_samples: 0,
        };

        progress.done.store(0, Ordering::Relaxed);
        progress.most.store(summary.total_frames.max(1), Ordering::Relaxed);
        let temporary = temporary_path(&spec.path);
        let result = match spec.format {
            ExportFormat::Wav(encoding) => {
                render_wav(&temporary, &mut state, summary, encoding, progress)
            }
            ExportFormat::Mp3(bitrate) => {
                render_mp3(&temporary, &mut state, summary, bitrate, progress)
            }
        };
        match result {
            Ok(rendered) => {
                summary.clipped_samples = rendered.clipped;
                summary.tail_frames = rendered.tail_frames;
                summary.total_frames = base_frames.saturating_add(rendered.tail_frames);
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        }
        summary.refused_events = state.refused_events();
        summary.overs = state.output_overs();
        summary.non_finite_samples = state.output_non_finite();
        report(&summary, &spec.path);
        // One rename over the target, which replaces it atomically: removing
        // it first left no file at all when the rename then failed.
        if let Err(error) = fs::rename(&temporary, &spec.path) {
            let _ = fs::remove_file(&temporary);
            return Err(error.into());
        }
        progress.done.store(summary.total_frames.max(1), Ordering::Relaxed);
        progress.most.store(summary.total_frames.max(1), Ordering::Relaxed);
        Ok(summary)
    }
}

/// Log whatever the render found that makes the file not quite the project:
/// the export itself succeeded, so these are warnings, and each one names
/// the file it is about.
fn report(summary: &RenderSummary, path: &Path) {
    if summary.refused_events > 0 {
        mooloop_core::log_warn!(
            "export",
            "{} parameter events had no room in their device's event list; \
             {} is missing some automation or modulation",
            summary.refused_events,
            path.display()
        );
    }
    if summary.non_finite_samples > 0 {
        mooloop_core::log_warn!(
            "export",
            "{} samples were NaN or infinite and were written as silence; \
             a device in the project blew up during the render of {}",
            summary.non_finite_samples,
            path.display()
        );
    }
    if summary.overs > 0 {
        mooloop_core::log_warn!(
            "export",
            "{} samples of the mix were over 0 dBFS; the master's safety \
             limiter held {} at the ceiling",
            summary.overs,
            path.display()
        );
    }
    if summary.clipped_samples > 0 {
        mooloop_core::log_warn!(
            "export",
            "{} samples were clipped to full scale by the 24-bit encoder in {}",
            summary.clipped_samples,
            path.display()
        );
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

/// Frames per offline block: what a live engine runs at, not the largest
/// block the graph can take.
///
/// Automation and modulation resolve once per `CONTROL_RATE_FRAMES`, into a
/// device's `EventList` of `MAX_EVENTS`. At `MAX_BLOCK_SIZE` that was 256
/// control ticks a block, so one automated parameter filled the list and
/// every one after it on the same device was refused: an export silenced the
/// second automated or modulated parameter while playback, at a JACK-sized
/// block, heard it. 512 frames is sixteen ticks, which leaves room for
/// sixteen moving parameters per device, and is a block size live playback
/// runs at, so what is exported is what was heard.
const OFFLINE_BLOCK_FRAMES: usize = 512;
const _: () = assert!(OFFLINE_BLOCK_FRAMES <= MAX_BLOCK_SIZE);

/// What rendering the blocks found, beyond the audio.
struct Rendered {
    /// Samples the 24-bit encoder had to clamp.
    clipped: u64,
    /// The tail actually rendered, which ends when the project is at rest.
    tail_frames: u64,
}

/// Render the bars, then the tail until the project falls silent or the cap
/// in `summary.tail_frames` runs out, handing each block to `sink`. Returns
/// how much tail it rendered.
fn render_blocks(
    state: &mut RenderState,
    summary: RenderSummary,
    progress: &ExportProgress,
    mut sink: impl FnMut(&[f32], &[f32]) -> Result<(), ExportError>,
) -> Result<u64, ExportError> {
    state.play();
    let mut remaining = summary.base_frames;
    while remaining > 0 {
        let frames = remaining.min(OFFLINE_BLOCK_FRAMES as u64) as usize;
        state.process_once_block(frames);
        sink(&state.master().l[..frames], &state.master().r[..frames])?;
        remaining -= frames as u64;
        progress.advance(frames)?;
    }

    // The tail runs until nothing in the project can sound any more -- a
    // reverb's decay and a delay's last echo included, which is what
    // `is_at_rest` asks -- and a block that was itself silent, so the file
    // does not end on the last audible block's final sample (MOO-125).
    state.pause();
    let mut rendered = 0u64;
    while rendered < summary.tail_frames {
        let frames = (summary.tail_frames - rendered).min(OFFLINE_BLOCK_FRAMES as u64) as usize;
        state.process_once_block(frames);
        let (left, right) = (&state.master().l[..frames], &state.master().r[..frames]);
        sink(left, right)?;
        rendered += frames as u64;
        progress.advance(frames)?;
        let silent = left
            .iter()
            .chain(right)
            .all(|sample| sample.abs() <= mooloop_dsp::SILENCE_PEAK);
        if silent && state.is_at_rest() {
            break;
        }
    }
    Ok(rendered)
}

fn render_wav(
    path: &Path,
    state: &mut RenderState,
    summary: RenderSummary,
    encoding: WavEncoding,
    progress: &ExportProgress,
) -> Result<Rendered, ExportError> {
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
    let mut clipped = 0u64;
    let tail_frames = render_blocks(state, summary, progress, |left, right| {
        for (&left, &right) in left.iter().zip(right) {
            match encoding {
                WavEncoding::Pcm24 => {
                    clipped += u64::from(left.abs() > 1.0) + u64::from(right.abs() > 1.0);
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
    Ok(Rendered {
        clipped,
        tail_frames,
    })
}

/// The 24-bit encoder: full scale is the most it can say, so anything past
/// it is clamped -- and counted by the caller, since the clamp is silent.
fn pcm24(sample: f32) -> i32 {
    (sample.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32
}

fn render_mp3(
    path: &Path,
    state: &mut RenderState,
    summary: RenderSummary,
    bitrate: Mp3Bitrate,
    progress: &ExportProgress,
) -> Result<Rendered, ExportError> {
    let file_rate = NonZeroU32::new(summary.file_sample_rate)
        .ok_or_else(|| ExportError::Invalid("sample rate cannot be zero".into()))?;
    let mut encoder = Builder::new()
        .ok_or_else(|| ExportError::Mp3("could not initialize LAME".into()))?
        .with_num_channels(2)
        .map_err(|error| ExportError::Mp3(error.to_string()))?
        .with_sample_rate(summary.sample_rate)
        .map_err(|error| ExportError::Mp3(error.to_string()))?
        .with_output_sample_rate(Some(file_rate))
        .map_err(|error| ExportError::Mp3(error.to_string()))?
        .with_brate(bitrate.lame())
        .map_err(|error| ExportError::Mp3(error.to_string()))?
        .with_quality(Quality::Best)
        .map_err(|error| ExportError::Mp3(error.to_string()))?
        .build()
        .map_err(|error| ExportError::Mp3(error.to_string()))?;
    let mut encoded = Vec::new();
    let tail_frames = render_blocks(state, summary, progress, |left, right| {
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
    Ok(Rendered {
        clipped: 0,
        tail_frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{NoteEvent, PatternPlacement, ProjectChannel};
    use tempfile::tempdir;

    fn audible_project() -> Project {
        let mut project = Project::default();
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 24, 60, 127));
        project
    }

    #[test]
    fn pattern_wav_has_one_pass_plus_a_tail_no_longer_than_the_cap() {
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
        assert!(summary.tail_frames > 0 && summary.tail_frames <= 24_000);
        assert_eq!(summary.file_sample_rate, 48_000);
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

    /// Two automated parameters on one device both reach it in an export.
    ///
    /// Shaped against the unfixed tree, which rendered offline in
    /// `MAX_BLOCK_SIZE` blocks: 256 control ticks a block, so the cutoff lane
    /// alone filled the filter's event list and every resonance event after
    /// it was refused -- the export dropped the second lane that playback
    /// played.
    #[test]
    fn an_export_refuses_no_automation_events() {
        use mooloop_core::{
            AutomationLane, AutomationPoint, DeviceId, EffectSlotState, EffectTarget,
            FilterParams, ParamAddr, FILTER_PARAM_CUTOFF_HZ, FILTER_PARAM_RESONANCE,
        };
        let temp = tempdir().unwrap();
        let path = temp.path().join("automated.wav");
        let mut channel = ProjectChannel::drum_synth(0, 1);
        channel
            .setup
            .push_effect(EffectSlotState::filter(FilterParams::default()));
        for (param, value) in [(FILTER_PARAM_CUTOFF_HZ, 0.3), (FILTER_PARAM_RESONANCE, 0.6)] {
            let mut lane = AutomationLane::new(ParamAddr::effect(
                EffectTarget::Channel(0),
                DeviceId(0),
                param,
            ));
            lane.upsert(AutomationPoint::new(1, 0, value));
            channel.automation[0].push(lane);
        }
        channel.notes[0].push(NoteEvent::new(1, 0, 24, 60, 127));
        let project = Project {
            channels: vec![channel],
            ..Project::default()
        };

        let summary = OfflineRenderer::render(
            &project,
            &[],
            48_000,
            &ExportSpec {
                path,
                scope: RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format: ExportFormat::Wav(WavEncoding::Float32),
            },
        )
        .unwrap();
        assert_eq!(summary.refused_events, 0);
    }

    /// A sampler at unity trim playing `sample`, its fader at `volume`.
    fn sampler_project(volume: f32) -> Project {
        let mut channel = ProjectChannel::sampler(0, 1);
        channel.setup.channel.volume = volume;
        channel.setup.sampler_state_mut().unwrap().params.output_gain = 1.0;
        channel.notes[0].push(NoteEvent::new(1, 0, 96, 60, 127));
        Project {
            channels: vec![channel],
            ..Project::default()
        }
    }

    fn sample_of(value: impl Fn(usize) -> f32) -> Arc<SampleData> {
        Arc::new(SampleData {
            frames: (0..24_000)
                .map(|index| {
                    let v = value(index);
                    [v, v]
                })
                .collect(),
            sample_rate: 48_000,
            root_note: 60,
        })
    }

    fn full_scale(index: usize) -> f32 {
        (index as f32 * std::f32::consts::TAU * 220.0 / 48_000.0).sin()
    }

    fn export(
        project: &Project,
        sample: Arc<SampleData>,
        path: &Path,
        format: ExportFormat,
    ) -> RenderSummary {
        OfflineRenderer::render(
            project,
            &[Some(sample)],
            48_000,
            &ExportSpec {
                path: path.to_path_buf(),
                scope: RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format,
            },
        )
        .unwrap()
    }

    /// **An over is reported, and none is written** (MOO-94).
    ///
    /// Shaped against the unfixed tree, where a float file held the mix at
    /// about +9 dBFS as it stood, a 24-bit one clipped it without a word, and
    /// `RenderSummary` had no field that could have said either.
    #[test]
    fn an_export_over_full_scale_reports_its_overs_and_writes_none() {
        let temp = tempdir().unwrap();
        let project = sampler_project(mooloop_core::MAX_LINEAR_GAIN);

        let path = temp.path().join("hot.wav");
        let summary = export(
            &project,
            sample_of(full_scale),
            &path,
            ExportFormat::Wav(WavEncoding::Float32),
        );
        assert!(summary.overs > 0, "an export of a +9 dBFS mix reported no overs");
        assert_eq!(summary.non_finite_samples, 0);
        let written: Vec<f32> = hound::WavReader::open(&path)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect();
        let peak = written.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
        assert!(peak <= 1.0, "the float file holds {peak}");
        assert!(peak > 0.99, "the limiter held the file at {peak}");

        let path = temp.path().join("hot-24.wav");
        let summary = export(
            &project,
            sample_of(full_scale),
            &path,
            ExportFormat::Wav(WavEncoding::Pcm24),
        );
        assert!(summary.overs > 0);
        assert_eq!(
            summary.clipped_samples, 0,
            "the encoder clamped what the limiter should have held"
        );
    }

    /// **A device that blows up leaves silence in the file, not NaN, and the
    /// summary says so** (MOO-94).
    #[test]
    fn an_export_of_a_device_that_emits_nan_writes_none_and_counts_them() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("broken.wav");
        let summary = export(
            &sampler_project(1.0),
            sample_of(|_| f32::NAN),
            &path,
            ExportFormat::Wav(WavEncoding::Float32),
        );
        assert!(summary.non_finite_samples > 0, "the render reported no fault");
        let mut reader = hound::WavReader::open(&path).unwrap();
        for (index, sample) in reader.samples::<f32>().map(Result::unwrap).enumerate() {
            assert!(sample.is_finite(), "sample {index} of the file is {sample}");
        }
    }

    fn tone(index: usize) -> f32 {
        0.5 * (index as f32 * std::f32::consts::TAU * 220.0 / 48_000.0).sin()
    }

    fn wav_spec(path: &Path, tail_seconds: f32) -> ExportSpec {
        ExportSpec {
            path: path.to_path_buf(),
            scope: RenderScope::Pattern { index: 0 },
            tail_seconds,
            format: ExportFormat::Wav(WavEncoding::Float32),
        }
    }

    fn with_reverb(mut project: Project) -> Project {
        use mooloop_core::{EffectParams, EffectSlotState, ReverbParams};
        project.channels[0]
            .setup
            .push_effect(EffectSlotState::new(EffectParams::Reverb(ReverbParams::default())));
        project
    }

    /// **The tail ends when the sound does, and never past the cap**
    /// (MOO-125).
    ///
    /// Shaped against the unfixed tree, where the tail was exactly
    /// `tail_seconds` whatever was still sounding: thirty seconds of silence
    /// after a dry sample, and a reverb cut off wherever the guess ran out.
    #[test]
    fn the_tail_ends_when_the_sound_does_and_never_passes_the_cap() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("tail.wav");
        let render = |project: &Project, tail_seconds: f32| {
            OfflineRenderer::render(project, &[Some(sample_of(tone))], 48_000, &wav_spec(&path, tail_seconds))
                .unwrap()
        };

        // A half-second sample that ended long before the last bar: the
        // project is at rest when the bars end, so one block of tail.
        let dry = render(&sampler_project(1.0), 30.0);
        assert!(
            dry.tail_frames <= OFFLINE_BLOCK_FRAMES as u64,
            "a dry render tailed {} frames of silence",
            dry.tail_frames
        );

        // A reverb rings on after the bars, and the tail waits for it, then
        // stops well short of the cap.
        let wet_project = with_reverb(sampler_project(1.0));
        let wet = render(&wet_project, 30.0);
        assert!(wet.tail_frames > dry.tail_frames, "the reverb's tail was cut at {}", wet.tail_frames);
        assert!(wet.tail_frames < 30 * 48_000, "the tail ran to the cap");
        let written: Vec<f32> = hound::WavReader::open(&path)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(written.len() as u64, wet.total_frames * 2);
        let last_block = &written[written.len() - OFFLINE_BLOCK_FRAMES * 2..];
        assert!(
            last_block.iter().all(|s| s.abs() <= mooloop_dsp::SILENCE_PEAK),
            "the file ends while the reverb is still audible"
        );

        // The cap holds even while something is still ringing.
        let capped = render(&wet_project, 0.25);
        assert_eq!(capped.tail_frames, 12_000);
    }

    /// **Cancel stops the render and leaves the target as it was**
    /// (MOO-125).
    #[test]
    fn a_cancelled_export_leaves_the_existing_file_untouched() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("keep.wav");
        fs::write(&path, b"the file that was here").unwrap();
        let progress = ExportProgress::new();
        progress.cancel();
        let result = OfflineRenderer::render_with_progress(
            &sampler_project(1.0),
            &[Some(sample_of(tone))],
            48_000,
            &wav_spec(&path, 2.0),
            &progress,
        );
        assert!(matches!(result, Err(ExportError::Cancelled)), "{result:?}");
        assert_eq!(fs::read(&path).unwrap(), b"the file that was here");
        let leftovers: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "a partial file was left behind: {leftovers:?}");
    }

    /// Progress runs from nothing to all of it, and a finished export
    /// replaces a file already at the target.
    #[test]
    fn progress_reaches_the_end_and_an_export_replaces_the_target() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("replace.wav");
        fs::write(&path, b"old").unwrap();
        let progress = ExportProgress::new();
        assert_eq!(progress.fraction(), None);
        OfflineRenderer::render_with_progress(
            &sampler_project(1.0),
            &[Some(sample_of(tone))],
            48_000,
            &wav_spec(&path, 5.0),
            &progress,
        )
        .unwrap();
        assert_eq!(progress.fraction(), Some(1.0));
        assert!(hound::WavReader::open(&path).is_ok(), "the old file is still there");
    }

    /// The sampling rate an MP3 file's first frame header declares.
    fn mp3_header_rate(bytes: &[u8]) -> u32 {
        let at = bytes
            .windows(2)
            .position(|pair| pair[0] == 0xff && pair[1] & 0xe0 == 0xe0)
            .expect("no MP3 frame");
        let version = (bytes[at + 1] >> 3) & 0b11;
        let index = (bytes[at + 2] >> 2) & 0b11;
        let base = [44_100, 48_000, 32_000][index as usize];
        match version {
            0b11 => base,
            0b10 => base / 2,
            _ => base / 4,
        }
    }

    /// **A 96 kHz session's MP3 renders its project at 96 kHz** (MOO-125),
    /// and the file holds 48 kHz, the fastest rate MP3 has.
    ///
    /// Shaped against the unfixed tree, which built the render at a fixed
    /// 48 kHz: `sample_rate` was 48 000 and `base_frames` half of this.
    #[test]
    fn a_96_khz_session_renders_its_mp3_at_96_khz() {
        let temp = tempdir().unwrap();
        for (session, file) in [(96_000, 48_000), (88_200, 44_100), (44_100, 44_100)] {
            let path = temp.path().join(format!("{session}.mp3"));
            let summary = OfflineRenderer::render(
                &audible_project(),
                &[],
                session,
                &ExportSpec {
                    path: path.clone(),
                    scope: RenderScope::Pattern { index: 0 },
                    tail_seconds: 0.0,
                    format: ExportFormat::Mp3(Mp3Bitrate::Kbps320),
                },
            )
            .unwrap();
            assert_eq!(summary.sample_rate, session);
            assert_eq!(summary.base_frames, u64::from(session) * 2);
            assert_eq!(summary.file_sample_rate, file);
            assert_eq!(mp3_header_rate(&fs::read(&path).unwrap()), file);
        }
    }

    #[test]
    fn synth_project_renders_offline_without_samples() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("drum-synth.wav");
        let mut project = Project {
            channels: vec![ProjectChannel::drum_synth(0, 1)],
            ..Project::default()
        };
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, 24, 60, 127));

        OfflineRenderer::render(
            &project,
            &[],
            48_000,
            &ExportSpec {
                path: path.clone(),
                scope: RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format: ExportFormat::Wav(WavEncoding::Float32),
            },
        )
        .unwrap();

        let mut reader = hound::WavReader::open(path).unwrap();
        assert!(reader
            .samples::<f32>()
            .map(Result::unwrap)
            .any(|sample| sample.abs() > 0.001));
    }
}
