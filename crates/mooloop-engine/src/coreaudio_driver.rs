//! Core Audio adapter, through cpal: one output stream on a chosen device,
//! whose data callback hands the shared [`Executor`] its buffers.
//!
//! JACK routes by connecting ports in a graph other programs share. Core Audio
//! has no graph: an application opens a device and writes to its channels. So
//! an output target here names a device and two of its channels, spelled
//! `<device>#<channel>` with channels counted from one, and `default` for the
//! device is whatever the system output is at the moment. A stream opened on
//! `default` follows the system when it changes; one opened on a named device
//! stops when that device goes away, and [`CoreAudioDriver::service`] moves it
//! to the system default until the device comes back.
//!
//! Reopening a stream -- another device, another buffer size -- replaces the
//! thread the callback runs on but not the executor. The callback owns the
//! executor outright, with no lock (MOO-22): when a stream is dropped its
//! callback parks what it owned, and the next stream's callback takes it on
//! its first block (`crate::handoff`). The two streams never run at once --
//! the old one is dropped before the new one plays -- so the hand-off costs
//! nothing a reopen did not already cost. What the control thread used to
//! change under a lock (the input ring, the start of a new run) it now sends
//! down a ring the callback reads at the top of each block.
//!
//! MIDI input comes from Core MIDI, through `midir`. There is no patchbay to
//! connect a keyboard to mooloop in, so mooloop listens to every source there
//! is, and [`CoreAudioDriver::service`] picks up a keyboard plugged in later.
//! Messages cross to the audio callback over a bounded ring and act at the
//! top of the next block: a callback's worth of timing, a few milliseconds.
//!
//! Audio input is a second stream, because cpal has no duplex stream: it runs
//! on its own thread and its own device clock, and hands stereo frames to the
//! output callback over a ring the same way MIDI does. Two clocks that are not
//! the same clock drift, and this driver **counts the drift rather than
//! resampling it** -- a ring that starves reads silence, a ring that fills
//! drops frames, and both are counted and reported. On the one machine that
//! matters most, where the input and output are the same device, there is no
//! drift to correct.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use midir::{Ignore, MidiInput, MidiInputConnection};
use mooloop_core::{MidiPortId, MidiPortInfo};
use mooloop_dsp::MAX_BLOCK_SIZE;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::driver::{AudioConfig, OutputTarget};
use crate::executor::Executor;
use crate::handoff::{Held, Parked};
use crate::Error;

/// The device half of a target that means "the system output".
const SYSTEM_DEFAULT: &str = "default";
const SYSTEM_DEFAULT_LABEL: &str = "System default";

/// How often a stream standing in for a missing device looks for it again.
/// Device enumeration is a round of HAL property reads, which is nothing once
/// a second and not nothing on every GUI frame.
const PROBE_INTERVAL: Duration = Duration::from_secs(1);

/// The Core MIDI client name, which MIDI utilities show as the listener.
const MIDI_CLIENT_NAME: &str = "mooloop";

/// MIDI messages that can wait for the audio callback. A callback runs every
/// few milliseconds, so this is seconds of playing even if one is missed.
const MIDI_QUEUE_CAPACITY: usize = 1024;

/// The most MIDI messages one callback hands the executor. The rest wait in
/// the ring for the next callback rather than being dropped.
const MAX_MIDI_PER_CALLBACK: usize = 256;

/// How many of the stream's buffers the input ring holds. Its job is to
/// absorb the jitter and drift between two device clocks, and eight buffers is
/// room for far more of both than a machine whose input and output are one
/// device will ever show.
const INPUT_RING_BUFFERS: usize = 8;

/// How many buffers are gathered before the output side reads its first.
///
/// `audio-recording/01-input-in-the-engine.md` says to fill the ring halfway,
/// which would be four. This is two, because a prefill is latency on the way
/// in that the performer hears, and four buffers of it is 85 ms at 1024
/// frames. Eight of capacity against two of prefill still leaves six buffers
/// of headroom, which is what the plan was buying.
const INPUT_PREFILL_BUFFERS: usize = 2;

/// The buffer size the ring is sized from when the stream was opened at the
/// device's own default, which cpal cannot report until it is running. The
/// ring only has to be the right order of magnitude.
const ASSUMED_INPUT_BUFFER: u32 = 1024;

/// One channel message, copied out of Core MIDI's buffer so it can cross
/// threads without allocating. System exclusive is filtered before it gets
/// here, so three bytes is every message mooloop reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MidiBytes {
    len: u8,
    /// Which source it came in on. Core MIDI gives every source its own
    /// connection, so unlike JACK this driver really can tell two keyboards
    /// apart -- which is what makes the input picker worth having here.
    port: MidiPortId,
    bytes: [u8; 3],
}

impl Default for MidiBytes {
    fn default() -> Self {
        Self {
            len: 0,
            port: MidiPortId::FIRST,
            bytes: [0; 3],
        }
    }
}

impl MidiBytes {
    fn new(port: MidiPortId, message: &[u8]) -> Option<Self> {
        if message.is_empty() || message.len() > 3 {
            return None;
        }
        // Clock, MTC quarter-frame and active sensing: a stream rather than a
        // gesture, and nothing reads them. Dropped here so they never take a
        // place in the queue. Start, Continue, Stop and Song Position are
        // *not* dropped -- they are transport gestures, and the transport
        // follows them.
        if matches!(message[0], 0xF1 | 0xF8 | 0xF9 | 0xFE) {
            return None;
        }
        let mut bytes = [0; 3];
        bytes[..message.len()].copy_from_slice(message);
        Some(Self {
            len: message.len() as u8,
            port,
            bytes,
        })
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

/// A device and the two of its channels the master bus plays through, both
/// counted from zero.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Route {
    device: String,
    left: u16,
    right: u16,
}

impl Route {
    fn system_default() -> Self {
        Self {
            device: SYSTEM_DEFAULT.to_owned(),
            left: 0,
            right: 1,
        }
    }

