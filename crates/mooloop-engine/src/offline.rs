//! Offline rendering and WAV/MP3 encoding.

use std::fmt;
use std::fs;
use std::io::Write;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use std::collections::BTreeMap;

use mooloop_core::{PlaybackMode, PluginSlotId, Project};
use mooloop_dsp::{AudioNode, SampleData, MAX_BLOCK_SIZE};
use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushGap, MonoPcm, Quality};

use crate::render::RenderState;
use mooloop_core::EngineCommand;

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

/// Which stretch of the timeline a pass renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderScope {
    /// One pass of a pattern, from its first tick.
    Pattern { index: usize },
    /// The whole arrangement, from the top to the song's last bar.
    Song,
    /// Part of the arrangement: song ticks from `start_tick` up to, not
    /// including, `end_tick` (MOO-181). The render locates to the start the
    /// way playback does, so automation and tempo-synced modulators read
    /// what they read there, a note that began before the start is not
    /// chased, and effects start empty. The range must lie inside the song.
    Range { start_tick: u32, end_tick: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavEncoding {
    /// 16-bit PCM, for a CD-style delivery (MOO-186). Dither it.
    Pcm16,
    Pcm24,
    Float32,
}

/// How many channels a file holds (MOO-186).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputChannels {
    #[default]
    Stereo,
    /// One channel, `(L + R) / 2`: a centred source keeps the level each
    /// side had, and a hard-panned one is 6 dB under the side it was on
    /// (`docs/GAIN_STRUCTURE.md`, "Mono files").
    Mono,
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
    /// Samples the PCM (16- or 24-bit) encoder had to clamp to full scale. The limiter
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
    /// Cancel once this many frames are done: a test's way to stop a job
    /// at a known point. Zero is never.
    #[cfg(test)]
    cancel_after: AtomicU64,
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

    /// Ask the render to stop. It stops at its next block, removes the
    /// partial files of the pass in progress, and returns
    /// [`ExportError::Cancelled`]. A job's files from passes that had
    /// already finished stay.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn advance(&self, frames: usize) -> Result<(), ExportError> {
        self.done.fetch_add(frames as u64, Ordering::Relaxed);
        #[cfg(test)]
        {
            let after = self.cancel_after.load(Ordering::Relaxed);
            if after > 0 && self.done.load(Ordering::Relaxed) >= after {
                self.cancel();
            }
        }
        if self.is_cancelled() {
            Err(ExportError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Where an output's audio is taken from. Every tap of a pass reads the same
/// render, so a job's stems line up to the frame with its master.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTap {
    /// The master mix, after the master's output guard and safety limiter.
    Master,
    /// A mixer track's own output, by its index in the song's tracks
    /// (MOO-182): after its rack, fader and balance, before it reaches what
    /// it feeds -- the direct out of a console channel. Its sends' returns
    /// are tracks of their own, and the master's rack is not in it. Silence
    /// while the track is muted or solo-silenced, as playback hears it. No
    /// limiter runs on it: a non-finite sample is written as silence and
    /// counted, an over is counted, and a PCM file clamps it.
    Track(u8),
    /// A channel's own output, by its index in the song's channels
    /// (MOO-183): its source, its rack, its fader and pan, and nothing the
    /// mixer does after -- the track's rack, sends and buses, and the
    /// master are all left out, so a channel whose track is muted still
    /// renders. Silence while the channel itself is muted or
    /// solo-silenced. Guarded the way a track stem is.
    Channel(u8),
}

/// One file a job writes: where, from which tap, in which format.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOutput {
    pub path: PathBuf,
    pub tap: RenderTap,
    pub format: ExportFormat,
    /// Stereo, or a mono mix of the two sides (MOO-186).
    pub channels: OutputChannels,
    /// TPDF dither of one LSB, added just before a PCM file's samples are
    /// rounded, after everything else (MOO-186). Its generator is seeded
    /// from the output's place in the job, so two renders of one job are
    /// bit-identical and two outputs of one job carry unrelated dither.
    /// Float and MP3 files take no dither and ignore it.
    pub dither: bool,
    /// Write no file when every sample stayed at or below `SILENCE_PEAK` --
    /// a muted track's stem, say (MOO-182). Judged on the signal before
    /// dither, which would otherwise make every file audible.
    pub skip_silent: bool,
    /// What becomes of a file already at `path` when the render is done
    /// (MOO-188).
    pub existing: ExistingFile,
}

/// What an output does about a file already at its path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExistingFile {
    /// Replace it, atomically. The dialog asked first.
    Replace,
    /// Never replace it. `path` is `base` with the job's number already
    /// chosen (`mooloop_core::file_names::numbered`), free when the job was
    /// built. A file that appeared there since is left alone, and this
    /// output lands on the next free number of `base` instead: the race
    /// costs a file its place in the job's numbering, never someone's file.
    Number { base: PathBuf },
}

impl RenderOutput {
    /// A stereo file at `path`, dithered where its format is 16-bit: the
    /// defaults the dialog offers.
    pub fn new(path: PathBuf, tap: RenderTap, format: ExportFormat) -> Self {
        Self {
            path,
            tap,
            format,
            channels: OutputChannels::Stereo,
            dither: format == ExportFormat::Wav(WavEncoding::Pcm16),
            skip_silent: false,
            existing: ExistingFile::Replace,
        }
    }
}

/// Outputs that share one timeline, rendered in **one** pass: the project is
/// rendered once and each block is handed to every output (MOO-180).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderPass {
    pub scope: RenderScope,
    /// The most tail the pass may add after the last bar, 0 to 30 s; see
    /// [`ExportSpec::tail_seconds`]. The tail ends when the whole project is
    /// at rest, so every output of a pass is the same length.
    pub tail_seconds: f32,
    pub outputs: Vec<RenderOutput>,
}

/// Everything one export writes: one or more passes, rendered in order,
/// under one [`ExportProgress`] (MOO-180).
///
/// The master mix alone is a job with one pass holding one output
/// ([`RenderJob::single`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderJob {
    pub passes: Vec<RenderPass>,
}

impl RenderJob {
    /// The job an [`ExportSpec`] describes: one pass, the master mix, one file.
    pub fn single(spec: &ExportSpec) -> Self {
        Self {
            passes: vec![RenderPass {
                scope: spec.scope,
                tail_seconds: spec.tail_seconds,
                outputs: vec![RenderOutput::new(
                    spec.path.clone(),
                    RenderTap::Master,
                    spec.format,
                )],
            }],
        }
    }

