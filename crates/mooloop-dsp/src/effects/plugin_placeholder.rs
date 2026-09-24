//! What a plugin device's rack slot holds when no plugin is running in it.
//!
//! `build_effect` cannot build a plugin and must not know plugins exist: the
//! host crate loads them, on the control thread, and the session swaps the
//! real processor into the slot (`docs/plans/plugin-hosting/`, step 06). Until
//! then, and for as long as the plugin is **missing** -- not installed, not
//! found by the scan, or refusing to load -- the slot holds this: a
//! pass-through for an effect, so the song still plays and the device is
//! still addressable, bypassable, saveable and movable. Its lanes and routes
//! are kept (`00-status.md`, "Failure").
//!
//! **Standing in for a latent plugin.** When the rack pulls a running
//! processor back for a restart, the chain is compensated for that plugin's
//! latency and stays so until the next processor reports its own. A
//! pass-through of no latency in between would move the channel earlier by
//! that much and back again: a jump in time either side of the swap, which
//! is a click on anything sustained (MOO-213). So the pull-back's
//! placeholder waits as long as the plugin did ([`PluginPlaceholder::with_latency`]):
//! still dry, and in time with everything compensated around it.

use mooloop_core::PluginSlotId;

use crate::align::IntegerDelay;
use crate::bus::StereoBus;
use crate::event::EventList;
use crate::node::{AudioNode, Discontinuity, ProcessContext};

/// A plugin slot's occupant with no plugin in it: transparent, and
/// stateless apart from the latency it may stand in for.
pub struct PluginPlaceholder {
    slot: PluginSlotId,
    /// The latency of the plugin it stands in for, as a ring; `None` for
    /// none, which is every placeholder but a restart's.
    delay: Option<IntegerDelay>,
}

impl PluginPlaceholder {
    pub fn new(slot: PluginSlotId) -> Self {
        Self { slot, delay: None }
    }

    /// A pass-through `latency` frames late: the placeholder for a plugin
    /// whose latency the chain is compensated for. Allocates the ring, so it
    /// is built on the control thread, like every node.
    pub fn with_latency(slot: PluginSlotId, latency: u32) -> Self {
        Self {
            slot,
            delay: IntegerDelay::new(latency),
        }
    }

    /// The `Project::plugins` slot this device names.
    pub fn slot(&self) -> PluginSlotId {
        self.slot
    }
}

impl AudioNode for PluginPlaceholder {
    fn tail_frames(&self) -> u32 {
        0
    }

    /// At rest whatever its ring holds: the host keeps a slot awake until it
    /// has seen silence for its `dry_path_latency_frames`, which is the ring.
    fn is_at_rest(&self) -> bool {
        true
    }

    /// Zero for a plain placeholder, and `EffectKind::Plugin::latency_frames`
    /// agrees: a pass-through adds nothing. A restart's stands in for the
    /// plugin's own.
    fn latency_frames(&self) -> u32 {
        self.delay.as_ref().map_or(0, |delay| delay.frames() as u32)
    }

    fn on_discontinuity(&mut self, kind: Discontinuity) {
        if kind.invalidates_tails() {
            if let Some(delay) = self.delay.as_mut() {
                delay.reset();
            }
        }
    }

    /// Parameter events are dropped: they name the plugin's parameters, and
    /// there is no plugin here to take them.
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        _events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        if let Some(delay) = self.delay.as_mut() {
            let frames = ctx.frames.min(bus.l.len()).min(bus.r.len());
            delay.process(&mut bus.l[..frames], &mut bus.r[..frames]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_placeholder_adds_no_latency() {
        assert_eq!(PluginPlaceholder::new(PluginSlotId(1)).latency_frames(), 0);
        assert_eq!(PluginPlaceholder::with_latency(PluginSlotId(1), 0).latency_frames(), 0);
    }

    #[test]
    fn a_restarts_placeholder_is_the_plugins_latency_late_and_otherwise_dry() {
        let mut placeholder = PluginPlaceholder::with_latency(PluginSlotId(1), 3);
        assert_eq!(placeholder.latency_frames(), 3);
        assert_eq!(placeholder.dry_path_latency_frames(), 3);
        let mut bus = StereoBus::with_capacity(6);
        bus.l[..6].copy_from_slice(&[1.0, 0.5, 0.0, 0.0, 0.0, 0.0]);
        bus.r[..6].copy_from_slice(&[-1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let ctx = ProcessContext {
            sample_rate: 48_000,
            frames: 6,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        };
        placeholder.process(&ctx, &mut bus, &EventList::empty(), None);
        assert_eq!(bus.l[..6], [0.0, 0.0, 0.0, 1.0, 0.5, 0.0]);
        assert_eq!(bus.r[..6], [0.0, 0.0, 0.0, -1.0, 0.0, 0.0]);
    }
}
