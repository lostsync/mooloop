//! Takes on the control side: arming one, draining its ring to a file, and
//! handing back what finished (`docs/plans/archive/audio-recording/03-capture.md`).
//!
//! The engine copies a channel's audio input into a ring and nothing else. A
//! **drain thread per take** is the only place a take touches the disk: it
//! pulls frames from the ring, writes a 32-bit float stereo WAV into the
//! recordings folder, and keeps a running peak summary the face draws the
//! growing waveform from (step 05) without reading the file back.
//!
//! A take ends three ways, and the drain treats them alike: the engine says
//! so (stop pressed, clip length reached, transport stopped) and the drain
//! has read every frame it was told about; or the ring is **abandoned** --
//! the producer dropped, because an install rebuilt the recording channel's
//! strip or the channel went -- and the drain has read what is left. Either
//! way the file is finalized and the take is reported, never left open.

use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use mooloop_core::ChannelId;
use mooloop_core::EngineCommand;
use mooloop_engine::{StructuralCommand, Take, TakeFrame, TakePhase, TakeStatus};

/// Frames per entry of a take's peak summary: about 21 ms at 48 kHz, which is
/// finer than any face will draw and small enough that an hour is a few
/// hundred kilobytes.
pub const PEAK_BUCKET_FRAMES: usize = 1024;

/// How long the ring holds while the drain is not reading it. It only has to
/// cover the drain thread's worst stall, never the whole take: the file is
/// what grows.
const RING_SECONDS: usize = 10;

/// How often an idle drain looks at the ring again.
const DRAIN_POLL: Duration = Duration::from_millis(5);

/// Seconds of audio between checkpoints. hound's `flush` rewrites the header
/// for everything written so far, so a take is a readable WAV up to its last
/// checkpoint whatever happens next: the process dies, a write fails, or the
/// machine loses power once the OS has written the page out. Without it only
/// `finalize` patched the header, and a take that never reached `finalize`
/// said zero frames and could not be opened at all.
const CHECKPOINT_SECONDS: u64 = 1;

/// How long [`TakeRecorder::finish_all`] waits for one drain to notice its
/// take ended and write what is left.
///
/// Generous against a ring that was nearly full at quit, and bounded because
/// the alternative is a window that will not close. A drain that overruns it
/// has its partial file removed and is reported, rather than being waited on.
const FINISH_DEADLINE: Duration = Duration::from_secs(5);

/// The per-bucket peak of a take so far, `[left, right]`, as absolute
/// values. Written by the drain, read by the interface.
pub type TakePeaks = Arc<Mutex<Vec<[f32; 2]>>>;

/// A take that has finished and been written.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedTake {
    /// The channel it was recorded on, by identity: it may have moved since.
    pub channel: ChannelId,
    pub path: PathBuf,
    pub frames: u64,
    /// Frames the ring could not hold. Non-zero means the file has a gap and
    /// the take is damaged.
    pub dropped: u64,
    /// The bar line it started on, in song ticks.
    pub start_tick: f64,
}

impl FinishedTake {
    pub fn is_damaged(&self) -> bool {
        self.dropped > 0
    }
}

/// What the drain thread comes back with.
#[derive(Debug)]
enum Drained {
    /// A file with at least one frame in it.
    Written { frames: u64 },
    /// The drain failed after at least one checkpoint. The file is a readable
    /// take of `frames` -- everything up to that checkpoint -- so it is kept
    /// and lands like a finished one, and `error` says what was lost.
    Partial { frames: u64, error: String },
    /// Nothing was recorded -- cancelled while waiting for its bar -- so the
    /// file was removed rather than left as an empty WAV.
    Empty,
    Failed(String),
}

/// One running take, as the control side holds it.
struct Running {
    channel: ChannelId,
    status: Arc<TakeStatus>,
    peaks: TakePeaks,
    path: PathBuf,
    drain: Option<JoinHandle<Drained>>,
}

/// What the interface reads about a channel's take while it runs.
#[derive(Debug, Clone)]
pub struct TakeView {
    pub phase: TakePhase,
    pub frames: u64,
    pub dropped: u64,
    pub peaks: TakePeaks,
}

/// Every take that is running or has not yet been collected.
///
/// Runtime state, not document state: nothing here is saved, and a take is
/// not an edit until step 04 turns a finished one into the channel's sample.
pub struct TakeRecorder {
    dir: PathBuf,
    running: Vec<Running>,
}

