//! Device-rack edits: the effect chain of whatever the rack is pointed at.
//!
//! `effect_target` is a channel or a bus, and every edit here goes through
//! `effect_chain_mut`, so none of them needs to know which.

use crate::session::Session;
use mooloop_core::gain::{db_to_linear, MIN_DB as METER_FLOOR_DB};
use mooloop_core::{
    insert_effect, insert_into_container, move_effect, move_effect_into_container, remove_effect,
    unwrap_container,
    wrap_in_container,
    DeviceId, EffectKind, EffectParams, EffectRun, EffectSlotState, ModTimeDivision,
    EffectTarget, EngineCommand,
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
    /// The identity minted for it. The engine's slot has to be given this or
    /// no route will ever find the device.
    pub device: DeviceId,
    pub kind: EffectKind,
    pub params: EffectParams,
}

/// A container preset that replaced a run.
///
/// The engine mirror is the removal it did and the insertion it did, in that
/// order, because the chain there is a flat array of installed nodes and this
/// changed both which nodes are in it and how many.
pub struct EffectRunLoaded {
    pub target: EffectTarget,
    /// Where the old container was, and where the new one now is.
    pub slot: usize,
    /// How many rows the old run had.
    pub removed: usize,
    /// The last index of the chain before the removal; step `i` of the engine
    /// mirror moves `slot` to `removed_tail - i` and drops it.
    pub removed_tail: usize,
    /// The identities minted for the preset's devices, in rack order.
    pub devices: Vec<mooloop_core::DeviceId>,
    /// How many rows the preset brought.
    pub landed: usize,
}

/// A reorder, and how the engine's flat chain gets to the same order.
pub struct EffectMoved {
    pub target: EffectTarget,
    /// `(from, to)` pairs in the same remove-then-insert sense
    /// `EngineCommand::MoveEffect` uses, applied in order.
    pub moves: Vec<(u8, u8)>,
}

/// The devices that were removed. The engine mirrors it the other way round:
/// move the device to the vacated `tail`, then drop the tail -- repeated once
/// per row, always from `slot`, because after each removal the next row of
/// the run has slid into that position.
///
/// More than one row when the removed device was a container: a box goes with
/// its contents.
/// A run that arrived from the device clipboard.
///
/// Simpler than [`EffectRunLoaded`] because nothing left to make room for it:
/// a paste is an insertion, so the engine mirror is the rows that arrived and
/// nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectRunInserted {
    pub target: EffectTarget,
    /// Where the run's head landed.
    pub slot: usize,
    /// The identities minted for it, in rack order.
    pub devices: Vec<DeviceId>,
}

pub struct EffectRemoved {
    pub target: EffectTarget,
    pub slot: usize,
    /// The last index of the chain *before* the removal. Step `i` of the
    /// engine mirror moves `slot` to `tail - i` and drops it.
    pub tail: usize,
    /// The identities that have just stopped existing, in rack order.
    pub devices: Vec<DeviceId>,
}

impl Session {
    /// Inserts `kind` before slot `insert_before`, minting it an identity.
    ///
    /// Nothing is retargeted. Every address in the project names a device, so
    /// an insert -- which changes only positions -- is invisible to all of
    /// them.
    pub fn insert_effect_at(
        &mut self,
        kind: EffectKind,
        insert_before: usize,
    ) -> Option<EffectInserted> {
        let target = self.effect_target;
        let (effects, next_id) = self.effect_chain_parts_mut()?;
        let tail = effects.len();
        let effect = EffectSlotState::of_kind(kind);
        let slot = insert_effect(effects, next_id, insert_before, effect)?;
        let device = effects[slot].id;
        Some(EffectInserted {
            target,
            slot,
            tail,
            device,
            kind,
            params: effect.params,
        })
    }

    /// Removes the effect in `slot` -- and its whole run when it is a
    /// container -- letting go of everything that named any of them.
    pub fn remove_effect_at(&mut self, slot: usize) -> Option<EffectRemoved> {
        let target = self.effect_target;
        let effects = self.effect_chain_mut()?;
        let before = effects.len();
        let removed = remove_effect(effects, slot)?;
        let devices: Vec<DeviceId> = removed.iter().map(|effect| effect.id).collect();
        for device in &devices {
            self.forget_device(target, *device);
        }
        Some(EffectRemoved {
            target,
            slot,
            tail: before - 1,
            devices,
        })
    }

    /// Inserts `kind` as the first device inside the container in `slot`.
    ///
    /// What a container's own rail `+` means. Separate from
    /// `insert_effect_at` because an index cannot say "into this box": the
    /// position just after a container's row is also the position just after
    /// the container, and for an empty one those are the same number.
    pub fn insert_effect_into_container(
        &mut self,
        kind: EffectKind,
        slot: usize,
    ) -> Option<EffectInserted> {
        let target = self.effect_target;
        let (effects, next_id) = self.effect_chain_parts_mut()?;
        let tail = effects.len();
        let effect = EffectSlotState::of_kind(kind);
        let landed = insert_into_container(effects, next_id, slot, effect)?;
        Some(EffectInserted {
            target,
            slot: landed,
            tail,
            device: effects[landed].id,
            kind,
            params: effect.params,
        })
    }