    fn parse(target: &(String, String)) -> Result<Self, String> {
        let (device_l, left) = parse_channel(&target.0)?;
        let (device_r, right) = parse_channel(&target.1)?;
        if device_l != device_r {
            return Err(format!(
                "left and right have to be channels of one device, not {device_l:?} and {device_r:?}"
            ));
        }
        Ok(Self {
            device: device_l.to_owned(),
            left,
            right,
        })
    }

    fn target(&self) -> (String, String) {
        (
            channel_address(&self.device, self.left),
            channel_address(&self.device, self.right),
        )
    }
}

fn channel_address(device: &str, channel: u16) -> String {
    format!("{device}#{}", u32::from(channel) + 1)
}

/// Split `<device>#<channel>` at its last `#`, so a device identifier that
/// happens to contain one survives.
fn parse_channel(address: &str) -> Result<(&str, u16), String> {
    address
        .rsplit_once('#')
        .and_then(|(device, channel)| {
            let channel = channel.parse::<u16>().ok()?.checked_sub(1)?;
            (!device.is_empty()).then_some((device, channel))
        })
        .ok_or_else(|| format!("{address:?} does not name a device channel"))
}

/// The output callback's end of the input ring.
///
/// Stereo pairs rather than two rings of samples: a pair cannot come apart,
/// and two rings can -- one overrun on one of them would swap the channels for
/// the rest of the run.
struct InputTap {
    rx: Consumer<[f32; 2]>,
    /// Frames to gather before the first read.
    prefill: usize,
    /// True until `prefill` frames have arrived, and true again once the ring
    /// has starved. Without the second half, one starved block becomes every
    /// block: the ring is read empty and is still empty when the next one
    /// asks.
    priming: bool,
}

impl InputTap {
    /// Copy this block's input into `l` and `r`, returning the frames the ring
    /// could not supply -- which are silence, not a gap: the executor renders
    /// a whole block whatever the input did.
    fn fill(&mut self, l: &mut [f32], r: &mut [f32]) -> u64 {
        let frames = l.len().min(r.len());
        if self.priming {
            if self.rx.slots() < self.prefill {
                l[..frames].fill(0.0);
                r[..frames].fill(0.0);
                // Not counted as an underrun. Priming is the ring filling up
                // as designed, and counting it would report drift on every
                // start.
                return 0;
            }
            self.priming = false;
        }
        let mut taken = 0;
        while taken < frames {
            let Ok(frame) = self.rx.pop() else {
                break;
            };
            l[taken] = frame[0];
            r[taken] = frame[1];
            taken += 1;
        }
        if taken == frames {
            return 0;
        }
        l[taken..frames].fill(0.0);
        r[taken..frames].fill(0.0);
        self.priming = true;
        (frames - taken) as u64
    }
}

/// What the audio callback owns. Never shared: it moves from one stream's
/// callback to the next through [`Shared::parked`].
struct Realtime {
    executor: Executor,
    midi_rx: Consumer<MidiBytes>,
    /// `None` until an input stream is open, and while one is being replaced.
    input: Option<InputTap>,
    /// What the control thread asks of the callback, read at the top of
    /// every block.
    control_rx: Consumer<ToCallback>,
    /// Input rings the callback has let go of, back to the control thread to
    /// free: dropping one here would free its buffer on the audio thread.
    retired_tx: Producer<InputTap>,
}

/// The control thread's side of what used to be done under the lock.
enum ToCallback {
    /// A stream was replaced: the next block is the first of a new run, and
    /// the gap before it is not a late wake-up.
    BeginRun,
    /// Read this input ring from now on, or none.
    Input(Option<InputTap>),
}

impl Realtime {
    /// Apply what the control thread has sent since the last block.
    fn apply_control(&mut self) {
        while let Ok(message) = self.control_rx.pop() {
            match message {
                ToCallback::BeginRun => self.executor.begin_run(),
                ToCallback::Input(tap) => {
                    if let Some(old) = std::mem::replace(&mut self.input, tap) {
                        if let Err(rtrb::PushError::Full(old)) = self.retired_tx.push(old) {
                            // Only if the control thread has stopped
                            // collecting: leaked rather than freed here.
                            std::mem::forget(old);
                        }
                    }
                }
            }
        }
    }
}

/// Room for what the control thread sends between two blocks: an input
/// replaced and a run begun, several times over.
const CONTROL_QUEUE_CAPACITY: usize = 16;

/// The control thread's ends of the callback's rings.
struct Control {
    tx: Producer<ToCallback>,
    retired_rx: Consumer<InputTap>,
}

impl Control {
    fn send(&mut self, message: ToCallback) {
        // Free what the callback has handed back first, so the rings never
        // fill with nobody collecting.
        while self.retired_rx.pop().is_ok() {}
        if self.tx.push(message).is_err() {
            mooloop_core::log_error!(
                "audio",
                "the audio callback has not run for {CONTROL_QUEUE_CAPACITY} changes; \
                 one was dropped"
            );
        }
    }
}

/// What the realtime callbacks share with the control thread.
struct Shared {
    /// The callback's state between two streams. Taken by a stream's
    /// callback on its first block, and parked again when the stream goes.
    parked: Arc<Parked<Realtime>>,
    /// Only the control thread locks this.
    control: Mutex<Control>,
    xrun_count: Arc<AtomicU64>,
    /// Set from cpal's error callback when the stream's device has gone. The
    /// callback may run on the realtime thread, so it sets a flag and nothing
    /// else; the control thread does the reopening.
    lost: AtomicBool,
    /// Set from the input stream's error callback, for the same reason and in
    /// the same way as [`Shared::lost`].
    input_lost: AtomicBool,
    /// Frames the output callback asked the input ring for and did not get,
    /// and frames the input callback could not fit into it, since the engine
    /// started. Nothing corrects either: this driver does not resample, so
    /// these two numbers are the whole account of how far the input and output
    /// clocks have drifted apart.
    input_underruns: AtomicU64,
    input_overruns: AtomicU64,
}

struct State {
    stream: Option<cpal::Stream>,
    /// Where the running stream plays.
    route: Route,
    /// Where it was asked to play. Differs from `route` only while the asked-
    /// for device is missing and the system default is standing in.
    wanted: Route,
    buffer_size: Option<u32>,
    last_probe: Option<Instant>,
    /// The input stream, the name of the device it reads, and the buffer size
    /// its ring was sized from. All three are absent when there is no input:
    /// this machine has none, macOS has refused mooloop the microphone, or the
    /// device would not run at the engine's sample rate.
    input_stream: Option<cpal::Stream>,
    input_name: Option<String>,
    input_buffer: u32,
    last_input_probe: Option<Instant>,
    /// The drift already reported, so a drifting pair of clocks is described
    /// when the number moves rather than once a second forever.
    reported_drift: u64,
}

/// One Core MIDI source mooloop is listening to.
struct Listening {
    /// Core MIDI's own id for the source, which survives a rename.
    id: String,
    /// What the source calls itself, which is what a project stores.
    name: String,
    /// This run's id for it.
    port: MidiPortId,
    /// Held because dropping it stops the listening.
    _connection: MidiInputConnection<()>,
}

/// Every Core MIDI source mooloop is listening to.
struct MidiInputs {
    /// Lists the sources. Connecting consumes a `MidiInput`, so each
    /// connection is made from a fresh one and this one is kept for looking.
    /// `None` when Core MIDI refused a client, and then there is no input.
    scanner: Option<MidiInput>,
    /// Shared by every connection's callback. Those run on Core MIDI's own
    /// thread, not the audio thread, so taking a lock there is fine.
    queue: Arc<Mutex<Producer<MidiBytes>>>,
    /// By source id, which survives a rename, with the port id this run gave
    /// it.
    connections: Vec<Listening>,
    /// The next port id to hand out. Monotonic rather than positional,
    /// because `connections` is pruned when a device is unplugged and a
    /// position would then be reused -- a project naming a port by index
    /// would end up pointing at somebody else's keyboard. Ids are matched to
    /// names once per run by `mooloop_core::midi::resolve_port_name`, so a
    /// gap in them costs nothing.
    next_port: u16,
    /// Sources that refused a connection, by id, so a broken one is reported
    /// once rather than once a second.
    refused: Vec<String>,
    last_scan: Option<Instant>,
}

impl MidiInputs {
    fn new(queue: Producer<MidiBytes>) -> Self {
        let scanner = match MidiInput::new(MIDI_CLIENT_NAME) {
            Ok(input) => Some(input),
            Err(e) => {
                mooloop_core::log_warn!("midi", "Core MIDI is not available ({e}); no MIDI input");
                None
            }
        };
        Self {
            scanner,
            queue: Arc::new(Mutex::new(queue)),
            connections: Vec::new(),
            next_port: 0,
            refused: Vec::new(),
            last_scan: None,
        }
    }

