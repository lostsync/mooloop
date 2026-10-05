//! Modulation edits: sources, routes, and the assignment gesture.
//!
//! The song owns the modulation set (`docs/plans/song-modulation/`), and
//! these verbs edit it. Until the engine runs the song's set itself (step
//! 02), it still runs one rack per channel, built from the set by
//! [`mooloop_core::SongModulation::channel_rack`]; the shelf shows the
//! selected channel's, and addresses a module by its slot there. So every
//! verb here is the same three moves: build the racks, edit the selected one
//! as a rack, write it back to the song, and tell the engine what changed in
//! **every** channel's rack -- a module can sit in more than one, as a guest
//! where a pasted channel's routes reach it.
//!
//! Selection and arming name a source -- a module by its durable id, an
//! outlet or the keyboard by its channel -- never a slot, so a reorder cannot
//! retarget the assignment gesture and a channel change does not lose it.
//!
//! A channel has a second kind of source beside its modules: the control
//! outlets its generator publishes. Those live in the upper half of the same
//! flat slot space (`mooloop_core::modulation`), which is what lets one
//! selection, one arming, and one assignment gesture serve both. What differs
//! is that an outlet is not a module -- nothing mints it, nothing reorders
//! it, and it cannot be removed -- so the places below that ask "does this
//! slot still name something" ask [`Session::control_source_exists`] rather
//! than looking in the rack.

use crate::session::Session;
use mooloop_core::modulation::{
    outlet_of_slot, performance_descriptor, performance_of_slot, MAX_MODULATORS_PER_CHANNEL,
};
use mooloop_core::{
    EngineCommand, InputSource, ModPolarity, ModRack, ModSourceId, ModSourceRef, ModulatorKind,
    ModulatorParams, OutletDescriptor, PublishesOutlets,
};

/// What the engine has to be told so its rack for `channel` goes from
/// `before` to `after`: modules first, so a route never arrives ahead of the
/// module it names, then the routes that went, then the ones that came or
/// changed.
fn rack_commands(channel: u8, before: &ModRack, after: &ModRack) -> Vec<EngineCommand> {
    let mut commands = Vec::new();
    for slot in 0..MAX_MODULATORS_PER_CHANNEL {
        let (was, now) = (before.slots[slot], after.slots[slot]);
        if was == now {
            continue;
        }
        commands.push(match now {
            None => EngineCommand::ClearModulator {
                channel,
                slot: slot as u8,
            },
            Some(entry) => EngineCommand::InstallModulator {
                channel,
                slot: slot as u8,
                source: entry.id,
                params: entry.params,
            },
        });
    }
    let key = |route: &mooloop_core::ModRoute| (route.source, route.destination);
    for route in before.routes.iter().flatten() {
        if !after.routes.iter().flatten().any(|kept| key(kept) == key(route)) {
            commands.push(EngineCommand::RemoveModRoute {
                channel,
                source: route.source,
                destination: route.destination,
            });
        }
    }
    for route in after.routes.iter().flatten() {
        if !before.routes.iter().flatten().any(|held| held == route) {
            commands.push(EngineCommand::SetModRoute {
                channel,
                route: *route,
            });
        }
    }
    commands
}

impl Session {
    /// The address of the selected channel's generator parameter `param`, as
    /// the device the channel runs now -- which is what a press on the
    /// source face means. `None` when nothing is selected.
    ///
    /// The one place the window builds a generator address from a face
    /// callback, so the kind cannot be left out of one (MOO-135).
    pub fn selected_source_address(&self, param: u32) -> Option<mooloop_core::ParamAddr> {
        let state = self.channels.get(self.selected)?;
        Some(mooloop_core::ParamAddr::source(
            mooloop_core::EffectTarget::Channel(self.selected as u8),
            state.kind(),
            param,
        ))
    }