    /// Wraps `run` in a new container, minting it an identity.
    ///
    /// The gesture that actually makes containers: one is far more often made
    /// around devices that already exist than inserted empty. Reported as an
    /// ordinary insert, because on the engine's side that is exactly what it
    /// is -- one row installed at the tail and moved into place, with the
    /// rows it now encloses never moving at all.
    ///
    /// `None` when the range is empty, out of range, or would cut a
    /// container's run in half.
    pub fn wrap_effects_in_container(
        &mut self,
        run: std::ops::Range<usize>,
    ) -> Option<EffectInserted> {
        let target = self.effect_target;
        let (effects, next_id) = self.effect_chain_parts_mut()?;
        let tail = effects.len();
        let container = EffectSlotState::of_kind(EffectKind::Chain);
        let slot = wrap_in_container(effects, next_id, run, container)?;
        let inserted = effects[slot];
        Some(EffectInserted {
            target,
            slot,
            tail,
            device: inserted.id,
            kind: EffectKind::Chain,
            params: inserted.params,
        })
    }

    /// Takes the container in `slot` out of the chain, leaving its children
    /// where they are.
    ///
    /// The escape hatch that makes "removing a box removes its contents" safe
    /// to have. Reported as an ordinary removal of one row.
    pub fn unwrap_container_at(&mut self, slot: usize) -> Option<EffectRemoved> {
        let target = self.effect_target;
        let effects = self.effect_chain_mut()?;
        let before = effects.len();
        let device = effects.get(slot)?.id;
        if !unwrap_container(effects, slot) {
            return None;
        }
        self.forget_device(target, device);
        Some(EffectRemoved {
            target,
            slot,
            tail: before - 1,
            devices: vec![device],
        })
    }

    /// Replace the container in `slot` and its whole run with `run`.
    ///
    /// Every device in the preset is minted a fresh identity here. A preset
    /// carries none of its own -- `save_effect_run_preset` strips them -- and
    /// it must not: which devices these are belongs to the chain they land
    /// on, so loading the same preset twice onto one chain has to give two
    /// independent runs rather than two rows claiming the same id. The same
    /// argument `install_with_id` makes for a modulator arriving with one.
    ///
    /// Routes and lanes pointing at the *old* run are dropped, because the
    /// devices they named are gone. Nothing is re-aimed: a preset load
    /// replaces what a group of devices is, and a lane drawn on the delay
    /// that used to be in slot 2 has no claim on whatever the preset put
    /// there.
    ///
    /// `None` when `slot` does not hold a container, when the preset is not a
    /// well-formed run, or when the chain has no room for it.
    pub fn load_effect_run(
        &mut self,
        slot: usize,
        run: &mooloop_core::EffectRun,
        name: &str,
    ) -> Option<EffectRunLoaded> {
        let target = self.effect_target;
        if run.effects.first().map(EffectSlotState::kind) != Some(EffectKind::Chain) {
            return None;
        }
        if mooloop_core::span_problem(&run.effects).is_some() {
            return None;
        }
        let (removed, devices, removed_tail) = {
            let (effects, next_id) = self.effect_chain_parts_mut()?;
            if effects.get(slot)?.kind() != EffectKind::Chain {
                return None;
            }
            let before = effects.len();
            // One call, because the boxes around this one lose the run that
            // left and gain the run that arrived, and those are two different
            // numbers.
            let removed = mooloop_core::replace_run(effects, next_id, slot, &run.effects)?;
            let devices: Vec<mooloop_core::DeviceId> = effects
                [slot..slot + run.effects.len()]
                .iter()
                .map(|effect| effect.id)
                .collect();
            (removed, devices, before - 1)
        };
        for effect in &removed {
            self.forget_device(target, effect.id);
        }
        self.set_effect_preset_name(target, devices[0], name);
        self.mark_dirty();
        Some(EffectRunLoaded {
            target,
            slot,
            removed: removed.len(),
            removed_tail,
            landed: devices.len(),
            devices,
        })
    }

    /// Selects the device in `slot`, by identity.
    ///
    /// `None` clears the selection, and so does a slot that names nothing.
    pub fn select_device(&mut self, slot: Option<usize>) {
        let target = self.effect_target;
        self.selected_device = slot
            .and_then(|slot| self.effect_chain()?.get(slot))
            .map(|effect| (target, effect.id));
        // One selection, not two. A rack that could show a lit generator and
        // a lit effect at once would have no answer to "what does Copy act
        // on", which is the only question the selection exists to answer.
        self.selected_source = None;
    }

    /// Selects the generator at the head of the chain the rack is pointed at,
    /// or clears that selection.
    ///
    /// Refused for a bus, which has no generator. Returns whether anything is
    /// selected afterwards, which is what the caller reports.
    pub fn select_source(&mut self, selected: bool) -> bool {
        let target = self.effect_target;
        if !selected || !matches!(target, EffectTarget::Channel(_)) {
            self.selected_source = None;
            return false;
        }
        self.selected_device = None;
        self.selected_source = Some(target);
        true
    }

    /// Whether the generator of the chain now being edited is the selection.
    pub fn source_is_selected(&self) -> bool {
        self.selected_source == Some(self.effect_target)
    }

    /// Where the selected device is now, or `None` when nothing is selected,
    /// the rack is pointed somewhere else, or it has been removed.
    ///
    /// Derived rather than stored, which is the whole reason the selection is
    /// an identity: a reorder moves the device and this answer follows it
    /// without anything having been rewritten.
    pub fn selected_device_slot(&self) -> Option<usize> {
        let (target, device) = self.selected_device?;
        if target != self.effect_target {
            return None;
        }
        mooloop_core::device_slot(self.effect_chain()?, device)
    }