    /// Every file the job writes, in the order it writes them.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.passes
            .iter()
            .flat_map(|pass| pass.outputs.iter().map(|output| output.path.as_path()))
    }

    /// The files the job would replace: the ones already on disk, of the
    /// outputs that replace. The export dialog asks once, saying how many,
    /// before it starts. A numbered output never replaces, so it is never
    /// among them (MOO-188).
    pub fn existing_targets(&self) -> Vec<PathBuf> {
        self.passes
            .iter()
            .flat_map(|pass| &pass.outputs)
            .filter(|output| output.existing == ExistingFile::Replace)
            .map(|output| output.path.as_path())
            .filter(|path| path.exists())
            .map(Path::to_path_buf)
            .collect()
    }

    fn validate(&self) -> Result<(), ExportError> {
        if self.passes.is_empty() {
            return Err(ExportError::Invalid("the export has nothing to write".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for pass in &self.passes {
            if !pass.tail_seconds.is_finite() || !(0.0..=30.0).contains(&pass.tail_seconds) {
                return Err(ExportError::Invalid(
                    "tail duration must be between 0 and 30 seconds".into(),
                ));
            }
            if pass.outputs.is_empty() {
                return Err(ExportError::Invalid("a render pass has no outputs".into()));
            }
            for output in &pass.outputs {
                if !seen.insert(output.path.as_path()) {
                    return Err(ExportError::Invalid(format!(
                        "{} is written twice by the same export",
                        output.path.display()
                    )));
                }
            }
        }
        Ok(())
    }
}

/// One file a job finished, with what its render found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedFile {
    pub path: PathBuf,
    pub summary: RenderSummary,
}

/// A job that stopped before its end: the error, and the files it had
/// already finished. Those are complete and stay on disk; the pass that was
/// in progress left nothing behind (MOO-180).
#[derive(Debug)]
pub struct JobFailure {
    pub written: Vec<RenderedFile>,
    pub error: ExportError,
}

impl fmt::Display for JobFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for JobFailure {}

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
        Self::render_with_plugins(
            project,
            samples,
            realtime_sample_rate,
            spec,
            progress,
            BTreeMap::new(),
        )
    }

    /// [`Self::render_with_progress`] with a processor for each hosted
    /// plugin the song names, by its slot (MOO-81). A plugin device with no
    /// processor here renders as its placeholder, a pass-through, which is
    /// what playback plays for a missing plugin too.
    ///
    /// The processors must have been built for `realtime_sample_rate` and
    /// for blocks of up to `mooloop_dsp::MAX_BLOCK_SIZE`. The live song's
    /// are in the engine, so an export's come from second instances
    /// (`Session::export_plugin_processors`). Each is swapped into its
    /// device and the compensation is derived from their latencies, the
    /// way the session's plan does it for playback.
    pub fn render_with_plugins(
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
        realtime_sample_rate: u32,
        spec: &ExportSpec,
        progress: &ExportProgress,
        plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>,
    ) -> Result<RenderSummary, ExportError> {
        let job = RenderJob::single(spec);
        let mut plugins = Some(plugins);
        match Self::render_job_with_plugins(
            project,
            samples,
            &[],
            realtime_sample_rate,
            &job,
            progress,
            &mut |_| plugins.take().unwrap_or_default(),
        ) {
            Ok(mut files) => Ok(files.remove(0).summary),
            Err(failure) => Err(failure.error),
        }
    }

    /// Render every pass of `job` in order, each pass once, handing each
    /// block to every output of the pass (MOO-180).
    ///
    /// `progress` covers the whole job. A cancel, or any failure, stops at
    /// the next block: the files of the passes that had finished stay, and
    /// the pass in progress removes its partial files, leaving whatever was
    /// at its targets as it was.
    pub fn render_job(
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
        realtime_sample_rate: u32,
        job: &RenderJob,
        progress: &ExportProgress,
    ) -> Result<Vec<RenderedFile>, JobFailure> {
        Self::render_job_with_plugins(
            project,
            samples,
            &[],
            realtime_sample_rate,
            job,
            progress,
            &mut |_| BTreeMap::new(),
        )
    }

    /// [`Self::render_job`] with hosted plugins' processors, as in
    /// [`Self::render_with_plugins`]. A processor renders one pass, so
    /// `plugins` is asked once for each pass by its index in `job.passes`,
    /// just before that pass is built. A caller whose job has one pass
    /// (every export the dialog builds so far) hands its map over on the
    /// first call; a pass given an empty map plays its plugins as
    /// placeholders.
    ///
    /// `zones` is each sampler's key-zone audio (MOO-14), `zones[channel]`
    /// parallel to that channel's `SamplerState::zones`. A song with a zone
    /// that names a file must bring its buffer: without one the render is
    /// refused rather than exported with the zone silent, since an export
    /// that differs from playback with nothing said is the fault to avoid.
    /// A zone whose buffer is here but `None` -- its file missing in the
    /// session too -- renders silent, as it plays.
    #[allow(clippy::too_many_arguments)]
    pub fn render_job_with_plugins(
        project: &Project,
        samples: &[Option<Arc<SampleData>>],
        zones: &[Vec<Option<Arc<SampleData>>>],
        realtime_sample_rate: u32,
        job: &RenderJob,
        progress: &ExportProgress,
        plugins: &mut dyn FnMut(usize) -> BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>,
    ) -> Result<Vec<RenderedFile>, JobFailure> {
        // Render as the callback does, subnormals flushed, so an export and
        // playback agree to the bit (MOO-223). The thread's own mode comes
        // back when this returns: it is the app's shared document worker.
        let _flush = crate::executor::FlushToZero::enable();
        let fail = |written, error| Err(JobFailure { written, error });
        if let Err(error) = job.validate() {
            return fail(Vec::new(), error);
        }
        if let Some(channel) = unsupplied_zones(project, zones) {
            return fail(
                Vec::new(),
                ExportError::Invalid(format!(
                    "channel {} has key zones whose audio was not given to the render",
                    channel + 1
                )),
            );
        }
        // The session's rate for every format: an MP3 rendered at 48 kHz
        // from a 96 kHz session was not what was heard wherever a sound
        // depends on the rate (MOO-125). The encoder converts afterwards.
        let sample_rate = realtime_sample_rate;
        if sample_rate == 0 {
            return fail(
                Vec::new(),
                ExportError::Invalid("sample rate cannot be zero".into()),
            );
        }

        // Measure every pass first, so the bar covers the whole job from its
        // first block. A pass's state is rebuilt when its turn comes rather
        // than held: a job of many patterns would otherwise hold a whole
        // device graph per pattern.
        // The first pass's state is kept, so a one-pass job builds it once,
        // with its plugins; the others are measured without theirs, which
        // change neither the bars nor the tail cap.
        let mut lengths = Vec::with_capacity(job.passes.len());
        let mut first = None;
        for (index, pass) in job.passes.iter().enumerate() {
            let hosted = if index == 0 { plugins(0) } else { BTreeMap::new() };
            match prepare_pass(project, samples, zones, sample_rate, pass, hosted) {
                Ok((state, base, cap)) => {
                    if first.is_none() {
                        first = Some(state);
                    }
                    lengths.push((base, cap));
                }
                Err(error) => return fail(Vec::new(), error),
            }
        }
        let most: u64 = lengths
            .iter()
            .map(|(base, cap)| base.saturating_add(*cap))
            .sum();
        progress.done.store(0, Ordering::Relaxed);
        progress.most.store(most.max(1), Ordering::Relaxed);

        let mut written = Vec::new();
        let mut rendered_frames = 0u64;
        // Each output's place in the whole job, which seeds its dither.
        let mut first_output = 0usize;
        for (index, (pass, &(base_frames, tail_cap))) in
            job.passes.iter().zip(&lengths).enumerate()
        {
            let mut state = match first.take() {
                Some(state) => state,
                None => match prepare_pass(project, samples, zones, sample_rate, pass, plugins(index)) {
                    Ok((state, _, _)) => state,
                    Err(error) => return fail(written, error),
                },
            };
            let before = progress.done.load(Ordering::Relaxed);
            let rendered = render_pass(
                &mut state,
                pass,
                first_output,
                sample_rate,
                base_frames,
                tail_cap,
                progress,
            );
            first_output += pass.outputs.len();
            match rendered {
                Ok(files) => {
                    rendered_frames = rendered_frames.saturating_add(
                        files
                            .first()
                            .map_or(base_frames, |file| file.summary.total_frames),
                    );
                    written.extend(files);
                    // A tail that fell silent early skips the rest of its
                    // cap, so the bar jumps to where the next pass starts.
                    progress.done.store(
                        before.saturating_add(base_frames.saturating_add(tail_cap)),
                        Ordering::Relaxed,
                    );
                }
                Err((files, error)) => {
                    written.extend(files);
                    return fail(written, error);
                }
            }
        }
        progress.done.store(rendered_frames.max(1), Ordering::Relaxed);
        progress.most.store(rendered_frames.max(1), Ordering::Relaxed);
        Ok(written)
    }
}

/// The first channel whose sampler names a key-zone file that `zones` holds
/// no entry for (MOO-14): an entry, even `None`, is the caller saying what
/// that zone plays.
fn unsupplied_zones(project: &Project, zones: &[Vec<Option<Arc<SampleData>>>]) -> Option<usize> {
    project.channels.iter().enumerate().find_map(|(index, channel)| {
        let state = channel.setup.source.sampler_state()?;
        let given = zones.get(index).map_or(0, Vec::len);
        state
            .zones
            .iter()
            .enumerate()
            .any(|(zone, spec)| {
                matches!(spec.sample, mooloop_core::SampleReference::File { .. }) && zone >= given
            })
            .then_some(index)
    })
}

/// The render state for `pass`, and its length: the bars, and the tail cap,
/// in frames at `sample_rate`.
fn prepare_pass(
    project: &Project,
    samples: &[Option<Arc<SampleData>>],
    zones: &[Vec<Option<Arc<SampleData>>>],
    sample_rate: u32,
    pass: &RenderPass,
    plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>,
) -> Result<(RenderState, u64, u64), ExportError> {
    let mut render_project = project.clone();
    match pass.scope {
        RenderScope::Pattern { index } => {
            if index >= render_project.pattern_lengths.len() {
                return Err(ExportError::Invalid("pattern is out of range".into()));
            }
            render_project.playback_mode = PlaybackMode::Pattern;
            render_project.current_pattern = index as u16;
        }
        RenderScope::Song | RenderScope::Range { .. } => {
            render_project.playback_mode = PlaybackMode::Song;
        }
    }
    let mut state =
        RenderState::from_project_with_zones(sample_rate, &render_project, samples, zones);
    state.host_plugins(&render_project, plugins);
    let base_ticks = match pass.scope {
        RenderScope::Pattern { index } => state
            .pattern_length_ticks(index)
            .ok_or_else(|| ExportError::Invalid("pattern is out of range".into()))?,
        RenderScope::Song => state.song_length_ticks(),
        RenderScope::Range {
            start_tick,
            end_tick,
        } => {
            if end_tick <= start_tick {
                return Err(ExportError::Invalid(
                    "the range must end after it starts".into(),
                ));
            }
            if end_tick > state.song_length_ticks() {
                return Err(ExportError::Invalid(
                    "the range ends after the song does".into(),
                ));
            }
            // A locate, as playback does it: the first block starts on the
            // range's first tick and every lane resolves there.
            state.apply_command(EngineCommand::Seek {
                tick: f64::from(start_tick),
            });
            end_tick - start_tick
        }
    };
    let base_frames = (f64::from(base_ticks) / state.ticks_per_sample()).ceil() as u64;
    let tail_cap = (f64::from(pass.tail_seconds) * f64::from(sample_rate)).round() as u64;
    Ok((state, base_frames, tail_cap))
}

