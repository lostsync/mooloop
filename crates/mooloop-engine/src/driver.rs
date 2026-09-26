//! Types describing the configured audio driver and its live status.
//!
//! JACK and Core Audio share these, and they stay pair-shaped rather than
//! sitting behind a generic driver trait: an output target is two
//! destinations under both drivers -- JACK port names, or `<device>#<channel>`
//! addresses on one Core Audio device -- and the platform picks its driver at
//! compile time. The one choice made at run time is between that driver and
//! none at all (`null_driver`), and an enum in `lib.rs` makes it.

/// Requested audio configuration, applied when the engine opens its client.
#[derive(Debug, Clone, Default)]
pub struct AudioConfig {
    /// Requested buffer size in frames. `None` leaves the driver's current
    /// buffer size alone. Under JACK the buffer is server-wide: this changes
    /// it for every client connected to the server, not only mooloop.
    pub buffer_size: Option<u32>,
    /// Destination pair the master output plays through: JACK port names, or
    /// Core Audio `<device>#<channel>` addresses. `None` uses the system
    /// playback default.
    pub output_target: Option<(String, String)>,
    /// The outputs picked before `output_target`, most recent first. When
    /// the output that is playing goes away, the engine moves to the most
    /// recent of these that is there. See [`remember_output`].
    pub earlier_outputs: Vec<(String, String)>,
    /// Whether to find another output when the one playing goes away. Under
    /// JACK this is the most recent pick that is available; a device that
    /// appears while the output is connected is left alone.
    pub auto_reconnect: bool,
}

/// How many picked outputs are remembered, the current one included.
pub const REMEMBERED_OUTPUTS: usize = 8;

/// Record `chosen` as the most recent output pick in `picks`, which is kept
/// most recent first, without duplicates, and at most [`REMEMBERED_OUTPUTS`]
/// long.
///
/// The one place the order is decided, because two sides keep the list: the
/// driver, which reconnects from it, and the settings file, which carries it
/// to the next launch.
pub fn remember_output(picks: &mut Vec<(String, String)>, chosen: (String, String)) {
    picks.retain(|pick| *pick != chosen);
    picks.insert(0, chosen);
    picks.truncate(REMEMBERED_OUTPUTS);
}

#[cfg(test)]
mod tests {
    use super::{remember_output, REMEMBERED_OUTPUTS};

    fn pair(name: &str) -> (String, String) {
        (format!("{name}:FL"), format!("{name}:FR"))
    }

    #[test]
    fn a_pick_goes_to_the_front_once() {
        let mut picks = vec![pair("speakers"), pair("headphones")];
        remember_output(&mut picks, pair("headphones"));
        assert_eq!(picks, vec![pair("headphones"), pair("speakers")]);
        remember_output(&mut picks, pair("usb"));
        assert_eq!(picks, vec![pair("usb"), pair("headphones"), pair("speakers")]);
    }

    #[test]
    fn the_oldest_pick_is_forgotten_past_the_limit() {
        let mut picks = Vec::new();
        for index in 0..=REMEMBERED_OUTPUTS {
            remember_output(&mut picks, pair(&index.to_string()));
        }
        assert_eq!(picks.len(), REMEMBERED_OUTPUTS);
        assert_eq!(picks[0], pair(&REMEMBERED_OUTPUTS.to_string()));
        assert!(!picks.contains(&pair("0")));
    }
}

/// One destination the driver offers: a JACK client, by its first two input
/// ports, or a Core Audio device, by its first two channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputTarget {
    pub client: String,
    pub port_l: String,
    pub port_r: String,
}

/// Whether the engine is being heard, as [`crate::EngineHandle::audio_state`]
/// reads it. The interface asks about either of the two that are not
/// [`AudioState::Running`], in its own question dialog, offering Reconnect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioState {
    /// Rendering through an audio device.
    Running,
    /// No audio device was opened, and why. The engine runs -- edits,
    /// transport and meters all work -- and nothing hears it.
    NoDevice(String),
    /// The device was open and has stopped, and why: the server shut down,
    /// changed its sample rate under the engine, or stopped calling it.
    Stopped(String),
}

/// What a driver's notification thread tells the control thread about the
/// server, through atomics, because those callbacks may do nothing else.
#[derive(Debug, Default)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) struct DriverHealth {
    /// The server shut the client down: JACK's `on_shutdown`, which is what
    /// a PipeWire or JACK restart looks like from inside the client.
    pub shut_down: std::sync::atomic::AtomicBool,
    /// The rate the server last reported, or zero before it has. Compared
    /// with the rate the engine was built for.
    pub server_rate: std::sync::atomic::AtomicU32,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
impl DriverHealth {
    /// Why the engine has stopped being heard, if the server has said
    /// anything that means it. `engine_rate` is the rate the render state was
    /// built for.
    pub fn stopped(&self, engine_rate: u32) -> Option<String> {
        use std::sync::atomic::Ordering;
        if self.shut_down.load(Ordering::Relaxed) {
            return Some("the audio server shut down".to_owned());
        }
        let server_rate = self.server_rate.load(Ordering::Relaxed);
        (server_rate != 0 && server_rate != engine_rate).then(|| {
            format!(
                "the audio server changed its sample rate to {server_rate} Hz, and mooloop is \
                 still running at {engine_rate} Hz"
            )
        })
    }
}

#[cfg(test)]
mod health_tests {
    use super::DriverHealth;
    use std::sync::atomic::Ordering;