    /// The device in `slot` -- and, when it is a container, its whole run --
    /// as a clipboard payload.
    ///
    /// Identity is stripped, for the reason `take_preset_save` gives about a
    /// preset: which devices these are belongs to the chain they were taken
    /// from, and `insert_run` mints fresh ones on the way back in. A
    /// clipboard holds a design, not a device.
    ///
    /// Read-only, so a copy is not an edit and does not touch history.
    pub fn copy_device(&self, slot: usize) -> Option<EffectRun> {
        let effects = self.effect_chain()?;
        if slot >= effects.len() {
            return None;
        }
        let effects = effects[mooloop_core::run_of(effects, slot)]
            .iter()
            .map(|effect| effect.with_id(DeviceId::UNASSIGNED))
            .collect();
        Some(EffectRun { effects })
    }

    /// Puts `run` into the chain immediately after the run at `after`, or at
    /// the head of an empty chain.
    ///
    /// **A paste lands beside the row you pasted onto, not inside it.**
    /// `run_of(after).end` is a run's end boundary, and `insert_run` treats
    /// that boundary the way `insert_effect` documents -- as *after* the
    /// container rather than in it -- so pasting onto a container's last
    /// child puts the arrival after the box. That is one rule at every
    /// depth, and it is the same rule the rack's own `+` follows.
    ///
    /// `None` when `after` names nothing, the run is malformed, or the chain
    /// has no room.
    pub fn paste_device(&mut self, run: &EffectRun, after: usize) -> Option<EffectRunInserted> {
        let target = self.effect_target;
        let (effects, next_id) = self.effect_chain_parts_mut()?;
        let at = if effects.is_empty() {
            0
        } else {
            mooloop_core::run_of(effects, after.min(effects.len() - 1)).end
        };
        let slot = mooloop_core::insert_run(effects, next_id, at, &run.effects)?;
        let devices = effects[slot..slot + run.effects.len()]
            .iter()
            .map(|effect| effect.id)
            .collect();
        self.mark_dirty();
        Some(EffectRunInserted {
            target,
            slot,
            devices,
        })
    }

    /// Copies the run at `slot` and pastes it straight after itself.
    ///
    /// Deliberately not "copy then paste": it must not disturb the clipboard,
    /// the same way `channel.clone` does not disturb the channel clipboard.
    pub fn duplicate_device(&mut self, slot: usize) -> Option<EffectRunInserted> {
        let run = self.copy_device(slot)?;
        self.paste_device(&run, slot)
    }

    /// Reorders the chain, returning what the rack is pointed at and the
    /// single-row moves the engine has to make to match.
    ///
    /// The moves are spelled out because a container takes its whole run with
    /// it, and the engine mirrors a reorder one row at a time. For a leaf the
    /// sequence is the one move it always was.
    pub fn move_effect_to(&mut self, from: usize, to: usize) -> Option<EffectMoved> {
        let target = self.effect_target;
        let effects = self.effect_chain_mut()?;
        let before: Vec<DeviceId> = effects.iter().map(|effect| effect.id).collect();
        // Dropping onto an *empty* container means dropping into it. Nothing
        // else can be meant: an empty box has no rows to aim at, and its span
        // covers no index, so under the plain index rule there was no gesture
        // anywhere that put a device back inside one. Emptying a box made it
        // unfillable except by its own `+`.
        //
        // Deliberately only when it is empty. For a box that still holds
        // something, its own row keeps meaning "before this box" -- its
        // children are there to be aimed at, and taking that index away would
        // make "just before a container" the thing that had no gesture.
        let into_empty_box = matches!(
            effects.get(to).map(|effect| effect.params),
            Some(EffectParams::Chain(chain)) if chain.children == 0
        );
        let moved = if into_empty_box {
            move_effect_into_container(effects, from, to)
        } else {
            move_effect(effects, from, to)
        };
        if !moved {
            return None;
        }
        let after: Vec<DeviceId> = effects.iter().map(|effect| effect.id).collect();
        Some(EffectMoved {
            target,
            moves: mooloop_core::move_sequence(&before, &after),
        })
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
        params.time_division = ModTimeDivision::from_index(division);
        self.mark_dirty();
        true
    }

    /// Turns a modulation effect's tempo sync on or off, resolving the rate
    /// it should now be running at.
    ///
    /// Unlike the delay's, which reports nothing and lets its knob push the
    /// resolved millisecond value through the ordinary parameter path, this
    /// returns the command. The mapping from a division to a rate has a
    /// clamp in it -- a 64th triplet asks for more than the device runs --
    /// and a clamp written in markup is a second copy of the range.
    pub fn set_modulation_tempo_sync(
        &mut self,
        slot: i32,
        enabled: bool,
        bpm: f64,
    ) -> Option<Option<EngineCommand>> {
        let target = self.effect_target;
        let params = self.modulation_params_mut(slot)?;
        params.tempo_sync = enabled;
        let command = resolved_modulation_rate(params, target, slot, bpm);
        self.mark_dirty();
        Some(command)
    }

