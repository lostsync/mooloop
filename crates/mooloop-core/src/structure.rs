//! Structural edits -- moving devices and channels -- and everything that has
//! to follow them.
//!
//! A parameter is addressed by *where* its owner sits: `ParamOwner::Effect {
//! slot }` is a position in a chain, and `EffectTarget::Channel(n)` is a
//! position in the channel list. Positions are what the realtime path indexes,
//! which is why they are cheap; the price is that a structural edit changes
//! them. Every route, every automation lane, and the lane the editor happens
//! to be showing named a device, not a number, and the number has to follow
//! the device.
//!
//! This module states each structural edit as one permutation, computed once
//! and applied everywhere a position is stored. The UI's model and the
//! engine's mirror both run the same [`SlotRemap`] for the same gesture, so
//! neither side can end up pointing at a different device than the other.
//! The alternative -- durable ids on effect slots, resolved to positions on
//! every read -- was what modulator sources needed, because a route names a
//! source *from elsewhere*. Nothing outside a chain names an effect slot
//! except through `ParamAddr`, and `ParamAddr` travels through here.

use crate::automation::AutomationLane;
use crate::effect::EffectSlotState;
use crate::mixer::EffectTarget;
use crate::modulation::{ParamAddr, ParamOwner};
use crate::MAX_EFFECTS_PER_CHANNEL;

/// Where every slot of a chain lands after one structural edit.
///
/// Built by [`move_effect`], [`insert_effect`] and [`remove_effect`] over
/// the whole addressable chain rather than its populated length, so the
/// engine -- which knows only which slots hold nodes -- computes exactly the
/// same table as the model that knows the `Vec`. `None` is a slot whose
/// device is gone.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SlotRemap {
    map: [Option<u8>; MAX_EFFECTS_PER_CHANNEL],
}

impl std::fmt::Debug for SlotRemap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let moved: Vec<(usize, Option<u8>)> = self
            .map
            .iter()
            .enumerate()
            .filter(|(old, new)| **new != Some(*old as u8))
            .map(|(old, new)| (old, *new))
            .collect();
        f.debug_struct("SlotRemap").field("moved", &moved).finish()
    }
}

impl SlotRemap {
    pub const fn identity() -> Self {
        let mut map = [None; MAX_EFFECTS_PER_CHANNEL];
        let mut slot = 0;
        while slot < MAX_EFFECTS_PER_CHANNEL {
            map[slot] = Some(slot as u8);
            slot += 1;
        }
        Self { map }
    }

    /// The permutation of moving the device in `from` to position `to`:
    /// it lands on `to`, and everything between shifts one place toward
    /// `from`. Out-of-range or equal positions are the identity.
    pub fn for_move(from: usize, to: usize) -> Self {
        let mut remap = Self::identity();
        if from == to || from >= MAX_EFFECTS_PER_CHANNEL || to >= MAX_EFFECTS_PER_CHANNEL {
            return remap;
        }
        remap.map[from] = Some(to as u8);
        if from < to {
            for slot in from + 1..=to {
                remap.map[slot] = Some((slot - 1) as u8);
            }
        } else {
            for slot in to..from {
                remap.map[slot] = Some((slot + 1) as u8);
            }
        }
        remap
    }

    /// The permutation of inserting a device at `at`: that slot and every
    /// one after it shift up by one. The last addressable slot has nowhere
    /// to go, which is why [`insert_effect`] refuses a full chain.
    pub fn for_insert(at: usize) -> Self {
        let mut remap = Self::identity();
        for slot in at..MAX_EFFECTS_PER_CHANNEL {
            remap.map[slot] = u8::try_from(slot + 1).ok();
        }
        remap
    }

    /// The permutation of removing the device at `at`: it resolves to
    /// nothing, and everything after it shifts down by one.
    pub fn for_remove(at: usize) -> Self {
        let mut remap = Self::identity();
        if at >= MAX_EFFECTS_PER_CHANNEL {
            return remap;
        }
        remap.map[at] = None;
        for slot in at + 1..MAX_EFFECTS_PER_CHANNEL {
            remap.map[slot] = Some((slot - 1) as u8);
        }
        remap
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::identity()
    }

    /// Where the device that was in `old` now sits.
    pub fn slot(&self, old: u8) -> Option<u8> {
        self.map[old as usize]
    }

    /// Where `address` points after the edit. An address outside `scope`, or
    /// one not owned by an effect slot, is untouched; `None` means the device
    /// it named is gone, and whatever held the address should let go of it.
    pub fn address(&self, scope: EffectTarget, address: ParamAddr) -> Option<ParamAddr> {
        if address.scope != scope {
            return Some(address);
        }
        let ParamOwner::Effect { slot } = address.owner else {
            return Some(address);
        };
        let slot = self.slot(slot)?;
        Some(ParamAddr {
            owner: ParamOwner::Effect { slot },
            ..address
        })
    }
}

