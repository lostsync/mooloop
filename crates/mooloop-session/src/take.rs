//! Takes on the control side: arming one, draining its ring to a file, and
//! handing back what finished (`docs/plans/audio-recording/03-capture.md`).
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

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime};

use mooloop_core::ChannelId;
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
    ) -> Result<StructuralCommand, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| format!("could not create {}: {error}", self.dir.display()))?;
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
            take: Take::new(producer, status, clip_ticks),
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
            let mut take = self.running.remove(index);
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
                Some(Drained::Failed(error)) => failures.push(error),
                Some(Drained::Empty) | None => {}
            }
        }
        (finished, failures)
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
    },
    /// The channel has no AUDIO input, so there is nothing to record.
    NoInput,
}

impl crate::session::Session {
    /// Decide what REC on the channel at `seat` does, given whether a take is
    /// already live there. `None` for a seat with no channel.
    pub fn record_press(&self, seat: usize, take_live: bool) -> Option<RecordPress> {
        let channel = self.channels.get(seat)?;
        let seat_u8 = u8::try_from(seat).ok()?;
        Some(if take_live {
            RecordPress::Stop { seat: seat_u8 }
        } else if channel.audio_input.is_off() {
            RecordPress::NoInput
        } else {
            RecordPress::Arm {
                channel: channel.id,
                seat: seat_u8,
                name: channel.name.clone(),
                clip_ticks: channel.record.clip_ticks(),
            }
        })
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

/// Pull `consumer` into `writer` until the take is over, then finalize.
fn drain(
    mut consumer: rtrb::Consumer<TakeFrame>,
    status: &TakeStatus,
    mut writer: hound::WavWriter<BufWriter<File>>,
    peaks: &Mutex<Vec<[f32; 2]>>,
    path: &Path,
) -> Drained {
    let mut written = 0u64;
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
                    return Drained::Failed(format!("writing {}: {error}", path.display()));
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
            written += available as u64;
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
    if let Err(error) = writer.finalize() {
        return Drained::Failed(format!("finishing {}: {error}", path.display()));
    }
    if written == 0 {
        let _ = std::fs::remove_file(path);
        return Drained::Empty;
    }
    Drained::Written { frames: written }
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
    fn run(recorder: &mut TakeRecorder, frames: &[TakeFrame], end: bool) -> FinishedTake {
        let command = recorder.arm(ChannelId(3), 0, "Kick 1", None, 48_000).expect("armed");
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
        let command = recorder.arm(ChannelId(0), 0, "Empty", None, 48_000).unwrap();
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

    /// REC stops a live take, refuses a channel with no AUDIO input, and
    /// otherwise arms one carrying the channel's identity and clip length.
    #[test]
    fn a_record_press_stops_arms_or_refuses() {
        use mooloop_core::AudioInputSource;
        let mut session = crate::session::Session::default();
        assert_eq!(session.record_press(0, false), Some(RecordPress::NoInput));
        assert_eq!(session.record_press(9, false), None);

        assert!(session.set_channel_audio_input(0, AudioInputSource::Master));
        assert!(session.set_record_clip(0, true));
        assert!(session.set_record_bars(0, 2));
        assert!(!session.set_record_bars(0, 2), "not an edit twice");
        let id = session.channels[0].id;
        assert_eq!(
            session.record_press(0, false),
            Some(RecordPress::Arm {
                channel: id,
                seat: 0,
                name: session.channels[0].name.clone(),
                clip_ticks: Some(2 * mooloop_core::TICKS_PER_BAR),
            })
        );
        assert_eq!(session.record_press(0, true), Some(RecordPress::Stop { seat: 0 }));
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

    #[test]
    fn a_take_is_named_by_date_time_and_channel() {
        let when = SystemTime::UNIX_EPOCH + Duration::from_secs(1_789_000_000);
        assert_eq!(take_file_name("Kick / 1", when), "20260910-002640-Kick___1.wav");
    }
}