impl TakeRecorder {
    /// Takes go into `dir`, the recordings folder, until a save moves them
    /// into the project (step 04). Created on first use.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            running: Vec::new(),
        }
    }

    /// Arm a take on the channel at `seat`, whose identity is `channel`.
    ///
    /// Opens the file, starts its drain and returns the command that hands
    /// the take to the engine. The caller sends it -- and, if the transport
    /// is stopped, sends `Play` *before* it, since a take waits for the next
    /// bar line of a running transport (decision 9). A take already running
    /// on the channel is displaced by the engine and ends like any other.
    pub fn arm(
        &mut self,
        channel: ChannelId,
        seat: u8,
        name: &str,
        clip_ticks: Option<u32>,
        sample_rate: u32,
        start_delay_frames: u32,
    ) -> Result<StructuralCommand, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("could not create {}: {error}", self.dir.display()))?;
        // Refused up front, the way a missing input is, rather than found
        // out a few seconds in as a take cut short (MOO-75).
        if let Some(why) = short_of_space(free_bytes(&self.dir), sample_rate, &self.dir) {
            return Err(why);
        }
        let path = self.dir.join(take_file_name(name, SystemTime::now()));
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let writer = hound::WavWriter::create(&path, spec)
            .map_err(|error| format!("could not create {}: {error}", path.display()))?;
        let (producer, consumer) = rtrb::RingBuffer::new(sample_rate as usize * RING_SECONDS);
        let status = TakeStatus::new();
        let peaks: TakePeaks = Arc::new(Mutex::new(Vec::new()));
        let drain = {
            let (status, peaks, path) = (status.clone(), peaks.clone(), path.clone());
            std::thread::Builder::new()
                .name("take-drain".into())
                .spawn(move || drain(consumer, &status, writer, &peaks, &path))
                .map_err(|error| format!("could not start a drain: {error}"))?
        };
        self.running.push(Running {
            channel,
            status: status.clone(),
            peaks,
            path,
            drain: Some(drain),
        });
        Ok(StructuralCommand::StartTake {
            channel: seat,
            take: Take::new(producer, status, clip_ticks, start_delay_frames),
        })
    }

    /// The newest take on `channel`, if one is running or waiting to be
    /// collected.
    pub fn view(&self, channel: ChannelId) -> Option<TakeView> {
        self.running
            .iter()
            .rev()
            .find(|take| take.channel == channel)
            .map(|take| TakeView {
                phase: take.status.phase(),
                frames: take.status.frames(),
                dropped: take.status.dropped(),
                peaks: take.peaks.clone(),
            })
    }

    /// Collect every take whose drain has finished. A take that recorded
    /// nothing is dropped here, its empty file already removed; one whose
    /// drain failed is reported through `failures`.
    pub fn collect(&mut self) -> (Vec<FinishedTake>, Vec<String>) {
        let mut finished = Vec::new();
        let mut failures = Vec::new();
        let mut index = 0;
        while index < self.running.len() {
            let done = self.running[index]
                .drain
                .as_ref()
                .is_none_or(|handle| handle.is_finished());
            if !done {
                index += 1;
                continue;
            }
            let take = self.running.remove(index);
            classify(take, &mut finished, &mut failures);
        }
        (finished, failures)
    }

    /// Whether any take is still in flight -- armed, waiting for its bar, or
    /// recording.
    ///
    /// **Nothing in the application reads this**, and that is worth stating
    /// rather than leaving to be assumed: it was written as a quit prompt, and
    /// open question 9 came back "no prompt" -- what is outstanding at quit is
    /// a fraction of a second, so quit finishes the take and says nothing.
    /// What keeps it is [`Self::finish_all`]'s tests, which need to state the
    /// precondition they are testing; the clean-up dialog
    /// (`audio-recording/06`) is the plausible first real caller.
    pub fn has_live(&self) -> bool {
        self.running
            .iter()
            .any(|take| take.status.phase() != TakePhase::Ended)
    }

    /// End every take still running and collect all of them, waiting for
    /// each drain rather than skipping the ones still going.
    ///
    /// This is the quit path, and it is **silent by design**: Adam's answer to
    /// open question 9, 2026-09-21 -- quit ends the take the way Stop does,
    /// flushes, finalizes, and only then leaves, with no dialog, because what
    /// is outstanding is a fraction of a second. What it promises is a
    /// complete *file*, not a sample in the song; the song is closing.
    ///
    /// [`Self::collect`] is the pump's version and skips a drain that has not
    /// finished, which is right when it will be asked again in sixteen
    /// milliseconds and wrong when this is the last time anything will ask:
    /// only `finalize`, at the end of the drain, patches the whole take into
    /// the WAV header, so a take whose drain never runs out leaves a file
    /// that stops at its last checkpoint.
    ///
    /// Each drain is given [`FINISH_DEADLINE`] so a stuck one cannot hang
    /// quit; one that overruns has its partial file removed and is reported.
    pub fn finish_all(&mut self) -> (Vec<FinishedTake>, Vec<String>) {
        let mut finished = Vec::new();
        let mut failures = Vec::new();
        // Every take first, then the waiting: the drains then run down in
        // parallel against one deadline instead of one after another.
        for take in &self.running {
            take.status.end();
        }
        let deadline = Instant::now() + FINISH_DEADLINE;
        for mut take in std::mem::take(&mut self.running) {
            let ready = take.drain.as_ref().is_none_or(|handle| {
                while !handle.is_finished() {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    std::thread::sleep(DRAIN_POLL);
                }
                true
            });
            if ready {
                classify(take, &mut finished, &mut failures);
            } else {
                // **A timed-out wait removes the partial file** (open question 9,
                // answered 2026-09-21). When that was decided its header had
                // never been patched, so what was on disk was a WAV nothing
                // could read. Since checkpoints (`CHECKPOINT_SECONDS`) it reads
                // up to its last one, so the reason went -- and Adam revisited
                // it on 2026-09-22 and kept it: "if you wanted the file, you
                // shouldn't have quit while recording."
                let removed = std::fs::remove_file(&take.path).is_ok();
                failures.push(format!(
                    "{} did not finish writing within {} seconds{}",
                    take.path.display(),
                    FINISH_DEADLINE.as_secs(),
                    if removed {
                        "; the partial recording was removed"
                    } else {
                        " and may be short"
                    },
                ));
                // Dropped without a join: waiting longer is the one thing
                // this path has already decided not to do.
                take.drain = None;
            }
        }
        (finished, failures)
    }
}

impl Drop for TakeRecorder {
    /// The backstop for every route out that is not the one quit path.
    ///
    /// `mooloop-session` had no `Drop` at all, so a take in flight when the
    /// process ended left an unreadable file and said nothing. The interface
    /// calls [`Self::finish_all`] itself, while it can still put the outcome
    /// in front of somebody; by the time this runs there is nobody to tell,
    /// so the failures are dropped and only the file is saved.
    fn drop(&mut self) {
        if !self.running.is_empty() {
            let _ = self.finish_all();
        }
    }
}

/// Join one finished drain and file it as a finished take or a failure.
///
/// Factored out of [`TakeRecorder::collect`] so the quit path classifies a
/// take exactly the way the pump does; the two differ in *which* takes they
/// wait for, and must not differ in what they make of one.
fn classify(mut take: Running, finished: &mut Vec<FinishedTake>, failures: &mut Vec<String>) {
    let outcome = take.drain.take().map(|handle| {
        handle
            .join()
            .unwrap_or_else(|_| Drained::Failed("the drain thread panicked".into()))
    });
    match outcome {
        Some(Drained::Written { frames }) => finished.push(FinishedTake {
            channel: take.channel,
            path: take.path,
            frames,
            dropped: take.status.dropped(),
            start_tick: take.status.start_tick(),
        }),
        Some(Drained::Partial { frames, error }) => {
            finished.push(FinishedTake {
                channel: take.channel,
                path: take.path,
                frames,
                dropped: take.status.dropped(),
                start_tick: take.status.start_tick(),
            });
            failures.push(error);
        }
        Some(Drained::Failed(error)) => failures.push(error),
        Some(Drained::Empty) | None => {}
    }
}