/// Render one pass into every one of its outputs, then move each finished
/// file over its target. On failure, the files already moved are returned
/// with the error, and every other output's partial file is removed.
/// `first_output` is the job-wide index of the pass's first output.
fn render_pass(
    state: &mut RenderState,
    pass: &RenderPass,
    first_output: usize,
    sample_rate: u32,
    base_frames: u64,
    tail_cap: u64,
    progress: &ExportProgress,
) -> Result<Vec<RenderedFile>, (Vec<RenderedFile>, ExportError)> {
    let temporaries: Vec<PathBuf> = pass
        .outputs
        .iter()
        .map(|output| temporary_path(&output.path))
        .collect();
    let discard = |from: usize| {
        for temporary in &temporaries[from..] {
            let _ = fs::remove_file(temporary);
        }
    };

    // A stem of a track the song does not have, or of the master's own bus
    // (which is the master tap, with its guard), is a defective job.
    for output in &pass.outputs {
        let missing = match output.tap {
            RenderTap::Master => None,
            RenderTap::Track(track) => (track == mooloop_core::MASTER_BUS
                || usize::from(track) >= state.track_count())
            .then(|| format!("the song has no track {track} to render")),
            RenderTap::Channel(channel) => (usize::from(channel) >= state.live_channel_count())
                .then(|| format!("the song has no channel {channel} to render")),
        };
        if let Some(missing) = missing {
            return Err((Vec::new(), ExportError::Invalid(missing)));
        }
    }

    let mut sinks = Vec::with_capacity(pass.outputs.len());
    for (index, (output, temporary)) in pass.outputs.iter().zip(&temporaries).enumerate() {
        match Sink::open(temporary, output, first_output + index, sample_rate) {
            Ok(sink) => sinks.push(sink),
            Err(error) => {
                drop(sinks);
                discard(0);
                return Err((Vec::new(), error));
            }
        }
    }

    let rendered = render_blocks(state, base_frames, tail_cap, progress, |state, frames| {
        for sink in &mut sinks {
            sink.feed(state, frames)?;
        }
        Ok(())
    });
    let tail_frames = match rendered {
        Ok(tail_frames) => tail_frames,
        Err(error) => {
            drop(sinks);
            discard(0);
            return Err((Vec::new(), error));
        }
    };

    let mut found = Vec::with_capacity(sinks.len());
    for (index, (sink, temporary)) in sinks.into_iter().zip(&temporaries).enumerate() {
        let (overs, non_finite, audible) = (sink.overs, sink.non_finite, sink.audible);
        match sink.finish(temporary) {
            Ok(clipped) => found.push((clipped, overs, non_finite, audible)),
            Err(error) => {
                discard(index);
                return Err((Vec::new(), error));
            }
        }
    }

    let mut files = Vec::with_capacity(pass.outputs.len());
    for (index, (output, temporary)) in pass.outputs.iter().zip(&temporaries).enumerate() {
        let (clipped, overs, non_finite, audible) = found[index];
        if output.skip_silent && !audible {
            let _ = fs::remove_file(temporary);
            mooloop_core::log_info!(
                "export",
                "{} was silent from start to end and was not written",
                output.path.display()
            );
            continue;
        }
        // The master's counts are its output guard's; a stem has no guard,
        // so its sink counted its own.
        let (overs, non_finite) = match output.tap {
            RenderTap::Master => (state.output_overs(), state.output_non_finite()),
            RenderTap::Track(_) | RenderTap::Channel(_) => (overs, non_finite),
        };
        let summary = RenderSummary {
            sample_rate,
            file_sample_rate: match output.format {
                ExportFormat::Wav(_) => sample_rate,
                ExportFormat::Mp3(_) => mp3_file_rate(sample_rate),
            },
            base_frames,
            tail_frames,
            total_frames: base_frames.saturating_add(tail_frames),
            refused_events: state.refused_events(),
            overs,
            clipped_samples: clipped,
            non_finite_samples: non_finite,
        };
        let placed = match &output.existing {
            // One rename over the target, which replaces it atomically:
            // removing it first left no file at all when the rename then
            // failed.
            ExistingFile::Replace => {
                fs::rename(temporary, &output.path).map(|()| output.path.clone())
            }
            ExistingFile::Number { base } => place_numbered(temporary, &output.path, base),
        };
        let path = match placed {
            Ok(path) => path,
            Err(error) => {
                discard(index);
                return Err((files, error.into()));
            }
        };
        report(&summary, &path, output.tap == RenderTap::Master);
        files.push(RenderedFile { path, summary });
    }
    Ok(files)
}