    /// Listen to every source not yet listened to, and let go of the ones
    /// that went away.
    fn scan(&mut self) {
        self.last_scan = Some(Instant::now());
        let Some(scanner) = self.scanner.as_ref() else {
            return;
        };
        let present: Vec<_> = scanner
            .ports()
            .into_iter()
            .filter_map(|port| Some((port.id(), scanner.port_name(&port).ok()?, port)))
            .collect();
        let is_present = |id: &String| present.iter().any(|(present, _, _)| present == id);
        self.connections.retain(|listening| {
            let keep = is_present(&listening.id);
            if !keep {
                let name = &listening.name;
                mooloop_core::log_info!("midi", "the MIDI input {name:?} went away");
            }
            keep
        });
        self.refused.retain(is_present);

        for (id, name, port) in present {
            let known = self
                .connections
                .iter()
                .any(|listening| listening.id == id);
            if known || self.refused.contains(&id) {
                continue;
            }
            let mut input = match MidiInput::new(MIDI_CLIENT_NAME) {
                Ok(input) => input,
                Err(e) => {
                    mooloop_core::log_warn!("midi", "could not listen to {name:?} ({e})");
                    self.refused.push(id);
                    continue;
                }
            };
            // System exclusive and active sensing: neither plays a note nor
            // moves the transport. **Not `Ignore::All`**, which also takes
            // timing -- and this driver now wants Start, Continue, Stop and
            // Song Position, which arrive in the same system range. The
            // clock itself is dropped in `MidiBytes::new`, where the rule is
            // stated once and applies to both drivers' traffic.
            input.ignore(Ignore::Sysex | Ignore::ActiveSense);
            let queue = self.queue.clone();
            let port_id = MidiPortId(self.next_port);
            self.next_port = self.next_port.wrapping_add(1);
            let listener = move |_timestamp: u64, message: &[u8], _: &mut ()| {
                if let Some(message) = MidiBytes::new(port_id, message) {
                    // Full means the audio callback has stopped taking; a
                    // note from then is not one anybody is waiting to hear.
                    let _ = lock(&queue).push(message);
                }
            };
            match input.connect(&port, "input", listener, ()) {
                Ok(connection) => {
                    mooloop_core::log_info!("midi", "listening to the MIDI input {name:?}");
                    self.connections.push(Listening {
                        id,
                        name,
                        port: port_id,
                        _connection: connection,
                    });
                }
                Err(e) => {
                    mooloop_core::log_warn!("midi", "could not listen to {name:?} ({e})");
                    self.refused.push(id);
                }
            }
        }
    }
}

/// A Core Audio host whose sample rate is known but whose stream is not yet
/// open: the render state has to be built for the rate first.
pub(crate) struct Opening {
    host: cpal::Host,
    sample_rate: u32,
}

impl Opening {
    /// Learn the sample rate from the system output. The engine keeps it for
    /// its lifetime, and later streams ask their devices for the same rate --
    /// Core Audio, unlike JACK, lets a client set one.
    pub(crate) fn connect() -> Result<Self, Error> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| Error::Device("this machine has no audio output device".into()))?;
        let config = device
            .default_output_config()
            .map_err(|e| Error::Device(e.to_string()))?;
        Ok(Self {
            host,
            sample_rate: config.sample_rate(),
        })
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn start(
        self,
        executor: Executor,
        xrun_count: Arc<AtomicU64>,
        config: AudioConfig,
    ) -> Result<CoreAudioDriver, Error> {
        let default = Route::system_default();
        let wanted = match config.output_target.as_ref().map(Route::parse) {
            Some(Ok(route)) => route,
            Some(Err(e)) => {
                mooloop_core::log_warn!("audio", "ignoring the saved audio output: {e}");
                default.clone()
            }
            None => default.clone(),
        };
        let (midi_tx, midi_rx) = RingBuffer::new(MIDI_QUEUE_CAPACITY);
        let (control_tx, control_rx) = RingBuffer::new(CONTROL_QUEUE_CAPACITY);
        let (retired_tx, retired_rx) = RingBuffer::new(CONTROL_QUEUE_CAPACITY);
        let driver = CoreAudioDriver {
            host: self.host,
            sample_rate: self.sample_rate,
            shared: Arc::new(Shared {
                parked: Arc::new(Parked::new(Box::new(Realtime {
                    executor,
                    midi_rx,
                    input: None,
                    control_rx,
                    retired_tx,
                }))),
                control: Mutex::new(Control {
                    tx: control_tx,
                    retired_rx,
                }),
                xrun_count,
                lost: AtomicBool::new(false),
                input_lost: AtomicBool::new(false),
                input_underruns: AtomicU64::new(0),
                input_overruns: AtomicU64::new(0),
            }),
            midi: Mutex::new(MidiInputs::new(midi_tx)),
            state: Mutex::new(State {
                stream: None,
                route: default.clone(),
                wanted: wanted.clone(),
                buffer_size: config.buffer_size,
                last_probe: None,
                input_stream: None,
                input_name: None,
                input_buffer: 0,
                last_input_probe: None,
                reported_drift: 0,
            }),
            auto_reconnect: AtomicBool::new(config.auto_reconnect),
        };

        // The same rule the JACK adapter follows for a saved destination that
        // is gone: being audible somewhere beats silence with nothing on
        // screen to explain it. And a buffer size the device refuses is a
        // worse reason still to play nothing.
        let mut attempts = vec![(wanted.clone(), config.buffer_size)];
        if config.buffer_size.is_some() {
            attempts.push((wanted.clone(), None));
        }
        if wanted != default {
            attempts.push((default.clone(), config.buffer_size));
            if config.buffer_size.is_some() {
                attempts.push((default, None));
            }
        }
        let mut state = lock(&driver.state);
        let mut first_error = None;
        for (route, buffer_size) in &attempts {
            match driver.open(&mut state, route, *buffer_size) {
                Ok(()) => break,
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        match (&state.stream, first_error) {
            (None, error) => {
                return Err(Error::Stream(error.unwrap_or_default()));
            }
            (Some(_), Some(error)) => mooloop_core::log_warn!(
                "audio",
                "could not open the saved audio output ({error}); playing through {:?} \
                 instead. Preferences -> Audio picks a different one",
                state.route.device
            ),
            (Some(_), None) => {}
        }
        // Failure here is ordinary rather than fatal, and is the one place
        // the reason is worth printing: a machine with no input device, or one
        // where macOS has not granted the microphone, would otherwise be told
        // about once a second for the length of the session.
        match driver.open_input(&mut state) {
            Ok(()) => mooloop_core::log_info!(
                "audio",
                "listening to the audio input {:?}",
                state.input_name.as_deref().unwrap_or_default()
            ),
            Err(e) => mooloop_core::log_info!("audio", "no audio input: {e}"),
        }
        state.last_input_probe = Some(Instant::now());
        drop(state);
        lock(&driver.midi).scan();
        Ok(driver)
    }
}

/// The running Core Audio stream. Dropping it stops audio.
pub(crate) struct CoreAudioDriver {
    host: cpal::Host,
    sample_rate: u32,
    shared: Arc<Shared>,
    state: Mutex<State>,
    midi: Mutex<MidiInputs>,
    auto_reconnect: AtomicBool,
}

impl CoreAudioDriver {
    /// What the AUDIO menu calls the hardware input: the input device's own
    /// name, because Core Audio opens a device rather than joining a graph, so
    /// unlike JACK there is a device here to name. `None` when no input stream
    /// is open, and then the menu offers no input row at all.
    pub(crate) fn audio_input_label(&self) -> Option<String> {
        lock(&self.state).input_name.clone()
    }

    /// An estimate, where JACK's is a measurement.
    ///
    /// JACK asks the server for the capture and playback latencies it actually
    /// knows. cpal reports neither, and the two streams are not even on one
    /// clock, so what can honestly be said is the path through this driver: a
    /// buffer out, the ring's prefill, and a buffer in. A take from the input
    /// is started that much after its bar line, so it is right to within the
    /// device's own converter delay rather than to the sample.
    pub(crate) fn input_latency_frames(&self) -> u32 {
        let state = lock(&self.state);
        if state.input_stream.is_none() {
            return 0;
        }
        let output = state
            .stream
            .as_ref()
            .and_then(|stream| stream.buffer_size().ok())
            .unwrap_or(state.input_buffer);
        output + state.input_buffer * (INPUT_PREFILL_BUFFERS as u32 + 1)
    }

    /// An estimate, like [`Self::input_latency_frames`]: one output buffer,
    /// the part of the path this driver can see. Recorded MIDI is stamped
    /// this much earlier (MOO-209). 0 with no output stream.
    ///
    /// It undercounts: the device's own latency, its safety offset and the
    /// stream's latency are HAL properties this cpal-based driver does not
    /// read yet (open: MOO-237).
    pub(crate) fn playback_latency_frames(&self) -> u32 {
        self.buffer_size()
    }

    /// Frames of input read as silence, and frames of input dropped, since the
    /// engine started. See [`Shared::input_underruns`].
    fn input_drift(&self) -> (u64, u64) {
        (
            self.shared.input_underruns.load(Ordering::Relaxed),
            self.shared.input_overruns.load(Ordering::Relaxed),
        )
    }

    /// Every Core MIDI source being listened to, for the input picker.
    ///
    /// Core MIDI connects to each source separately, so this really is a list
    /// of devices rather than JACK's single merged port -- and a project that
    /// names one of them is naming a keyboard.
    pub(crate) fn midi_ports(&self) -> Vec<MidiPortInfo> {
        lock(&self.midi)
            .connections
            .iter()
            .map(|listening| MidiPortInfo {
                id: listening.port,
                name: listening.name.clone(),
            })
            .collect()
    }

    /// The system default first, then every device with at least two output
    /// channels, addressed by its first two.
    pub(crate) fn available_output_targets(&self) -> Vec<OutputTarget> {
        let default = Route::system_default().target();
        let mut targets = vec![OutputTarget {
            client: SYSTEM_DEFAULT_LABEL.to_owned(),
            port_l: default.0,
            port_r: default.1,
        }];
        let Ok(devices) = self.host.output_devices() else {
            return targets;
        };
        for device in devices {
            let (Ok(id), Ok(description), Ok(config)) = (
                device.id(),
                device.description(),
                device.default_output_config(),
            ) else {
                continue;
            };
            if config.channels() < 2 {
                continue;
            }
            let route = Route {
                device: id.to_string(),
                left: 0,
                right: 1,
            }
            .target();
            targets.push(OutputTarget {
                client: description.name().to_owned(),
                port_l: route.0,
                port_r: route.1,
            });
        }
        targets
    }

    pub(crate) fn set_output_target(&self, target: Option<(String, String)>) -> Result<(), String> {
        let route = match target {
            Some(target) => Route::parse(&target)?,
            None => Route::system_default(),
        };
        let mut state = lock(&self.state);
        let buffer_size = state.buffer_size;
        self.open(&mut state, &route, buffer_size)?;
        state.wanted = route;
        Ok(())
    }

    /// Per device, unlike JACK's server-wide buffer: reopens this stream and
    /// touches nothing else on the machine.
    pub(crate) fn set_buffer_size(&self, frames: u32) -> Result<(), String> {
        let mut state = lock(&self.state);
        let route = state.route.clone();
        self.open(&mut state, &route, Some(frames))
    }

    pub(crate) fn set_auto_reconnect(&self, enabled: bool) {
        self.auto_reconnect.store(enabled, Ordering::Relaxed);
    }

    pub(crate) fn buffer_size(&self) -> u32 {
        lock(&self.state)
            .stream
            .as_ref()
            .and_then(|stream| stream.buffer_size().ok())
            .unwrap_or(0)
    }

    pub(crate) fn current_target(&self) -> (String, String) {
        lock(&self.state).route.target()
    }

    /// Core Audio has no server to shut the engine down, and the sample rate
    /// is asked of each device rather than imposed by one, so there is
    /// nothing to report here: a device that goes away is reopened by
    /// [`Self::service`], and a stream that stops for good is seen by the
    /// handle's own watch on the callback.
    pub(crate) fn stopped(&self, _engine_rate: u32) -> Option<String> {
        None
    }

    /// Control-thread upkeep, called from the handle's event poll: listen to
    /// MIDI sources that have appeared, reopen a stream whose device went
    /// away, return to the asked-for device when it is back, and the same for
    /// the input stream ([`Self::service_input`]).
    pub(crate) fn service(&self) {
        {
            let mut midi = lock(&self.midi);
            if midi
                .last_scan
                .is_none_or(|scanned| scanned.elapsed() >= PROBE_INTERVAL)
            {
                midi.scan();
            }
        }
        let lost = self.shared.lost.swap(false, Ordering::Relaxed);
        let mut state = lock(&self.state);
        self.service_input(&mut state);
        if lost {
            state.stream = None;
            mooloop_core::log_warn!(
                "audio",
                "the audio output {:?} went away",
                state.route.device
            );
        }
        let stranded = state.stream.is_none();
        let away = state.route != state.wanted && self.auto_reconnect.load(Ordering::Relaxed);
        if !stranded && !away {
            return;
        }
        if !lost
            && state
                .last_probe
                .is_some_and(|probed| probed.elapsed() < PROBE_INTERVAL)
        {
            return;
        }
        state.last_probe = Some(Instant::now());

        let buffer_size = state.buffer_size;
        let wanted = state.wanted.clone();
        if self.open(&mut state, &wanted, buffer_size).is_ok() {
            if !lost {
                mooloop_core::log_info!("audio", "the audio output {:?} is back", wanted.device);
            }
            return;
        }
        let default = Route::system_default();
        if stranded && wanted != default {
            match self.open(&mut state, &default, buffer_size) {
                Ok(()) => mooloop_core::log_warn!(
                    "audio",
                    "playing through the system default until {:?} returns",
                    wanted.device
                ),
                Err(e) => mooloop_core::log_warn!("audio", "no audio output could be opened: {e}"),
            }
        }
    }

    /// Open an input stream on the system default input device and hand its
    /// ring to the audio callback.
    ///
    /// The input device is the system's, not a picked one: an input target
    /// would be a second `<device>#<channel>` pair through the settings and
    /// the preferences page, and nothing has asked for one. What this step
    /// owes is an input bus that exists (`audio-recording/01`).
    fn open_input(&self, state: &mut State) -> Result<(), String> {
        let device = self
            .host
            .default_input_device()
            .ok_or_else(|| "this machine has no audio input device".to_owned())?;
        let name = device
            .description()
            .map(|description| description.name().to_owned())
            .unwrap_or_else(|_| crate::AUDIO_IN_LABEL.to_owned());
        let supported = device
            .default_input_config()
            .map_err(|e| format!("could not read {name:?}'s input format: {e}"))?;
        let channels = supported.channels();
        if channels == 0 {
            return Err(format!("{name:?} offers no input channels"));
        }
        // The engine learns one sample rate from the system output and keeps
        // it for its lifetime. An input device that will not run at that rate
        // cannot be mixed into the same block without resampling, which this
        // driver does not do, so the open fails and there is no input row.
        let config = cpal::StreamConfig {
            channels,
            sample_rate: self.sample_rate,
            buffer_size: match state.buffer_size {
                Some(frames) => cpal::BufferSize::Fixed(frames),
                None => cpal::BufferSize::Default,
            },
        };
        let buffer = state.buffer_size.unwrap_or(ASSUMED_INPUT_BUFFER).max(1);
        let (tx, rx) = RingBuffer::new(buffer as usize * INPUT_RING_BUFFERS);
        // The old stream stops, and the callback lets go of the old ring,
        // before the new one starts. Otherwise a failed open would leave the
        // callback reading a ring whose producer has gone.
        state.input_stream = None;
        state.input_name = None;
        lock(&self.shared.control).send(ToCallback::Input(None));
        let stream = device
            .build_input_stream::<f32, _, _>(
                config,
                input_callback(tx, channels, self.shared.clone()),
                input_error_callback(self.shared.clone()),
                None,
            )
            .map_err(|e| match e.kind() {
                // Worth its own sentence: macOS refuses the microphone
                // silently the first time and the stream simply never
                // delivers, which reads as a broken driver rather than as a
                // permission nobody granted.
                cpal::ErrorKind::PermissionDenied => format!(
                    "macOS has not granted mooloop access to {name:?}. \
                     System Settings -> Privacy & Security -> Microphone"
                ),
                _ => format!("could not open {name:?} for input: {e}"),
            })?;
        stream
            .play()
            .map_err(|e| format!("could not start {name:?}: {e}"))?;
        lock(&self.shared.control).send(ToCallback::Input(Some(InputTap {
            rx,
            prefill: buffer as usize * INPUT_PREFILL_BUFFERS,
            priming: true,
        })));
        state.input_stream = Some(stream);
        state.input_name = Some(name);
        state.input_buffer = buffer;
        Ok(())
    }

    /// Reopen an input whose device went away or was never there, and report
    /// drift between the two clocks when the number moves.
    fn service_input(&self, state: &mut State) {
        let lost = self.shared.input_lost.swap(false, Ordering::Relaxed);
        if lost {
            mooloop_core::log_warn!(
                "audio",
                "the audio input {:?} went away",
                state.input_name.as_deref().unwrap_or(crate::AUDIO_IN_LABEL)
            );
            state.input_stream = None;
            state.input_name = None;
            lock(&self.shared.control).send(ToCallback::Input(None));
        }
        if !lost
            && state
                .last_input_probe
                .is_some_and(|probed| probed.elapsed() < PROBE_INTERVAL)
        {
            return;
        }
        state.last_input_probe = Some(Instant::now());
        if state.input_stream.is_none() {
            // Silent on failure, unlike the startup attempt: the reason does
            // not change, and a machine with no microphone would otherwise
            // say so once a second forever.
            if self.open_input(state).is_ok() {
                mooloop_core::log_info!(
                    "audio",
                    "listening to the audio input {:?}",
                    state.input_name.as_deref().unwrap_or_default()
                );
            }
            return;
        }
        let (under, over) = self.input_drift();
        if under + over > state.reported_drift {
            state.reported_drift = under + over;
            mooloop_core::log_warn!(
                "audio",
                "the audio input and output clocks are drifting: {under} frames read as \
                 silence and {over} frames dropped since the engine started. This driver \
                 counts drift; it does not resample it"
            );
        }
    }

    fn device(&self, name: &str) -> Option<cpal::Device> {
        if name == SYSTEM_DEFAULT {
            // Not the device the default happens to be now: cpal opens this
            // one so that it follows the system when the default changes.
            return self.host.default_output_device();
        }
        let id = name.parse::<cpal::DeviceId>().ok()?;
        self.host.device_by_id(&id)
    }

    /// Replace the running stream with one on `route`. The new stream is
    /// built before the old one is dropped, so a device that refuses leaves
    /// the old stream playing.
    fn open(
        &self,
        state: &mut State,
        route: &Route,
        buffer_size: Option<u32>,
    ) -> Result<(), String> {
        let device = self
            .device(&route.device)
            .ok_or_else(|| format!("the audio device {:?} is not available", route.device))?;
        let channels = device
            .default_output_config()
            .map_err(|e| format!("could not read {:?}'s output format: {e}", route.device))?
            .channels();
        if route.left.max(route.right) >= channels {
            return Err(format!(
                "{:?} has {channels} output channels; channel {} does not exist",
                route.device,
                route.left.max(route.right) + 1
            ));
        }
        let config = cpal::StreamConfig {
            channels,
            sample_rate: self.sample_rate,
            buffer_size: match buffer_size {
                Some(frames) => cpal::BufferSize::Fixed(frames),
                None => cpal::BufferSize::Default,
            },
        };
        let stream = device
            .build_output_stream::<f32, _, _>(
                config,
                render_callback(self.shared.clone(), route, channels),
                error_callback(self.shared.clone()),
                None,
            )
            .map_err(|e| format!("could not open {:?}: {e}", route.device))?;

        // Stop the old stream before the new one plays; then the next block
        // arrives on a new thread, after a gap that is not a late wake-up.
        state.stream = None;
        lock(&self.shared.control).send(ToCallback::BeginRun);
        if let Err(e) = stream.play() {
            // Nothing is playing now. Say so to `service`, which will find
            // something that does.
            self.shared.lost.store(true, Ordering::Relaxed);
            return Err(format!("could not start {:?}: {e}", route.device));
        }
        state.stream = Some(stream);
        state.route = route.clone();
        state.buffer_size = buffer_size;
        Ok(())
    }
}

/// The realtime callback: render in blocks of at most [`MAX_BLOCK_SIZE`] and
/// interleave the master bus into the route's two channels, silencing the
/// device's others.
fn render_callback(
    shared: Arc<Shared>,
    route: &Route,
    channels: u16,
) -> impl FnMut(&mut [f32], &cpal::OutputCallbackInfo) + Send + 'static {
    let channels = usize::from(channels);
    let (left, right) = (usize::from(route.left), usize::from(route.right));
    // Allocated here, on the control thread, at the largest block the executor
    // renders at once.
    let mut scratch_l = vec![0.0f32; MAX_BLOCK_SIZE];
    let mut scratch_r = vec![0.0f32; MAX_BLOCK_SIZE];
    let mut scratch_in_l = vec![0.0f32; MAX_BLOCK_SIZE];
    let mut scratch_in_r = vec![0.0f32; MAX_BLOCK_SIZE];
    let mut midi = [MidiBytes::default(); MAX_MIDI_PER_CALLBACK];
    // Empty until the first block takes the state the previous stream's
    // callback parked; dropped with this stream, which parks it again.
    let mut held = Held::new(shared.parked.clone());
    move |data, _info| {
        data.fill(0.0);
        // No lock (MOO-22). `None` only while the previous stream's callback
        // has not yet let go. A reopen drops the old stream first, but cpal's
        // disconnect and default-output threads can hold it alive for a
        // moment longer (they upgrade a `Weak` to it), and until its audio
        // unit is disposed its callback keeps the state. Blocks of silence
        // are the answer meanwhile, never two callbacks at once. MIDI stays in
        // the ring through them, so no key is lost.
        let Some(realtime) = held.get() else {
            return;
        };
        realtime.apply_control();
        let Realtime {
            executor,
            midi_rx,
            input,
            ..
        } = realtime;
        let mut arrived = 0;
        while arrived < midi.len() {
            let Ok(message) = midi_rx.pop() else {
                break;
            };
            midi[arrived] = message;
            arrived += 1;
        }
        for (index, block) in data.chunks_mut(MAX_BLOCK_SIZE * channels).enumerate() {
            let frames = block.len() / channels;
            let (out_l, out_r) = (&mut scratch_l[..frames], &mut scratch_r[..frames]);
            // Everything that arrived since the last callback acts at the top
            // of its first block: Core MIDI's timestamps are on another clock,
            // and a key's lateness is already smaller than one callback.
            let block_midi = if index == 0 { &midi[..arrived] } else { &[] };
            // The input is read a block at a time for the same reason the
            // output is written one: a callback may hand over more frames
            // than the executor renders at once.
            let (in_l, in_r): (&[f32], &[f32]) = match input.as_mut() {
                Some(tap) => {
                    let silent =
                        tap.fill(&mut scratch_in_l[..frames], &mut scratch_in_r[..frames]);
                    if silent > 0 {
                        shared.input_underruns.fetch_add(silent, Ordering::Relaxed);
                    }
                    (&scratch_in_l[..frames], &scratch_in_r[..frames])
                }
                // No input stream: empty slices, and the input bus stays
                // silent, which is what every caller saw before this step.
                None => (&[], &[]),
            };
            // Contained, so a device that panics costs this block rather than
            // unwinding into Core Audio's I/O thread.
            executor.process_contained(
                block_midi
                    .iter()
                    .map(|message| (message.port, 0, message.as_slice())),
                in_l,
                in_r,
                out_l,
                out_r,
            );
            for (frame, (l, r)) in block
                .chunks_exact_mut(channels)
                .zip(out_l.iter().zip(out_r.iter()))
            {
                frame[left] = *l;
                frame[right] = *r;
            }
        }
    }
}

/// Copy one input callback's interleaved frames into the ring as stereo
/// pairs, returning the frames that did not fit.
fn push_input(tx: &mut Producer<[f32; 2]>, data: &[f32], channels: usize) -> u64 {
    let mut dropped = 0;
    for frame in data.chunks_exact(channels) {
        // A mono device is heard in both ears rather than only in the left
        // one, which is how a microphone should arrive; channels past the
        // second are dropped, because the input bus is a stereo pair.
        let right = if channels > 1 { frame[1] } else { frame[0] };
        if tx.push([frame[0], right]).is_err() {
            dropped += 1;
        }
    }
    dropped
}

/// The input stream's realtime callback. It does no more than copy: the
/// output callback owns the executor, and this thread never waits for it.
fn input_callback(
    mut tx: Producer<[f32; 2]>,
    channels: u16,
    shared: Arc<Shared>,
) -> impl FnMut(&[f32], &cpal::InputCallbackInfo) + Send + 'static {
    let channels = usize::from(channels);
    move |data, _info| {
        let dropped = push_input(&mut tx, data, channels);
        if dropped > 0 {
            shared.input_overruns.fetch_add(dropped, Ordering::Relaxed);
        }
    }
}

