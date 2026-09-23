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

use mooloop_core::PluginSlotId;

use crate::bus::StereoBus;
use crate::event::EventList;
use crate::node::{AudioNode, ProcessContext};

/// A plugin slot's occupant with no plugin in it: transparent and stateless.
pub struct PluginPlaceholder {
    slot: PluginSlotId,
}

impl PluginPlaceholder {
    pub fn new(slot: PluginSlotId) -> Self {
        Self { slot }
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

    fn is_at_rest(&self) -> bool {
        true
    }

    /// Zero, and `EffectKind::Plugin::latency_frames` agrees: a
    /// pass-through adds nothing. The real processor reports its own.
    fn latency_frames(&self) -> u32 {
        0
    }

    /// Parameter events are dropped: they name the plugin's parameters, and
    /// there is no plugin here to take them.
    fn process(
        &mut self,
        _ctx: &ProcessContext,
        _bus: &mut StereoBus,
        _events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
    }
}