/// What pressing a sampler's REC does (`audio-recording/05`).
#[derive(Debug, Clone, PartialEq)]
pub enum RecordPress {
    /// A take is waiting or recording on this channel: end it.
    Stop { seat: u8 },
    /// Arm a new take. The caller starts the transport first if it is
    /// stopped, then sends [`TakeRecorder::arm`]'s command.
    Arm {
        channel: ChannelId,
        seat: u8,
        name: String,
        clip_ticks: Option<u32>,
        /// Whether the take records the hardware input, whose round-trip
        /// latency the caller passes to [`TakeRecorder::arm`] as its start
        /// delay.
        from_input: bool,
    },
    /// The channel has no AUDIO input, so there is nothing to record.
    NoInput,
    /// The channel or track this one resamples has been deleted.
    ///
    /// Split from [`Self::NoInputDevice`] on Adam's answer to open question
    /// 10, 2026-09-21:
    /// the two need different fixes from the user -- pick another source here,
    /// go and look at your audio hardware there -- so one message would send
    /// half of them looking in the wrong place. Both are distinct from
    /// [`Self::NoInput`], whose advice is to pick an input at all.
    SourceGone,
    /// The channel records the hardware input and there is none: unplugged,
    /// changed, or a microphone permission macOS refused.
    NoInputDevice,
}

/// Why a finished take has nowhere to land.
///
/// A take outlives the state it was armed against. It records for as long as
/// it is told to, and in that time its channel can be deleted, or have its
/// device swapped -- a channel is a dumb slot (`audio-recording` decision 5,
/// 2026-09-18), so nothing stops a sampler becoming a drum synth mid-take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeMiss {
    /// The channel it was recorded on is gone.
    ChannelGone,
    /// The channel is still there and no longer holds a sampler, so there is
    /// nothing for a sample to be written to.
    NotASampler,
}

impl TakeMiss {
    /// What to tell the user. Both keep the file, so both say where it is:
    /// the take is not lost, it just has nowhere to go by itself.
    pub fn message(self) -> &'static str {
        match self {
            Self::ChannelGone => {
                "The take's channel is gone; the recording is still in the recordings folder"
            }
            Self::NotASampler => {
                "The take's channel no longer holds a sampler; the recording is still in the \
                 recordings folder"
            }
        }
    }
}

impl crate::session::Session {
    /// Where a take recorded on `id` should land, or why it cannot.
    ///
    /// `apply_take` used to spell the channel lookup inline and check only
    /// that the channel still existed. The kind matters just as much:
    /// `reset_channel_source` clears a channel's sample state when its device
    /// changes, and `project_snapshot` writes `sample`/`embedded` only in its
    /// `Sampler` arm -- so a take written onto a channel that is now a drum
    /// synth produces a sample the face cannot draw, the save will not keep,
    /// and an undo entry that restores nothing visible.
    pub fn take_target(&self, id: ChannelId) -> Result<usize, TakeMiss> {
        let seat = self
            .channels
            .iter()
            .position(|channel| channel.id == id)
            .ok_or(TakeMiss::ChannelGone)?;
        if self.channels[seat].kind != mooloop_core::DeviceKind::Sampler {
            return Err(TakeMiss::NotASampler);
        }
        Ok(seat)
    }

    /// Whether `source` names something the picker no longer offers.
    ///
    /// The picker's rule, deliberately, and not [`mooloop_core::AudioInputSource::resolve`]'s:
    /// `resolve` answers `Some(AudioTap::Input)` for the hardware input
    /// whatever the driver actually has, so a channel set to Audio In under a
    /// driver with no input reads as present there and missing here. The face
    /// already draws that case as missing, and REC has to agree with the face.
    fn audio_input_missing(
        &self,
        source: mooloop_core::AudioInputSource,
        input_label: Option<&str>,
    ) -> bool {
        let rows = self.audio_source_rows(input_label);
        mooloop_core::AudioInputPicker::new(&rows).is_missing(source)
    }

    /// Decide what REC on the channel at `seat` does, given whether a take is
    /// already live there. `None` for a seat with no channel.
    ///
    /// `input_label` is the hardware input's name as the driver reports it,
    /// the same value the sidebar's AUDIO row is built from; `None` when the
    /// driver offers no input at all.
    pub fn record_press(
        &self,
        seat: usize,
        take_live: bool,
        input_label: Option<&str>,
    ) -> Option<RecordPress> {
        let channel = self.channels.get(seat)?;
        let seat_u8 = u8::try_from(seat).ok()?;
        Some(if take_live {
            RecordPress::Stop { seat: seat_u8 }
        } else if channel.audio_input.is_off() {
            RecordPress::NoInput
        } else if self.audio_input_missing(channel.audio_input, input_label) {
            // Arming here would record a ring of zeros into a file and make
            // it the sampler's sample, silently. Which cause it is decides
            // what the user has to go and do about it (open question 10).
            if channel.audio_input == mooloop_core::AudioInputSource::Input {
                RecordPress::NoInputDevice
            } else {
                RecordPress::SourceGone
            }
        } else {
            RecordPress::Arm {
                channel: channel.id,
                seat: seat_u8,
                name: channel.name.clone(),
                clip_ticks: channel.record.clip_ticks(),
                from_input: channel.audio_input == mooloop_core::AudioInputSource::Input,
            }
        })
    }

    /// Whether the channel at `seat` is monitoring the hardware input.
    pub fn is_monitoring(&self, seat: usize) -> bool {
        self.channels
            .get(seat)
            .is_some_and(|channel| self.input_monitor.contains(&channel.id))
    }

    /// Monitor the hardware input on the channel at `seat`, or stop. Returns
    /// the command for the engine when anything changed.
    pub fn set_input_monitor(&mut self, seat: usize, on: bool) -> Option<EngineCommand> {
        let id = self.channels.get(seat)?.id;
        let changed = if on {
            self.input_monitor.insert(id)
        } else {
            self.input_monitor.remove(&id)
        };
        changed.then_some(EngineCommand::SetInputMonitor {
            channel: seat as u8,
            on,
        })
    }

    /// Which of `project`'s channels monitor the hardware input, by seat, for
    /// an install's `InputState`.
    pub fn monitor_seats(&self, project: &mooloop_core::Project) -> Vec<bool> {
        project
            .channels
            .iter()
            .map(|channel| self.input_monitor.contains(&channel.id))
            .collect()
    }

    /// Turn the Record page's Clip on or off. Returns whether it changed.
    pub fn set_record_clip(&mut self, seat: usize, clip: bool) -> bool {
        let Some(channel) = self.channels.get_mut(seat) else {
            return false;
        };
        let changed = channel.record.clip != clip;
        channel.record.clip = clip;
        changed
    }

    /// Set the Record page's clip length, clamped to what it offers.
    pub fn set_record_bars(&mut self, seat: usize, bars: i32) -> bool {
        let Some(channel) = self.channels.get_mut(seat) else {
            return false;
        };
        let bars = bars.clamp(1, i32::from(mooloop_core::MAX_RECORD_BARS)) as u8;
        let changed = channel.record.bars != bars;
        channel.record.bars = bars;
        changed
    }
}

