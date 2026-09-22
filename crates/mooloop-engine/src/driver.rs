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

/// Live driver state for populating the preferences dialog.
#[derive(Debug, Clone)]
pub struct DriverStatus {
    pub sample_rate: u32,
    pub buffer_size: u32,
    pub current_target: (String, String),
}
