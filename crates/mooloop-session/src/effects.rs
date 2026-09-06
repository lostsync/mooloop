//! Device-rack edits: the effect chain of whatever the rack is pointed at.
//!
//! `effect_target` is a channel or a bus, and every edit here goes through
//! `effect_chain_mut`, so none of them needs to know which.

use crate::session::Session;
use mooloop_core::gain::{db_to_linear, MIN_DB as METER_FLOOR_DB};
use mooloop_core::{
    DelayTimeDivision, DeviceId, EffectKind, EffectParams, EffectSlotState, EffectTarget,
    EngineCommand,
};

/// Trim knobs work in dB from unity and stop at the container's headroom; the
/// project and the wire carry linear gain.
const MAX_TRIM_DB: f32 = 12.0;

/// An effect that was inserted into the chain.
///
/// It is installed into the vacant `tail` and then moved to `slot`. Keeping
/// that on the ordered command stream is what lets the realtime chain reach
/// the same order as the model without allocating in its callback.
pub struct EffectInserted {
    pub target: EffectTarget,
    pub slot: usize,
    pub tail: usize,
    /// The identity the chain minted. The engine is told it rather than
    /// deriving one, so both sides agree on what a route names.
    pub device: DeviceId,
    pub kind: EffectKind,
    pub params: EffectParams,
}

/// An effect that was removed. The engine mirrors it the other way round:
/// move the device to the vacated `tail`, then drop the tail.
pub struct EffectRemoved {
    pub target: EffectTarget,
    pub slot: usize,
    pub tail: usize,
}

impl Session {
    /// Inserts `kind` before slot `insert_before`, minting the device its
    /// identity.
    ///
    /// Nothing has to be retargeted: no route and no lane named the row this
    /// pushes down, because none of them ever named a row.
    pub fn insert_effect_at(
        &mut self,
        kind: EffectKind,
        insert_before: usize,
    ) -> Option<EffectInserted> {
        let target = self.effect_target;
        let effects = self.effect_chain_mut()?;
        let tail = effects.len();
        let effect = EffectSlotState::of_kind(kind);
        let (slot, device) = effects.insert(insert_before, effect)?;
        Some(EffectInserted {
            target,
            slot,
            tail,
            device,
            kind,
            params: effect.params,
        })
    }

    /// Removes the effect in `slot`, and with it every route, lane and label
    /// that named the device that was in it.
    pub fn remove_effect_at(&mut self, slot: usize) -> Option<EffectRemoved> {
        let target = self.effect_target;
        let effects = self.effect_chain_mut()?;
        let removed = effects.remove(slot)?;
        let tail = effects.len();
        self.forget_device(target, removed.id);
        Some(EffectRemoved { target, slot, tail })
    }

    /// Reorders the chain, returning what the rack is pointed at.
    ///
    /// A reorder is not an addressing event. The devices change places and
    /// every address in the project goes on meaning exactly what it meant.
    pub fn move_effect_to(&mut self, from: usize, to: usize) -> Option<EffectTarget> {
        let target = self.effect_target;
        if !self
            .effect_chain_mut()
            .is_some_and(|effects| effects.move_device(from, to))
        {
            return None;
        }
        Some(target)
    }

    /// Flips an effect's bypass.
    pub fn toggle_effect_bypass(&mut self, slot: i32) -> Option<EngineCommand> {
        let target = self.effect_target;
        let slot = usize::try_from(slot).ok()?;
        let effect = self.effect_chain_mut()?.get_mut(slot)?;
        effect.bypassed = !effect.bypassed;
        Some(EngineCommand::SetEffectBypassed {
            target,
            slot: slot as u8,
            bypassed: effect.bypassed,
        })
    }

    /// Sets an effect's wet/dry blend.
    pub fn set_effect_wet_dry(&mut self, slot: i32, wet_dry: f32) -> Option<EngineCommand> {
        let target = self.effect_target;
        let slot = usize::try_from(slot).ok()?;
        let effect = self.effect_chain_mut()?.get_mut(slot)?;
        effect.wet_dry = wet_dry.clamp(0.0, 1.0);
        Some(EngineCommand::SetEffectWetDry {
            target,
            slot: slot as u8,
            wet_dry: effect.wet_dry,
        })
    }