impl TakeView {
    /// Whether this take is still waiting or recording.
    pub fn is_live(&self) -> bool {
        self.phase != TakePhase::Ended
    }

    /// The take's peaks folded to at most `bins` bars, each the louder
    /// channel's peak, for the Record page to draw.
    pub fn bars(&self, bins: usize) -> Vec<f32> {
        let peaks = self.peaks.lock().map(|peaks| peaks.clone()).unwrap_or_default();
        if peaks.is_empty() || bins == 0 {
            return Vec::new();
        }
        let per = peaks.len().div_ceil(bins);
        peaks
            .chunks(per)
            .map(|chunk| chunk.iter().fold(0.0f32, |peak, frame| peak.max(frame[0]).max(frame[1])))
            .map(|peak| peak.min(1.0))
            .collect()
    }
}

/// A drain that could not finish. The one place a failure is worded, so
/// every call site agrees.
///
/// With a checkpoint behind it the file is a readable take up to that point
/// (see [`CHECKPOINT_SECONDS`]), so it is kept: a performance cut short by a
/// full disk is still the performance. With none, its header still says zero
/// frames and what is left on disk is a WAV nothing can read, so it is
/// removed -- left there, the recordings folder would accumulate unreadable
/// files that only step 06's sweeper would ever remove, listed as ordinary
/// unused takes.
///
/// The writer is gone by the time this runs. Its `Drop` tries to patch the
/// header once more, and on a full disk that fails at the seek that has to
/// flush the buffer first, so the header stays at the last checkpoint.
fn failed(
    path: &Path,
    doing: &str,
    error: impl std::fmt::Display,
    checkpointed: u64,
    sample_rate: u32,
) -> Drained {
    if checkpointed > 0 {
        let seconds = checkpointed as f64 / f64::from(sample_rate.max(1));
        return Drained::Partial {
            frames: checkpointed,
            error: format!(
                "{doing} {}: {error}; the first {seconds:.1} s of the take were kept",
                path.display()
            ),
        };
    }
    let note = match std::fs::remove_file(path) {
        Ok(()) => "; the partial recording was removed",
        Err(_) => "",
    };
    Drained::Failed(format!("{doing} {}: {error}{note}", path.display()))
}

/// Pull `consumer` into `writer` until the take is over, then finalize.
///
/// Generic over the sink so a test can hand it one that fails partway and
/// assert what is left on disk; production always passes the `BufWriter<File>`
/// [`TakeRecorder::arm`] opened.
fn drain<W: Write + Seek>(
    mut consumer: rtrb::Consumer<TakeFrame>,
    status: &TakeStatus,
    mut writer: hound::WavWriter<W>,
    peaks: &Mutex<Vec<[f32; 2]>>,
    path: &Path,
) -> Drained {
    let sample_rate = writer.spec().sample_rate;
    let checkpoint_every = u64::from(sample_rate).max(1) * CHECKPOINT_SECONDS;
    let mut written = 0u64;
    let mut checkpointed = 0u64;
    let mut bucket = [0.0f32; 2];
    let mut in_bucket = 0usize;
    loop {
        let available = consumer.slots();
        if available > 0 {
            let Ok(chunk) = consumer.read_chunk(available) else {
                continue;
            };
            let (first, second) = chunk.as_slices();
            for frame in first.iter().chain(second) {
                if let Err(error) = writer
                    .write_sample(frame[0])
                    .and_then(|()| writer.write_sample(frame[1]))
                {
                    // A failed take owns its file: `failed` keeps it as a
                    // `Partial` take when a checkpoint made it readable, and
                    // removes it otherwise. A fragment nothing names would
                    // fill `recordings/` with files a user has no way to tell
                    // from a take that worked
                    // (`reports/fable-2026-09-21.md`, finding 3).
                    //
                    // Closed before the removal, so the file is not unlinked
                    // out from under an open handle still holding buffered
                    // bytes. `chunk` is abandoned unread on purpose: the take
                    // is already lost and the ring dies with the producer.
                    drop(writer);
                    return failed(path, "writing", error, checkpointed, sample_rate);
                }
                written += 1;
                // Counted in frames rather than per chunk, so a checkpoint is
                // a second of audio however the ring happened to hand it over.
                if written - checkpointed >= checkpoint_every {
                    if let Err(error) = writer.flush() {
                        drop(writer);
                        return failed(path, "writing", error, checkpointed, sample_rate);
                    }
                    checkpointed = written;
                }
                bucket[0] = bucket[0].max(frame[0].abs());
                bucket[1] = bucket[1].max(frame[1].abs());
                in_bucket += 1;
                if in_bucket == PEAK_BUCKET_FRAMES {
                    if let Ok(mut peaks) = peaks.lock() {
                        peaks.push(bucket);
                    }
                    bucket = [0.0; 2];
                    in_bucket = 0;
                }
            }
            chunk.commit_all();
            continue;
        }
        let told_everything = status.phase() == TakePhase::Ended && written >= status.frames();
        if told_everything || consumer.is_abandoned() && consumer.slots() == 0 {
            break;
        }
        std::thread::sleep(DRAIN_POLL);
    }
    if in_bucket > 0 {
        if let Ok(mut peaks) = peaks.lock() {
            peaks.push(bucket);
        }
    }
    // `finalize` consumes the writer, so the handle is already closed here.
    if let Err(error) = writer.finalize() {
        // Same rule as the write above: what the last checkpoint covered is
        // kept, and a header that was never written at all is removed.
        return failed(path, "finishing", error, checkpointed, sample_rate);
    }
    if written == 0 {
        let _ = std::fs::remove_file(path);
        return Drained::Empty;
    }
    Drained::Written { frames: written }
}

/// Seconds of audio a take must have room for before it is armed (MOO-75).
///
/// A minute: long enough that a take is not cut short the moment it starts,
/// short enough that a nearly full disk with room for a short take still
/// records one. At 48 kHz that is about 23 MB of 32-bit stereo.
pub const MIN_FREE_SECONDS: u64 = 60;

/// Bytes one frame of a take takes on disk: two channels of 32-bit float.
const BYTES_PER_FRAME: u64 = 2 * 4;

/// Bytes free to this user on the file system holding `dir`, or `None` when
/// the platform cannot say.
fn free_bytes(dir: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let stat = rustix::fs::statvfs(dir).ok()?;
        Some(stat.f_bavail.saturating_mul(stat.f_frsize))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
}