/// Move a finished file to `path` without replacing anything there, or, if
/// a file appeared at `path` after the job was built, to the first free
/// number of `base` (MOO-188). Where it landed.
fn place_numbered(temporary: &Path, path: &Path, base: &Path) -> std::io::Result<PathBuf> {
    use mooloop_core::file_names::{numbered, rename_no_replace};
    let candidates = std::iter::once(path.to_path_buf()).chain((1..).map(|n| numbered(base, n)));
    for candidate in candidates {
        match rename_no_replace(temporary, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!("the numbers never run out")
}

/// Log whatever the render found that makes the file not quite the project:
/// the export itself succeeded, so these are warnings, and each one names
/// the file it is about.
/// `limited` says whether the file came through the master's safety
/// limiter; a stem's overs are written as they are (MOO-182).
fn report(summary: &RenderSummary, path: &Path, limited: bool) {
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
    if summary.overs > 0 && limited {
        mooloop_core::log_warn!(
            "export",
            "{} samples of the mix were over 0 dBFS; the master's safety \
             limiter held {} at the ceiling",
            summary.overs,
            path.display()
        );
    } else if summary.overs > 0 {
        mooloop_core::log_warn!(
            "export",
            "{} samples of {} were over 0 dBFS; a stem has no limiter, so \
             they are written as they are",
            summary.overs,
            path.display()
        );
    }
    if summary.clipped_samples > 0 {
        mooloop_core::log_warn!(
            "export",
            "{} samples were clipped to full scale by the PCM encoder in {}",
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

/// Render the bars, then the tail until the project falls silent or
/// `tail_cap` runs out, handing the state to `sink` after each block with
/// the block's length. Returns how much tail it rendered.
///
/// Nothing on the master delays its output -- the safety limiter has no
/// lookahead (MOO-217) -- so a file starts on the bar line as rendered.
fn render_blocks(
    state: &mut RenderState,
    base_frames: u64,
    tail_cap: u64,
    progress: &ExportProgress,
    mut sink: impl FnMut(&RenderState, usize) -> Result<(), ExportError>,
) -> Result<u64, ExportError> {
    state.play();
    let mut remaining = base_frames;
    while remaining > 0 {
        let frames = remaining.min(OFFLINE_BLOCK_FRAMES as u64) as usize;
        state.process_once_block(frames);
        sink(state, frames)?;
        remaining -= frames as u64;
        progress.advance(frames)?;
    }

    // The tail runs until nothing in the project can sound any more -- a
    // reverb's decay and a delay's last echo included, which is what
    // `is_at_rest` asks -- and a block that was itself silent, so the file
    // does not end on the last audible block's final sample (MOO-125).
    state.pause();
    let mut rendered = 0u64;
    while rendered < tail_cap {
        let frames = (tail_cap - rendered).min(OFFLINE_BLOCK_FRAMES as u64) as usize;
        state.process_once_block(frames);
        sink(state, frames)?;
        rendered += frames as u64;
        progress.advance(frames)?;
        let (left, right) = (&state.master().l[..frames], &state.master().r[..frames]);
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

/// One output's encoder, fed a block at a time.
struct Sink {
    tap: RenderTap,
    channels: OutputChannels,
    encoder: Encoder,
    /// The block's mono mix, for a mono output: sized once, at open.
    mono: Vec<f32>,
    /// A stem's block, copied out of its track so it can be scrubbed
    /// without touching the render: sized once, at open, for a track tap.
    stem: [Vec<f32>; 2],
    /// A stem's own counts, since no output guard runs on a track: samples
    /// over full scale, and non-finite samples written as silence.
    overs: u64,
    non_finite: u64,
    /// Whether any sample so far rose above `SILENCE_PEAK`.
    audible: bool,
}

enum Encoder {
    Wav {
        writer: hound::WavWriter<std::io::BufWriter<fs::File>>,
        encoding: WavEncoding,
        /// The dither added before a PCM sample is rounded, when the output
        /// asked for it and its format is PCM.
        dither: Option<Tpdf>,
        /// Samples the PCM encoder had to clamp.
        clipped: u64,
    },
    /// LAME, and what it has encoded so far; written to the file at the end.
    Mp3 {
        encoder: Box<mp3lame_encoder::Encoder>,
        encoded: Vec<u8>,
    },
}

/// Triangular (TPDF) dither of one LSB either way (MOO-186): the difference
/// of two uniform draws, which makes the rounding error independent of the
/// signal, so a quiet tone rounds into a flat noise floor rather than
/// into harmonics.
///
/// The seed comes from the output's place in the job, so a render is
/// repeatable to the bit and every output of one job has its own sequence:
/// stems summed in another program add their floors as power, not
/// coherently, which one shared sequence would do (+20 log N rather than
/// +10 log N). Left and right take alternate draws of one generator, so
/// they never share values either.
struct Tpdf {
    state: u64,
}

impl Tpdf {
    const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

    fn new(index: usize) -> Self {
        // splitmix64 of the base plus the index: neighbouring indices land
        // far apart, and xorshift must never start at zero.
        let mut z = Self::SEED.wrapping_add((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Self {
            state: if z == 0 { Self::SEED } else { z },
        }
    }

    /// xorshift64*, uniform over [0, 1).
    fn uniform(&mut self) -> f64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// The next dither value in LSBs, triangular over (-1, 1).
    fn next(&mut self) -> f64 {
        self.uniform() - self.uniform()
    }
}

/// Whether a block holds anything above `SILENCE_PEAK`.
fn is_audible(left: &[f32], right: &[f32]) -> bool {
    left.iter()
        .chain(right)
        .any(|sample| sample.abs() > mooloop_dsp::SILENCE_PEAK)
}

/// The largest code of a PCM depth.
fn full_scale(encoding: WavEncoding) -> Option<f32> {
    match encoding {
        WavEncoding::Pcm16 => Some(32_767.0),
        WavEncoding::Pcm24 => Some(8_388_607.0),
        WavEncoding::Float32 => None,
    }
}

/// A PCM sample: full scale is the most it can say, so anything past it is
/// clamped -- and counted by the caller, since the clamp is silent. With
/// dither, the dither is added last, just before rounding.
fn pcm(sample: f32, full_scale: f32, dither: Option<&mut Tpdf>) -> i32 {
    let clamped = sample.clamp(-1.0, 1.0);
    match dither {
        None => (clamped * full_scale).round() as i32,
        Some(dither) => {
            let scale = f64::from(full_scale);
            (f64::from(clamped) * scale + dither.next())
                .round()
                .clamp(-scale - 1.0, scale) as i32
        }
    }
}

impl Sink {
    /// `index` is the output's place in the job, which seeds its dither.
    fn open(
        path: &Path,
        output: &RenderOutput,
        index: usize,
        sample_rate: u32,
    ) -> Result<Self, ExportError> {
        let channels: u16 = match output.channels {
            OutputChannels::Stereo => 2,
            OutputChannels::Mono => 1,
        };
        let encoder = match output.format {
            ExportFormat::Wav(encoding) => {
                let spec = match encoding {
                    WavEncoding::Pcm16 | WavEncoding::Pcm24 => hound::WavSpec {
                        channels,
                        sample_rate,
                        bits_per_sample: if encoding == WavEncoding::Pcm16 { 16 } else { 24 },
                        sample_format: hound::SampleFormat::Int,
                    },
                    WavEncoding::Float32 => hound::WavSpec {
                        channels,
                        sample_rate,
                        bits_per_sample: 32,
                        sample_format: hound::SampleFormat::Float,
                    },
                };
                Encoder::Wav {
                    writer: hound::WavWriter::create(path, spec)?,
                    encoding,
                    dither: (output.dither && full_scale(encoding).is_some()).then(|| Tpdf::new(index)),
                    clipped: 0,
                }
            }
            ExportFormat::Mp3(bitrate) => {
                let file_rate = NonZeroU32::new(mp3_file_rate(sample_rate))
                    .ok_or_else(|| ExportError::Invalid("sample rate cannot be zero".into()))?;
                let mut builder = Builder::new()
                    .ok_or_else(|| ExportError::Mp3("could not initialize LAME".into()))?
                    .with_num_channels(channels as u8)
                    .map_err(lame)?;
                if output.channels == OutputChannels::Mono {
                    builder = builder.with_mode(mp3lame_encoder::Mode::Mono).map_err(lame)?;
                }
                let encoder = builder
                    .with_sample_rate(sample_rate)
                    .map_err(lame)?
                    .with_output_sample_rate(Some(file_rate))
                    .map_err(lame)?
                    .with_brate(bitrate.lame())
                    .map_err(lame)?
                    .with_quality(Quality::Best)
                    .map_err(lame)?
                    .build()
                    .map_err(lame)?;
                Encoder::Mp3 {
                    encoder: Box::new(encoder),
                    encoded: Vec::new(),
                }
            }
        };
        Ok(Self {
            tap: output.tap,
            channels: output.channels,
            encoder,
            mono: match output.channels {
                OutputChannels::Stereo => Vec::new(),
                OutputChannels::Mono => vec![0.0; OFFLINE_BLOCK_FRAMES],
            },
            stem: match output.tap {
                RenderTap::Master => [Vec::new(), Vec::new()],
                RenderTap::Track(_) | RenderTap::Channel(_) => {
                    [vec![0.0; OFFLINE_BLOCK_FRAMES], vec![0.0; OFFLINE_BLOCK_FRAMES]]
                }
            },
            overs: 0,
            non_finite: 0,
            audible: false,
        })
    }

    /// Hand this block of the render to the file, from the sink's tap.
    fn feed(&mut self, state: &RenderState, frames: usize) -> Result<(), ExportError> {
        match self.tap {
            RenderTap::Master => {
                let master = state.master();
                let (left, right) = (&master.l[..frames], &master.r[..frames]);
                self.audible |= is_audible(left, right);
                self.write(left, right)
            }
            RenderTap::Track(_) | RenderTap::Channel(_) => {
                let source = match self.tap {
                    RenderTap::Track(track) => state.track_output(usize::from(track)),
                    RenderTap::Channel(channel) => state.channel_output(usize::from(channel)),
                    RenderTap::Master => None,
                };
                let [mut left, mut right] = std::mem::take(&mut self.stem);
                match source {
                    Some(bus) => {
                        left[..frames].copy_from_slice(&bus.l[..frames]);
                        right[..frames].copy_from_slice(&bus.r[..frames]);
                    }
                    None => {
                        left[..frames].fill(0.0);
                        right[..frames].fill(0.0);
                    }
                }
                for sample in left[..frames].iter_mut().chain(&mut right[..frames]) {
                    if !sample.is_finite() {
                        *sample = 0.0;
                        self.non_finite += 1;
                    } else if sample.abs() > 1.0 {
                        self.overs += 1;
                    }
                }
                self.audible |= is_audible(&left[..frames], &right[..frames]);
                let written = self.write(&left[..frames], &right[..frames]);
                self.stem = [left, right];
                written
            }
        }
    }

    fn write(&mut self, left: &[f32], right: &[f32]) -> Result<(), ExportError> {
        let mono = match self.channels {
            OutputChannels::Stereo => None,
            OutputChannels::Mono => {
                let mono = &mut self.mono[..left.len()];
                for ((mono, &left), &right) in mono.iter_mut().zip(left).zip(right) {
                    *mono = (left + right) * 0.5;
                }
                Some(&*mono)
            }
        };
        match &mut self.encoder {
            Encoder::Wav {
                writer,
                encoding,
                dither,
                clipped,
            } => {
                let mut put = |sample: f32| -> Result<(), ExportError> {
                    match full_scale(*encoding) {
                        Some(scale) => {
                            *clipped += u64::from(sample.abs() > 1.0);
                            writer.write_sample(pcm(sample, scale, dither.as_mut()))?;
                        }
                        None => writer.write_sample(sample)?,
                    }
                    Ok(())
                };
                match mono {
                    Some(mono) => {
                        for &sample in mono {
                            put(sample)?;
                        }
                    }
                    None => {
                        for (&left, &right) in left.iter().zip(right) {
                            put(left)?;
                            put(right)?;
                        }
                    }
                }
            }
            Encoder::Mp3 { encoder, encoded } => {
                encoded.reserve(mp3lame_encoder::max_required_buffer_size(left.len()));
                match mono {
                    Some(mono) => encoder.encode_to_vec(MonoPcm(mono), encoded),
                    None => encoder.encode_to_vec(DualPcm { left, right }, encoded),
                }
                .map_err(|error| ExportError::Mp3(error.to_string()))?;
            }
        }
        Ok(())
    }

    /// Close the file at `path`, returning how many samples were clamped.
    fn finish(self, path: &Path) -> Result<u64, ExportError> {
        match self.encoder {
            Encoder::Wav {
                writer, clipped, ..
            } => {
                writer.finalize()?;
                Ok(clipped)
            }
            Encoder::Mp3 {
                mut encoder,
                mut encoded,
            } => {
                encoded.reserve(7200);
                encoder
                    .flush_to_vec::<FlushGap>(&mut encoded)
                    .map_err(|error| ExportError::Mp3(error.to_string()))?;
                let mut file = fs::File::create(path)?;
                file.write_all(&encoded)?;
                file.flush()?;
                Ok(0)
            }
        }
    }
}

/// An encoder error, as the export reports it.
fn lame(error: impl fmt::Display) -> ExportError {
    ExportError::Mp3(error.to_string())
}


#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{EffectSlotState, NoteEvent, PatternPlacement, ProjectChannel};
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

    fn master(path: &Path, format: ExportFormat) -> RenderOutput {
        RenderOutput::new(path.to_path_buf(), RenderTap::Master, format)
    }

    fn pattern_pass(tail_seconds: f32, outputs: Vec<RenderOutput>) -> RenderPass {
        RenderPass {
            scope: RenderScope::Pattern { index: 0 },
            tail_seconds,
            outputs,
        }
    }

    /// **A numbered output never replaces a file that appeared at its path
    /// after the job was built** (MOO-188): it lands on the next free number
    /// of its name, says so in the file it reports, and the file that was
    /// there is untouched. The dialog has nothing to ask about it.
    #[test]
    fn a_numbered_output_never_replaces_a_file_that_appeared_since() {
        let temp = tempdir().unwrap();
        let base = temp.path().join("song.wav");
        let job = RenderJob {
            passes: vec![pattern_pass(
                0.0,
                vec![RenderOutput {
                    existing: ExistingFile::Number { base: base.clone() },
                    ..master(&base, ExportFormat::Wav(WavEncoding::Float32))
                }],
            )],
        };
        assert!(job.existing_targets().is_empty());
        // Between resolving the name and the render's last step, somebody
        // else writes `song.wav` and `song-001.wav`.
        fs::write(&base, b"theirs").unwrap();
        fs::write(temp.path().join("song-001.wav"), b"theirs too").unwrap();
        assert!(job.existing_targets().is_empty(), "a numbered output never asks");

        let files = OfflineRenderer::render_job(
            &sampler_project(1.0),
            &[Some(sample_of(tone))],
            48_000,
            &job,
            &ExportProgress::new(),
        )
        .unwrap();

        let landed = temp.path().join("song-002.wav");
        assert_eq!(files[0].path, landed);
        assert!(hound::WavReader::open(&landed).is_ok());
        assert_eq!(fs::read(&base).unwrap(), b"theirs");
        assert_eq!(fs::read(temp.path().join("song-001.wav")).unwrap(), b"theirs too");
        let leftovers = fs::read_dir(temp.path()).unwrap().count();
        assert_eq!(leftovers, 3, "no partial file is left behind");
    }

    /// **Two outputs of one timeline are one render, under one bar**
    /// (MOO-180).
    #[test]
    fn a_job_writes_every_output_of_a_pass_from_one_render() {
        let temp = tempdir().unwrap();
        let float = temp.path().join("master.wav");
        let pcm = temp.path().join("master-24.wav");
        let mp3 = temp.path().join("master.mp3");
        let job = RenderJob {
            passes: vec![pattern_pass(
                1.0,
                vec![
                    master(&float, ExportFormat::Wav(WavEncoding::Float32)),
                    master(&pcm, ExportFormat::Wav(WavEncoding::Pcm24)),
                    master(&mp3, ExportFormat::Mp3(Mp3Bitrate::Kbps192)),
                ],
            )],
        };
        let progress = ExportProgress::new();
        let files = OfflineRenderer::render_job(
            &sampler_project(1.0),
            &[Some(sample_of(tone))],
            48_000,
            &job,
            &progress,
        )
        .unwrap();

        let paths: Vec<_> = files.iter().map(|file| file.path.clone()).collect();
        assert_eq!(paths, [float.clone(), pcm.clone(), mp3.clone()]);
        let total = files[0].summary.total_frames;
        assert!(files.iter().all(|file| file.summary.total_frames == total));
        // One pass: the bar ran over the timeline once, not once per file.
        assert_eq!(progress.most.load(Ordering::Relaxed), total);
        assert_eq!(progress.fraction(), Some(1.0));

        let read_float: Vec<f32> = hound::WavReader::open(&float)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect();
        let read_pcm: Vec<i32> = hound::WavReader::open(&pcm)
            .unwrap()
            .samples::<i32>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(read_float.len() as u64, total * 2);
        assert_eq!(read_pcm.len(), read_float.len());
        assert!(read_float.iter().any(|sample| sample.abs() > 0.1));
        for (index, (&float, &written)) in read_float.iter().zip(&read_pcm).enumerate() {
            assert_eq!(written, super::pcm(float, 8_388_607.0, None), "sample {index} differs between the two files");
        }
        assert!(fs::read(&mp3).unwrap().len() > 1_000);
    }

    /// **Cancelling a job keeps the files that had finished, and leaves the
    /// one in progress as it was** (MOO-180).
    #[test]
    fn cancelling_a_job_keeps_the_passes_that_had_finished() {
        let temp = tempdir().unwrap();
        let first = temp.path().join("first.wav");
        let second = temp.path().join("second.wav");
        fs::write(&second, b"the file that was here").unwrap();
        let job = RenderJob {
            passes: vec![
                pattern_pass(0.0, vec![master(&first, ExportFormat::Wav(WavEncoding::Float32))]),
                pattern_pass(0.0, vec![master(&second, ExportFormat::Wav(WavEncoding::Float32))]),
            ],
        };
        let progress = ExportProgress::new();
        // The pattern is two bars at 120 BPM: 96 000 frames a pass. Cancel a
        // few blocks into the second.
        progress
            .cancel_after
            .store(96_000 + 4 * OFFLINE_BLOCK_FRAMES as u64, Ordering::Relaxed);
        let failure = OfflineRenderer::render_job(
            &sampler_project(1.0),
            &[Some(sample_of(tone))],
            48_000,
            &job,
            &progress,
        )
        .unwrap_err();

        assert!(matches!(failure.error, ExportError::Cancelled), "{failure:?}");
        assert_eq!(failure.written.len(), 1);
        assert_eq!(failure.written[0].path, first);
        assert_eq!(
            u64::from(hound::WavReader::open(&first).unwrap().duration()),
            96_000
        );
        assert_eq!(fs::read(&second).unwrap(), b"the file that was here");
        let mut names: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["first.wav", "second.wav"], "a partial file was left behind");
    }

    /// A job names the files it would replace, and refuses to write one file
    /// twice.
    #[test]
    fn a_job_lists_what_it_would_replace_and_refuses_a_path_twice() {
        let temp = tempdir().unwrap();
        let here = temp.path().join("here.wav");
        let new = temp.path().join("new.wav");
        fs::write(&here, b"old").unwrap();
        let wav = ExportFormat::Wav(WavEncoding::Float32);
        let job = RenderJob {
            passes: vec![pattern_pass(0.0, vec![master(&here, wav), master(&new, wav)])],
        };
        assert_eq!(job.existing_targets(), [here]);

        let twice = RenderJob {
            passes: vec![
                pattern_pass(0.0, vec![master(&new, wav)]),
                pattern_pass(0.0, vec![master(&new, wav)]),
            ],
        };
        let failure = OfflineRenderer::render_job(
            &audible_project(),
            &[],
            48_000,
            &twice,
            &ExportProgress::new(),
        )
        .unwrap_err();
        assert!(matches!(failure.error, ExportError::Invalid(_)), "{failure:?}");
        assert!(!new.exists());
    }

    const FRAMES_PER_TICK: u64 = 250; // 48 kHz at 120 BPM, 96 PPQ.
    const BAR: u32 = mooloop_core::TICKS_PER_BAR;

    fn long_sample(frames: usize, value: impl Fn(usize) -> f32) -> Arc<SampleData> {
        Arc::new(SampleData {
            frames: (0..frames)
                .map(|index| {
                    let v = value(index);
                    [v, v]
                })
                .collect(),
            sample_rate: 48_000,
            root_note: 60,
        })
    }

    fn render_float(
        project: &Project,
        sample: &Arc<SampleData>,
        scope: RenderScope,
        path: &Path,
    ) -> (RenderSummary, Vec<f32>) {
        let summary = OfflineRenderer::render(
            project,
            &[Some(sample.clone())],
            48_000,
            &ExportSpec {
                path: path.to_path_buf(),
                scope,
                tail_seconds: 0.0,
                format: ExportFormat::Wav(WavEncoding::Float32),
            },
        )
        .unwrap();
        let left = hound::WavReader::open(path)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .step_by(2)
            .collect();
        (summary, left)
    }

    fn onset(left: &[f32]) -> Option<usize> {
        left.iter().position(|sample| sample.abs() > 1e-4)
    }

    /// **A range starts on its first tick and is exactly its length**
    /// (MOO-181): a note on bar 3, rendered over bars 3 to 5, sounds on the
    /// file's first frames exactly as it does in the whole song, and the
    /// bars are `end - start` ticks of frames.
    #[test]
    fn a_range_starts_on_its_first_tick_and_is_exactly_its_length() {
        let temp = tempdir().unwrap();
        let mut project = sampler_project(1.0);
        // One bar of pattern, played on bars 1, 3 and 5: the song is five
        // bars and bar 3's note is the range's first.
        project.channels[0].notes[0].clear();
        project.channels[0].notes[0].push(NoteEvent::new(1, 0, BAR / 2, 60, 127));
        for bar in [0, 2, 4] {
            project.playlist.push(PatternPlacement::new(0, bar * BAR));
        }
        let sample = long_sample(96_000, |_| 0.5);

        let (whole, whole_left) =
            render_float(&project, &sample, RenderScope::Song, &temp.path().join("song.wav"));
        assert_eq!(whole.base_frames, 5 * u64::from(BAR) * FRAMES_PER_TICK);
        let range = RenderScope::Range {
            start_tick: 2 * BAR,
            end_tick: 4 * BAR,
        };
        let (summary, left) = render_float(&project, &sample, range, &temp.path().join("range.wav"));
        assert_eq!(summary.base_frames, 2 * u64::from(BAR) * FRAMES_PER_TICK);
        assert_eq!(left.len() as u64, summary.total_frames);

        let bar_3 = (2 * u64::from(BAR) * FRAMES_PER_TICK) as usize;
        let in_song = onset(&whole_left[bar_3..]).expect("bar 3's note sounds in the song");
        assert_eq!(onset(&left), Some(in_song), "the range's note is not where the song's is");
        assert!(in_song < 64, "bar 3's note starts {in_song} frames late");
    }

    /// **A range reads automation and a synced LFO where it starts, as
    /// playing through does** (MOO-181, MOO-127).
    ///
    /// The range starts on beat 2 of bar 2, five beats in: a three-beat LFO
    /// is two thirds through its cycle there, and a lane drawn from the top
    /// is part-way up its ramp. Either one read from zero would move the
    /// level for the whole file, so the range is held window by window
    /// against the same stretch of the whole song.
    #[test]
    fn a_range_holds_synced_lfos_and_automation_at_its_start() {
        use mooloop_core::{
            AutomationLane, AutomationPoint, EffectSlotState, EffectTarget, FilterMode,
            FilterParams, ModLfoParams, ModLfoWaveform, ModPolarity, ModRoute, ModTimeDivision,
            ModulatorParams, ParamAddr, FILTER_PARAM_CUTOFF_HZ,
        };
        let temp = tempdir().unwrap();
        let mut project = sampler_project(1.0);
        project.pattern_lengths[0] = 64; // four bars
        project.playlist.push(PatternPlacement::new(0, 0));
        let start = BAR + BAR / mooloop_core::BEATS_PER_BAR;
        let end = 3 * BAR;
        let channel = &mut project.channels[0];
        channel.notes[0].clear();
        channel.notes[0].push(NoteEvent::new(1, 0, BAR, 60, 127));
        channel.notes[0].push(NoteEvent::new(2, start, end - start, 60, 127));
        let filter = |cutoff_hz| {
            EffectSlotState::filter(FilterParams {
                cutoff_hz,
                resonance: 0.0,
                mode: FilterMode::LowPass,
                ..FilterParams::default()
            })
        };
        let swept = channel.setup.push_effect(filter(1_500.0)).expect("pushed");
        let automated = channel.setup.push_effect(filter(8_000.0)).expect("pushed");
        channel.setup.modulation.install(
            0,
            ModulatorParams::Lfo(ModLfoParams {
                tempo_sync: true,
                rate_division: ModTimeDivision::DottedHalf,
                waveform: ModLfoWaveform::Saw,
                ..ModLfoParams::default()
            }),
        );
        let cutoff = |device| ParamAddr::effect(EffectTarget::Channel(0), device, FILTER_PARAM_CUTOFF_HZ);
        assert!(channel
            .setup
            .modulation
            .add_route(ModRoute::to_slot(0, cutoff(swept), 0.4, ModPolarity::Bipolar))
            .is_some());
        let mut lane = AutomationLane::new(cutoff(automated));
        lane.upsert(AutomationPoint::new(1, 0, 0.95));
        lane.upsert(AutomationPoint::new(2, 4 * BAR, 0.55));
        channel.automation[0].push(lane);
        // A bright tone, so where the cutoffs sit is the level.
        let sample = long_sample(4 * 96_000, |index| {
            0.5 * (index as f32 * std::f32::consts::TAU * 2_000.0 / 48_000.0).sin()
        });

        let (_, whole) =
            render_float(&project, &sample, RenderScope::Song, &temp.path().join("song.wav"));
        let range = RenderScope::Range {
            start_tick: start,
            end_tick: end,
        };
        let (summary, part) = render_float(&project, &sample, range, &temp.path().join("range.wav"));
        let from = (u64::from(start) * FRAMES_PER_TICK) as usize;
        assert_eq!(summary.base_frames, u64::from(end - start) * FRAMES_PER_TICK);
        let through = &whole[from..from + part.len()];

        const WINDOW: usize = 2_400;
        let rms = |block: &[f32]| {
            let power = block.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / block.len() as f64;
            10.0 * power.max(1e-20).log10()
        };
        let mut levels = Vec::new();
        // The first windows hold the parameters' smoothing from the state's
        // defaults, which a locate has in playback too.
        for (index, (ours, theirs)) in part
            .as_chunks::<WINDOW>()
            .0
            .iter()
            .zip(through.as_chunks::<WINDOW>().0)
            .enumerate()
            .skip(2)
        {
            let (ours, theirs) = (rms(&ours[..]), rms(&theirs[..]));
            assert!(
                (ours - theirs).abs() < 0.5,
                "window {index}: the range reads {ours:.2} dB, playing through {theirs:.2} dB"
            );
            levels.push(ours);
        }
        let spread = levels.iter().cloned().fold(f64::MIN, f64::max)
            - levels.iter().cloned().fold(f64::MAX, f64::min);
        assert!(spread > 3.0, "the modulation moves the level by only {spread:.2} dB");
    }

    /// A range that is empty or runs past the song is refused, not rendered.
    #[test]
    fn a_range_outside_the_song_is_refused() {
        let temp = tempdir().unwrap();
        let mut project = audible_project();
        project.playlist.push(PatternPlacement::new(0, 0));
        for (start_tick, end_tick) in [(BAR, BAR), (BAR, 0), (0, 2 * BAR)] {
            let path = temp.path().join("refused.wav");
            let result = OfflineRenderer::render(
                &project,
                &[],
                48_000,
                &ExportSpec {
                    path: path.clone(),
                    scope: RenderScope::Range {
                        start_tick,
                        end_tick,
                    },
                    tail_seconds: 0.0,
                    format: ExportFormat::Wav(WavEncoding::Float32),
                },
            );
            assert!(matches!(result, Err(ExportError::Invalid(_))), "{result:?}");
            assert!(!path.exists());
        }
    }

    fn render_one(
        project: &Project,
        sample: &Arc<SampleData>,
        path: &Path,
        format: ExportFormat,
        channels: OutputChannels,
        dither: bool,
    ) -> RenderSummary {
        let job = RenderJob {
            passes: vec![pattern_pass(
                0.0,
                vec![RenderOutput {
                    channels,
                    dither,
                    ..master(path, format)
                }],
            )],
        };
        OfflineRenderer::render_job(project, &[Some(sample.clone())], 48_000, &job, &ExportProgress::new())
            .unwrap()
            .remove(0)
            .summary
    }

    /// Every channel of a WAV, one `Vec` per channel, as floats at full
    /// scale 1.0 whatever the depth.
    fn wav_channels(path: &Path) -> Vec<Vec<f32>> {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map(Result::unwrap).collect(),
            hound::SampleFormat::Int => {
                let scale = ((1i64 << (spec.bits_per_sample - 1)) - 1) as f32;
                reader
                    .samples::<i32>()
                    .map(|sample| sample.unwrap() as f32 / scale)
                    .collect()
            }
        };
        let channels = usize::from(spec.channels);
        (0..channels)
            .map(|channel| samples.iter().skip(channel).step_by(channels).copied().collect())
            .collect()
    }

    /// **A 16-bit file of a quiet tone has a flat dither floor, not
    /// rounding's harmonics** (MOO-186).
    ///
    /// A 1 kHz sine about one LSB tall at 16 bits (-90 dBFS). Rounded bare,
    /// it becomes a stepped wave whose odd harmonics stand far above the
    /// floor; with TPDF dither the harmonics sink into noise that is the
    /// same across the band, and the tone itself is still there at its
    /// level. The undithered file is rendered too, so the measure is shown
    /// to see what it is looking for.
    #[test]
    fn a_dithered_16_bit_tone_has_a_flat_floor_and_no_harmonics() {
        use mooloop_dsp::testkit::{band_rms, tone_amplitude};
        const LSB: f32 = 1.0 / 32_767.0;
        let temp = tempdir().unwrap();
        // The master hears a centred sampler 3 dB down (the pan law), so
        // the sample is that much hotter than the tone wanted in the file.
        let wanted = mooloop_dsp::testkit::from_db(-90.0);
        let project = sampler_project(1.0);
        let sample = long_sample(48_000, |index| {
            wanted * std::f32::consts::SQRT_2
                * (index as f32 * std::f32::consts::TAU * 1_000.0 / 48_000.0).sin()
        });
        let wav16 = ExportFormat::Wav(WavEncoding::Pcm16);
        let render = |name: &str, format, dither| {
            let path = temp.path().join(name);
            render_one(&project, &sample, &path, format, OutputChannels::Stereo, dither);
            // The note is 24 000 frames long; measure inside it.
            wav_channels(&path).remove(0)[2_000..22_000].to_vec()
        };
        let float = render("float.wav", ExportFormat::Wav(WavEncoding::Float32), false);
        let bare = render("bare.wav", wav16, false);
        let dithered = render("dithered.wav", wav16, true);

        let tone = tone_amplitude(&float, 48_000, 1_000.0);
        assert!(
            (0.5 * LSB..3.0 * LSB).contains(&tone),
            "the tone is {} LSB, not about one",
            tone / LSB
        );
        let harmonic = |samples: &[f32], k: f32| tone_amplitude(samples, 48_000, 1_000.0 * k) / LSB;
        let worst = |samples: &[f32]| [3.0, 5.0, 7.0].map(|k| harmonic(samples, k)).into_iter().fold(0.0, f32::max);
        assert!(worst(&bare) > 0.05, "rounding bare showed no harmonics: {}", worst(&bare));
        assert!(
            worst(&dithered) < 0.02,
            "dither left a harmonic {} LSB tall",
            worst(&dithered)
        );
        let kept = tone_amplitude(&dithered, 48_000, 1_000.0);
        assert!(
            (kept / tone - 1.0).abs() < 0.12,
            "dither moved the tone from {} to {} LSB",
            tone / LSB,
            kept / LSB
        );
        // Flat: the floor's density, per root hertz, is the same low, middle
        // and high in the band.
        let density = |band: (f32, f32)| band_rms(&dithered, 48_000, band) / (band.1 - band.0).sqrt();
        let floors = [density((2_500.0, 4_500.0)), density((8_000.0, 12_000.0)), density((15_000.0, 20_000.0))];
        let (low, high) = floors
            .iter()
            .fold((f32::MAX, 0.0f32), |(low, high), &f| (low.min(f), high.max(f)));
        assert!(
            mooloop_dsp::testkit::db(high / low) < 2.0,
            "the floor is not flat: {floors:?}"
        );
    }

    /// **Two dithered renders of one song are the same file** (MOO-186).
    #[test]
    fn two_dithered_renders_are_bit_identical() {
        let temp = tempdir().unwrap();
        let project = sampler_project(1.0);
        let sample = sample_of(tone);
        let bytes = |name: &str, format| {
            let path = temp.path().join(name);
            render_one(&project, &sample, &path, format, OutputChannels::Stereo, true);
            fs::read(path).unwrap()
        };
        for format in [
            ExportFormat::Wav(WavEncoding::Pcm16),
            ExportFormat::Wav(WavEncoding::Pcm24),
        ] {
            assert_eq!(bytes("a.wav", format), bytes("b.wav", format), "{format:?}");
        }
        // And dither is really there: the 24-bit dithered file is not the
        // bare one.
        let path = temp.path().join("bare.wav");
        render_one(&project, &sample, &path, ExportFormat::Wav(WavEncoding::Pcm24), OutputChannels::Stereo, false);
        assert_ne!(fs::read(path).unwrap(), bytes("c.wav", ExportFormat::Wav(WavEncoding::Pcm24)));
        // A float file ignores it.
        let float = ExportFormat::Wav(WavEncoding::Float32);
        let path = temp.path().join("float-bare.wav");
        render_one(&project, &sample, &path, float, OutputChannels::Stereo, false);
        assert_eq!(fs::read(path).unwrap(), bytes("float.wav", float));
    }

    /// **Two dithered outputs of one job carry different dither** (MOO-186):
    /// the rounding error of two 24-bit files of one silent-but-for-dither
    /// render is uncorrelated between the files, and between a file's left
    /// and right, so stems summed elsewhere add their floors as power.
    #[test]
    fn two_outputs_of_one_job_have_uncorrelated_dither() {
        let temp = tempdir().unwrap();
        let pcm24 = ExportFormat::Wav(WavEncoding::Pcm24);
        let output = |name: &str| RenderOutput {
            dither: true,
            ..master(&temp.path().join(name), pcm24)
        };
        let job = RenderJob {
            passes: vec![pattern_pass(0.0, vec![output("one.wav"), output("two.wav")])],
        };
        // Nothing plays: every non-zero code in the files is dither.
        let project = Project::default();
        OfflineRenderer::render_job(&project, &[], 48_000, &job, &ExportProgress::new()).unwrap();
        let one = wav_channels(&temp.path().join("one.wav"));
        let two = wav_channels(&temp.path().join("two.wav"));
        let correlation = |a: &[f32], b: &[f32]| {
            let dot: f64 = a.iter().zip(b).map(|(x, y)| f64::from(*x) * f64::from(*y)).sum();
            let norm = |v: &[f32]| v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
            dot / (norm(a) * norm(b)).max(1e-30)
        };
        assert!(one[0].iter().any(|s| *s != 0.0), "the silent render carried no dither");
        for (a, b, what) in [
            (&one[0], &two[0], "the two files' left"),
            (&one[1], &two[1], "the two files' right"),
            (&one[0], &one[1], "one file's left and right"),
        ] {
            let r = correlation(a, b);
            assert!(r.abs() < 0.02, "{what} correlate at {r:.4}");
        }
    }

    /// **A mono file is one channel of (L + R) / 2** (MOO-186): a centred
    /// source keeps the level each side had, and one panned hard left is
    /// half of what its side had, 6 dB down (`GAIN_STRUCTURE.md`, "Mono
    /// files").
    #[test]
    fn a_mono_file_is_one_channel_of_the_sides_average() {
        let temp = tempdir().unwrap();
        let float = ExportFormat::Wav(WavEncoding::Float32);
        let sample = sample_of(tone);
        for (pan, mono_over_left) in [(0.0f32, 1.0f32), (-1.0, 0.5)] {
            let mut project = sampler_project(1.0);
            project.channels[0].setup.channel.pan = pan;
            let stereo_path = temp.path().join("stereo.wav");
            let mono_path = temp.path().join("mono.wav");
            render_one(&project, &sample, &stereo_path, float, OutputChannels::Stereo, false);
            render_one(&project, &sample, &mono_path, float, OutputChannels::Mono, false);
            let stereo = wav_channels(&stereo_path);
            let mono = wav_channels(&mono_path);
            assert_eq!(mono.len(), 1, "a mono file has one channel");
            assert_eq!(mono[0].len(), stereo[0].len());
            for (index, ((&m, &l), &r)) in mono[0].iter().zip(&stereo[0]).zip(&stereo[1]).enumerate() {
                assert_eq!(m, (l + r) * 0.5, "frame {index}");
            }
            let (m, l) = (
                mooloop_dsp::testkit::rms(&mono[0]),
                mooloop_dsp::testkit::rms(&stereo[0]),
            );
            assert!(
                (m / l - mono_over_left).abs() < 1e-3,
                "pan {pan}: mono is {} of the left side, not {mono_over_left}",
                m / l
            );
        }

        // An MP3 in mono is LAME's mono mode: channel mode bits 11.
        let path = temp.path().join("mono.mp3");
        render_one(
            &sampler_project(1.0),
            &sample,
            &path,
            ExportFormat::Mp3(Mp3Bitrate::Kbps192),
            OutputChannels::Mono,
            false,
        );
        let bytes = fs::read(path).unwrap();
        let at = bytes
            .windows(2)
            .position(|pair| pair[0] == 0xff && pair[1] & 0xe0 == 0xe0)
            .expect("no MP3 frame");
        assert_eq!(bytes[at + 3] >> 6, 0b11, "the MP3 is not mono");
    }

    /// **An export flushes denormals the way playback does, so the two are
    /// the same to the bit** (MOO-223).
    ///
    /// A sampler playing a sample whose every value is subnormal. Native
    /// devices put themselves to sleep before a decaying tail gets that
    /// small -- a first draft of this test used a drum through a low
    /// lowpass and passed against the unfixed tree -- so the subnormals are
    /// fed in directly, the way a plugin's tail delivers them (the LSP
    /// filter's, in the report). The executor's thread flushes them to
    /// zero; the export's must too, or the two differ in exactly those
    /// samples. And the thread the export ran on gets its own mode back:
    /// the app's document worker is shared.
    #[test]
    fn an_export_flushes_denormals_like_playback_and_restores_the_thread() {
        use mooloop_dsp::sampler::ChannelAudioSnapshot;
        let temp = tempdir().unwrap();
        let project = sampler_project(1.0);
        let sample = long_sample(24_000, |_| 3.0e-39);
        assert!(sample.frames[0][0].is_subnormal());
        let path = temp.path().join("denormal.wav");
        let summary = OfflineRenderer::render(
            &project,
            &[Some(sample.clone())],
            48_000,
            &ExportSpec {
                path: path.clone(),
                scope: RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format: ExportFormat::Wav(WavEncoding::Float32),
            },
        )
        .unwrap();
        let exported: Vec<f32> = hound::WavReader::open(&path)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect();
        let subnormal = exported.iter().filter(|s| s.is_subnormal()).count();
        assert_eq!(subnormal, 0, "the export kept {subnormal} subnormal samples");

        let state = project.channels[0].setup.source.sampler_state().expect("a sampler");
        let audio = vec![ChannelAudioSnapshot::for_sampler(
            Some(sample),
            &state.slices,
            state.keys,
            Vec::new(),
        )];
        let played = crate::live_check::play_audio_through_executor(
            &project,
            audio,
            48_000,
            summary.base_frames as usize,
            OFFLINE_BLOCK_FRAMES,
        );
        assert_eq!(played.len(), exported.len());
        let differing = played
            .iter()
            .zip(&exported)
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        assert_eq!(differing, 0, "{differing} samples differ between playback and the export");

        // This thread still makes subnormals: the export put its mode back.
        let tiny = std::hint::black_box(f32::MIN_POSITIVE) * std::hint::black_box(0.5f32);
        assert!(tiny.is_subnormal(), "the export left flush-to-zero on its caller's thread");
    }

    /// Two drum channels, each on its own track, both tracks straight to the
    /// master; track 2 has a reverb on it and track 1 a lowered fader. The
    /// master has nothing on it and sits at unity, so its output is its
    /// input as long as it stays under the ceiling.
    fn two_track_project() -> Project {
        use mooloop_core::{EffectParams, ReverbParams};
        let mut project = Project::default();
        project.ensure_tracks(3);
        let mut kick = ProjectChannel::drum_synth(0, 1);
        kick.notes[0].push(NoteEvent::new(1, 0, 24, 36, 100));
        kick.setup.channel.bus = 1;
        let mut snare = ProjectChannel::drum_synth(1, 1);
        snare.notes[0].push(NoteEvent::new(1, 96, 24, 38, 100));
        snare.setup.channel.bus = 2;
        project.channels = vec![kick, snare];
        project.buses[1].bus.volume = 0.7;
        project.buses[2].push_effect(EffectSlotState::new(EffectParams::Reverb(
            ReverbParams::default(),
        )));
        project
    }

    fn stem(path: &Path, track: u8) -> RenderOutput {
        RenderOutput::new(
            path.to_path_buf(),
            RenderTap::Track(track),
            ExportFormat::Wav(WavEncoding::Float32),
        )
    }

    /// **Every top-level track's stem, summed, is the master's input, and
    /// every stem is as long as the master** (MOO-182).
    ///
    /// One pass renders the master and both stems. Track 2's reverb rings on
    /// into the tail, which runs until the whole project is at rest, so the
    /// dry track's stem is as long as the wet one's.
    #[test]
    fn the_stems_of_every_top_level_track_sum_to_the_masters_input() {
        let temp = tempdir().unwrap();
        let project = two_track_project();
        let paths = ["master.wav", "kick.wav", "snare.wav"].map(|name| temp.path().join(name));
        let float = ExportFormat::Wav(WavEncoding::Float32);
        let job = RenderJob {
            passes: vec![pattern_pass(
                5.0,
                vec![master(&paths[0], float), stem(&paths[1], 1), stem(&paths[2], 2)],
            )],
        };
        let files =
            OfflineRenderer::render_job(&project, &[], 48_000, &job, &ExportProgress::new()).unwrap();
        assert_eq!(files.len(), 3);
        let total = files[0].summary.total_frames;
        assert!(files[0].summary.tail_frames > 0, "the reverb left no tail");
        assert!(files.iter().all(|file| file.summary.total_frames == total));

        let [mix, kick, snare] = paths.map(|path| wav_channels(&path));
        for channels in [&mix, &kick, &snare] {
            assert_eq!(channels[0].len() as u64, total, "a stem is not the master's length");
        }
        assert!(mooloop_dsp::testkit::peak(&kick[0]) > 0.01, "the kick stem is silent");
        assert!(mooloop_dsp::testkit::peak(&snare[0]) > 0.01, "the snare stem is silent");
        assert!(mooloop_dsp::testkit::peak(&mix[0]) < 1.0, "the mix hit the limiter");
        let mut worst = 0.0f32;
        for side in 0..2 {
            for ((&m, &k), &s) in mix[side].iter().zip(&kick[side]).zip(&snare[side]) {
                worst = worst.max((m - (k + s)).abs());
            }
        }
        assert!(worst < 1e-6, "the stems summed are {worst} from the master's input");
    }

    /// **A muted track's stem is silence, and with "skip silent" it is not
    /// written at all** (MOO-182). The master still renders the other.
    #[test]
    fn a_muted_tracks_stem_is_silent_and_can_be_skipped() {
        let temp = tempdir().unwrap();
        let mut project = two_track_project();
        project.buses[2].bus.muted = true;
        let kept = temp.path().join("snare-kept.wav");
        let skipped = temp.path().join("snare-skipped.wav");
        let kick = temp.path().join("kick.wav");
        let job = RenderJob {
            passes: vec![pattern_pass(
                0.0,
                vec![
                    stem(&kick, 1),
                    stem(&kept, 2),
                    RenderOutput {
                        skip_silent: true,
                        ..stem(&skipped, 2)
                    },
                ],
            )],
        };
        let files =
            OfflineRenderer::render_job(&project, &[], 48_000, &job, &ExportProgress::new()).unwrap();
        let written: Vec<_> = files.iter().map(|file| file.path.clone()).collect();
        assert_eq!(written, [kick, kept.clone()]);
        assert!(!skipped.exists(), "a silent stem was written with skip on");
        assert!(wav_channels(&kept).iter().flatten().all(|sample| *sample == 0.0));
        let names = fs::read_dir(temp.path()).unwrap().count();
        assert_eq!(names, 2, "a partial file was left behind");
    }

    /// A stem has no limiter: an over is written as it is in float, counted,
    /// and a non-finite sample is silence, counted -- per file.
    #[test]
    fn a_stem_counts_its_own_overs_and_writes_them_as_they_are() {
        let temp = tempdir().unwrap();
        let mut project = sampler_project(mooloop_core::MAX_LINEAR_GAIN);
        project.ensure_tracks(2);
        project.channels[0].setup.channel.bus = 1;
        let hot = temp.path().join("hot-stem.wav");
        let mix = temp.path().join("mix.wav");
        let float = ExportFormat::Wav(WavEncoding::Float32);
        let job = RenderJob {
            passes: vec![pattern_pass(0.0, vec![stem(&hot, 1), master(&mix, float)])],
        };
        let files = OfflineRenderer::render_job(
            &project,
            &[Some(sample_of(full_scale))],
            48_000,
            &job,
            &ExportProgress::new(),
        )
        .unwrap();
        assert!(files[0].summary.overs > 0, "the stem reported no overs");
        let peak = mooloop_dsp::testkit::peak(&wav_channels(&hot)[0]);
        assert!(peak > 1.0, "the stem was limited to {peak}");
        let limited = mooloop_dsp::testkit::peak(&wav_channels(&mix)[0]);
        assert!(limited <= 1.0, "the master was not limited: {limited}");
    }

    /// Two drum channels on one track with nothing on it, at unity: the
    /// track's output is its input.
    fn one_track_two_channels() -> Project {
        let mut project = two_track_project();
        for channel in &mut project.channels {
            channel.setup.channel.bus = 1;
        }
        project.buses[1].bus.volume = 1.0;
        project
    }

    fn channel_stem(path: &Path, channel: u8) -> RenderOutput {
        RenderOutput::new(
            path.to_path_buf(),
            RenderTap::Channel(channel),
            ExportFormat::Wav(WavEncoding::Float32),
        )
    }

    /// **Two channels feeding one track, summed, are that track's input**
    /// (MOO-183): each channel file is its source, rack, fader and pan, and
    /// nothing after.
    #[test]
    fn two_channel_stems_sum_to_their_tracks_input() {
        let temp = tempdir().unwrap();
        let project = one_track_two_channels();
        let paths = ["track.wav", "kick.wav", "snare.wav"].map(|name| temp.path().join(name));
        let job = RenderJob {
            passes: vec![pattern_pass(
                1.0,
                vec![
                    stem(&paths[0], 1),
                    channel_stem(&paths[1], 0),
                    channel_stem(&paths[2], 1),
                ],
            )],
        };
        let files =
            OfflineRenderer::render_job(&project, &[], 48_000, &job, &ExportProgress::new()).unwrap();
        assert_eq!(files.len(), 3);
        let [track, kick, snare] = paths.map(|path| wav_channels(&path));
        assert!(mooloop_dsp::testkit::peak(&kick[0]) > 0.01, "the kick channel is silent");
        assert!(mooloop_dsp::testkit::peak(&snare[0]) > 0.01, "the snare channel is silent");
        let mut worst = 0.0f32;
        for side in 0..2 {
            assert_eq!(kick[side].len(), track[side].len());
            for ((&t, &k), &s) in track[side].iter().zip(&kick[side]).zip(&snare[side]) {
                worst = worst.max((t - (k + s)).abs());
            }
        }
        assert!(worst < 1e-6, "the channels summed are {worst} from their track's input");
    }

    /// **A channel renders with its track muted, since the mixer is
    /// bypassed; a muted channel renders silence** (MOO-183).
    #[test]
    fn a_channel_stem_bypasses_its_tracks_mute_but_not_its_own() {
        let temp = tempdir().unwrap();
        let mut project = one_track_two_channels();
        project.buses[1].bus.muted = true;
        project.channels[1].setup.channel.muted = true;
        let (kick, snare) = (temp.path().join("kick.wav"), temp.path().join("snare.wav"));
        let job = RenderJob {
            passes: vec![pattern_pass(
                0.0,
                vec![channel_stem(&kick, 0), channel_stem(&snare, 1)],
            )],
        };
        OfflineRenderer::render_job(&project, &[], 48_000, &job, &ExportProgress::new()).unwrap();
        assert!(
            mooloop_dsp::testkit::peak(&wav_channels(&kick)[0]) > 0.01,
            "a channel on a muted track rendered silence"
        );
        assert!(
            wav_channels(&snare).iter().flatten().all(|sample| *sample == 0.0),
            "a muted channel rendered sound"
        );
        let missing = RenderJob {
            passes: vec![pattern_pass(0.0, vec![channel_stem(&temp.path().join("x.wav"), 9)])],
        };
        let failure =
            OfflineRenderer::render_job(&project, &[], 48_000, &missing, &ExportProgress::new())
                .unwrap_err();
        assert!(matches!(failure.error, ExportError::Invalid(_)), "{failure:?}");
    }

    /// A stem of a track the song does not have, or of the master's own
    /// bus, is refused before anything is written.
    #[test]
    fn a_stem_of_no_track_is_refused() {
        let temp = tempdir().unwrap();
        for track in [0, 7] {
            let path = temp.path().join("nothing.wav");
            let job = RenderJob {
                passes: vec![pattern_pass(0.0, vec![stem(&path, track)])],
            };
            let failure = OfflineRenderer::render_job(
                &two_track_project(),
                &[],
                48_000,
                &job,
                &ExportProgress::new(),
            )
            .unwrap_err();
            assert!(matches!(failure.error, ExportError::Invalid(_)), "{failure:?}");
            assert!(!path.exists());
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