    /// Sets an effect's input trim, given the knob's dB.
    pub fn set_effect_input_trim(&mut self, slot: i32, db: f32) -> Option<EngineCommand> {
        let target = self.effect_target;
        let slot = usize::try_from(slot).ok()?;
        let effect = self.effect_chain_mut()?.get_mut(slot)?;
        effect.input_trim = db_to_linear(db.clamp(METER_FLOOR_DB, MAX_TRIM_DB));
        Some(EngineCommand::SetEffectInputTrim {
            target,
            slot: slot as u8,
            input_trim: effect.input_trim,
        })
    }

    /// Sets an effect's output trim, given the knob's dB.
    pub fn set_effect_output_trim(&mut self, slot: i32, db: f32) -> Option<EngineCommand> {
        let target = self.effect_target;
        let slot = usize::try_from(slot).ok()?;
        let effect = self.effect_chain_mut()?.get_mut(slot)?;
        effect.output_trim = db_to_linear(db.clamp(METER_FLOOR_DB, MAX_TRIM_DB));
        Some(EngineCommand::SetEffectOutputTrim {
            target,
            slot: slot as u8,
            output_trim: effect.output_trim,
        })
    }

    /// Sets one parameter of one effect.
    ///
    /// The rack addresses a parameter by its position in the kind's descriptor
    /// table and hands over normalized knob travel; the descriptor converts to
    /// the natural units the wire and the DSP use.
    pub fn set_effect_param(
        &mut self,
        slot: i32,
        param_index: i32,
        normalized: f32,
    ) -> Option<EngineCommand> {
        let target = self.effect_target;
        let slot = usize::try_from(slot).ok()?;
        let param_index = usize::try_from(param_index).ok()?;
        let effect = self.effect_chain_mut()?.get_mut(slot)?;
        let descriptor = effect.kind().descriptors().get(param_index)?;
        let id = descriptor.id;
        let value = effect.params.set(id, descriptor.from_normalized(normalized))?;
        Some(EngineCommand::SetEffectParam {
            target,
            slot: slot as u8,
            id,
            value,
        })
    }

    /// Turns a delay's tempo sync on or off.
    ///
    /// No command: the resolved millisecond time is restated by
    /// `set_tempo`, and until the next tempo change nothing about what the
    /// engine is running has moved.
    pub fn set_delay_tempo_sync(&mut self, slot: i32, enabled: bool) -> bool {
        let Some(params) = self.delay_params_mut(slot) else {
            return false;
        };
        params.tempo_sync = enabled;
        self.mark_dirty();
        true
    }

    /// Picks which musical division a synced delay resolves against.
    pub fn set_delay_time_division(&mut self, slot: i32, division: i32) -> bool {
        let Some(params) = self.delay_params_mut(slot) else {
            return false;
        };
        params.time_division = DelayTimeDivision::from_index(division);
        self.mark_dirty();
        true
    }

    /// Replaces the row in `slot` with a loaded effect preset, returning what
    /// the rack is pointed at.
    ///
    /// A preset of another kind is refused and the rack left untouched:
    /// loading a delay into a filter row is not a coercion to attempt but an
    /// error to report, and the directory layout already makes it unlikely.
    /// The slot's device identity is unchanged, so every route and lane
    /// aimed at it keeps meaning the same knob.
    ///
    /// No command: the caller queues the whole project as one undoable edit,
    /// which is what rebuilds the engine's node with the new parameters.
    ///
    /// `name` is what the row then wears in its header. Pass an empty string
    /// for a load that should not claim one.
    pub fn load_effect_preset(
        &mut self,
        slot: usize,
        preset: &EffectSlotState,
        name: &str,
    ) -> Option<EffectTarget> {
        let target = self.effect_target;
        let effect = self.effect_chain_mut()?.get_mut(slot)?;
        if effect.kind() != preset.kind() {
            return None;
        }
        // The row's identity is the row's, not the preset's: a preset is a
        // patch and carries none. Everything aimed at this device goes on
        // meaning the same knob, which is what makes loading a preset an
        // edit rather than a replacement.
        let device = effect.id;
        self.effect_chain_mut()?.replace_state(slot, *preset);
        self.set_effect_preset_name(target, device, name);
        self.mark_dirty();
        Some(target)
    }


