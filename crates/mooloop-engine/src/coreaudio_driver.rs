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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use mooloop_dsp::MAX_BLOCK_SIZE;

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

/// What the realtime callbacks share with the control thread.
struct Shared {
    executor: Mutex<Executor>,
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
        let driver = CoreAudioDriver {
            host: self.host,
            sample_rate: self.sample_rate,
            shared: Arc::new(Shared {
                executor: Mutex::new(executor),
                xrun_count,
                lost: AtomicBool::new(false),
            }),
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
        Ok(driver)
    }
}

/// The running Core Audio stream. Dropping it stops audio.
pub(crate) struct CoreAudioDriver {
    host: cpal::Host,
    sample_rate: u32,
    shared: Arc<Shared>,
    state: Mutex<State>,
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

    /// Control-thread upkeep, called from the handle's event poll: reopen a
    /// stream whose device went away, and return to the asked-for device when
    /// it is back.
    pub(crate) fn service(&self) {
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
        lock(&self.shared.executor).begin_run();
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
    move |data, _info| {
        data.fill(0.0);
        // `try_lock`, never `lock`: the only other holder is a reopen on the
        // control thread, and a block of silence is the right answer to one.
        let Ok(mut executor) = shared.executor.try_lock() else {
            return;
        };
        for block in data.chunks_mut(MAX_BLOCK_SIZE * channels) {
            let frames = block.len() / channels;
            let (out_l, out_r) = (&mut scratch_l[..frames], &mut scratch_r[..frames]);
            executor.process(std::iter::empty(), out_l, out_r);
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
    use super::{channel_address, parse_channel, Route};

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