    /// The rack the engine runs for the channel at `seat`, built from the
    /// song's set. The shim song-modulation step 02 removes.
    pub fn channel_rack(&self, seat: usize) -> ModRack {
        let Some(channel) = self.channels.get(seat) else {
            return ModRack::default();
        };
        self.modulation.channel_rack(channel.id, seat as u8, |id| {
            self.channel_index(id).and_then(|seat| u8::try_from(seat).ok())
        })
    }

    /// The selected channel's rack, as the shelf shows it.
    pub fn selected_rack(&self) -> ModRack {
        self.channel_rack(self.selected)
    }

    fn all_racks(&self) -> Vec<ModRack> {
        (0..self.channels.len()).map(|seat| self.channel_rack(seat)).collect()
    }

    /// What changed between two sets of racks, as commands.
    fn racks_commands(before: &[ModRack], after: &[ModRack]) -> Vec<EngineCommand> {
        before
            .iter()
            .zip(after)
            .enumerate()
            .flat_map(|(seat, (before, after))| rack_commands(seat as u8, before, after))
            .collect()
    }

    /// Edit the selected channel's rack as a rack, write it back into the
    /// song's set, and return what `edit` returned with the commands that
    /// bring the engine along. `edit` returning `None` changes nothing.
    pub fn edit_selected_rack<R>(
        &mut self,
        edit: impl FnOnce(&mut ModRack) -> Option<R>,
    ) -> Option<(R, Vec<EngineCommand>)> {
        self.edit_channel_rack(self.selected, edit)
    }

    /// [`Self::edit_selected_rack`] for the channel at `seat`.
    pub fn edit_channel_rack<R>(
        &mut self,
        seat: usize,
        edit: impl FnOnce(&mut ModRack) -> Option<R>,
    ) -> Option<(R, Vec<EngineCommand>)> {
        let channel = self.channels.get(seat)?;
        let (id, name) = (channel.id, channel.name.clone());
        let before_all = self.all_racks();
        let before = before_all[seat];
        let mut after = before;
        let result = edit(&mut after)?;
        self.modulation.store_channel_rack(id, &name, &before, &after);
        let commands = Self::racks_commands(&before_all, &self.all_racks());
        Some((result, commands))
    }

    /// Edit the song's set directly, returning the commands that bring every
    /// channel's rack along.
    fn edit_song_modulation(
        &mut self,
        edit: impl FnOnce(&mut mooloop_core::SongModulation) -> bool,
    ) -> Vec<EngineCommand> {
        let before = self.all_racks();
        if !edit(&mut self.modulation) {
            return Vec::new();
        }
        Self::racks_commands(&before, &self.all_racks())
    }

    /// The source a slot of the selected channel's rack names: a module by
    /// its id, an outlet or the keyboard by this channel.
    fn source_at(&self, slot: u8) -> Option<ModSourceRef> {
        let channel = self.channel_id(self.selected)?;
        if let Some(source) = performance_of_slot(slot) {
            return Some(ModSourceRef::Performance { channel, source });
        }
        if let Some(outlet) = outlet_of_slot(slot) {
            return Some(ModSourceRef::GeneratorOutlet { channel, outlet });
        }
        self.selected_rack()
            .source_id(slot as usize)
            .map(ModSourceRef::Id)
    }

    /// Where `source` sits in the selected channel's rack, if it is there.
    fn slot_of_source(&self, source: ModSourceRef) -> Option<u8> {
        match source {
            ModSourceRef::Id(id) => self.selected_rack().slot_of(id),
            ModSourceRef::GeneratorOutlet { channel, outlet } => (Some(channel)
                == self.channel_id(self.selected))
            .then(|| mooloop_core::modulation::outlet_slot(outlet)),
            ModSourceRef::Performance { channel, source } => (Some(channel)
                == self.channel_id(self.selected))
            .then(|| mooloop_core::modulation::performance_slot(source)),
            ModSourceRef::LocalSlot(_) => None,
        }
    }

