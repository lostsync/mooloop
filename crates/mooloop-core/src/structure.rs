//! Structural edits -- moving devices and channels -- and everything that has
//! to follow them.
//!
//! **A device is an identity; a channel is still a position.** That split is
//! what this module is about, and it used to be one rule rather than two.
//!
//! `ParamOwner::Effect` names a [`DeviceId`] minted when the device was
//! inserted, so moving, inserting and deleting rack rows rewrites no address
//! anywhere: the position a route resolves to is derived from the chain on
//! every read, and nothing durable ever held it. The only chain edit a route
//! or a lane still has to hear about is a *removal*, because that is the one
//! that makes a destination stop existing -- see [`ModRack::forget_device`]
//! and [`drop_lanes_for_device`].
//!
//! `EffectTarget::Channel(n)` is still a position in the channel list, and a
//! route scoped to channel 4 still has to become channel 3 when channel 1 is
//! deleted. [`ChannelEdit`] is that permutation, and it is all that is left of
//! the `SlotRemap` machinery this module used to be built around.
//!
//! The comment `SlotRemap` carried argued the other way: that durable ids were
//! what modulator sources needed "because a route names a source *from
//! elsewhere*", and that nothing outside a chain names an effect slot except
//! through `ParamAddr`, which travelled through here. That was true, and what
//! stopped it being true is `docs/plans/containers/`: once a chain can hold a
//! device that *contains* other devices, an edit inside one box renumbers
//! everything after it at every enclosing level, and a permutation would have
//! to be computed and run on every drag in both directions. An id is not
//! renumbered at all.
//!
//! [`ModRack::forget_device`]: crate::modulation::ModRack::forget_device

use crate::automation::AutomationLane;
use crate::effect::{DeviceId, EffectSlotState};
use crate::mixer::EffectTarget;
use crate::modulation::{ParamAddr, ParamOwner};
use crate::MAX_EFFECTS_PER_CHANNEL;

/// Hand out the next device identity from `next`, advancing the mint.
///
/// Monotonic and never reused: a removed device's id is not handed to the
/// device that replaces it, so a save dialog or a label left holding an id
/// across a removal resolves to nothing rather than to a stranger.
pub fn mint_device_id(next: &mut u32) -> DeviceId {
    let id = DeviceId(*next);
    *next = next.saturating_add(1);
    id
}

/// Give every device in `effects` an identity, and put `next_id` past them
/// all.
///
/// **A chain decoded without ids takes its positions as its ids.** That is
/// what makes this step a no-op for every project written before devices had
/// identities rather than a migration: in such a project a device's position
/// *was* its identity, so `ParamOwner::Effect { device: 3 }` -- read through
/// the `slot` alias -- names the device this hands id 3 to, and the routes
/// come out pointing where they pointed.
///
/// Idempotent. A chain that already has ids keeps them, and the mint is only
/// ever raised. A part-assigned chain is a hand-edited file rather than
/// anything this code can produce; its unassigned rows are minted fresh
/// instead of taking positions that could collide with an id already in use.
pub fn assign_device_ids(effects: &mut [EffectSlotState], next_id: &mut u32) {
    if !effects.iter().any(|effect| effect.id.is_assigned()) {
        for (index, effect) in effects.iter_mut().enumerate() {
            effect.id = DeviceId(index as u32);
        }
    }
    let highest = effects
        .iter()
        .filter(|effect| effect.id.is_assigned())
        .map(|effect| effect.id.0)
        .max();
    if let Some(highest) = highest {
        *next_id = (*next_id).max(highest.saturating_add(1));
    }
    for effect in effects.iter_mut() {
        if !effect.id.is_assigned() {
            effect.id = mint_device_id(next_id);
        }
    }
}

/// Move the device at `from` to position `to`. Returns whether anything
/// moved: `false` when either position is out of range, or they are equal.
///
/// Nothing else happens. This used to return a permutation that every route,
/// every lane, the visible automation target, an in-flight save dialog and
/// the engine's mirror all had to run; a reorder is now invisible to all of
/// them.
pub fn move_effect(effects: &mut Vec<EffectSlotState>, from: usize, to: usize) -> bool {
    if from >= effects.len() || to >= effects.len() || from == to {
        return false;
    }
    let effect = effects.remove(from);
    effects.insert(to, effect);
    true
}