/// Why a take cannot be armed for lack of room, or `None` when there is
/// enough, or no way to tell. `free` is what the disk has left.
fn short_of_space(free: Option<u64>, sample_rate: u32, dir: &Path) -> Option<String> {
    let need = MIN_FREE_SECONDS * u64::from(sample_rate) * BYTES_PER_FRAME;
    let free = free?;
    (free < need).then(|| {
        format!(
            "the disk holding {} has {} free, and a take needs room for at least a minute ({})",
            dir.display(),
            crate::recordings::size_text(free),
            crate::recordings::size_text(need),
        )
    })
}

/// Every take in `dir` whose WAV header says less than the file holds, put
/// right from the file's length (MOO-75).
///
/// Two things leave one. A crash inside a take's first second, before the
/// first checkpoint, leaves a header that says zero frames, which nothing can
/// open; and a crash later leaves the audio after the last checkpoint
/// uncounted. Takes written before checkpoints (`fe1e9ce`) are all the first
/// kind. At startup nothing is writing to the folder, so the length on disk
/// is all there will ever be.
///
/// Returns the files repaired, and a line for each that could not be.
pub fn repair_headers(dir: &Path) -> (Vec<PathBuf>, Vec<String>) {
    let mut repaired = Vec::new();
    let mut failures = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (repaired, failures);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("wav") {
            continue;
        }
        match repair_header(&path) {
            Ok(true) => repaired.push(path),
            Ok(false) => {}
            Err(error) => failures.push(format!("{}: {error}", path.display())),
        }
    }
    repaired.sort();
    (repaired, failures)
}

/// Patch one file's RIFF and `data` sizes to what it holds. `Ok(false)` for
/// a file that needed nothing, or is not a shape this can safely patch.
fn repair_header(path: &Path) -> std::io::Result<bool> {
    use std::io::{Read, SeekFrom};
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let length = file.metadata()?.len();
    let mut riff = [0u8; 12];
    if file.read_exact(&mut riff).is_err() || &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Ok(false);
    }
    let mut block_align: u64 = 0;
    let mut offset: u64 = 12;
    // Walk the chunks to `data`. hound writes `fmt ` first and `data` last,
    // but a walk costs nothing and does not depend on it.
    while offset + 8 <= length {
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header)?;
        let size = u64::from(u32::from_le_bytes(header[4..8].try_into().unwrap()));
        let body = offset + 8;
        if &header[0..4] == b"fmt " {
            let mut fmt = [0u8; 14];
            file.read_exact(&mut fmt)?;
            block_align = u64::from(u16::from_le_bytes([fmt[12], fmt[13]]));
        } else if &header[0..4] == b"data" {
            if block_align == 0 {
                return Ok(false);
            }
            let held = (length - body) / block_align * block_align;
            // Only ever grows the count: a header that says *more* than the
            // file holds is a different fault, and shrinking it could throw
            // away a chunk that follows.
            let Ok(held32) = u32::try_from(held) else {
                return Ok(false);
            };
            if held <= size {
                return Ok(false);
            }
            let riff_size = u32::try_from(body - 8 + held).unwrap_or(u32::MAX);
            file.seek(SeekFrom::Start(offset + 4))?;
            file.write_all(&held32.to_le_bytes())?;
            file.seek(SeekFrom::Start(4))?;
            file.write_all(&riff_size.to_le_bytes())?;
            file.sync_all()?;
            return Ok(true);
        }
        // Chunks are padded to an even length.
        offset = body + size + (size & 1);
    }
    Ok(false)
}

