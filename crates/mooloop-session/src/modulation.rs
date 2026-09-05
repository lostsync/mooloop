//! Modulation-shelf edits: sources, routes, and the assignment gesture.
//!
//! All of these act on the selected channel's `ModRack`. What they have in
//! common, and what makes them worth stating once, is that selection and
//! arming follow a *module* rather than the slot it happens to occupy -- a
//! reorder that retargeted the assignment gesture would be silent and wrong.
//!
//! A channel has a second kind of source beside its rack: the control outlets
//! its generator publishes. Those live in the upper half of the same flat
//! slot space (`mooloop_core::modulation`), which is what lets one selection,
//! one arming, and one assignment gesture serve both. What differs is that an
//! outlet is not owned by the rack -- nothing mints it, nothing reorders it,
//! and it cannot be removed -- so the places below that ask "does this slot
//! still name something" ask [`Session::control_source_exists`] rather than
//! looking in the rack.

use crate::session::Session;
use mooloop_core::modulation::outlet_of_slot;
use mooloop_core::{
    EngineCommand, ModPolarity, ModSourceId, ModulatorKind, ModulatorParams, OutletDescriptor,
    PublishesOutlets,
};

impl Session {
    /// The selected channel's modulation rack, when there is one.
    fn rack_mut(&mut self) -> Option<&mut mooloop_core::ModRack> {
        let selected = self.selected;
        Some(&mut self.channels.get_mut(selected)?.modulation)
    }

