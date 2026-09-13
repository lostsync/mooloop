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
//! thread the callback runs on but not the executor, which lives behind a
//! mutex the callback only ever `try_lock`s. The two streams never run at
//! once: the old one is dropped before the new one plays, so the lock is
//! uncontended in every block except, at most, the one a reopen interrupts,
//! and that block plays silence rather than waiting.
//!
//! MIDI input comes from Core MIDI, through `midir`. There is no patchbay to
//! connect a keyboard to mooloop in, so mooloop listens to every source there
//! is, and [`CoreAudioDriver::service`] picks up a keyboard plugged in later.
//! Messages cross to the audio callback over a bounded ring and act at the
//! top of the next block: a callback's worth of timing, a few milliseconds.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use midir::{Ignore, MidiInput, MidiInputConnection};
use mooloop_dsp::MAX_BLOCK_SIZE;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::driver::{AudioConfig, OutputTarget};
use crate::executor::Executor;
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

/// One channel message, copied out of Core MIDI's buffer so it can cross
/// threads without allocating. System exclusive is filtered before it gets
/// here, so three bytes is every message mooloop reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MidiBytes {
    len: u8,
    bytes: [u8; 3],
}

impl MidiBytes {
    fn new(message: &[u8]) -> Option<Self> {
        if message.is_empty() || message.len() > 3 {
            return None;
        }
        let mut bytes = [0; 3];
        bytes[..message.len()].copy_from_slice(message);
        Some(Self {
            len: message.len() as u8,
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

/// What the audio callback owns while it runs, under one lock.
struct Realtime {
    executor: Executor,
    midi_rx: Consumer<MidiBytes>,
}

/// What the realtime callbacks share with the control thread.
struct Shared {
    realtime: Mutex<Realtime>,
    xrun_count: Arc<AtomicU64>,
    /// Set from cpal's error callback when the stream's device has gone. The
    /// callback may run on the realtime thread, so it sets a flag and nothing
    /// else; the control thread does the reopening.
    lost: AtomicBool,
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
    /// By source id, which survives a rename.
    connections: Vec<(String, String, MidiInputConnection<()>)>,
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
        self.connections.retain(|(id, name, _)| {
            let keep = is_present(id);
            if !keep {
                mooloop_core::log_info!("midi", "the MIDI input {name:?} went away");
            }
            keep
        });
        self.refused.retain(is_present);

        for (id, name, port) in present {
            let known = self
                .connections
                .iter()
                .any(|(connected, _, _)| *connected == id);
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
            // Clock, active sensing and system exclusive: none of it plays a
            // note, and a clock alone is 24 messages a beat.
            input.ignore(Ignore::All);
            let queue = self.queue.clone();
            let listener = move |_timestamp: u64, message: &[u8], _: &mut ()| {
                if let Some(message) = MidiBytes::new(message) {
                    // Full means the audio callback has stopped taking; a
                    // note from then is not one anybody is waiting to hear.
                    let _ = lock(&queue).push(message);
                }
            };
            match input.connect(&port, "input", listener, ()) {
                Ok(connection) => {
                    mooloop_core::log_info!("midi", "listening to the MIDI input {name:?}");
                    self.connections.push((id, name, connection));
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
        let driver = CoreAudioDriver {
            host: self.host,
            sample_rate: self.sample_rate,
            shared: Arc::new(Shared {
                realtime: Mutex::new(Realtime { executor, midi_rx }),
                xrun_count,
                lost: AtomicBool::new(false),
            }),
            midi: Mutex::new(MidiInputs::new(midi_tx)),
            state: Mutex::new(State {
                stream: None,
                route: default.clone(),
                wanted: wanted.clone(),
                buffer_size: config.buffer_size,
                last_probe: None,
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

    /// Control-thread upkeep, called from the handle's event poll: listen to
    /// MIDI sources that have appeared, reopen a stream whose device went
    /// away, and return to the asked-for device when it is back.
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
        lock(&self.shared.realtime).executor.begin_run();
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
    let mut midi = [MidiBytes::default(); MAX_MIDI_PER_CALLBACK];
    move |data, _info| {
        data.fill(0.0);
        // `try_lock`, never `lock`: the only other holder is a reopen on the
        // control thread, and a block of silence is the right answer to one.
        // MIDI stays in the ring through a missed callback, so no key is lost.
        let Ok(mut realtime) = shared.realtime.try_lock() else {
            return;
        };
        let Realtime { executor, midi_rx } = &mut *realtime;
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
            executor.process(
                block_midi.iter().map(|message| (0, message.as_slice())),
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
    use super::{channel_address, parse_channel, MidiBytes, Route};

    /// A channel message crosses whole; system exclusive, which Core MIDI
    /// hands over in one piece, and an empty packet do not cross at all.
    #[test]
    fn only_a_channel_message_fits_the_ring() {
        let note = MidiBytes::new(&[0x90, 60, 100]).expect("a note-on fits");
        assert_eq!(note.as_slice(), [0x90, 60, 100]);
        let program = MidiBytes::new(&[0xC0, 5]).expect("a program change fits");
        assert_eq!(program.as_slice(), [0xC0, 5]);
        assert_eq!(MidiBytes::new(&[]), None);
        assert_eq!(MidiBytes::new(&[0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7]), None);
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