    #[test]
    fn a_server_that_says_nothing_has_not_stopped() {
        assert_eq!(DriverHealth::default().stopped(48_000), None);
    }

    #[test]
    fn a_shutdown_is_a_stop() {
        let health = DriverHealth::default();
        health.shut_down.store(true, Ordering::Relaxed);
        assert_eq!(health.stopped(48_000).as_deref(), Some("the audio server shut down"));
    }

    /// The sample-rate notification also fires with the rate the engine
    /// already has; only a different one means the render state is wrong.
    #[test]
    fn only_a_different_rate_is_a_stop() {
        let health = DriverHealth::default();
        health.server_rate.store(48_000, Ordering::Relaxed);
        assert_eq!(health.stopped(48_000), None);
        health.server_rate.store(44_100, Ordering::Relaxed);
        assert!(health.stopped(48_000).is_some_and(|why| why.contains("44100 Hz")));
    }
}

/// One direction of a Core Audio device's latency, in frames, as the HAL
/// reports it: the four properties Apple sums for the time between a sample
/// crossing the IO boundary and it reaching the converter, or the other way
/// round for input (MOO-237). The arithmetic lives here rather than in
/// `coreaudio_driver.rs` so that it is compiled, and tested, everywhere; the
/// reads are the Mac's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct DeviceLatency {
    /// `kAudioDevicePropertyLatency`: the device's own, converters included.
    pub device: u32,
    /// `kAudioDevicePropertySafetyOffset`: how far ahead of the hardware the
    /// HAL keeps the IO cycle.
    pub safety_offset: u32,
    /// `kAudioDevicePropertyBufferFrameSize`: the IO buffer.
    pub buffer: u32,
    /// `kAudioStreamPropertyLatency` of the direction's first stream.
    pub stream: u32,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl DeviceLatency {
    /// All four, summed.
    pub fn frames(self) -> u32 {
        self.buffer.saturating_add(self.beyond_buffer())
    }

    /// What the device adds beyond its IO buffer.
    pub fn beyond_buffer(self) -> u32 {
        self.device
            .saturating_add(self.safety_offset)
            .saturating_add(self.stream)
    }
}

/// What the Core Audio driver reports as its playback latency: the HAL's sum
/// for the output device, or, when any of the four reads failed, one buffer of
/// the running stream, the estimate this replaced.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn coreaudio_playback_latency(hal: Option<DeviceLatency>, stream_buffer: u32) -> u32 {
    hal.map_or(stream_buffer, DeviceLatency::frames)
}

/// What the Core Audio driver reports as its input latency: a sound played
/// `playback` frames after it was rendered, then the input device's own
/// latency beyond its buffer (none when its reads failed), then the path
/// through the driver's ring, which is the input buffer and the
/// `prefill_buffers` the ring is primed with.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn coreaudio_input_latency(
    playback: u32,
    input: Option<DeviceLatency>,
    input_buffer: u32,
    prefill_buffers: u32,
) -> u32 {
    playback
        .saturating_add(input.map_or(0, DeviceLatency::beyond_buffer))
        .saturating_add(input_buffer.saturating_mul(prefill_buffers.saturating_add(1)))
}

#[cfg(test)]
mod coreaudio_latency_tests {
    use super::{coreaudio_input_latency, coreaudio_playback_latency, DeviceLatency};

    /// Illustrative values of the size a built-in output reports at 48 kHz,
    /// not a measurement of any device.
    const BUILT_IN: DeviceLatency = DeviceLatency {
        device: 11,
        safety_offset: 144,
        buffer: 512,
        stream: 0,
    };

    #[test]
    fn playback_is_the_four_properties_summed() {
        let hal = DeviceLatency {
            device: 24,
            safety_offset: 32,
            buffer: 256,
            stream: 7,
        };
        assert_eq!(hal.frames(), 24 + 32 + 256 + 7);
        assert_eq!(coreaudio_playback_latency(Some(hal), 512), 319);
        assert_eq!(coreaudio_playback_latency(Some(BUILT_IN), 512), 667);
    }

    /// Any read that failed keeps the old estimate: one buffer of the stream.
    #[test]
    fn a_failed_read_is_one_stream_buffer() {
        assert_eq!(coreaudio_playback_latency(None, 512), 512);
        assert_eq!(coreaudio_playback_latency(None, 0), 0);
    }

    /// The input adds its device's latency, safety offset and stream latency,
    /// but not its buffer again: that is already the ring's last buffer.
    #[test]
    fn input_adds_the_input_device_beyond_its_buffer() {
        let input = DeviceLatency {
            device: 40,
            safety_offset: 20,
            buffer: 256,
            stream: 5,
        };
        assert_eq!(coreaudio_input_latency(667, Some(input), 256, 2), 667 + 65 + 256 * 3);
        assert_eq!(coreaudio_input_latency(512, None, 256, 2), 512 + 256 * 3);
    }

    /// A HAL answering nonsense saturates rather than wrapping to a small
    /// number that would pass for a real one.
    #[test]
    fn nonsense_saturates() {
        let wild = DeviceLatency {
            device: u32::MAX,
            safety_offset: 1,
            buffer: 1,
            stream: 1,
        };
        assert_eq!(wild.frames(), u32::MAX);
        assert_eq!(coreaudio_input_latency(u32::MAX, Some(wild), 1, 1), u32::MAX);
    }
}

/// Live driver state for populating the preferences dialog.
#[derive(Debug, Clone)]
pub struct DriverStatus {
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub current_target: (String, String),
}