    /// The selected source's slot in the selected channel's rack. `None`
    /// when nothing is selected, or what is selected is not on this channel.
    pub fn modulation_selected_slot(&self) -> Option<u8> {
        self.modulation_selected
            .get()
            .and_then(|source| self.slot_of_source(source))
    }

    /// The armed source's slot in the selected channel's rack, as
    /// [`Self::modulation_selected_slot`].
    pub fn modulation_armed_slot(&self) -> Option<u8> {
        self.modulation_armed
            .get()
            .and_then(|source| self.slot_of_source(source))
    }

    /// Select the source in `slot` of the selected channel's rack, or clear
    /// the selection.
    pub fn set_modulation_selected_slot(&self, slot: Option<u8>) {
        self.modulation_selected
            .set(slot.and_then(|slot| self.source_at(slot)));
    }

    /// Arm the source in `slot` of the selected channel's rack, or disarm.
    pub fn set_modulation_armed_slot(&self, slot: Option<u8>) {
        self.modulation_armed
            .set(slot.and_then(|slot| self.source_at(slot)));
    }

    /// The control outlet `slot` names on the selected channel, if its
    /// generator publishes one there.
    ///
    /// Returns `None` for a rack slot, for an outlet band this generator
    /// leaves empty, and for the unresolved slot -- so an ML-P8 route that
    /// survives the channel becoming a sampler reads as naming nothing,
    /// which is the same fate a route to a departed module gets.
    ///
    /// The performance band -- the keyboard's mod wheel and aftertouch
    /// (MOO-128) -- answers here too, with its own declarations: it is a
    /// source that is neither a module nor editable, which is exactly what
    /// the outlet half of the shelf already knows how to show and arm.
    pub fn selected_channel_outlet(&self, slot: u8) -> Option<&'static OutletDescriptor> {
        let channel = self.channels.get(self.selected)?;
        if let Some(performance) = performance_descriptor(slot) {
            return Some(performance);
        }
        channel.kind().control_outlet(outlet_of_slot(slot)?)
    }

    /// Whether `slot` names a source the selected channel actually has: an
    /// occupied rack slot, or a published control outlet.
    pub fn control_source_exists(&self, slot: u8) -> bool {
        self.selected_rack().params(slot as usize).is_some()
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
        let params = self.selected_rack().params(slot as usize)?;
        Some(format!("{} {}", params.kind().badge(), slot + 1))
    }

    /// Whether a direct modulation-knob gesture is currently open.
    pub fn gesture_open(&self) -> bool {
        self.gesture_before.is_some()
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
        self.set_modulation_selected_slot(Some(slot));
        if self.modulation_armed.get().is_some() {
            self.set_modulation_armed_slot(Some(slot));
        }
        self.modulation_shelf_open = true;
        true
    }

    /// Reorders two modulator slots.
    ///
    /// Selection and arming name modules by durable id, so they follow the
    /// module rather than the slot number. The engine runs the same
    /// permutation, so its copy carries routes, a math module's input slot,
    /// and every module's running state across the move exactly as this one
    /// does.
    pub fn move_modulation_source(&mut self, slot: i32, target: i32) -> Vec<EngineCommand> {
        let (Ok(slot), Ok(target)) = (usize::try_from(slot), usize::try_from(target)) else {
            return Vec::new();
        };
        let Some(channel) = self.channels.get(self.selected) else {
            return Vec::new();
        };
        let (id, name) = (channel.id, channel.name.clone());
        let mut before_all = self.all_racks();
        let before = before_all[self.selected];
        let mut after = before;
        if !after.move_module(slot, target) {
            return Vec::new();
        }
        self.modulation.store_channel_rack(id, &name, &before, &after);
        // The engine runs the move itself, so what is left to diff is
        // everything else the move touched: a guest seated by slot number
        // elsewhere, which nothing a user does in this step makes.
        let mut commands = vec![EngineCommand::MoveModulator {
            channel: self.selected as u8,
            from: slot as u8,
            to: target as u8,
        }];
        before_all[self.selected].move_module(slot, target);
        commands.extend(Self::racks_commands(&before_all, &self.all_racks()));
        commands
    }

    /// Arms or disarms the assignment gesture.
    ///
    /// Returns the armed source's badge, or `None` when assignment is now off
    /// -- which is also the answer when nothing was selected to arm.
    pub fn toggle_modulation_assignment(&mut self) -> Option<String> {
        let next = if self.modulation_armed.get().is_some() {
            None
        } else {
            self.modulation_selected.get()
        };
        self.modulation_armed.set(next);
        self.modulation_shelf_open = true;
        self.control_source_name(self.modulation_armed_slot()?)
    }

    /// Adds a new module to the song, on the selected channel's rack.
    ///
    /// Its input defaults to the selected channel's notes, so an Envelope
    /// added with a channel selected gates from that channel as it always
    /// did, and an LFO retriggers from it.
    pub fn add_modulation_source(&mut self, kind: ModulatorKind) -> Vec<EngineCommand> {
        let selected = self.selected;
        let Some(selected_id) = self.channel_id(selected) else {
            return Vec::new();
        };
        let added = self.edit_selected_rack(|rack| {
            let slot = rack.free_slot()?;
            let mut params = kind.default_params();
            // The envelope's gate is a jack rather than a descriptor id, so
            // its only sensible default is set here.
            if let ModulatorParams::Envelope(envelope) = &mut params {
                envelope.input_channel = selected as u8;
                envelope.input_channel_id = selected_id;
            }
            rack.install(slot, params).map(|id| (slot, id))
        });
        let Some(((slot, id), commands)) = added else {
            return Vec::new();
        };
        debug_assert!(self.modulation.module(id).is_some());
        self.set_modulation_selected_slot(Some(slot as u8));
        self.modulation_armed.set(None);
        self.modulation_shelf_open = true;
        commands
    }

    /// Sets one parameter of one modulator. Empty when nothing moved.
    ///
    /// A change inside an open knob gesture is marked so the gesture's own
    /// undo entry is recorded on release rather than one per frame. Sent as
    /// one fact to every rack the module sits in, rather than as a reinstall,
    /// because a knob drag sends one of these a frame.
    pub fn set_modulator_param(&mut self, slot: i32, id: i32, value: f32) -> Vec<EngineCommand> {
        let (Ok(slot), Ok(id)) = (usize::try_from(slot), u32::try_from(id)) else {
            return Vec::new();
        };
        let Some(source) = self.selected_rack().source_id(slot) else {
            return Vec::new();
        };
        let Some(module) = self.modulation.module_mut(source) else {
            return Vec::new();
        };
        let previous = module.params.get(id);
        module.params.set(id, value);
        if module.params.get(id) == previous {
            return Vec::new();
        }
        if self.gesture_open() {
            self.gesture_changed = true;
        }
        self.all_racks()
            .iter()
            .enumerate()
            .filter_map(|(seat, rack)| {
                Some(EngineCommand::SetModulatorParam {
                    channel: seat as u8,
                    slot: rack.slot_of(source)?,
                    id,
                    value,
                })
            })
            .collect()
    }

    /// Removes a module from the song and everything routed from it.
    pub fn remove_modulation_source(&mut self, slot: i32) -> Vec<EngineCommand> {
        let Ok(slot) = u8::try_from(slot) else {
            return Vec::new();
        };
        let Some(source) = self.selected_rack().source_id(slot as usize) else {
            return Vec::new();
        };
        let commands = self.edit_song_modulation(|modulation| modulation.remove_module(source));
        if commands.is_empty() {
            return commands;
        }
        let gone = Some(ModSourceRef::Id(source));
        if self.modulation_selected.get() == gone {
            self.modulation_selected.set(None);
        }
        if self.modulation_armed.get() == gone {
            self.modulation_armed.set(None);
        }
        commands
    }

    /// Points a module's note input at an outlet: the Envelope's gate, the
    /// LFO's retrigger, the Step's advance, the Random's trigger.
    ///
    /// Replaces the Envelope-only gate picker. A Math module reads another
    /// module, not notes, and takes none; nor does an input naming a channel
    /// the song does not have.
    pub fn set_module_input(&mut self, id: ModSourceId, input: InputSource) -> Vec<EngineCommand> {
        if input
            .channel()
            .is_some_and(|channel| self.channel_index(channel).is_none())
        {
            return Vec::new();
        }
        self.edit_song_modulation(|modulation| {
            let Some(module) = modulation.module_mut(id) else {
                return false;
            };
            if matches!(module.params, ModulatorParams::Math(_)) || module.input == input {
                return false;
            }
            module.input = input;
            true
        })
    }

    /// The module in `slot` of the selected channel's rack, by identity.
    pub fn module_in_slot(&self, slot: i32) -> Option<ModSourceId> {
        self.selected_rack().source_id(usize::try_from(slot).ok()?)
    }

    /// Sets a route's polarity. Empty when it is already that.
    pub fn set_route_polarity(&mut self, index: i32, polarity: i32) -> Vec<EngineCommand> {
        let Ok(index) = usize::try_from(index) else {
            return Vec::new();
        };
        let next = if polarity == 1 {
            ModPolarity::Unipolar
        } else {
            ModPolarity::Bipolar
        };
        self.edit_selected_rack(|rack| {
            let route = rack.routes.get_mut(index)?.as_mut()?;
            if route.polarity == next {
                return None;
            }
            route.polarity = next;
            Some(())
        })
        .map(|((), commands)| commands)
        .unwrap_or_default()
    }

    /// Removes one route, by its source and destination rather than by its
    /// row, so every rack it is in lets go of the same one.
    pub fn remove_route(&mut self, index: i32) -> Vec<EngineCommand> {
        let Ok(index) = usize::try_from(index) else {
            return Vec::new();
        };
        let Some(removed) = self.selected_rack().routes.get(index).copied().flatten() else {
            return Vec::new();
        };
        self.edit_song_modulation(|modulation| {
            let before = modulation.routes.len();
            modulation.routes.retain(|route| {
                !(route.source == removed.source && route.destination == removed.destination)
            });
            modulation.routes.len() != before
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
        assert!(
            !session.add_modulation_source(ModulatorKind::Lfo).is_empty(),
            "an empty rack has a free slot"
        );
        session
    }

    /// The reason selection and arming are re-derived by id: a reorder that
    /// left them pointing at slot numbers would silently retarget the
    /// assignment gesture at whatever moved into that slot.
    #[test]
    fn reordering_sources_carries_selection_and_arming_with_the_module() {
        let mut session = armed_lfo();
        assert!(
            !session.add_modulation_source(ModulatorKind::Envelope).is_empty(),
            "rack has room"
        );
        // Slot 1 holds the envelope and is both selected and armed.
        assert_eq!(session.modulation_selected_slot(), Some(1));
        session.toggle_modulation_assignment();
        assert_eq!(session.modulation_armed_slot(), Some(1));

        assert!(
            !session.move_modulation_source(1, 0).is_empty(),
            "both slots are occupied"
        );

        assert_eq!(
            session.modulation_selected_slot(),
            Some(0),
            "selection stayed on the slot instead of following the module"
        );
        assert_eq!(session.modulation_armed_slot(), Some(0));
        assert!(matches!(
            session.channel_rack(0).params(0),
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

        assert!(
            !session.add_modulation_source(ModulatorKind::Envelope).is_empty(),
            "rack has room"
        );

        let Some(ModulatorParams::Envelope(envelope)) = session.channel_rack(1).params(0) else {
            panic!("the envelope was not installed");
        };
        assert_eq!(envelope.input_channel, 1);
        let id = session.module_in_slot(0).expect("slot 0 holds the envelope");
        assert_eq!(
            session.modulation.module(id).map(|module| module.input),
            session.channel_id(1).map(InputSource::ChannelNotes)
        );

        // A gate pointed at a channel the song does not have is refused.
        let stranger = mooloop_core::ChannelId(999);
        assert!(session
            .set_module_input(id, InputSource::ChannelNotes(stranger))
            .is_empty());
        let first = session.channel_id(0).expect("the song has a first channel");
        assert!(!session
            .set_module_input(id, InputSource::ChannelNotes(first))
            .is_empty());
        let Some(ModulatorParams::Envelope(envelope)) = session.channel_rack(1).params(0) else {
            panic!("the envelope left its channel");
        };
        assert_eq!(envelope.input_channel, 0);
    }

    /// Arming toggles, and reports the badge the status bar names.
    #[test]
    fn assignment_arms_the_selected_source_and_disarms_on_the_second_press() {
        let mut session = armed_lfo();

        let armed = session
            .toggle_modulation_assignment()
            .expect("a source is selected");
        assert!(armed.ends_with(" 1"), "{armed}");
        assert_eq!(session.modulation_armed_slot(), Some(0));

        assert_eq!(session.toggle_modulation_assignment(), None);
        assert_eq!(session.modulation_armed_slot(), None);
    }

    /// A parameter set to the value it already holds is not an edit, so it
    /// must not reach the engine or the undo history.
    #[test]
    fn setting_a_parameter_to_what_it_already_is_reports_nothing() {
        let mut session = armed_lfo();
        let id = 0;

        let first = session.set_modulator_param(0, id, 0.25);
        assert!(!first.is_empty());
        assert!(session.set_modulator_param(0, id, 0.25).is_empty());

        assert!(session.set_modulator_param(9, id, 0.5).is_empty());
        assert!(session.set_modulator_param(-1, id, 0.5).is_empty());
    }

    /// Removing a source clears the selection and arming that pointed at it,
    /// and takes its routes with it.
    #[test]
    fn removing_a_source_disarms_it_and_drops_its_routes() {
        let mut session = armed_lfo();
        session.toggle_modulation_assignment();
        let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        session
            .edit_selected_rack(|rack| {
                rack.add_route(ModRoute::to_slot(0, destination, 0.5, ModPolarity::Bipolar))
            })
            .expect("the matrix is empty");

        assert!(
            !session.remove_modulation_source(0).is_empty(),
            "slot 0 is occupied"
        );

        assert_eq!(session.modulation_selected_slot(), None);
        assert_eq!(session.modulation_armed_slot(), None);
        assert!(session.modulation.routes.is_empty());
        assert!(session.remove_modulation_source(0).is_empty());
    }

    /// The keyboard's mod wheel is a source on every channel, whatever its
    /// generator (MOO-128): selectable, armable, named for what it is, and
    /// authored as a durable performance route rather than as an outlet.
    #[test]
    fn the_mod_wheel_arms_and_authors_a_performance_route() {
        let mut session = Session::default();
        assert_eq!(session.channels[0].kind(), mooloop_core::DeviceKind::Sampler);
        let wheel = mooloop_core::modulation::performance_slot(
            mooloop_core::modulation::PERFORMANCE_MOD_WHEEL,
        );
        assert!(session.select_modulation_source(wheel.into()));
        assert_eq!(
            session.toggle_modulation_assignment().as_deref(),
            Some("Mod Wheel")
        );
        let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        let crate::session::ArmedRoute::Added(route) =
            session.arm_modulation_route(destination, 0.5)
        else {
            panic!("the armed wheel did not author a route");
        };
        assert_eq!(
            route.source,
            mooloop_core::ModSourceRef::Performance {
                channel: session.channels[0].id,
                source: mooloop_core::modulation::PERFORMANCE_MOD_WHEEL,
            }
        );
        assert_eq!(route.source_slot, wheel);
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
        assert_eq!(session.modulation_selected_slot(), Some(gate));
        assert_eq!(
            session.toggle_modulation_assignment().as_deref(),
            Some("Gate"),
            "the badge showed a slot number instead of the outlet"
        );
        assert_eq!(session.modulation_armed_slot(), Some(gate));
    }

    /// An outlet belongs to whichever generator the channel holds. A sampler
    /// publishes nothing, so the band is not there to select from -- the same
    /// answer an empty rack slot gives.
    #[test]
    fn a_generator_that_publishes_nothing_offers_no_outlets() {
        let mut session = Session::default();
        assert_eq!(session.channels[0].kind(), mooloop_core::DeviceKind::Sampler);
        for outlet in 0..mooloop_core::modulation::MAX_GENERATOR_OUTLETS as u16 {
            let slot = outlet_slot(outlet);
            assert!(!session.control_source_exists(slot));
            assert!(!session.select_modulation_source(slot.into()));
        }
        assert_eq!(session.modulation_selected_slot(), None);

        // Nor does an audio outlet become selectable by living in the same
        // table as the control ones: ML-P8 publishes fourteen and offers
        // seven, and `Osc 1` is above the band a route can name.
        let mut mlp8 = mlp8_channel();
        let osc = outlet_slot(mooloop_core::mlp8::OUTLET_OSC1);
        assert!(!mlp8.control_source_exists(osc));
        assert!(!mlp8.select_modulation_source(osc.into()));
    }

    /// The gesture authors an outlet route by its durable outlet id rather
    /// than by the slot it was armed from, and leaves the polarity at the
    /// destination's default.
    ///
    /// The polarity is worth asserting because the obvious guess is wrong.
    /// `ModPolarity::Unipolar` lifts a rack module's `-1..1` into `0..1`; an
    /// outlet already publishes in its declared range, so a unipolar outlet
    /// under a unipolar route would rest half a depth *above* the base and
    /// reach only half the swing.
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
            mooloop_core::ModSourceRef::GeneratorOutlet {
                channel: session.channels[0].id,
                outlet: mooloop_core::mlp8::OUTLET_GATE,
            }
        );
        assert_eq!(gate.source_slot, outlet_slot(mooloop_core::mlp8::OUTLET_GATE));
        assert_eq!(gate.polarity, ModPolarity::Bipolar);

        session.select_modulation_source(outlet_slot(mooloop_core::mlp8::OUTLET_LFO).into());
        let crate::session::ArmedRoute::Added(lfo) =
            session.arm_modulation_route(destination, 0.4)
        else {
            panic!("selecting a second outlet did not follow the arming");
        };
        assert_eq!(lfo.polarity, ModPolarity::Bipolar);
        assert_eq!(
            lfo.source,
            mooloop_core::ModSourceRef::GeneratorOutlet {
                channel: session.channels[0].id,
                outlet: mooloop_core::mlp8::OUTLET_LFO,
            }
        );

        // Two rows, one per outlet: an outlet route dedupes on its source the
        // way a module's does, rather than stacking.
        session.arm_modulation_route(destination, 0.6);
        let rows = session.modulation.routes.len();
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
        assert!(session.modulation.routes.is_empty());
    }

    /// A reorder moves modules. An outlet is not in the rack and does not
    /// move, so a gesture armed on one has to survive a drag it had nothing
    /// to do with.
    #[test]
    fn reordering_modules_leaves_an_armed_outlet_alone() {
        let mut session = mlp8_channel();
        assert!(
            !session.add_modulation_source(ModulatorKind::Lfo).is_empty(),
            "an empty rack has a free slot"
        );
        assert!(
            !session.add_modulation_source(ModulatorKind::Envelope).is_empty(),
            "rack has room"
        );
        let trigger = outlet_slot(mooloop_core::mlp8::OUTLET_TRIGGER);
        session.select_modulation_source(trigger.into());
        session.toggle_modulation_assignment();

        assert!(
            !session.move_modulation_source(1, 0).is_empty(),
            "both slots are occupied"
        );

        assert_eq!(session.modulation_selected_slot(), Some(trigger));
        assert_eq!(session.modulation_armed_slot(), Some(trigger));
    }
}