    /// Picks which musical division a synced modulation LFO resolves against.
    pub fn set_modulation_rate_division(
        &mut self,
        slot: i32,
        division: i32,
        bpm: f64,
    ) -> Option<Option<EngineCommand>> {
        let target = self.effect_target;
        let params = self.modulation_params_mut(slot)?;
        params.rate_division = ModTimeDivision::from_index(division);
        let command = resolved_modulation_rate(params, target, slot, bpm);
        self.mark_dirty();
        Some(command)
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
        // A preset carries no identity of its own, and must not take this
        // row's: the routes and lanes pointing here are pointing at the
        // *device*, and loading a preset changes what it sounds like rather
        // than which one it is.
        let device = effect.id;
        *effect = preset.with_id(device);
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

    fn modulation_params_mut(&mut self, slot: i32) -> Option<&mut mooloop_core::ModulationParams> {
        let slot = usize::try_from(slot).ok()?;
        let effect = self.effect_chain_mut()?.get_mut(slot)?;
        match &mut effect.params {
            EffectParams::Modulation(params) => Some(params),
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

    /// Both tempo-following effects resolve against the transport, and the
    /// modulation LFO's resolution has a ceiling in it: a 64th triplet at
    /// 120 BPM asks for 48 Hz from a device that runs to 12.
    #[test]
    fn a_synced_modulation_rate_follows_the_tempo_up_to_what_the_lfo_runs() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Modulation, 0);

        // Free-running: a tempo change moves nothing.
        let before = session.update_tempo_synced_effects(90.0);
        assert!(before.is_empty(), "a free-running LFO answered a tempo change");

        assert!(session
            .set_modulation_tempo_sync(0, true, 120.0)
            .is_some_and(|command| command.is_some()));
        // A whole note at 120 BPM is two seconds, so half a hertz.
        assert!(session
            .set_modulation_rate_division(0, ModTimeDivision::Whole.to_index(), 120.0)
            .is_some());
        let changes = session.update_tempo_synced_effects(120.0);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].2, mooloop_core::MODULATION_PARAM_RATE_HZ);
        assert!((changes[0].3 - 0.5).abs() < 1.0e-4, "got {}", changes[0].3);

        // Half the tempo, half the rate.
        let slower = session.update_tempo_synced_effects(60.0);
        assert!((slower[0].3 - 0.25).abs() < 1.0e-4, "got {}", slower[0].3);