/// `<date>-<time>-<channel>.wav`, in UTC, with anything a file system might
/// object to in the channel's name replaced.
fn take_file_name(channel: &str, when: SystemTime) -> String {
    let seconds = when
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    let name: String = channel
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{name}.wav",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// `"2026-09-23 01:12 UTC"`, for the clean-up dialog's date column. UTC and
/// labelled so, like the take's own file name: local time needs a time-zone
/// database this crate does not carry.
pub(crate) fn utc_minutes(when: SystemTime) -> String {
    let seconds = when
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest / 60 % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `civil_from_days`), so naming a file needs no date crate.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Stand in for the engine: push `frames` into the take's ring, then end
    /// it or abandon it.
    /// A finished take, and with `crash` its header put back the way a crash
    /// leaves it: zero frames, and a RIFF size that counts nothing after it.
    fn written_take(dir: &Path, name: &str, frames: u32, crash: bool) -> PathBuf {
        let path = dir.join(name);
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 1_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..frames * 2 {
            writer.write_sample(0.25f32).unwrap();
        }
        writer.finalize().unwrap();
        if !crash {
            return path;
        }
        let mut bytes = std::fs::read(&path).unwrap();
        let data = bytes.windows(4).position(|window| window == b"data").unwrap();
        bytes[data + 4..data + 8].copy_from_slice(&0u32.to_le_bytes());
        let riff = u32::try_from(data + 8 - 8).unwrap();
        bytes[4..8].copy_from_slice(&riff.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// **A take a crash left with a zero-length header opens again after
    /// startup's repair** (MOO-75). Nothing could open one before: the
    /// header said no frames. A take whose header is already right is left
    /// alone, and so is a file that is not a WAV at all.
    #[test]
    fn startup_repairs_a_header_a_crash_left_at_zero() {
        let dir = tempfile::tempdir().unwrap();
        let crashed = written_take(dir.path(), "20260923-010000-crashed.wav", 2_500, true);
        assert_eq!(
            hound::WavReader::open(&crashed).map(|reader| reader.duration()).unwrap_or(0),
            0,
            "the fixture must look like a crash"
        );
        let whole = written_take(dir.path(), "20260923-020000-whole.wav", 10, false);
        let before = std::fs::read(&whole).unwrap();
        std::fs::write(dir.path().join("notes.wav"), b"not a wav").unwrap();

        let (repaired, failures) = repair_headers(dir.path());

        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(repaired, std::slice::from_ref(&crashed));
        assert_eq!(std::fs::read(&whole).unwrap(), before, "a whole take is not touched");
        let reader = hound::WavReader::open(&crashed).unwrap();
        assert_eq!(reader.duration(), 2_500);
        assert_eq!(
            reader.into_samples::<f32>().map(Result::unwrap).collect::<Vec<_>>().len(),
            5_000
        );
    }

    /// The repair reports what it touched, once, and a missing folder is no
    /// takes rather than a failure.
    #[test]
    fn the_repair_names_each_file_it_mended() {
        let dir = tempfile::tempdir().unwrap();
        let crashed = written_take(dir.path(), "20260923-010000-crashed.wav", 100, true);
        let (repaired, failures) = repair_headers(dir.path());
        assert_eq!(repaired, [crashed]);
        assert!(failures.is_empty());
        assert!(repair_headers(dir.path()).0.is_empty());
        assert!(repair_headers(&dir.path().join("missing")).0.is_empty());
    }

    /// **Arming refuses a disk without room for a minute of audio, and says
    /// how much there is** (MOO-75), the way it refuses a missing input.
    #[test]
    fn arming_refuses_a_disk_without_room_for_a_minute() {
        let dir = Path::new("/takes");
        let need = MIN_FREE_SECONDS * 48_000 * BYTES_PER_FRAME;
        let refused = short_of_space(Some(need - 1), 48_000, dir).expect("one byte short");
        assert!(refused.contains("/takes"), "{refused}");
        assert!(refused.contains("free"), "{refused}");
        assert_eq!(short_of_space(Some(need), 48_000, dir), None);
        assert_eq!(short_of_space(None, 48_000, dir), None, "unknown is not refused");
    }

    fn run(recorder: &mut TakeRecorder, frames: &[TakeFrame], end: bool) -> FinishedTake {
        let command = recorder.arm(ChannelId(3), 0, "Kick 1", None, 48_000, 0).expect("armed");
        let StructuralCommand::StartTake { take, .. } = command else {
            panic!("expected a take");
        };
        let mut take = *take;
        take.push_for_test(frames);
        if end {
            take.stop();
            // Held until the drain is done, so the ring is *not* abandoned
            // and the drain has to finish on the status alone.
            let finished = wait(recorder);
            drop(take);
            finished
        } else {
            drop(take);
            wait(recorder)
        }
    }

    fn wait(recorder: &mut TakeRecorder) -> FinishedTake {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (mut finished, failures) = recorder.collect();
            assert!(failures.is_empty(), "{failures:?}");
            if let Some(take) = finished.pop() {
                return take;
            }
            assert!(Instant::now() < deadline, "the drain never finished");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Which of the drain's two failure routes a [`FailingSink`] takes.
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum FailWhen {
        /// Writes stop working partway through the samples.
        Writing,
        /// Writes always work; the seek `finalize` needs in order to patch
        /// the header does not. The sample path never seeks, so this fails
        /// `finalize` and nothing before it.
        Finalizing,
    }

    /// A `Write + Seek` sink that fails on demand, so the drain's failure
    /// handling can be reached without a full disk.
    struct FailingSink {
        /// Shared, so a test can read what reached the "disk" after the
        /// writer that owned the sink is gone.
        inner: std::rc::Rc<std::cell::RefCell<std::io::Cursor<Vec<u8>>>>,
        when: FailWhen,
        allowance: usize,
    }

    impl FailingSink {
        fn new(when: FailWhen) -> Self {
            // Enough for the header and a few frames, so the failure lands
            // mid-take rather than before anything is written.
            Self::with_allowance(when, 128)
        }

        fn with_allowance(when: FailWhen, allowance: usize) -> Self {
            Self {
                inner: Default::default(),
                when,
                allowance,
            }
        }

        /// Everything written so far, as a file would hold it.
        fn contents(&self) -> std::rc::Rc<std::cell::RefCell<std::io::Cursor<Vec<u8>>>> {
            self.inner.clone()
        }
    }

    impl Write for FailingSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if self.when == FailWhen::Writing {
                if self.allowance == 0 {
                    return Err(std::io::Error::other("the disk went away"));
                }
                let n = buf.len().min(self.allowance);
                self.allowance -= n;
                return self.inner.borrow_mut().write(&buf[..n]);
            }
            self.inner.borrow_mut().write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.borrow_mut().flush()
        }
    }

    impl Seek for FailingSink {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            if self.when == FailWhen::Finalizing {
                return Err(std::io::Error::other("the disk went away"));
            }
            self.inner.borrow_mut().seek(pos)
        }
    }

    fn ramp(frames: usize) -> Vec<TakeFrame> {
        (0..frames)
            .map(|i| [i as f32 / frames as f32, -(i as f32) / frames as f32])
            .collect()
    }

    /// **The file is the ring, bit for bit**, and its peak summary covers it.
    #[test]
    fn a_take_becomes_a_wav_of_exactly_its_frames() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = TakeRecorder::new(dir.path().join("recordings"));
        let frames = ramp(5000);
        let finished = run(&mut recorder, &frames, true);

        assert_eq!(finished.channel, ChannelId(3));
        assert_eq!(finished.frames, 5000);
        assert!(!finished.is_damaged());
        let mut reader = hound::WavReader::open(&finished.path).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
        let flat: Vec<f32> = frames.iter().flat_map(|frame| *frame).collect();
        assert_eq!(samples, flat);
    }

    /// An abandoned ring -- the strip rebuilt or the channel deleted -- still
    /// ends in a finished file, not an open one.
    #[test]
    fn an_abandoned_take_is_finished_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = TakeRecorder::new(dir.path());
        let finished = run(&mut recorder, &ramp(3000), false);
        assert_eq!(finished.frames, 3000);
        assert!(hound::WavReader::open(&finished.path).is_ok());
    }

    /// A take cancelled before its bar leaves no empty file behind.
    #[test]
    fn a_take_that_recorded_nothing_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = TakeRecorder::new(dir.path());
        let command = recorder.arm(ChannelId(0), 0, "Empty", None, 48_000, 0).unwrap();
        drop(command);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !recorder.running.is_empty() {
            let (finished, failures) = recorder.collect();
            assert!(finished.is_empty() && failures.is_empty());
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// **Quit finishes a take rather than abandoning it.** `hound` patches
    /// the real frame count into the WAV header in `finalize`, at the end of
    /// the drain and nowhere else, so a take whose drain is still running
    /// when the process ends leaves a file whose header says it holds
    /// nothing.
    ///
    /// Shaped against the unfixed tree, where `finish_all` did not exist:
    /// the same sequence with `drop(recorder)` in its place left a file on
    /// disk that `hound` opened with `duration() == 0` and no samples to
    /// read, and nothing said so.
    #[test]
    fn quitting_mid_take_still_writes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut recorder = TakeRecorder::new(dir.path());
        let command = recorder.arm(ChannelId(3), 0, "Kick 1", None, 48_000, 0).expect("armed");
        let StructuralCommand::StartTake { take, .. } = command else {
            panic!("expected a take");
        };
        let mut take = *take;
        let frames = ramp(5000);
        take.push_for_test(&frames);
        assert!(recorder.has_live(), "a take is recording when quit arrives");

        // `take` is deliberately still held, so the ring is *not* abandoned:
        // the drain has to leave on the status alone, which is the thing
        // quit has to be able to make happen.
        let (finished, failures) = recorder.finish_all();

        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].frames, 5000);
        assert!(!recorder.has_live(), "nothing is left running");
        let mut reader = hound::WavReader::open(&finished[0].path).unwrap();
        assert_eq!(reader.duration(), 5000, "the header was never patched");
        let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
        let flat: Vec<f32> = frames.iter().flat_map(|frame| *frame).collect();
        assert_eq!(samples, flat);
        drop(take);
    }

    /// When a drain fails before its first checkpoint there is no way to write
    /// the header, so what is on disk is a WAV nothing can read. Both routes
    /// out -- a write that fails partway, and a `finalize` that cannot patch
    /// the header -- remove it. Two hundred frames is far short of a second,
    /// so neither case here reaches a checkpoint.
    ///
    /// Shaped against the unfixed tree, where only `Drained::Empty` removed
    /// anything: both cases left one unreadable file in the folder and said
    /// nothing about it, and the only thing that would ever have swept it up
    /// is step 06, which would have listed it as an ordinary unused take.
    #[test]
    fn a_drain_that_fails_removes_its_partial_file() {
        for when in [FailWhen::Writing, FailWhen::Finalizing] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("partial.wav");
            // Stands in for the file `arm` opened: the sink below is where
            // the bytes actually go, and this is what has to be cleaned up.
            std::fs::write(&path, b"a partial recording").unwrap();

            let spec = hound::WavSpec {
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            };
            let writer = hound::WavWriter::new(FailingSink::new(when), spec).unwrap();
            let (mut producer, consumer) = rtrb::RingBuffer::new(1024);
            for frame in ramp(200) {
                producer.push(frame).unwrap();
            }
            drop(producer);
            let status = TakeStatus::new();
            status.end();
            let peaks = Mutex::new(Vec::new());

            let outcome = drain(consumer, &status, writer, &peaks, &path);

            let Drained::Failed(message) = outcome else {
                panic!("{when:?} should have failed, got {outcome:?}");
            };
            assert!(
                message.contains("the partial recording was removed"),
                "the message does not say the file went: {message}"
            );
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                0,
                "{when:?} left its partial file behind"
            );
        }
    }

    /// A take still recording is a readable file on disk, up to its last
    /// checkpoint -- which is what survives the process dying, since nothing
    /// after that runs `finalize`.
    ///
    /// Shaped against the unfixed tree, where only `finalize` patched the
    /// header: the live file said zero frames and hound refused it or read it
    /// as empty, for the whole length of the take.
    #[test]
    fn a_take_in_progress_is_readable_up_to_its_last_checkpoint() {
        const RATE: u32 = 1_000;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("live.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let writer = hound::WavWriter::create(&path, spec).unwrap();
        let (mut producer, consumer) = rtrb::RingBuffer::new(4_096);
        let frames = ramp(2_500);
        for frame in &frames {
            producer.push(*frame).unwrap();
        }
        let status = Arc::new(TakeStatus::new());
        let peaks = Arc::new(Mutex::new(Vec::new()));
        let handle = {
            let (status, peaks, path) = (status.clone(), peaks.clone(), path.clone());
            std::thread::spawn(move || drain(consumer, &status, writer, &peaks, &path))
        };

        // Two checkpoints' worth has gone by and the take is still live.
        let deadline = Instant::now() + Duration::from_secs(10);
        let readable = loop {
            let frames_on_disk = hound::WavReader::open(&path)
                .map(|reader| reader.duration())
                .unwrap_or(0);
            if frames_on_disk >= 2_000 || Instant::now() >= deadline {
                break frames_on_disk;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(readable, 2_000, "the live file is not readable to its last checkpoint");
        let mut reader = hound::WavReader::open(&path).unwrap();
        let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
        let flat: Vec<f32> = frames[..2_000].iter().flat_map(|frame| *frame).collect();
        assert_eq!(samples, flat);

        status.end();
        drop(producer);
        let outcome = handle.join().unwrap();
        assert!(
            matches!(outcome, Drained::Written { frames: 2_500 }),
            "{outcome:?}"
        );
        assert_eq!(hound::WavReader::open(&path).unwrap().duration(), 2_500);
    }

    /// A write that fails after a checkpoint keeps the take: what the header
    /// covers is a performance, and a full disk is no reason to throw it away.
    /// It is handed back as `Partial`, so it lands on the channel *and* the
    /// failure is reported.
    ///
    /// Shaped against the unfixed tree, where every failure removed the file.
    #[test]
    fn a_drain_that_fails_after_a_checkpoint_keeps_the_take() {
        const RATE: u32 = 1_000;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("partial.wav");
        std::fs::write(&path, b"a partial recording").unwrap();
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        // Room for the header and about a second and a half of frames, so
        // the failure lands after the first checkpoint and before the second.
        let sink = FailingSink::with_allowance(FailWhen::Writing, 256 + 1_500 * 8);
        let disk = sink.contents();
        let writer = hound::WavWriter::new(sink, spec).unwrap();
        let (mut producer, consumer) = rtrb::RingBuffer::new(4_096);
        let frames = ramp(3_000);
        for frame in &frames {
            producer.push(*frame).unwrap();
        }
        drop(producer);
        let status = TakeStatus::new();
        status.end();
        let peaks = Mutex::new(Vec::new());

        let outcome = drain(consumer, &status, writer, &peaks, &path);

        let Drained::Partial { frames: kept, error } = outcome else {
            panic!("a failure after a checkpoint should keep the take, got {outcome:?}");
        };
        assert_eq!(kept, 1_000);
        assert!(error.contains("were kept"), "the message does not say so: {error}");
        assert!(path.exists(), "the kept take was removed");
        let bytes = disk.borrow().get_ref().clone();
        let mut reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(reader.duration(), 1_000, "the header is not at the checkpoint");
        let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
        let flat: Vec<f32> = frames[..1_000].iter().flat_map(|frame| *frame).collect();
        assert_eq!(samples, flat);
    }

    /// A take lands only on a channel that still exists **and** still holds a
    /// sampler. A channel is a dumb slot, so its device can be swapped while
    /// a take records; `reset_channel_source` clears the sample state a take
    /// would write, and `project_snapshot` saves a sample only under a
    /// `Sampler`.
    ///
    /// Shaped against the unfixed tree, where `apply_take` spelled this as a
    /// bare `position(|c| c.id == id)`: after the kind change that lookup
    /// still answered `Some(0)`, and the take was written onto it.
    #[test]
    fn a_take_lands_only_on_a_sampler_that_is_still_there() {
        use mooloop_core::DeviceKind;
        let mut session = crate::session::Session::default();
        let id = session.channels[0].id;
        assert_eq!(session.take_target(id), Ok(0));

        session.reset_channel_source(0, DeviceKind::DrumSynth);
        assert_eq!(session.take_target(id), Err(TakeMiss::NotASampler));
        assert_eq!(
            session.take_target(ChannelId(4242)),
            Err(TakeMiss::ChannelGone)
        );
        assert_ne!(
            TakeMiss::NotASampler.message(),
            TakeMiss::ChannelGone.message(),
            "the two misses are different things to be told"
        );
    }

    /// REC on a channel whose AUDIO source has been deleted refuses, rather
    /// than arming a take that records a ring of zeros and makes the silence
    /// the sampler's sample.
    ///
    /// The rule is the picker's, which is the one the face already draws
    /// (`audio_input_missing`). Shaped against the unfixed tree, where
    /// `record_press` asked only `is_off()` and both cases below returned
    /// `Arm { .. }`.
    #[test]
    fn rec_refuses_a_source_that_is_gone() {
        use mooloop_core::{AudioInputSource, DeviceKind};
        let mut session = crate::session::Session::default();
        session.add_channel(DeviceKind::Sampler).expect("a second channel");
        let second = session.channels[1].id;
        assert!(session.set_channel_audio_input(0, AudioInputSource::Channel(second)));
        assert_eq!(
            session.record_press(0, false, None),
            Some(RecordPress::Arm {
                channel: session.channels[0].id,
                seat: 0,
                name: session.channels[0].name.clone(),
                clip_ticks: None,
                from_input: false,
            }),
            "the source is still there"
        );

        // Straight off the list: the picker's rows are built from it, and
        // there is no session-level delete to go through.
        session.channels.remove(1);
        assert_eq!(
            session.record_press(0, false, None),
            Some(RecordPress::SourceGone),
            "the channel it records went away"
        );

        // The hardware input under a driver that offers none: `resolve` calls
        // this present and the picker calls it missing, and the picker is the
        // one the face agrees with. **A different answer from the one above**,
        // per open question 10: this user goes and looks at their audio
        // device, not at the AUDIO row.
        assert!(session.set_channel_audio_input(0, AudioInputSource::Input));
        assert_eq!(
            session.record_press(0, false, None),
            Some(RecordPress::NoInputDevice)
        );
        assert_eq!(
            session.record_press(0, false, Some("Scarlett 2i2")),
            Some(RecordPress::Arm {
                channel: session.channels[0].id,
                seat: 0,
                name: session.channels[0].name.clone(),
                clip_ticks: None,
                from_input: true,
            }),
            "with a driver input, the same channel arms"
        );
    }

    /// REC stops a live take, refuses a channel with no AUDIO input, and
    /// otherwise arms one carrying the channel's identity and clip length.
    #[test]
    fn a_record_press_stops_arms_or_refuses() {
        use mooloop_core::AudioInputSource;
        let mut session = crate::session::Session::default();
        assert_eq!(session.record_press(0, false, None), Some(RecordPress::NoInput));
        assert_eq!(session.record_press(9, false, None), None);

        assert!(session.set_channel_audio_input(0, AudioInputSource::Master));
        assert!(session.set_record_clip(0, true));
        assert!(session.set_record_bars(0, 2));
        assert!(!session.set_record_bars(0, 2), "not an edit twice");
        let id = session.channels[0].id;
        assert_eq!(
            session.record_press(0, false, None),
            Some(RecordPress::Arm {
                channel: id,
                seat: 0,
                name: session.channels[0].name.clone(),
                clip_ticks: Some(2 * mooloop_core::TICKS_PER_BAR),
                from_input: false,
            })
        );
        assert_eq!(session.record_press(0, true, None), Some(RecordPress::Stop { seat: 0 }));
        assert!(session.set_record_bars(0, 1000));
        assert_eq!(session.channels[0].record.bars, mooloop_core::MAX_RECORD_BARS);
    }

    /// The Record page's settings travel with the sampler through a document.
    #[test]
    fn the_record_settings_survive_a_round_trip() {
        let mut session = crate::session::Session::default();
        session.set_record_clip(0, true);
        session.set_record_bars(0, 4);
        let snapshot = session.project_snapshot(120, 0);
        let mut reopened = crate::session::Session::default();
        reopened.replace_project(&snapshot, &[]);
        assert_eq!(reopened.channels[0].record, session.channels[0].record);
    }

    /// Monitoring is kept by identity: a channel move keeps it, the seats an
    /// install gets follow the channel, and it is never on by default.
    #[test]
    fn monitoring_follows_its_channel() {
        let mut session = crate::session::Session::default();
        session.channels.push(crate::channel::ChannelState::new(1));
        session.channels[1].id = mooloop_core::ChannelId(7);
        assert!(!session.is_monitoring(0) && !session.is_monitoring(1));
        assert_eq!(
            session.set_input_monitor(1, true),
            Some(EngineCommand::SetInputMonitor { channel: 1, on: true })
        );
        assert_eq!(session.set_input_monitor(1, true), None, "not a change twice");

        let mut project = session.project_snapshot(120, 0);
        assert_eq!(session.monitor_seats(&project), [false, true]);
        project.move_channel(1, 0).expect("a real move");
        assert_eq!(session.monitor_seats(&project), [true, false]);
    }

    /// A channel that is genuinely gone must lose its monitor toggle rather
    /// than leave it for some later channel's id to inherit --
    /// `docs/LOOSE_ENDS.md`'s `Session::input_monitor` entry, "is never
    /// pruned when a channel goes."
    #[test]
    fn input_monitor_forgets_a_channel_that_is_removed() {
        let mut session = crate::session::Session::default();
        session.channels.push(crate::channel::ChannelState::new(1));
        session.channels[1].id = mooloop_core::ChannelId(7);
        session.set_input_monitor(0, true);
        session.set_input_monitor(1, true);

        let mut project = session.project_snapshot(120, 0);
        project.remove_channel(1);
        session.replace_project(&project, &[]);

        assert!(session.is_monitoring(0), "channel 0 was not touched");
        assert!(
            !session.input_monitor.contains(&mooloop_core::ChannelId(7)),
            "the removed channel's id must not linger in the set"
        );
    }

    #[test]
    fn a_take_is_named_by_date_time_and_channel() {
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_789_000_000);
        assert_eq!(take_file_name("Kick / 1", when), "20260910-002640-Kick___1.wav");
    }
}