/// Move the device at `from` to position `to`, returning the permutation
/// everything that names a slot in this chain must now run. `None` when
/// nothing moved: either position is out of range, or they are equal.
pub fn move_effect(
    effects: &mut Vec<EffectSlotState>,
    from: usize,
    to: usize,
) -> Option<SlotRemap> {
    if from >= effects.len() || to >= effects.len() || from == to {
        return None;
    }
    let effect = effects.remove(from);
    effects.insert(to, effect);
    Some(SlotRemap::for_move(from, to))
}

/// Insert `effect` at `at` (clamped to the end of the chain), returning the
/// slot it landed in and the permutation. `None` when the chain is full.
pub fn insert_effect(
    effects: &mut Vec<EffectSlotState>,
    at: usize,
    effect: EffectSlotState,
) -> Option<(usize, SlotRemap)> {
    if effects.len() >= MAX_EFFECTS_PER_CHANNEL {
        return None;
    }
    let at = at.min(effects.len());
    effects.insert(at, effect);
    Some((at, SlotRemap::for_insert(at)))
}

/// Remove the device at `at`, returning it and the permutation. `None` when
/// there is nothing there.
pub fn remove_effect(
    effects: &mut Vec<EffectSlotState>,
    at: usize,
) -> Option<(EffectSlotState, SlotRemap)> {
    if at >= effects.len() {
        return None;
    }
    let effect = effects.remove(at);
    Some((effect, SlotRemap::for_remove(at)))
}

/// Re-point every lane that addresses `scope`'s chain, dropping those whose
/// device is gone. Returns whether anything changed. Removal is in place:
/// the engine's lane storage is preallocated, and this runs on its thread.
pub fn retarget_lanes(
    lanes: &mut Vec<AutomationLane>,
    scope: EffectTarget,
    remap: &SlotRemap,
) -> bool {
    let mut changed = false;
    lanes.retain_mut(|lane| match remap.address(scope, lane.target) {
        Some(target) => {
            changed |= target != lane.target;
            lane.target = target;
            true
        }
        None => {
            changed = true;
            false
        }
    });
    changed
}

/// One edit to the channel list, and where every channel index lands after
/// it. Channels are addressed by position exactly as effect slots are, so a
/// route or lane scoped to channel 4 has to become channel 3 when channel 1
/// is deleted -- and has to be dropped when channel 4 itself is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelEdit {
    Removed(u8),
    Inserted(u8),
}

impl ChannelEdit {
    /// Where the channel that was at `old` now sits.
    pub fn channel(self, old: u8) -> Option<u8> {
        match self {
            Self::Removed(at) if old == at => None,
            Self::Removed(at) if old > at => Some(old - 1),
            Self::Inserted(at) if old >= at => old.checked_add(1),
            _ => Some(old),
        }
    }

    /// Where `address` points after the edit. Bus scopes are untouched: a bus
    /// exists independently of which channels feed it.
    pub fn address(self, address: ParamAddr) -> Option<ParamAddr> {
        let EffectTarget::Channel(channel) = address.scope else {
            return Some(address);
        };
        let channel = self.channel(channel)?;
        Some(ParamAddr {
            scope: EffectTarget::Channel(channel),
            ..address
        })
    }
}