    /// The delay parameters in `slot`, when that slot holds a delay at all.
    fn delay_params_mut(&mut self, slot: i32) -> Option<&mut mooloop_core::DelayParams> {

        let slot = usize::try_from(slot).ok()?;
        let effect = self.effect_chain_mut()?.get_mut(slot)?;
        match &mut effect.params {
            EffectParams::Delay(params) => Some(params),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::gain::linear_to_db;

    fn chain(session: &Session) -> Vec<EffectKind> {
        session
            .effect_chain()
            .map(|chain| chain.iter().map(|effect| effect.kind()).collect())
            .unwrap_or_default()
    }

    /// An insert lands where it was asked to, and reports the vacant tail the
    /// engine installs into before the move.
    #[test]
    fn an_insert_reports_both_the_slot_and_the_tail_it_travels_from() {
        let mut session = Session::default();

        let first = session
            .insert_effect_at(EffectKind::Delay, 0)
            .expect("an empty chain has room");
        assert_eq!((first.slot, first.tail), (0, 0));

        let second = session
            .insert_effect_at(EffectKind::Filter, 0)
            .expect("chain has room");
        assert_eq!(
            (second.slot, second.tail),
            (0, 1),
            "an insert at the head still installs at the tail first"
        );
        assert_eq!(chain(&session), vec![EffectKind::Filter, EffectKind::Delay]);
    }

    #[test]
    fn removing_reports_the_tail_the_engine_drops() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0);
        session.insert_effect_at(EffectKind::Filter, 1);

        let removed = session.remove_effect_at(0).expect("slot 0 is occupied");
        assert_eq!((removed.slot, removed.tail), (0, 1));
        assert_eq!(chain(&session), vec![EffectKind::Filter]);

        assert!(session.remove_effect_at(5).is_none());
    }