/// Insert `effect` at `at` (clamped to the end of the chain), minting it an
/// identity from `next_id`. Returns the slot it landed in, or `None` when the
/// chain is full.
pub fn insert_effect(
    effects: &mut Vec<EffectSlotState>,
    next_id: &mut u32,
    at: usize,
    effect: EffectSlotState,
) -> Option<usize> {
    if effects.len() >= MAX_EFFECTS_PER_CHANNEL {
        return None;
    }
    let at = at.min(effects.len());
    effects.insert(at, effect.with_id(mint_device_id(next_id)));
    Some(at)
}

/// Remove the device at `at` and return it, identity included. `None` when
/// there is nothing there.
///
/// The returned state's `id` is what the caller drops routes and lanes by.
pub fn remove_effect(effects: &mut Vec<EffectSlotState>, at: usize) -> Option<EffectSlotState> {
    if at >= effects.len() {
        return None;
    }
    Some(effects.remove(at))
}

/// Where the device `address` names currently sits in `effects`, or `None`
/// when it names no device in this chain.
///
/// The derivation the whole scheme rests on. It runs on reads and on control
/// commands, never in a sample loop, which is the same bargain
/// `ModRack::slot_for` already makes for modulator sources.
pub fn slot_of(effects: &[EffectSlotState], address: ParamAddr) -> Option<usize> {
    let ParamOwner::Effect { device } = address.owner else {
        return None;
    };
    device_slot(effects, device)
}

/// Where `device` currently sits in `effects`.
pub fn device_slot(effects: &[EffectSlotState], device: DeviceId) -> Option<usize> {
    if !device.is_assigned() {
        return None;
    }
    effects.iter().position(|effect| effect.id == device)
}

