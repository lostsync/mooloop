//! Types describing the configured audio driver and its live status.
//!
//! JACK and Core Audio share these, and they stay pair-shaped rather than
//! sitting behind a generic driver trait: an output target is two
//! destinations under both drivers -- JACK port names, or `<device>#<channel>`
//! addresses on one Core Audio device -- and the platform picks the driver at
//! compile time, so there is nothing for a trait to dispatch.

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
    /// Whether to return to `output_target` when it disappears and comes
    /// back -- under JACK, when a hot-plugged device re-registers its ports.
    pub auto_reconnect: bool,
}

/// One destination the driver offers: a JACK client, by its first two input
/// ports, or a Core Audio device, by its first two channels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputTarget {
    pub client: String,
    pub port_l: String,
    pub port_r: String,
}

/// Live driver state for populating the preferences dialog.
#[derive(Debug, Clone)]
pub struct DriverStatus {
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub current_target: (String, String),
}