    /// Trims are knob dB in and linear gain out, clamped to the headroom the
    /// container allows.
    #[test]
    fn trims_convert_from_db_and_clamp_to_the_headroom() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0);

        session.set_effect_input_trim(0, 100.0).expect("slot exists");
        let effect = session.effect_chain().expect("a chain")[0];
        assert!((linear_to_db(effect.input_trim) - 12.0).abs() < 1.0e-3);

        session.set_effect_output_trim(0, -1_000.0).expect("exists");
        let effect = session.effect_chain().expect("a chain")[0];
        assert!(linear_to_db(effect.output_trim) <= METER_FLOOR_DB + 1.0e-3);

        assert!(session.set_effect_input_trim(9, 0.0).is_none());
        assert!(session.set_effect_input_trim(-1, 0.0).is_none());
    }

    #[test]
    fn bypass_and_blend_round_trip_through_the_slot() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0);

        assert!(matches!(
            session.toggle_effect_bypass(0),
            Some(EngineCommand::SetEffectBypassed { bypassed: true, .. })
        ));
        assert!(matches!(
            session.toggle_effect_bypass(0),
            Some(EngineCommand::SetEffectBypassed {
                bypassed: false,
                ..
            })
        ));
        assert!(matches!(
            session.set_effect_wet_dry(0, 9.0),
            Some(EngineCommand::SetEffectWetDry { wet_dry, .. }) if wet_dry == 1.0
        ));
    }

    /// A slot that is not a delay must not be quietly reinterpreted as one.
    #[test]
    fn the_delay_controls_refuse_a_slot_holding_something_else() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Filter, 0);

        assert!(!session.set_delay_tempo_sync(0, true));
        assert!(!session.set_delay_time_division(0, 2));
        assert!(!session.dirty, "a refused edit still marked the document");

        session.insert_effect_at(EffectKind::Delay, 1);
        assert!(session.set_delay_tempo_sync(1, true));
        assert!(session.dirty);
    }

    /// A parameter index the kind does not have is refused rather than
    /// applied to whatever happens to be at that position.
    #[test]
    fn an_unknown_parameter_index_is_refused() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0);

        assert!(session.set_effect_param(0, 0, 0.5).is_some());
        assert!(session.set_effect_param(0, 9_999, 0.5).is_none());
        assert!(session.set_effect_param(0, -1, 0.5).is_none());
    }

    // --- Effect presets (docs/plans/preset-system/02) ---------------------

    use crate::history::{Entry, History};
    use crate::session::PresetSaveTarget;
    use mooloop_core::{DelayMode, EffectTarget};
    use mooloop_project::{list_presets, load_bundle, save_effect_preset, AssetMode, PresetInfo};

    fn info(name: &str) -> PresetInfo {
        PresetInfo {
            name: name.into(),
            category: String::new(),
            tags: Vec::new(),
        }
    }

    /// A delay with nothing at its defaults, so a field the round trip lost
    /// would show.
    fn dialled_in_delay() -> EffectSlotState {
        let mut effect = EffectSlotState::of_kind(EffectKind::Delay);
        if let EffectParams::Delay(delay) = &mut effect.params {
            delay.feedback = 0.62;
            delay.mode = DelayMode::Reverse;
            delay.cross = 1.0;
        }
        effect.bypassed = true;
        effect.wet_dry = 0.3;
        effect.input_trim = 0.5;
        effect.output_trim = 1.5;
        effect
    }

    /// The whole path the window will drive, with no window: save the row
    /// the dialog was opened from, and load the bundle into a different row.
    #[test]
    fn a_row_saved_from_slot_two_loads_into_slot_zero_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0).expect("room");
        session.insert_effect_at(EffectKind::Filter, 1).expect("room");
        session.insert_effect_at(EffectKind::Delay, 2).expect("room");
        session.channels[0].effects.replace_state(2, dialled_in_delay());
        let device = session.channels[0].effects.id_at(2).expect("three rows");

        session.pending_preset_save = Some(PresetSaveTarget::Effect {
            target: EffectTarget::Channel(0),
            device,
        });
        let source = session.take_preset_save(120, 50).expect("a save was pending");
        assert!(session.pending_preset_save.is_none(), "a taken save is spent");
        let effect = source.effect.expect("an effect save carries its row");
        assert_eq!(effect, dialled_in_delay());

        let path = temp.path().join("dialled.mooloop-effect");
        save_effect_preset(&path, &effect, info("Dialled"), AssetMode::Embedded).unwrap();
        let mooloop_project::LoadedDocument::Effect(loaded) =
            load_bundle(&path).unwrap().document
        else {
            panic!("not an effect");
        };

        let row_zero = session.channels[0].effects.id_at(0).expect("three rows");
        assert_eq!(
            session.load_effect_preset(0, &loaded, "Dialled"),
            Some(EffectTarget::Channel(0))
        );
        assert_eq!(session.channels[0].effects[0].as_patch(), dialled_in_delay());
        // ...and it is still the device it was. Loading a patch changes what
        // the row does, not which device it is, so every route and lane aimed
        // at it goes on meaning the same knob. The preset carried no identity
        // to overwrite it with, either.
        assert_eq!(session.channels[0].effects.id_at(0), Some(row_zero));
        assert!(loaded.id.is_unset(), "a saved preset carried an identity");
        // The row in between was not touched, and the source row is as it was.
        assert_eq!(session.channels[0].effects[1].kind(), EffectKind::Filter);
        assert_eq!(session.channels[0].effects[2].as_patch(), dialled_in_delay());
    }

    #[test]
    fn a_preset_of_another_kind_is_refused_and_the_rack_is_unchanged() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Filter, 0).expect("room");
        let before = session.channels[0].effects.clone();
        let dirty = session.dirty;

        assert_eq!(session.load_effect_preset(0, &dialled_in_delay(), "Dialled"), None);
        assert_eq!(session.channels[0].effects, before);
        assert_eq!(session.dirty, dirty, "a refused load is not an edit");
        // As is a slot that does not exist.
        assert_eq!(session.load_effect_preset(3, &dialled_in_delay(), "Dialled"), None);
    }

    /// Undo is the project snapshot machinery every rack edit uses: one
    /// entry, whose `before` puts the previous slot back exactly -- bypass
    /// and the three trims included, since those default on load and a
    /// restore that dropped them would be silent.
    #[test]
    fn loading_a_preset_is_one_undo_step_that_restores_the_slot_exactly() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0).expect("room");
        let original = EffectSlotState {
            bypassed: true,
            wet_dry: 0.8,
            input_trim: 0.25,
            output_trim: 1.25,
            ..EffectSlotState::of_kind(EffectKind::Delay)
        };
        session.channels[0].effects.replace_state(0, original);
        let original = session.channels[0].effects[0];

        let mut history = History::default();
        let before = session.project_snapshot(120, 50);
        session
            .load_effect_preset(0, &dialled_in_delay(), "Dialled")
            .expect("same kind");
        let after = session.project_snapshot(120, 50);
        history.record(Entry {
            before,
            after,
            label: "Effect preset loaded",
            gesture: None,
        });
        assert_eq!(session.channels[0].effects[0].as_patch(), dialled_in_delay());

        let restore = history.undo_target().expect("one entry").before.clone();
        let samples = session.sample_snapshots();
        session.replace_project(&restore, &samples);
        history.commit_undo();
        assert!(!history.can_undo(), "exactly one step was recorded");
        assert_eq!(session.channels[0].effects[0], original);
    }

    /// The dialog names a device, not a position, so a reorder or an insert
    /// made while it is open changes nothing about what it is pointed at --
    /// there is no permutation to run and none to forget. Removing the device
    /// it names drops the save rather than re-aiming it at whatever slid in.
    #[test]
    fn a_pending_effect_save_names_its_device_not_its_row() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0).expect("room");
        session.insert_effect_at(EffectKind::Filter, 1).expect("room");
        session.insert_effect_at(EffectKind::Drive, 2).expect("room");
        let target = EffectTarget::Channel(0);
        let filter = session.device_at(target, 1).expect("three rows");

        let pending = PresetSaveTarget::Effect {
            target,
            device: filter,
        };
        session.pending_preset_save = Some(pending);
        session.move_effect_to(1, 2).expect("in range");
        assert_eq!(session.pending_preset_save, Some(pending));
        let source = session.take_preset_save(120, 50).expect("still pending");
        assert_eq!(
            source.effect.map(|effect| effect.kind()),
            Some(EffectKind::Filter),
            "the save names the filter it was opened from"
        );

        // An insert ahead of it moves the filter down a row and moves the
        // save not at all; removing the filter drops it.
        session.pending_preset_save = Some(pending);
        session.insert_effect_at(EffectKind::Gate, 0).expect("room");
        assert_eq!(session.pending_preset_save, Some(pending));
        assert_eq!(session.device_slot(target, filter), Some(3));
        session.remove_effect_at(3).expect("in range");
        assert_eq!(session.pending_preset_save, None);

        // An edit on some other chain leaves it alone.
        let delay = session.device_at(target, 0).expect("still populated");
        let elsewhere = PresetSaveTarget::Effect {
            target,
            device: delay,
        };
        session.pending_preset_save = Some(elsewhere);
        session.effect_target = EffectTarget::Bus(0);
        session.insert_effect_at(EffectKind::Limiter, 0).expect("room");
        assert_eq!(session.pending_preset_save, Some(elsewhere));
    }

    /// A rack row on a bus is a row like any other; the save reads from the
    /// chain the dialog was opened on, not from the selected channel.
    #[test]
    fn an_effect_save_on_a_bus_reads_the_bus_row() {
        let mut session = Session {
            effect_target: EffectTarget::Bus(1),
            ..Session::default()
        };
        session.insert_effect_at(EffectKind::Compressor, 0).expect("room");
        let device = session
            .device_at(EffectTarget::Bus(1), 0)
            .expect("one row");
        session.pending_preset_save = Some(PresetSaveTarget::Effect {
            target: EffectTarget::Bus(1),
            device,
        });
        let source = session.take_preset_save(120, 50).expect("pending");
        assert_eq!(
            source.effect.map(|effect| effect.kind()),
            Some(EffectKind::Compressor)
        );
    }

    #[test]
    fn an_empty_effect_presets_directory_lists_nothing() {
        let temp = tempfile::tempdir().unwrap();
        assert!(list_presets(&temp.path().join("effects").join("delay")).is_empty());
        std::fs::create_dir_all(temp.path().join("effects").join("delay")).unwrap();
        assert!(list_presets(&temp.path().join("effects").join("delay")).is_empty());
    }

    /// The header label is how the row got to these settings, so it names the
    /// device rather than the position: it rides the reorder, is dropped with
    /// the device, and a refused load never claims it.
    #[test]
    fn a_rows_preset_label_follows_the_device_and_dies_with_it() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0).expect("room");
        session.insert_effect_at(EffectKind::Filter, 1).expect("room");
        let target = EffectTarget::Channel(0);
        let delay = session.device_at(target, 0).expect("two rows");
        let filter = session.device_at(target, 1).expect("two rows");

        session
            .load_effect_preset(0, &dialled_in_delay(), "Dub Runaway")
            .expect("same kind");
        assert_eq!(session.effect_preset_name(target, delay), Some("Dub Runaway"));
        assert_eq!(session.effect_preset_name(target, filter), None);

        // A refused load leaves the label alone rather than claiming a row it
        // did not change.
        assert_eq!(session.load_effect_preset(1, &dialled_in_delay(), "Dub Runaway"), None);
        assert_eq!(session.effect_preset_name(target, filter), None);

        // The label travels with its device, and nothing had to be rewritten
        // for it to: it was never keyed by the row.
        session.move_effect_to(0, 1).expect("in range");
        assert_eq!(session.device_slot(target, delay), Some(1));
        assert_eq!(session.effect_preset_name(target, delay), Some("Dub Runaway"));
        assert_eq!(session.effect_preset_name(target, filter), None);

        // A knob move keeps it: the settings still came from that preset.
        session.set_effect_wet_dry(1, 0.2).expect("a delay is there");
        assert_eq!(session.effect_preset_name(target, delay), Some("Dub Runaway"));

        // Removing the device takes it.
        session.remove_effect_at(1).expect("in range");
        assert_eq!(session.effect_preset_name(target, delay), None);
        assert!(session.effect_preset_names.is_empty());
    }

    /// Two chains do not share labels, and a reorder on one leaves the
    /// other's alone.
    #[test]
    fn a_label_belongs_to_one_chain() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Gate, 0).expect("room");
        let channel_gate = session
            .device_at(EffectTarget::Channel(0), 0)
            .expect("one row");
        session.set_effect_preset_name(EffectTarget::Channel(0), channel_gate, "Tight Drum Gate");

        session.effect_target = EffectTarget::Bus(1);
        session.insert_effect_at(EffectKind::Limiter, 0).expect("room");
        session.insert_effect_at(EffectKind::Gate, 0).expect("room");
        let bus_gate = session.device_at(EffectTarget::Bus(1), 0).expect("one row");

        assert_eq!(
            session.effect_preset_name(EffectTarget::Channel(0), channel_gate),
            Some("Tight Drum Gate")
        );
        assert_eq!(
            session.effect_preset_name(EffectTarget::Bus(1), bus_gate),
            None,
            "two chains can mint the same number and must not share a label"
        );
    }
}