fn input_error_callback(shared: Arc<Shared>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |error| match error.kind() {
        cpal::ErrorKind::Xrun => {
            shared.input_overruns.fetch_add(1, Ordering::Relaxed);
        }
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated => {
            shared.input_lost.store(true, Ordering::Relaxed);
        }
        _ => {}
    }
}

fn error_callback(shared: Arc<Shared>) -> impl FnMut(cpal::Error) + Send + 'static {
    move |error| match error.kind() {
        // Reported by an overload listener that fires on the I/O thread.
        cpal::ErrorKind::Xrun => {
            shared.xrun_count.fetch_add(1, Ordering::Relaxed);
        }
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated => {
            shared.lost.store(true, Ordering::Relaxed);
        }
        // A stream on the system default rerouted itself, which is what it was
        // opened to do; and a refused realtime promotion is already visible in
        // the executor's own scheduling readout.
        _ => {}
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::{
        channel_address, parse_channel, push_input, InputTap, MidiBytes, MidiPortId, Route,
    };
    use rtrb::RingBuffer;

    /// `prefill` frames with a known value in the ring, read back a block at a
    /// time through a tap whose prefill is `prefill`.
    fn tap_over(frames: &[[f32; 2]], capacity: usize, prefill: usize) -> InputTap {
        let (mut tx, rx) = RingBuffer::new(capacity);
        for frame in frames {
            tx.push(*frame).expect("the ring was sized for these");
        }
        // `tx` is dropped here on purpose. rtrb lets a consumer drain a ring
        // whose producer has gone, so the frames pushed above are still
        // readable -- and a leaked producer would be a leaked ring.
        InputTap {
            rx,
            prefill,
            priming: true,
        }
    }

    /// A ring that has not reached its prefill reads as silence rather than as
    /// the few frames it does hold, and it is **not** counted as drift: the
    /// ring filling up is what starting is.
    #[test]
    fn the_input_ring_is_silent_until_it_has_its_prefill() {
        let mut tap = tap_over(&[[0.5, -0.5]; 3], 64, 8);
        let (mut l, mut r) = ([9.0f32; 4], [9.0f32; 4]);
        assert_eq!(tap.fill(&mut l, &mut r), 0);
        assert_eq!(l, [0.0; 4]);
        assert_eq!(r, [0.0; 4]);

        // With the prefill reached, the same tap hands over what it holds, in
        // order and with the channels still paired.
        let mut tap = tap_over(&[[0.25, -0.25], [0.5, -0.5]], 64, 2);
        let (mut l, mut r) = ([9.0f32; 2], [9.0f32; 2]);
        assert_eq!(tap.fill(&mut l, &mut r), 0);
        assert_eq!(l, [0.25, 0.5]);
        assert_eq!(r, [-0.25, -0.5]);
    }

    /// A ring that runs dry mid-block pads the rest with silence, reports the
    /// frames it could not supply, and primes again -- so the next block waits
    /// for a refill instead of reading the same empty ring.
    #[test]
    fn a_starved_input_ring_pads_with_silence_and_primes_again() {
        let mut tap = tap_over(&[[1.0, 1.0], [1.0, 1.0]], 64, 2);
        let (mut l, mut r) = ([9.0f32; 4], [9.0f32; 4]);
        assert_eq!(tap.fill(&mut l, &mut r), 2);
        assert_eq!(l, [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(r, [1.0, 1.0, 0.0, 0.0]);
        assert!(tap.priming, "a starved ring refills before it is read again");

        // And the refill really is waited for: the ring is empty, so the next
        // block is silence rather than a second underrun report.
        let (mut l, mut r) = ([9.0f32; 4], [9.0f32; 4]);
        assert_eq!(tap.fill(&mut l, &mut r), 0);
        assert_eq!(l, [0.0; 4]);
    }

    /// A one-channel device is heard in both ears, and a device with more
    /// channels than the input bus has is read from its first two.
    #[test]
    fn a_mono_input_device_is_heard_in_both_ears() {
        let (mut tx, mut rx) = RingBuffer::new(8);
        assert_eq!(push_input(&mut tx, &[0.5, -0.25], 1), 0);
        assert_eq!(rx.pop(), Ok([0.5, 0.5]));
        assert_eq!(rx.pop(), Ok([-0.25, -0.25]));

        let (mut tx, mut rx) = RingBuffer::new(8);
        assert_eq!(push_input(&mut tx, &[0.5, -0.5, 0.125], 3), 0);
        assert_eq!(rx.pop(), Ok([0.5, -0.5]));

        // A ring with no room drops rather than blocking, and says how much.
        let (mut tx, _rx) = RingBuffer::new(1);
        assert_eq!(push_input(&mut tx, &[0.1, 0.2, 0.3, 0.4], 2), 1);
    }

    /// A channel message crosses whole; system exclusive, which Core MIDI
    /// hands over in one piece, and an empty packet do not cross at all.
    #[test]
    fn only_a_channel_message_fits_the_ring() {
        let note = MidiBytes::new(MidiPortId::FIRST, &[0x90, 60, 100]).expect("a note-on fits");
        assert_eq!(note.as_slice(), [0x90, 60, 100]);
        let program =
            MidiBytes::new(MidiPortId::FIRST, &[0xC0, 5]).expect("a program change fits");
        assert_eq!(program.as_slice(), [0xC0, 5]);
        assert_eq!(MidiBytes::new(MidiPortId::FIRST, &[]), None);
        assert_eq!(
            MidiBytes::new(MidiPortId::FIRST, &[0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]),
            None
        );
        // Clock and active sensing never take a place in the queue; the
        // transport messages in the same range do.
        assert_eq!(MidiBytes::new(MidiPortId::FIRST, &[0xF8]), None);
        assert_eq!(MidiBytes::new(MidiPortId::FIRST, &[0xFE]), None);
        assert!(MidiBytes::new(MidiPortId::FIRST, &[0xFA]).is_some());
        assert!(MidiBytes::new(MidiPortId::FIRST, &[0xF2, 16, 0]).is_some());
    }

    #[test]
    fn a_route_survives_its_own_spelling() {
        let route = Route {
            device: "coreaudio:AppleUSBAudioEngine:Focusrite:1".to_owned(),
            left: 2,
            right: 3,
        };
        assert_eq!(Route::parse(&route.target()), Ok(route));
    }

    #[test]
    fn channels_are_counted_from_one_in_the_address() {
        assert_eq!(channel_address("default", 0), "default#1");
        assert_eq!(parse_channel("default#2"), Ok(("default", 1)));
    }

    /// A `#` inside the device identifier belongs to the device.
    #[test]
    fn only_the_last_hash_separates_the_channel() {
        assert_eq!(parse_channel("coreaudio:a#b#1"), Ok(("coreaudio:a#b", 0)));
    }

    #[test]
    fn a_malformed_address_is_refused_rather_than_guessed() {
        for address in [
            "default",
            "default#0",
            "default#",
            "#1",
            "default#x",
            "system:playback_1",
        ] {
            assert!(parse_channel(address).is_err(), "{address:?}");
        }
    }

    /// JACK routes each side to any port; a Core Audio stream is one device.
    #[test]
    fn left_and_right_on_different_devices_are_refused() {
        let target = ("coreaudio:a#1".to_owned(), "coreaudio:b#2".to_owned());
        assert!(Route::parse(&target).is_err());
    }
}
