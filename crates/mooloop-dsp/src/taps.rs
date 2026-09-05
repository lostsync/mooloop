//! The auxiliary outputs a producer fills for the duration of one process
//! call.
//!
//! `AUDIO_ARCHITECTURE.md` is explicit that "a node must not retain a borrowed
//! bus reference received at construction", so the taps are supplied per call
//! like a plugin's port group and go out of scope with it.
//!
//! ## Why a slice indexed by tap number
//!
//! Two other shapes were tried on paper and neither works:
//!
//! - **Passing the outlet *ids* and matching on them** puts a lookup in the
//!   sample loop, once per source per frame, for a feature that is off on
//!   almost every project.
//! - **Handing the device one buffer at a time** cannot work at all: ML-P8
//!   computes `Osc 1`, `Osc 2` and `Osc 3` in the same pass and would need
//!   three simultaneous mutable borrows out of one slice.
//!
//! Indexing by the device's own tap number is a fixed offset, costs one
//! untaken branch per tap when nobody is subscribed, and keeps the
//! id-to-index mapping in the engine, where the descriptor table already is.

use crate::bus::StereoBus;
use mooloop_core::MAX_DEVICE_AUDIO_TAPS;

/// The port group handed to a producer for one block.
///
/// Indexed by [`mooloop_core::audio_tap_index`] -- the outlet's position in
/// the device's declared audio run -- so a device's tap numbering is part of
/// its interface in the same way its parameter ids are, and there is one
/// declared list rather than two that can disagree.
#[derive(Default)]
pub struct AudioTaps<'a> {
    ports: [Option<&'a mut StereoBus>; MAX_DEVICE_AUDIO_TAPS],
}

impl<'a> AudioTaps<'a> {
    /// Nothing subscribed: what every producer is handed on a project that
    /// has never authored an edge.
    pub fn none() -> Self {
        Self::default()
    }

    /// Hand this group the buffer for one tap number. Out-of-range numbers
    /// are dropped rather than panicking: the caller is the engine, working
    /// from a device's own declared table, and a device that grew an eighth
    /// audio outlet should lose the tap rather than the callback.
    pub fn set(&mut self, tap: usize, bus: &'a mut StereoBus) {
        if let Some(slot) = self.ports.get_mut(tap) {
            *slot = Some(bus);
        }
    }

    /// The buffer for `tap`, when somebody is listening to it.
    ///
    /// Borrowed for as long as the returned reference lives, so a device
    /// writing several taps in one frame takes them one statement at a time.
    /// That is what the inner loops below actually do.
    pub fn port(&mut self, tap: usize) -> Option<&mut StereoBus> {
        self.ports.get_mut(tap)?.as_deref_mut()
    }

    /// Whether anybody is reading `tap`.
    ///
    /// Asked *outside* the sample loop, by a device deciding whether a source
    /// still has to run: an oscillator turned down to silence in the device's
    /// own mix is exactly the one a pre-level subscriber wants, so "nobody
    /// needs this" has to include "nobody is listening to it either".
    pub fn wants(&self, tap: usize) -> bool {
        matches!(self.ports.get(tap), Some(Some(_)))
    }

    /// Whether this producer owes nothing at all this block.
    pub fn is_empty(&self) -> bool {
        self.ports.iter().all(Option::is_none)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsubscribed_group_wants_nothing() {
        let mut taps = AudioTaps::none();
        assert!(taps.is_empty());
        assert!(!taps.wants(0));
        assert!(taps.port(0).is_none());
    }

    #[test]
    fn a_tap_is_reachable_by_its_number_and_nothing_else_is() {
        let mut bus = StereoBus::with_capacity(8);
        let mut taps = AudioTaps::none();
        taps.set(2, &mut bus);
        assert!(!taps.is_empty());
        assert!(taps.wants(2));
        assert!(!taps.wants(1));
        taps.port(2).expect("tap 2 is subscribed").l[0] = 0.5;
        assert!(taps.port(3).is_none());
        assert_eq!(bus.l[0], 0.5);
    }

    /// A device that declared more audio outlets than the port group can
    /// carry loses the tap, not the callback. `check_table` is what stops
    /// that reaching a running engine.
    #[test]
    fn a_tap_number_past_the_ceiling_is_dropped_rather_than_panicking() {
        let mut bus = StereoBus::with_capacity(8);
        let mut taps = AudioTaps::none();
        taps.set(MAX_DEVICE_AUDIO_TAPS, &mut bus);
        assert!(taps.is_empty());
    }
}