    /// The control outlet `slot` names on the selected channel, if its
    /// generator publishes one there.
    ///
    /// Returns `None` for a rack slot, for an outlet band this generator
    /// leaves empty, and for the unresolved slot -- so an ML-P8 route that
    /// survives the channel becoming a sampler reads as naming nothing,
    /// which is the same fate a route to a departed module gets.
    pub fn selected_channel_outlet(&self, slot: u8) -> Option<&'static OutletDescriptor> {
        let channel = self.channels.get(self.selected)?;
        channel.kind.control_outlet(outlet_of_slot(slot)?)
    }

    /// Whether `slot` names a source the selected channel actually has: an
    /// occupied rack slot, or a published control outlet.
    pub fn control_source_exists(&self, slot: u8) -> bool {
        self.channels
            .get(self.selected)
            .is_some_and(|channel| channel.modulation.params(slot as usize).is_some())
            || self.selected_channel_outlet(slot).is_some()
    }

    /// What the shelf and the assignment badge call the source in `slot`.
    ///
    /// A module is named by its badge and its slot number, the way it has
    /// always been; an outlet is named by the outlet, because its number is a
    /// device-interface id rather than a position and showing it would invite
    /// somebody to count from it.
    pub fn control_source_name(&self, slot: u8) -> Option<String> {
        if let Some(outlet) = self.selected_channel_outlet(slot) {
            return Some(outlet.name.to_string());
        }
        let params = self.channels.get(self.selected)?.modulation.params(slot as usize)?;
        Some(format!("{} {}", params.kind().badge(), slot + 1))
    }

    /// The command that reinstalls one modulator slot whole.
    ///
    /// Used wherever the change is not addressable by descriptor id -- an
    /// envelope's gate is a jack, not a parameter -- so the module travels
    /// entire rather than as a parameter update.
    pub fn install_modulator_command(&self, slot: usize) -> Option<EngineCommand> {
        let rack = self.channels.get(self.selected)?.modulation;
        let (source, params) = (rack.source_id(slot)?, rack.params(slot)?);
        Some(EngineCommand::InstallModulator {
            channel: self.selected as u8,
            slot: slot as u8,
            source,
            params,
        })
    }

    /// Whether a direct modulation-knob gesture is currently open.
    pub fn modulation_gesture_open(&self) -> bool {
        self.modulation_edit_before.is_some()
    }

    /// Opens or closes the modulation shelf.
    pub fn toggle_modulation_shelf(&mut self) {
        self.modulation_shelf_open = !self.modulation_shelf_open;
    }

    /// Opens a source's editor in the shelf.
    ///
    /// Selection is separate from assignment, so looking at an LFO does not
    /// hijack knob gestures throughout the rack. If assignment is already
    /// active it follows the newly selected source; otherwise this has no
    /// effect on ordinary parameter edits.
    ///
    /// A published outlet is selectable on the same terms as a module. It has
    /// no editor -- the device that publishes it owns its behaviour, and the
    /// shelf shows its declaration instead -- but it is armable, which is the
    /// whole point of it appearing in the picker.
    pub fn select_modulation_source(&mut self, slot: i32) -> bool {
        let Ok(slot) = u8::try_from(slot) else {
            return false;
        };
        if !self.control_source_exists(slot) {
            return false;
        }
        self.modulation_selected_slot.set(Some(slot));
        if self.modulation_armed_slot.get().is_some() {
            self.modulation_armed_slot.set(Some(slot));
        }
        self.modulation_shelf_open = true;
        true
    }

    /// Reorders two modulator slots.
    ///
    /// Selection and arming are re-derived from the modules' durable ids, so
    /// they follow the module rather than the slot number. Both racks run the
    /// same permutation, so the engine's copy carries routes and a math
    /// module's input slot across the move exactly as this one does.
    pub fn move_modulation_source(&mut self, slot: i32, target: i32) -> Option<EngineCommand> {
        let (slot, target) = (usize::try_from(slot).ok()?, usize::try_from(target).ok()?);
        let channel = self.selected;
        let selected_slot = self.modulation_selected_slot.get();
        let armed_slot = self.modulation_armed_slot.get();
        let rack = self.rack_mut()?;
        let source_of =
            |rack: &mooloop_core::ModRack, slot: Option<u8>| {
                slot.and_then(|slot| rack.source_id(slot as usize))
            };
        let selected_id = source_of(rack, selected_slot);
        let armed_id = source_of(rack, armed_slot);
        if !rack.move_module(slot, target) {
            return None;
        }
        // A module follows its durable id across the move. An outlet is not
        // in the rack, does not move, and keeps the slot it had -- otherwise
        // reordering two LFOs would silently disarm a Trigger the gesture was
        // pointed at, which is the same silent retargeting the id lookup
        // exists to prevent.
        let follow = |rack: &mooloop_core::ModRack, was: Option<u8>, id: Option<ModSourceId>| match was {
            Some(slot) if outlet_of_slot(slot).is_some() => Some(slot),
            _ => id.and_then(|id| rack.slot_of(id)),
        };
        let next_selected = follow(rack, selected_slot, selected_id);
        let next_armed = follow(rack, armed_slot, armed_id);
        self.modulation_selected_slot.set(next_selected);
        self.modulation_armed_slot.set(next_armed);
        Some(EngineCommand::MoveModulator {
            channel: channel as u8,
            from: slot as u8,
            to: target as u8,
        })
    }

    /// Arms or disarms the assignment gesture.
    ///
    /// Returns the armed source's badge, or `None` when assignment is now off
    /// -- which is also the answer when nothing was selected to arm.
    pub fn toggle_modulation_assignment(&mut self) -> Option<String> {
        let next = if self.modulation_armed_slot.get().is_some() {
            None
        } else {
            self.modulation_selected_slot.get()
        };
        self.modulation_armed_slot.set(next);
        self.modulation_shelf_open = true;
        self.control_source_name(next?)
    }

    /// Installs a new modulator in the first free slot.
    pub fn add_modulation_source(&mut self, kind: ModulatorKind) -> Option<EngineCommand> {
        let selected = self.selected;
        let rack = self.rack_mut()?;
        let slot = rack.free_slot()?;
        let mut params = kind.default_params();
        // The envelope's gate is a jack rather than a descriptor id, so its
        // only sensible default is set here.
        if let ModulatorParams::Envelope(envelope) = &mut params {
            envelope.input_channel = selected as u8;
        }
        rack.install(slot, params);
        self.modulation_selected_slot.set(Some(slot as u8));
        self.modulation_armed_slot.set(None);
        self.modulation_shelf_open = true;
        self.install_modulator_command(slot)
    }

    /// Sets one parameter of one modulator, or `None` when nothing moved.
    ///
    /// A change inside an open knob gesture is marked so the gesture's own
    /// undo entry is recorded on release rather than one per frame.
    pub fn set_modulator_param(&mut self, slot: i32, id: i32, value: f32) -> Option<EngineCommand> {
        let (slot, id) = (usize::try_from(slot).ok()?, u32::try_from(id).ok()?);
        let channel = self.selected;
        let in_gesture = self.modulation_gesture_open();
        let params = self.rack_mut()?.params_mut(slot)?;
        let previous = params.get(id);
        params.set(id, value);
        if params.get(id) == previous {
            return None;
        }
        if in_gesture {
            self.modulation_edit_changed = true;
        }
        Some(EngineCommand::SetModulatorParam {
            channel: channel as u8,
            slot: slot as u8,
            id,
            value,
        })
    }

    /// Removes a modulator and everything routed from it.
    ///
    /// The rack drops the module's routes by identity, so a route aimed at a
    /// different module that later occupied the same slot cannot be caught up
    /// in the removal.
    pub fn remove_modulation_source(&mut self, slot: i32) -> Option<EngineCommand> {
        let slot = u8::try_from(slot).ok()?;
        let channel = self.selected;
        if !self.rack_mut()?.clear(slot as usize) {
            return None;
        }
        if self.modulation_selected_slot.get() == Some(slot) {
            self.modulation_selected_slot.set(None);
        }
        if self.modulation_armed_slot.get() == Some(slot) {
            self.modulation_armed_slot.set(None);
        }
        Some(EngineCommand::ClearModulator {
            channel: channel as u8,
            slot,
        })
    }

    /// Points an envelope's gate at a channel.
    pub fn set_envelope_input_channel(&mut self, slot: i32, channel: i32) -> Option<EngineCommand> {
        let slot = usize::try_from(slot).ok()?;
        let channel = u8::try_from(channel).ok()?;
        if channel as usize >= self.channels.len() {
            return None;
        }
        self.modulation_envelope_mut(slot)?.input_channel = channel;
        self.install_modulator_command(slot)
    }

    /// Sets a route's polarity, or `None` when it is already that.
    pub fn set_route_polarity(&mut self, index: i32, polarity: i32) -> Option<EngineCommand> {
        let index = usize::try_from(index).ok()?;
        let channel = self.selected;
        let route = self
            .rack_mut()?
            .routes
            .get_mut(index)
            .and_then(Option::as_mut)?;
        let next = if polarity == 1 {
            ModPolarity::Unipolar
        } else {
            ModPolarity::Bipolar
        };
        if route.polarity == next {
            return None;
        }
        route.polarity = next;
        Some(EngineCommand::SetModRoute {
            channel: channel as u8,
            route: *route,
        })
    }

    /// Removes one route.
    ///
    /// The row's durable source is read before it is taken, so the engine is
    /// told which assignment ended rather than which matrix position emptied,
    /// and the two racks cannot drift into removing different routes.
    pub fn remove_route(&mut self, index: i32) -> Option<EngineCommand> {
        let index = usize::try_from(index).ok()?;
        let channel = self.selected;
        let removed = self.rack_mut()?.routes.get_mut(index)?.take()?;
        Some(EngineCommand::RemoveModRoute {
            channel: channel as u8,
            source: removed.source,
            destination: removed.destination,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::modulation::outlet_slot;
    use mooloop_core::{EffectTarget, ModRoute, ParamAddr, STRIP_PARAM_VOLUME};

    fn armed_lfo() -> Session {
        let mut session = Session::default();
        session
            .add_modulation_source(ModulatorKind::Lfo)
            .expect("an empty rack has a free slot");
        session
    }

    /// The reason selection and arming are re-derived by id: a reorder that
    /// left them pointing at slot numbers would silently retarget the
    /// assignment gesture at whatever moved into that slot.
    #[test]
    fn reordering_sources_carries_selection_and_arming_with_the_module() {
        let mut session = armed_lfo();
        session
            .add_modulation_source(ModulatorKind::Envelope)
            .expect("rack has room");
        // Slot 1 holds the envelope and is both selected and armed.
        assert_eq!(session.modulation_selected_slot.get(), Some(1));
        session.toggle_modulation_assignment();
        assert_eq!(session.modulation_armed_slot.get(), Some(1));

        session
            .move_modulation_source(1, 0)
            .expect("both slots are occupied");

        assert_eq!(
            session.modulation_selected_slot.get(),
            Some(0),
            "selection stayed on the slot instead of following the module"
        );
        assert_eq!(session.modulation_armed_slot.get(), Some(0));
        assert!(matches!(
            session.channels[0].modulation.params(0),
            Some(ModulatorParams::Envelope(_))
        ));
    }

    /// A new envelope's gate defaults to the channel it was added on, because
    /// the gate is a jack and has no descriptor default to fall back on.
    #[test]
    fn a_new_envelope_gates_from_its_own_channel() {
        let mut session = Session::default();
        session.add_channel(mooloop_core::DeviceKind::Sampler);
        assert_eq!(session.selected, 1);

        session
            .add_modulation_source(ModulatorKind::Envelope)
            .expect("rack has room");

        let Some(ModulatorParams::Envelope(envelope)) = session.channels[1].modulation.params(0)
        else {
            panic!("the envelope was not installed");
        };
        assert_eq!(envelope.input_channel, 1);

        // A gate pointed at a channel that does not exist is refused.
        assert!(session.set_envelope_input_channel(0, 9).is_none());
        assert!(session.set_envelope_input_channel(0, 0).is_some());
    }

    /// Arming toggles, and reports the badge the status bar names.
    #[test]
    fn assignment_arms_the_selected_source_and_disarms_on_the_second_press() {
        let mut session = armed_lfo();

        let armed = session
            .toggle_modulation_assignment()
            .expect("a source is selected");
        assert!(armed.ends_with(" 1"), "{armed}");
        assert_eq!(session.modulation_armed_slot.get(), Some(0));

        assert_eq!(session.toggle_modulation_assignment(), None);
        assert_eq!(session.modulation_armed_slot.get(), None);
    }

    /// A parameter set to the value it already holds is not an edit, so it
    /// must not reach the engine or the undo history.
    #[test]
    fn setting_a_parameter_to_what_it_already_is_reports_nothing() {
        let mut session = armed_lfo();
        let id = 0;

        let first = session.set_modulator_param(0, id, 0.25);
        assert!(first.is_some());
        assert!(session.set_modulator_param(0, id, 0.25).is_none());

        assert!(session.set_modulator_param(9, id, 0.5).is_none());
        assert!(session.set_modulator_param(-1, id, 0.5).is_none());
    }

    /// Removing a source clears the selection and arming that pointed at it,
    /// and takes its routes with it.
    #[test]
    fn removing_a_source_disarms_it_and_drops_its_routes() {
        let mut session = armed_lfo();
        session.toggle_modulation_assignment();
        let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        session.channels[0]
            .modulation
            .add_route(ModRoute::to_slot(0, destination, 0.5, ModPolarity::Bipolar))
            .expect("the matrix is empty");

        session
            .remove_modulation_source(0)
            .expect("slot 0 is occupied");

        assert_eq!(session.modulation_selected_slot.get(), None);
        assert_eq!(session.modulation_armed_slot.get(), None);
        assert!(session.channels[0]
            .modulation
            .routes
            .iter()
            .all(Option::is_none));
        assert!(session.remove_modulation_source(0).is_none());
    }

    /// An ML-P8 channel, selected, with its outlet band available.
    fn mlp8_channel() -> Session {
        let mut session = Session::default();
        session.reset_channel_source(0, mooloop_core::DeviceKind::MlP8);
        session
    }

    /// The whole point of the picker: an outlet is selectable and armable on
    /// the same terms as a module, and it is named by the outlet rather than
    /// by the slot it occupies in the flat address space.
    #[test]
    fn a_published_outlet_selects_and_arms_like_a_module() {
        let mut session = mlp8_channel();
        let gate = outlet_slot(mooloop_core::mlp8::OUTLET_GATE);

        assert!(session.select_modulation_source(gate.into()));
        assert_eq!(session.modulation_selected_slot.get(), Some(gate));
        assert_eq!(
            session.toggle_modulation_assignment().as_deref(),
            Some("Gate"),
            "the badge showed a slot number instead of the outlet"
        );
        assert_eq!(session.modulation_armed_slot.get(), Some(gate));
    }

    /// An outlet belongs to whichever generator the channel holds. A sampler
    /// publishes nothing, so the band is not there to select from -- the same
    /// answer an empty rack slot gives.
    #[test]
    fn a_generator_that_publishes_nothing_offers_no_outlets() {
        let mut session = Session::default();
        assert_eq!(session.channels[0].kind, mooloop_core::DeviceKind::Sampler);
        for outlet in 0..mooloop_core::modulation::MAX_GENERATOR_OUTLETS as u16 {
            let slot = outlet_slot(outlet);
            assert!(!session.control_source_exists(slot));
            assert!(!session.select_modulation_source(slot.into()));
        }
        assert_eq!(session.modulation_selected_slot.get(), None);

        // Nor does an audio outlet become selectable by living in the same
        // table as the control ones: ML-P8 publishes fourteen and offers
        // seven, and `Osc 1` is above the band a route can name.
        let mut mlp8 = mlp8_channel();
        let osc = outlet_slot(mooloop_core::mlp8::OUTLET_OSC1);
        assert!(!mlp8.control_source_exists(osc));
        assert!(!mlp8.select_modulation_source(osc.into()));
    }

    /// The gesture authors an outlet route by its durable outlet id, not by
    /// the slot it was armed from, and takes its polarity from the outlet's
    /// declared shape -- a Gate that defaulted to bipolar would rest half a
    /// depth below the base value with nothing playing.
    #[test]
    fn assigning_from_an_outlet_authors_a_durable_outlet_route() {
        let mut session = mlp8_channel();
        let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);

        session.select_modulation_source(outlet_slot(mooloop_core::mlp8::OUTLET_GATE).into());
        session.toggle_modulation_assignment();
        let crate::session::ArmedRoute::Added(gate) =
            session.arm_modulation_route(destination, 0.4)
        else {
            panic!("the armed outlet did not author a route");
        };
        assert_eq!(
            gate.source,
            mooloop_core::ModSourceRef::GeneratorOutlet(mooloop_core::mlp8::OUTLET_GATE)
        );
        assert_eq!(gate.source_slot, outlet_slot(mooloop_core::mlp8::OUTLET_GATE));
        assert_eq!(gate.polarity, ModPolarity::Unipolar);

        // The LFO outlet swings both ways and says so, so it keeps the
        // destination's own default.
        session.select_modulation_source(outlet_slot(mooloop_core::mlp8::OUTLET_LFO).into());
        let crate::session::ArmedRoute::Added(lfo) =
            session.arm_modulation_route(destination, 0.4)
        else {
            panic!("selecting a second outlet did not follow the arming");
        };
        assert_eq!(lfo.polarity, ModPolarity::Bipolar);

        // Two rows, one per outlet: an outlet route dedupes on its source the
        // way a module's does, rather than stacking.
        session.arm_modulation_route(destination, 0.6);
        let rows = session.channels[0].modulation.routes.iter().flatten().count();
        assert_eq!(rows, 2);
    }

    /// An outlet belongs to the generator, not to the channel. Swapping the
    /// device out from under an armed gesture leaves a slot naming nothing,
    /// and the gesture has to say so -- reporting a full matrix instead would
    /// send the user looking for routes to delete.
    #[test]
    fn swapping_the_generator_disarms_rather_than_misreporting() {
        let mut session = mlp8_channel();
        let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        session.select_modulation_source(outlet_slot(mooloop_core::mlp8::OUTLET_GATE).into());
        session.toggle_modulation_assignment();

        session.reset_channel_source(0, mooloop_core::DeviceKind::Sampler);

        assert!(matches!(
            session.arm_modulation_route(destination, 0.4),
            crate::session::ArmedRoute::Unchanged
        ));
        assert!(session.channels[0]
            .modulation
            .routes
            .iter()
            .all(Option::is_none));
    }

    /// A reorder moves modules. An outlet is not in the rack and does not
    /// move, so a gesture armed on one has to survive a drag it had nothing
    /// to do with.
    #[test]
    fn reordering_modules_leaves_an_armed_outlet_alone() {
        let mut session = mlp8_channel();
        session
            .add_modulation_source(ModulatorKind::Lfo)
            .expect("an empty rack has a free slot");
        session
            .add_modulation_source(ModulatorKind::Envelope)
            .expect("rack has room");
        let trigger = outlet_slot(mooloop_core::mlp8::OUTLET_TRIGGER);
        session.select_modulation_source(trigger.into());
        session.toggle_modulation_assignment();

        session
            .move_modulation_source(1, 0)
            .expect("both slots are occupied");

        assert_eq!(session.modulation_selected_slot.get(), Some(trigger));
        assert_eq!(session.modulation_armed_slot.get(), Some(trigger));
    }
}