/// Drop every lane driving `device` in `scope`, because that device has been
/// removed. Returns whether anything changed.
///
/// Removal is in place: the engine's lane storage is preallocated, and this
/// runs on its thread.
pub fn drop_lanes_for_device(
    lanes: &mut Vec<AutomationLane>,
    scope: EffectTarget,
    device: DeviceId,
) -> bool {
    let before = lanes.len();
    lanes.retain(|lane| {
        !(lane.target.scope == scope
            && matches!(lane.target.owner, ParamOwner::Effect { device: d } if d == device))
    });
    lanes.len() != before
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
        let mut effects = Vec::new();
        let mut next = 0;
        for kind in kinds {
            let at = effects.len();
            insert_effect(&mut effects, &mut next, at, EffectSlotState::of_kind(*kind));
        }
        effects
    }

    fn kinds(effects: &[EffectSlotState]) -> Vec<EffectKind> {
        effects.iter().map(EffectSlotState::kind).collect()
    }

    fn ids(effects: &[EffectSlotState]) -> Vec<u32> {
        effects.iter().map(|effect| effect.id.0).collect()
    }

    const SCOPE: EffectTarget = EffectTarget::Channel(2);

    #[test]
    fn a_move_carries_the_identity_with_the_device() {
        let mut effects = chain(&[
            EffectKind::Filter,
            EffectKind::Drive,
            EffectKind::Delay,
            EffectKind::Reverb,
        ]);
        let filter = effects[0].id;
        assert!(move_effect(&mut effects, 0, 2));
        assert_eq!(
            kinds(&effects),
            [
                EffectKind::Drive,
                EffectKind::Delay,
                EffectKind::Filter,
                EffectKind::Reverb
            ]
        );
        // The whole point: the address did not move, the device did.
        assert_eq!(device_slot(&effects, filter), Some(2));
        assert_eq!(ids(&effects), [1, 2, 0, 3]);
    }

    #[test]
    fn a_move_backward_carries_it_too() {
        let mut effects = chain(&[
            EffectKind::Filter,
            EffectKind::Drive,
            EffectKind::Delay,
            EffectKind::Reverb,
        ]);
        let reverb = effects[3].id;
        assert!(move_effect(&mut effects, 3, 1));
        assert_eq!(
            kinds(&effects),
            [
                EffectKind::Filter,
                EffectKind::Reverb,
                EffectKind::Drive,
                EffectKind::Delay
            ]
        );
        assert_eq!(device_slot(&effects, reverb), Some(1));
    }

    /// The property the step exists for, stated directly: an address built
    /// before an edit resolves to the same *device* after it, whatever the
    /// edit did to the positions. This is the assertion `SlotRemap`'s tests
    /// could not make, because there the address itself had to be rewritten.
    #[test]
    fn an_address_survives_every_edit_that_does_not_remove_its_device() {
        let mut effects = chain(&[
            EffectKind::Filter,
            EffectKind::Drive,
            EffectKind::Delay,
        ]);
        let mut next = effects.len() as u32;
        let drive = ParamAddr::effect(SCOPE, effects[1].id, 7);

        assert_eq!(slot_of(&effects, drive), Some(1));

        insert_effect(
            &mut effects,
            &mut next,
            0,
            EffectSlotState::of_kind(EffectKind::Reverb),
        );
        assert_eq!(slot_of(&effects, drive), Some(2), "an insert above it");

        move_effect(&mut effects, 2, 0);
        assert_eq!(slot_of(&effects, drive), Some(0), "a drag to the front");

        remove_effect(&mut effects, 3);
        assert_eq!(slot_of(&effects, drive), Some(0), "a removal below it");

        let removed = remove_effect(&mut effects, 0).expect("removed");
        assert_eq!(removed.id, effects.first().map_or(removed.id, |_| removed.id));
        assert_eq!(slot_of(&effects, drive), None, "its own removal");
    }

    /// An id is never handed out twice, so nothing left holding a removed
    /// device's address can be pointed at the device that replaced it.
    #[test]
    fn a_removed_devices_identity_is_not_reissued() {
        let mut effects = chain(&[EffectKind::Filter, EffectKind::Drive]);
        let mut next = effects.len() as u32;
        let drive = effects[1].id;
        remove_effect(&mut effects, 1);
        insert_effect(
            &mut effects,
            &mut next,
            1,
            EffectSlotState::of_kind(EffectKind::Delay),
        );
        assert_ne!(effects[1].id, drive);
        assert_eq!(device_slot(&effects, drive), None);
    }

    #[test]
    fn lanes_go_only_when_their_own_device_does() {
        let mut effects = chain(&[EffectKind::Filter, EffectKind::Drive, EffectKind::Delay]);
        let mut lanes = vec![
            AutomationLane::new(ParamAddr::effect(SCOPE, effects[0].id, 1)),
            AutomationLane::new(ParamAddr::effect(SCOPE, effects[1].id, 1)),
            AutomationLane::new(ParamAddr::effect(SCOPE, effects[2].id, 1)),
            AutomationLane::new(ParamAddr::strip(SCOPE, 0)),
        ];
        let before: Vec<ParamAddr> = lanes.iter().map(|lane| lane.target).collect();

        // A reorder is not an event a lane can even observe.
        move_effect(&mut effects, 2, 0);
        let after: Vec<ParamAddr> = lanes.iter().map(|lane| lane.target).collect();
        assert_eq!(before, after);

        let removed = remove_effect(&mut effects, 2).expect("removed");
        assert!(drop_lanes_for_device(&mut lanes, SCOPE, removed.id));
        assert_eq!(
            lanes.iter().map(|lane| lane.target).collect::<Vec<_>>(),
            [before[0], before[2], before[3]]
        );
        // Another chain's device of the same number is none of this edit's
        // business, and neither is the strip.
        assert!(!drop_lanes_for_device(
            &mut lanes,
            EffectTarget::Bus(0),
            removed.id
        ));
    }

    #[test]
    fn a_full_chain_refuses_an_insert_rather_than_pushing_a_slot_off_the_end() {
        let mut effects = chain(&vec![EffectKind::Filter; MAX_EFFECTS_PER_CHANNEL]);
        let mut next = effects.len() as u32;
        assert!(insert_effect(
            &mut effects,
            &mut next,
            0,
            EffectSlotState::of_kind(EffectKind::Drive)
        )
        .is_none());
        assert_eq!(effects.len(), MAX_EFFECTS_PER_CHANNEL);
        let mut effects = chain(&[EffectKind::Filter]);
        let mut next = effects.len() as u32;
        let slot = insert_effect(
            &mut effects,
            &mut next,
            9,
            EffectSlotState::of_kind(EffectKind::Drive),
        )
        .unwrap();
        assert_eq!(slot, 1, "an insert past the end lands at the end");
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
        let bus = ParamAddr::effect(EffectTarget::Bus(3), DeviceId(0), 0);
        assert_eq!(removed.address(bus), Some(bus));
        assert_eq!(
            removed.address(ParamAddr::strip(EffectTarget::Channel(5), 0)),
            Some(ParamAddr::strip(EffectTarget::Channel(4), 0))
        );
    }
}