/// Re-scope every channel-addressed lane after a channel edit, dropping the
/// ones whose channel is gone. Returns whether anything changed.
pub fn rescope_lanes(lanes: &mut Vec<AutomationLane>, edit: ChannelEdit) -> bool {
    let mut changed = false;
    lanes.retain_mut(|lane| match edit.address(lane.target) {
        Some(target) => {
            changed |= target != lane.target;
            lane.target = target;
            true
        }
        None => {
            changed = true;
            false
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::EffectKind;

    fn chain(kinds: &[EffectKind]) -> Vec<EffectSlotState> {
        kinds.iter().map(|kind| EffectSlotState::of_kind(*kind)).collect()
    }

    fn kinds(effects: &[EffectSlotState]) -> Vec<EffectKind> {
        effects.iter().map(EffectSlotState::kind).collect()
    }

    const SCOPE: EffectTarget = EffectTarget::Channel(2);

    #[test]
    fn a_move_forward_shifts_the_slots_it_passes_over_down() {
        let mut effects = chain(&[
            EffectKind::Filter,
            EffectKind::Drive,
            EffectKind::Delay,
            EffectKind::Reverb,
        ]);
        let remap = move_effect(&mut effects, 0, 2).expect("moved");
        assert_eq!(
            kinds(&effects),
            [
                EffectKind::Drive,
                EffectKind::Delay,
                EffectKind::Filter,
                EffectKind::Reverb
            ]
        );
        assert_eq!(remap.slot(0), Some(2));
        assert_eq!(remap.slot(1), Some(0));
        assert_eq!(remap.slot(2), Some(1));
        assert_eq!(remap.slot(3), Some(3));
    }

    #[test]
    fn a_move_backward_shifts_the_slots_it_passes_over_up() {
        let mut effects = chain(&[
            EffectKind::Filter,
            EffectKind::Drive,
            EffectKind::Delay,
            EffectKind::Reverb,
        ]);
        let remap = move_effect(&mut effects, 3, 1).expect("moved");
        assert_eq!(
            kinds(&effects),
            [
                EffectKind::Filter,
                EffectKind::Reverb,
                EffectKind::Drive,
                EffectKind::Delay
            ]
        );
        assert_eq!(remap.slot(3), Some(1));
        assert_eq!(remap.slot(1), Some(2));
        assert_eq!(remap.slot(2), Some(3));
        assert_eq!(remap.slot(0), Some(0));
    }

    /// The model applies one permutation for a move; the engine, which only
    /// has a move primitive too, applies the same one. Insert and remove are
    /// spelled on the engine as install-at-tail-then-move and
    /// move-to-tail-then-remove, so those compositions must equal the
    /// model's single-step tables on every populated slot.
    #[test]
    fn the_engines_two_step_insert_and_remove_compose_to_the_models_tables() {
        let len = 5;
        for at in 0..len {
            let model = SlotRemap::for_insert(at);
            let engine = SlotRemap::for_move(len, at);
            for slot in 0..len as u8 {
                assert_eq!(model.slot(slot), engine.slot(slot), "insert at {at}, slot {slot}");
            }
        }
        for at in 0..len {
            let model = SlotRemap::for_remove(at);
            let to_tail = SlotRemap::for_move(at, len - 1);
            let drop_tail = SlotRemap::for_remove(len - 1);
            for slot in 0..len as u8 {
                let engine = to_tail.slot(slot).and_then(|slot| drop_tail.slot(slot));
                assert_eq!(model.slot(slot), engine, "remove at {at}, slot {slot}");
            }
        }
    }

    #[test]
    fn an_address_follows_its_device_and_a_removed_device_takes_its_address_with_it() {
        let remap = SlotRemap::for_remove(1);
        let before = ParamAddr::effect(SCOPE, 2, 7);
        assert_eq!(
            remap.address(SCOPE, before),
            Some(ParamAddr::effect(SCOPE, 1, 7))
        );
        assert_eq!(remap.address(SCOPE, ParamAddr::effect(SCOPE, 1, 7)), None);
        // Another chain, or another owner, is none of this edit's business.
        let elsewhere = ParamAddr::effect(EffectTarget::Bus(0), 1, 7);
        assert_eq!(remap.address(SCOPE, elsewhere), Some(elsewhere));
        let strip = ParamAddr::strip(SCOPE, 0);
        assert_eq!(remap.address(SCOPE, strip), Some(strip));
    }

    #[test]
    fn lanes_follow_the_permutation_and_the_orphan_is_dropped() {
        let mut lanes = vec![
            AutomationLane::new(ParamAddr::effect(SCOPE, 0, 1)),
            AutomationLane::new(ParamAddr::effect(SCOPE, 1, 1)),
            AutomationLane::new(ParamAddr::effect(SCOPE, 2, 1)),
            AutomationLane::new(ParamAddr::strip(SCOPE, 0)),
        ];
        assert!(retarget_lanes(&mut lanes, SCOPE, &SlotRemap::for_remove(1)));
        let targets: Vec<ParamAddr> = lanes.iter().map(|lane| lane.target).collect();
        assert_eq!(
            targets,
            [
                ParamAddr::effect(SCOPE, 0, 1),
                ParamAddr::effect(SCOPE, 1, 1),
                ParamAddr::strip(SCOPE, 0),
            ]
        );
        assert!(!retarget_lanes(&mut lanes, SCOPE, &SlotRemap::identity()));
    }

    #[test]
    fn a_full_chain_refuses_an_insert_rather_than_pushing_a_slot_off_the_end() {
        let mut effects = chain(&vec![EffectKind::Filter; MAX_EFFECTS_PER_CHANNEL]);
        assert!(insert_effect(&mut effects, 0, EffectSlotState::of_kind(EffectKind::Drive)).is_none());
        assert_eq!(effects.len(), MAX_EFFECTS_PER_CHANNEL);
        let mut effects = chain(&[EffectKind::Filter]);
        let (slot, remap) =
            insert_effect(&mut effects, 9, EffectSlotState::of_kind(EffectKind::Drive)).unwrap();
        assert_eq!(slot, 1, "an insert past the end lands at the end");
        assert!(remap.is_identity() || remap.slot(1) == Some(2));
    }

    #[test]
    fn channel_indices_close_up_after_a_deletion_and_open_after_an_insert() {
        let removed = ChannelEdit::Removed(1);
        assert_eq!(removed.channel(0), Some(0));
        assert_eq!(removed.channel(1), None);
        assert_eq!(removed.channel(2), Some(1));
        let inserted = ChannelEdit::Inserted(1);
        assert_eq!(inserted.channel(0), Some(0));
        assert_eq!(inserted.channel(1), Some(2));
        assert_eq!(inserted.channel(2), Some(3));
        let bus = ParamAddr::effect(EffectTarget::Bus(3), 0, 0);
        assert_eq!(removed.address(bus), Some(bus));
        assert_eq!(
            removed.address(ParamAddr::strip(EffectTarget::Channel(5), 0)),
            Some(ParamAddr::strip(EffectTarget::Channel(4), 0))
        );
    }
}
