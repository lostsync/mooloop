//! Types describing the configured audio driver and its live status.
//!
//! Only JACK exists today, so these stay JACK-shaped rather than sitting
//! behind a generic driver trait (see `docs/FOCUS.md` on not building
//! generality nothing uses yet). A second driver would extract a trait from
//! this one concrete shape mechanically.

/// Requested audio configuration, applied when the engine opens its client.
#[derive(Debug, Clone, Default)]
pub struct AudioConfig {
    /// Requested JACK buffer size in frames. `None` leaves the server's
    /// current buffer size alone. JACK buffer size is server-wide: this
    /// changes it for every client connected to the server, not only
    /// mooloop.
    pub buffer_size: Option<u32>,
    /// Destination port pair `out_l`/`out_r` auto-connect to on startup.
    /// `None` uses the system playback default.
    pub output_target: Option<(String, String)>,
    /// Whether to retry connecting `output_target` when the JACK port graph
    /// changes and the target is currently unconnected (e.g. a hot-plugged
    /// device re-registers its ports).
    pub auto_reconnect: bool,
}

/// One JACK client discovered as a possible output destination, found by
/// grouping that client's input ports.
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