        // And the ceiling holds rather than handing the DSP 48 Hz.
        let _ = session.set_modulation_rate_division(
            0,
            ModTimeDivision::SixtyFourthTriplet.to_index(),
            120.0,
        );
        let fast = session.update_tempo_synced_effects(120.0);
        assert!(
            (fast[0].3 - mooloop_core::MODULATION_MAX_RATE_HZ).abs() < 1.0e-4,
            "got {}",
            fast[0].3
        );
    }

    /// A slot that is not a modulation effect is not quietly treated as one,
    /// the same rule the delay controls already hold to.
    #[test]
    fn the_modulation_sync_controls_refuse_a_slot_holding_something_else() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Filter, 0);
        assert!(session.set_modulation_tempo_sync(0, true, 120.0).is_none());
        assert!(session.set_modulation_rate_division(0, 4, 120.0).is_none());
        assert!(!session.dirty, "a refused edit still marked the document");
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

    /// The identity of the device in `slot` of the selected channel.
    fn device_at(session: &Session, slot: usize) -> DeviceId {
        session.channels[0].effects[slot].id
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
        let delay = device_at(&session, 2);
        session.channels[0].effects[2] = dialled_in_delay().with_id(delay);

        session.pending_preset_save = Some(PresetSaveTarget::Effect {
            target: EffectTarget::Channel(0),
            device: delay,
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

        assert_eq!(
            session.load_effect_preset(0, &loaded, "Dialled"),
            Some(EffectTarget::Channel(0))
        );
        // Everything but the identity, which stays the loaded-into row's.
        let landed = device_at(&session, 0);
        assert_eq!(session.channels[0].effects[0], dialled_in_delay().with_id(landed));
        assert_ne!(landed, delay, "the preset brought the source row's identity");
        // The row in between was not touched, and the source row is as it was.
        assert_eq!(session.channels[0].effects[1].kind(), EffectKind::Filter);
        assert_eq!(session.channels[0].effects[2], dialled_in_delay().with_id(delay));
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
        let device = device_at(&session, 0);
        let original = original.with_id(device);
        session.channels[0].effects[0] = original;

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
        assert_eq!(
            session.channels[0].effects[0],
            dialled_in_delay().with_id(device)
        );

        let restore = history.undo_target().expect("one entry").before.clone();
        let samples = session.sample_snapshots();
        session.replace_project(&restore, &samples);
        history.commit_undo();
        assert!(!history.can_undo(), "exactly one step was recorded");
        assert_eq!(session.channels[0].effects[0], original);
    }

    /// The dialog names a device, and after this step it does so literally.
    /// Dropping a device onto an emptied box puts it back in the box.
    ///
    /// The rack reports a drop as "the row the pointer was over", and for an
    /// empty container that row is the box itself. Under the plain index rule
    /// that meant "before the box", so a box you had just dragged the last
    /// device out of could not be refilled by dragging one back.
    #[test]
    fn a_drop_on_an_emptied_container_lands_inside_it() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Filter, 0).expect("room");
        session.wrap_effects_in_container(0..1).expect("wrapped");
        let depths = |session: &Session| -> Vec<(usize, EffectKind)> {
            let effects = session.effect_chain().expect("a chain");
            (0..effects.len())
                .map(|slot| (mooloop_core::depth_at(effects, slot), effects[slot].kind()))
                .collect()
        };
        assert_eq!(
            depths(&session),
            [(0, EffectKind::Chain), (1, EffectKind::Filter)]
        );

        session.move_effect_to(1, 0).expect("in range");
        assert_eq!(
            depths(&session),
            [(0, EffectKind::Filter), (0, EffectKind::Chain)],
            "dragging the filter out did not empty the box"
        );

        session.move_effect_to(0, 1).expect("in range");
        assert_eq!(
            depths(&session),
            [(0, EffectKind::Chain), (1, EffectKind::Filter)],
            "a drop on the emptied box landed beside it instead of inside it"
        );
    }

    /// A box that still holds something keeps meaning "before this box" when
    /// it is dropped on, because its children are there to be aimed at and
    /// the position before it would otherwise have no gesture at all.
    #[test]
    fn a_drop_on_a_container_that_holds_something_still_lands_before_it() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Filter, 0).expect("room");
        session.wrap_effects_in_container(0..1).expect("wrapped");
        session.insert_effect_at(EffectKind::Drive, 2).expect("room");
        let depths = |session: &Session| -> Vec<(usize, EffectKind)> {
            let effects = session.effect_chain().expect("a chain");
            (0..effects.len())
                .map(|slot| (mooloop_core::depth_at(effects, slot), effects[slot].kind()))
                .collect()
        };
        assert_eq!(
            depths(&session),
            [
                (0, EffectKind::Chain),
                (1, EffectKind::Filter),
                (0, EffectKind::Drive)
            ],
            "the fixture is not the shape the test is about"
        );

        session.move_effect_to(2, 0).expect("in range");
        assert_eq!(
            depths(&session),
            [
                (0, EffectKind::Drive),
                (0, EffectKind::Chain),
                (1, EffectKind::Filter)
            ],
            "a drop on an occupied box swallowed the device instead of \
             landing before it"
        );
    }

    /// Reordering the rack while it is open cannot move the pending save,
    /// because there is no position in it to move; removing that row drops it.
    #[test]
    fn a_pending_effect_save_names_a_device_that_a_reorder_cannot_move() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0).expect("room");
        session.insert_effect_at(EffectKind::Filter, 1).expect("room");
        session.insert_effect_at(EffectKind::Drive, 2).expect("room");
        let target = EffectTarget::Channel(0);
        let filter = device_at(&session, 1);

        session.pending_preset_save = Some(PresetSaveTarget::Effect { target, device: filter });
        session.move_effect_to(1, 2).expect("in range");
        assert_eq!(
            session.pending_preset_save,
            Some(PresetSaveTarget::Effect { target, device: filter }),
            "a reorder is not something a pending save can observe"
        );
        let source = session.take_preset_save(120, 50).expect("still pending");
        assert_eq!(
            source.effect.map(|effect| effect.kind()),
            Some(EffectKind::Filter),
            "the save names the filter it was opened from"
        );

        // An insert ahead of it is equally invisible; a removal of it drops it.
        session.pending_preset_save = Some(PresetSaveTarget::Effect { target, device: filter });
        session.insert_effect_at(EffectKind::Gate, 0).expect("room");
        assert_eq!(
            session.pending_preset_save,
            Some(PresetSaveTarget::Effect { target, device: filter })
        );
        let slot = mooloop_core::device_slot(&session.channels[0].effects, filter).expect("there");
        session.remove_effect_at(slot).expect("in range");
        assert_eq!(session.pending_preset_save, None);

        // A save opened on one chain is not disturbed by an edit on another.
        let delay = device_at(&session, 0);
        session.pending_preset_save = Some(PresetSaveTarget::Effect { target, device: delay });
        session.effect_target = EffectTarget::Bus(0);
        session.insert_effect_at(EffectKind::Limiter, 0).expect("room");
        assert_eq!(
            session.pending_preset_save,
            Some(PresetSaveTarget::Effect { target, device: delay })
        );
    }

    /// A rack row on a bus is a row like any other; the save reads from the
    /// chain the dialog was opened on, not from the selected channel.
    #[test]
    fn an_effect_save_on_a_bus_reads_the_bus_row() {
        let mut session = Session {
            effect_target: EffectTarget::Bus(1),
            ..Session::default()
        };
        let compressor = session
            .insert_effect_at(EffectKind::Compressor, 0)
            .expect("room")
            .device;
        session.pending_preset_save = Some(PresetSaveTarget::Effect {
            target: EffectTarget::Bus(1),
            device: compressor,
        });
        let source = session.take_preset_save(120, 50).expect("pending");
        assert_eq!(
            source.effect.map(|effect| effect.kind()),
            Some(EffectKind::Compressor)
        );
    }

    /// The two gestures step 04 puts on the rail, through the session: a box
    /// made around a device, and the box taken away without its contents.
    ///
    /// Wrapping a *container* wraps its whole run, which is what makes
    /// The selection is an identity, so a reorder moves the device and the
    /// answer follows it. Under the slot scheme this assertion could not have
    /// been written -- there, the selection would have had to be rewritten.
    #[test]
    fn the_selected_device_survives_a_reorder_and_dies_with_its_device() {
        let mut session = Session::default();
        for kind in [EffectKind::Delay, EffectKind::Filter, EffectKind::Drive] {
            session.insert_effect_at(kind, usize::MAX).expect("room");
        }
        session.select_device(Some(2));
        assert_eq!(session.selected_device_slot(), Some(2));

        session.move_effect_to(2, 0).expect("a reorder");
        assert_eq!(
            session.selected_device_slot(),
            Some(0),
            "the drive is still selected, and it is now first"
        );
        assert_eq!(kinds(&session)[0], EffectKind::Drive);

        // An insert above it moves it again, and still changes nothing.
        session.insert_effect_at(EffectKind::Gate, 0).expect("room");
        assert_eq!(session.selected_device_slot(), Some(1));

        session.remove_effect_at(1).expect("the drive");
        assert_eq!(
            session.selected_device_slot(),
            None,
            "a removed device is not selected, it is gone"
        );
        assert_eq!(session.selected_device, None);
    }

    /// The point of the whole step: a device copied out of one chain lands in
    /// another as a *different* device that sounds the same.
    #[test]
    fn a_pasted_device_is_a_new_device_with_the_same_sound() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0).expect("room");
        // Any parameter will do; what matters is that the copy carries it.
        session.set_effect_param(0, 1, 0.75).expect("slot 0 is a delay");
        let original = session.channels[0].effects[0];

        let run = session.copy_device(0).expect("slot 0 is occupied");
        assert_eq!(
            run.effects[0].id,
            DeviceId::UNASSIGNED,
            "a clipboard holds a design, not a device"
        );

        let pasted = session.paste_device(&run, 0).expect("chain has room");
        assert_eq!(kinds(&session), [EffectKind::Delay, EffectKind::Delay]);
        assert_eq!(pasted.slot, 1, "a paste lands after the row it was taken from");

        let copy = session.channels[0].effects[1];
        assert_ne!(copy.id, original.id, "two rows no route could tell apart");
        assert_eq!(copy.id, pasted.devices[0]);
        assert_eq!(
            copy.params, original.params,
            "and it must still sound like what was copied"
        );
    }

    /// A container is copied as its whole run, the same unit it is deleted
    /// and saved as.
    #[test]
    fn copying_a_container_takes_everything_in_it() {
        let mut session = Session::default();
        for kind in [EffectKind::Delay, EffectKind::Filter, EffectKind::Drive] {
            session.insert_effect_at(kind, usize::MAX).expect("room");
        }
        session.wrap_effects_in_container(1..2).expect("wrapped");
        // Delay, [Chain, Filter], Drive
        let run = session.copy_device(1).expect("the container");
        assert_eq!(
            run.effects.iter().map(|e| e.kind()).collect::<Vec<_>>(),
            [EffectKind::Chain, EffectKind::Filter],
            "the box and what is in it, and nothing after it"
        );

        session.paste_device(&run, 1).expect("room");
        assert_eq!(
            kinds(&session),
            [
                EffectKind::Delay,
                EffectKind::Chain,
                EffectKind::Filter,
                EffectKind::Chain,
                EffectKind::Filter,
                EffectKind::Drive,
            ]
        );
        assert_eq!(
            depths(&session),
            [0, 0, 1, 0, 1, 0],
            "the pasted box encloses its own child and nothing else"
        );
        assert_eq!(
            mooloop_core::span_problem(&session.channels[0].effects),
            None,
            "and the chain is still well formed"
        );
    }

    /// The boundary rule, which is the only thing about paste that is not
    /// obvious: pasting onto a container's last child lands *after* the box,
    /// because a run's end boundary is outside it -- the same rule
    /// `insert_effect` follows for the rack's own `+`.
    #[test]
    fn pasting_onto_a_containers_last_child_lands_outside_the_box() {
        let mut session = Session::default();
        for kind in [EffectKind::Delay, EffectKind::Filter] {
            session.insert_effect_at(kind, usize::MAX).expect("room");
        }
        session.wrap_effects_in_container(1..2).expect("wrapped");
        // Delay, [Chain, Filter]
        assert_eq!(depths(&session), [0, 0, 1]);

        let run = session.copy_device(0).expect("the delay");
        session.paste_device(&run, 2).expect("room");

        assert_eq!(
            depths(&session),
            [0, 0, 1, 0],
            "the paste landed beside the box, not inside it"
        );
        assert_eq!(
            session.channels[0].effects[1].params,
            EffectParams::Chain(mooloop_core::ChainParams {
                children: 1,
                ..Default::default()
            }),
            "and the box did not grow a child it never gained"
        );
    }

    /// Duplicate is copy-and-paste that does not touch the clipboard, the
    /// same relationship `channel.clone` has to channel copy/paste.
    #[test]
    fn duplicate_leaves_the_clipboard_alone() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Reverb, 0).expect("room");
        session.insert_effect_at(EffectKind::Gate, 1).expect("room");

        let held = session.copy_device(0).expect("the reverb");
        session.duplicate_device(1).expect("room");

        assert_eq!(
            kinds(&session),
            [EffectKind::Reverb, EffectKind::Gate, EffectKind::Gate]
        );
        assert_eq!(
            held.effects.iter().map(|e| e.kind()).collect::<Vec<_>>(),
            [EffectKind::Reverb],
            "duplicating something else must not overwrite what was copied"
        );
    }

    #[test]
    fn a_paste_refuses_what_it_cannot_take() {
        let mut session = Session::default();
        assert!(session.copy_device(0).is_none(), "nothing to copy yet");

        session.insert_effect_at(EffectKind::Delay, 0).expect("room");
        let run = session.copy_device(0).expect("the delay");

        assert!(
            session.paste_device(&EffectRun { effects: Vec::new() }, 0).is_none(),
            "an empty run is not a paste"
        );

        // A headless run -- a child with no box in front of it -- must not be
        // able to straddle its way into a well-formed chain.
        let mut headless = run.clone();
        headless.effects[0].params = EffectParams::Chain(mooloop_core::ChainParams {
            children: 4,
            ..Default::default()
        });
        assert!(
            session.paste_device(&headless, 0).is_none(),
            "a container claiming children it did not bring is a straddle"
        );
        assert_eq!(
            mooloop_core::span_problem(&session.channels[0].effects),
            None,
            "and the refusal left the chain untouched"
        );
    }

    /// nesting reachable from a single button rather than needing a selection
    /// model to exist first.
    #[test]
    fn wrapping_and_unwrapping_leave_every_device_where_it_was() {
        let mut session = Session::default();
        for kind in [EffectKind::Delay, EffectKind::Filter, EffectKind::Drive] {
            session.insert_effect_at(kind, usize::MAX).expect("room");
        }
        let ids: Vec<DeviceId> = session.channels[0]
            .effects
            .iter()
            .map(|effect| effect.id)
            .collect();

        // A box around the filter alone.
        let inner = session
            .wrap_effects_in_container(1..2)
            .expect("wrapped")
            .device;
        assert_eq!(
            kinds(&session),
            [
                EffectKind::Delay,
                EffectKind::Chain,
                EffectKind::Filter,
                EffectKind::Drive
            ]
        );

        // A box around that box, which is what clicking wrap on a container
        // does: it wraps the run, not the row.
        let run = mooloop_core::run_of(&session.channels[0].effects, 1);
        assert_eq!(run, 1..3, "the container's run is itself and its child");
        session.wrap_effects_in_container(run).expect("wrapped");
        assert_eq!(
            depths(&session),
            [0, 0, 1, 2, 0],
            "the second box did not enclose the first"
        );
        assert_eq!(
            mooloop_core::span_problem(&session.channels[0].effects),
            None
        );

        // Every original device is still on the chain, in order.
        for (position, id) in ids.iter().enumerate() {
            let slot = mooloop_core::device_slot(&session.channels[0].effects, *id);
            assert!(slot.is_some(), "device {position} left the chain");
        }

        // Unwrapping the inner box leaves its child where it is and costs the
        // outer box exactly the one row.
        let inner_slot =
            mooloop_core::device_slot(&session.channels[0].effects, inner).expect("still there");
        session.unwrap_container_at(inner_slot).expect("a container");
        assert_eq!(
            kinds(&session),
            [
                EffectKind::Delay,
                EffectKind::Chain,
                EffectKind::Filter,
                EffectKind::Drive
            ]
        );
        assert_eq!(depths(&session), [0, 0, 1, 0]);
        assert_eq!(
            mooloop_core::span_problem(&session.channels[0].effects),
            None
        );

        // And a leaf is not a container, so the gesture refuses it rather
        // than removing a device the user did not ask to remove.
        assert!(session.unwrap_container_at(0).is_none());
        assert_eq!(session.channels[0].effects.len(), 4);
    }

    /// **The step 05 acceptance case.** A container saves as one preset,
    /// loads onto a different chain, and the run comes back whole.
    ///
    /// Loaded twice onto the same chain on purpose: that is the test that the
    /// identities are actually re-minted rather than carried, because two
    /// copies claiming the same ids would be two rows that every route and
    /// every lane could not tell apart.
    #[test]
    fn a_container_saves_as_one_preset_and_lands_twice_independently() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("run.mooloop-effect-run");

        let mut session = Session::default();
        for kind in [EffectKind::Delay, EffectKind::Filter, EffectKind::Drive] {
            session.insert_effect_at(kind, usize::MAX).expect("room");
        }
        // A box around the filter and the drive, with a dialled-in mix.
        session.wrap_effects_in_container(1..3).expect("wrapped");
        if let mooloop_core::EffectParams::Chain(chain) =
            &mut session.channels[0].effects[1].params
        {
            chain.mix = 0.4;
        }
        session.channels[0].effects[2].bypassed = true;
        session.channels[0].effects[3].wet_dry = 0.25;

        let container = session.channels[0].effects[1].id;
        session.pending_preset_save = Some(PresetSaveTarget::Effect {
            target: EffectTarget::Channel(0),
            device: container,
        });
        let source = session.take_preset_save(120, 50).expect("a save was pending");
        let run = source.run.expect("a container save carries its run");
        assert_eq!(run.effects.len(), 3, "the box did not bring its contents");
        assert!(
            run.effects.iter().all(|effect| !effect.id.is_assigned()),
            "the preset carried identities out of the chain it was lifted from"
        );

        mooloop_project::save_effect_run_preset(&path, &run, info("Boxed"), AssetMode::Embedded)
            .unwrap();
        let mooloop_project::LoadedDocument::EffectRun(loaded) =
            load_bundle(&path).unwrap().document
        else {
            panic!("not a run");
        };

        // Onto a bus, which is a different chain entirely.
        let mut destination = Session::default();
        destination.effect_target = EffectTarget::Bus(1);
        destination
            .insert_effect_at(EffectKind::Chain, 0)
            .expect("room");
        destination
            .insert_effect_at(EffectKind::Chain, 1)
            .expect("room");

        let first = destination
            .load_effect_run(0, &loaded, "Boxed")
            .expect("a container is there");
        let second_slot = first.landed;
        let second = destination
            .load_effect_run(second_slot, &loaded, "Boxed")
            .expect("the second container is there");

        let chain = &destination.buses[1].effects;
        assert_eq!(
            chain.iter().map(EffectSlotState::kind).collect::<Vec<_>>(),
            [
                EffectKind::Chain,
                EffectKind::Filter,
                EffectKind::Drive,
                EffectKind::Chain,
                EffectKind::Filter,
                EffectKind::Drive,
            ]
        );
        assert_eq!(mooloop_core::span_problem(chain), None);
        // The settings came with it, host controls included -- those default
        // on load, so a restore that dropped them would be silent.
        assert!(chain[1].bypassed && chain[4].bypassed);
        assert!((chain[2].wet_dry - 0.25).abs() < 1.0e-6);
        assert!((chain[5].wet_dry - 0.25).abs() < 1.0e-6);

        // Two copies, six distinct identities.
        let mut ids: Vec<_> = chain.iter().map(|effect| effect.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 6, "the two copies share identities");
        assert!(first.devices.iter().all(|id| !second.devices.contains(id)));
    }

    /// A run that does not start with its container is not a container
    /// preset, and a chain row that is not a container has nothing to replace.
    /// Both are refused rather than half-applied.
    #[test]
    fn a_malformed_run_is_refused_and_the_chain_is_unchanged() {
        let mut session = Session::default();
        session.insert_effect_at(EffectKind::Delay, 0).expect("room");
        let before = session.channels[0].effects.clone();

        let headless = mooloop_core::EffectRun {
            effects: vec![EffectSlotState::of_kind(EffectKind::Filter)],
        };
        assert!(session.load_effect_run(0, &headless, "No").is_none());

        let fine = mooloop_core::EffectRun {
            effects: vec![EffectSlotState::of_kind(EffectKind::Chain)],
        };
        // Slot 0 holds a delay, not a container.
        assert!(session.load_effect_run(0, &fine, "No").is_none());
        assert_eq!(session.channels[0].effects, before);
    }

    fn kinds(session: &Session) -> Vec<EffectKind> {
        session.channels[0]
            .effects
            .iter()
            .map(EffectSlotState::kind)
            .collect()
    }

    fn depths(session: &Session) -> Vec<usize> {
        let effects = &session.channels[0].effects;
        (0..effects.len())
            .map(|slot| mooloop_core::depth_at(effects, slot))
            .collect()
    }

    #[test]
    fn an_empty_effect_presets_directory_lists_nothing() {
        let temp = tempfile::tempdir().unwrap();
        assert!(list_presets(&temp.path().join("effects").join("delay")).is_empty());
        std::fs::create_dir_all(temp.path().join("effects").join("delay")).unwrap();
        assert!(list_presets(&temp.path().join("effects").join("delay")).is_empty());
    }

    /// The header label is how the row got to these settings, so it names the
    /// device rather than the position: a reorder cannot move it, it is
    /// dropped with the device, and a refused load never claims it.
    #[test]
    fn a_rows_preset_label_follows_the_device_and_dies_with_it() {
        let mut session = Session::default();
        let delay = session
            .insert_effect_at(EffectKind::Delay, 0)
            .expect("room")
            .device;
        let filter = session
            .insert_effect_at(EffectKind::Filter, 1)
            .expect("room")
            .device;
        let target = EffectTarget::Channel(0);

        session
            .load_effect_preset(0, &dialled_in_delay(), "Dub Runaway")
            .expect("same kind");
        assert_eq!(
            session.effect_preset_name(target, delay),
            Some("Dub Runaway")
        );
        assert_eq!(session.effect_preset_name(target, filter), None);

        // A refused load leaves the label alone rather than claiming a row it
        // did not change.
        assert_eq!(
            session.load_effect_preset(1, &dialled_in_delay(), "Dub Runaway"),
            None
        );
        assert_eq!(session.effect_preset_name(target, filter), None);

        // The label is keyed by the device, so a reorder is not an event it
        // can observe. Under the slot scheme this map had to be rebuilt.
        session.move_effect_to(0, 1).expect("in range");
        assert_eq!(
            session.effect_preset_name(target, delay),
            Some("Dub Runaway")
        );
        assert_eq!(session.effect_preset_name(target, filter), None);

        // A knob move keeps it: the settings still came from that preset.
        session.set_effect_wet_dry(1, 0.2).expect("a delay is there");
        assert_eq!(
            session.effect_preset_name(target, delay),
            Some("Dub Runaway")
        );

        // Removing the device takes it.
        session.remove_effect_at(1).expect("in range");
        assert_eq!(session.effect_preset_name(target, delay), None);
        assert!(session.effect_preset_names.is_empty());
    }

    /// Two chains do not share labels, and an identity minted on one is not
    /// the identity of the device that happens to sit in the same position on
    /// the other.
    #[test]
    fn a_label_belongs_to_one_chain() {
        let mut session = Session::default();
        let gate = session
            .insert_effect_at(EffectKind::Gate, 0)
            .expect("room")
            .device;
        session.set_effect_preset_name(EffectTarget::Channel(0), gate, "Tight Drum Gate");

        session.effect_target = EffectTarget::Bus(1);
        session.insert_effect_at(EffectKind::Limiter, 0).expect("room");
        let bus_gate = session
            .insert_effect_at(EffectKind::Gate, 0)
            .expect("room")
            .device;

        assert_eq!(
            session.effect_preset_name(EffectTarget::Channel(0), gate),
            Some("Tight Drum Gate")
        );
        assert_eq!(session.effect_preset_name(EffectTarget::Bus(1), bus_gate), None);
        assert_eq!(session.effect_preset_name(EffectTarget::Bus(1), gate), None);
    }
}

/// The rate a synced modulation effect should now be running at, as a command,
/// or `None` while it is free-running and there is nothing to restate.
fn resolved_modulation_rate(
    params: &mut mooloop_core::ModulationParams,
    target: EffectTarget,
    slot: i32,
    bpm: f64,
) -> Option<EngineCommand> {
    if !params.tempo_sync {
        return None;
    }
    params.rate_hz = params.synced_rate_hz(bpm);
    Some(EngineCommand::SetEffectParam {
        target,
        slot: u8::try_from(slot).ok()?,
        id: mooloop_core::MODULATION_PARAM_RATE_HZ,
        value: params.rate_hz,
    })
}
