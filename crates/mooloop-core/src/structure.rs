//! The device chain, and the structural edits that rearrange it.
//!
//! A rack row is named by *what it is*, not by where it sits.
//! [`DeviceId`](crate::DeviceId) is minted when a device is added, carried
//! through every reorder, and never reused; `ParamOwner::Effect { device }`
//! is what a modulation route and an automation lane persist. Position is a
//! derived thing -- the realtime path indexes by it, because a bounded array
//! lookup is what a callback can afford, and nothing saves it.
//!
//! So reordering a chain is not an addressing event. Nothing has to be
//! remapped, because nothing that was saved ever mentioned position. Only
//! *removal* still reaches outside the chain, and it reaches with an
//! identity: the routes and lanes that named the departed device go with it.
//!
//! This is the division [`ModSourceId`](crate::ModSourceId) already draws for
//! modulator slots, and it is the reason a container -- a device holding
//! devices -- is a tree of the same rows rather than a format migration:
//! nesting changes structure, and structure is not identity.
//!
//! Channels are still addressed by position, and [`ChannelEdit`] is what
//! carries an address across a channel insertion or deletion.

use crate::automation::AutomationLane;
use crate::effect::{DeviceId, EffectSlotState};
use crate::mixer::EffectTarget;
use crate::modulation::{ParamAddr, ParamOwner};
use crate::MAX_EFFECTS_PER_CHANNEL;

/// One chain of devices, and the counter that keeps their identities unique.
///
/// Reads go through `Deref` to the slice, so a caller that only wants the
/// device in row 2 indexes it exactly as it indexed the `Vec` this replaced.
/// Structural change does not: [`push`](Self::push), [`insert`](Self::insert),
/// [`remove`](Self::remove) and [`move_device`](Self::move_device) are the
/// only ways in and out, which is what makes "mint on add" impossible to skip.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceChain {
    slots: Vec<EffectSlotState>,
    /// Next identity to mint. Monotonic within the chain, so removing a
    /// device and adding another never hands the newcomer the departed
    /// device's routes.
    next_id: u32,
}

impl Default for DeviceChain {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceChain {
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            next_id: 0,
        }
    }

    /// Take a bare list of rows and give every one of them an identity.
    ///
    /// A row that already has one keeps it; a row that does not takes its
    /// position, which is exactly the number every address written before
    /// device identity was already using. That is the whole compatibility
    /// story: an old project's `slot = 2` decodes as `DeviceId(2)`, and the
    /// device it named is the one this hands `DeviceId(2)` to.
    pub fn adopt(slots: Vec<EffectSlotState>) -> Self {
        let mut slots = slots;
        for (position, slot) in slots.iter_mut().enumerate() {
            if slot.id.is_unset() {
                slot.id = DeviceId(position as u32);
            }
        }
        let next_id = slots
            .iter()
            .map(|slot| slot.id.0.wrapping_add(1))
            .max()
            .unwrap_or(0);
        Self { slots, next_id }
    }

    fn mint(&mut self) -> DeviceId {
        // `UNSET` is the one value that means "no identity", so it is stepped
        // over rather than handed out. Reaching it takes four billion inserts
        // into one chain; the guard costs a comparison and removes the need
        // to think about it again.
        if self.next_id == DeviceId::UNSET.0 {
            self.next_id = self.next_id.wrapping_add(1);
        }
        let id = DeviceId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Append a device, stamping it with a fresh identity. `None` when the
    /// chain is already at the addressable limit.
    pub fn push(&mut self, state: EffectSlotState) -> Option<DeviceId> {
        let at = self.slots.len();
        self.insert(at, state).map(|(_, id)| id)
    }

    /// Insert `state` at `at` (clamped to the end), stamping it with a fresh
    /// identity and returning where it landed. `None` when the chain is full.
    pub fn insert(&mut self, at: usize, state: EffectSlotState) -> Option<(usize, DeviceId)> {
        if self.slots.len() >= MAX_EFFECTS_PER_CHANNEL {
            return None;
        }
        let at = at.min(self.slots.len());
        let id = self.mint();
        self.slots.insert(at, EffectSlotState { id, ..state });
        Some((at, id))
    }

    /// Take the device at `at` out, returning it. `None` when there is
    /// nothing there. The caller is what drops the routes and lanes that
    /// named it -- see [`drop_lanes_for_device`].
    pub fn remove(&mut self, at: usize) -> Option<EffectSlotState> {
        (at < self.slots.len()).then(|| self.slots.remove(at))
    }

    /// Move the device at `from` to position `to`. Returns whether anything
    /// moved.
    ///
    /// Nothing else in the project has to hear about this. Every route and
    /// every lane named the device, and the device is the thing that moved.
    pub fn move_device(&mut self, from: usize, to: usize) -> bool {
        if from >= self.slots.len() || to >= self.slots.len() || from == to {
            return false;
        }
        let device = self.slots.remove(from);
        self.slots.insert(to, device);
        true
    }

    /// The identity of the device in `slot`, if any.
    pub fn id_at(&self, slot: usize) -> Option<DeviceId> {
        self.slots.get(slot).map(|state| state.id)
    }

    /// Where `device` currently sits: the runtime locator, resolved from the
    /// identity whenever the chain changes rather than stored anywhere.
    pub fn position_of(&self, device: DeviceId) -> Option<u8> {
        self.slots
            .iter()
            .position(|state| state.id == device)
            .and_then(|slot| u8::try_from(slot).ok())
    }

    /// One row, to change what the device *is doing*: a knob, a bypass, a
    /// trim. Deliberately not a whole-row assignment -- see
    /// [`replace_state`](Self::replace_state).
    pub fn get_mut(&mut self, slot: usize) -> Option<&mut EffectSlotState> {
        self.slots.get_mut(slot)
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, EffectSlotState> {
        self.slots.iter_mut()
    }

    /// Put different settings in `slot`, keeping the device that is there.
    ///
    /// This is what loading a preset into a row is: the patch changes and the
    /// device does not, so every route and lane aimed at it goes on meaning
    /// the same knob. Any identity `state` arrives with is discarded, which
    /// is why this exists rather than an assignment through the slice.
    pub fn replace_state(&mut self, slot: usize, state: EffectSlotState) -> bool {
        let Some(row) = self.slots.get_mut(slot) else {
            return false;
        };
        *row = EffectSlotState { id: row.id, ..state };
        true
    }
}

impl std::ops::Deref for DeviceChain {
    type Target = [EffectSlotState];

    fn deref(&self) -> &Self::Target {
        &self.slots
    }
}

impl FromIterator<EffectSlotState> for DeviceChain {
    fn from_iter<T: IntoIterator<Item = EffectSlotState>>(iter: T) -> Self {
        Self::adopt(iter.into_iter().collect())
    }
}

impl<'a> IntoIterator for &'a DeviceChain {
    type Item = &'a EffectSlotState;
    type IntoIter = std::slice::Iter<'a, EffectSlotState>;

    fn into_iter(self) -> Self::IntoIter {
        self.slots.iter()
    }
}

/// The persisted form is the bare list of rows it always was, each now
/// carrying its own `id`. Nothing about the chain's shape is written: the
/// mint counter is derived on load from the ids present, so a saved project
/// cannot disagree with itself about which identities are already spent.
impl serde::Serialize for DeviceChain {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.slots.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for DeviceChain {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(Self::adopt(Vec::<EffectSlotState>::deserialize(
            deserializer,
        )?))
    }
}

impl ParamAddr {
    /// Whether this address names `device` in `scope`.
    pub fn names_device(&self, scope: EffectTarget, device: DeviceId) -> bool {
        self.scope == scope && self.owner == ParamOwner::Effect { device }
    }
}

/// Drop every lane that names `device` in `scope`. Returns whether anything
/// went.
///
/// This is all that is left of what a chain edit used to owe the rest of the
/// project. A reorder owes it nothing; only a departure does, and a departure
/// names one identity rather than permuting a table. Removal is in place: the
/// engine's lane storage is preallocated, and this runs on its thread.
pub fn drop_lanes_for_device(
    lanes: &mut Vec<AutomationLane>,
    scope: EffectTarget,
    device: DeviceId,
) -> bool {
    let before = lanes.len();
    lanes.retain(|lane| !lane.target.names_device(scope, device));
    lanes.len() != before
}

/// One edit to the channel list, and where every channel index lands after
/// it. Channels are still addressed by position -- a route or lane scoped to
/// channel 4 has to become channel 3 when channel 1 is deleted, and has to be
/// dropped when channel 4 itself is.
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

    fn chain(kinds: &[EffectKind]) -> DeviceChain {
        kinds
            .iter()
            .copied()
            .map(EffectSlotState::of_kind)
            .collect()
    }

    fn kinds(chain: &DeviceChain) -> Vec<EffectKind> {
        chain.iter().map(EffectSlotState::kind).collect()
    }

    fn ids(chain: &DeviceChain) -> Vec<u32> {
        chain.iter().map(|slot| slot.id.0).collect()
    }

    const SCOPE: EffectTarget = EffectTarget::Channel(2);

    #[test]
    fn every_added_device_is_minted_a_fresh_identity() {
        let mut chain = chain(&[EffectKind::Filter, EffectKind::Drive]);
        assert_eq!(ids(&chain), [0, 1]);
        let (slot, id) = chain
            .insert(0, EffectSlotState::of_kind(EffectKind::Delay))
            .expect("room");
        assert_eq!((slot, id), (0, DeviceId(2)));
        assert_eq!(ids(&chain), [2, 0, 1]);
    }

    /// The property the whole change exists for: a reorder is not an
    /// addressing event, so an address written before it means the same
    /// device after it, with nothing having been rewritten.
    #[test]
    fn a_reorder_moves_no_identity() {
        let mut chain = chain(&[
            EffectKind::Filter,
            EffectKind::Drive,
            EffectKind::Delay,
            EffectKind::Reverb,
        ]);
        let filter = chain.id_at(0).expect("occupied");
        assert!(chain.move_device(0, 2));
        assert_eq!(
            kinds(&chain),
            [
                EffectKind::Drive,
                EffectKind::Delay,
                EffectKind::Filter,
                EffectKind::Reverb
            ]
        );
        assert_eq!(chain.position_of(filter), Some(2));
        assert_eq!(chain.id_at(2), Some(filter));
        // And the address itself never moved: it named the filter before the
        // drag and names the filter after it, unrewritten.
        let address = ParamAddr::effect(SCOPE, filter, 7);
        assert_eq!(address, ParamAddr::effect(SCOPE, chain.id_at(2).unwrap(), 7));
    }

    /// A departed device never comes back, and its identity never goes to
    /// anybody else -- which is what stops a new device inheriting the routes
    /// of the one it replaced.
    #[test]
    fn an_identity_is_never_reused() {
        let mut chain = chain(&[EffectKind::Filter, EffectKind::Drive]);
        let drive = chain.id_at(1).expect("occupied");
        assert_eq!(chain.remove(1).map(|state| state.id), Some(drive));
        let replacement = chain
            .push(EffectSlotState::of_kind(EffectKind::Delay))
            .expect("room");
        assert_ne!(replacement, drive);
        assert_eq!(chain.position_of(drive), None);
    }

    #[test]
    fn a_full_chain_refuses_an_insert() {
        let mut full = chain(&vec![EffectKind::Filter; MAX_EFFECTS_PER_CHANNEL]);
        assert!(full
            .insert(0, EffectSlotState::of_kind(EffectKind::Drive))
            .is_none());
        assert_eq!(full.len(), MAX_EFFECTS_PER_CHANNEL);
        let mut chain = chain(&[EffectKind::Filter]);
        let (slot, _) = chain
            .insert(9, EffectSlotState::of_kind(EffectKind::Drive))
            .expect("room");
        assert_eq!(slot, 1, "an insert past the end lands at the end");
    }

    /// The compatibility path: a project written before device identity has
    /// no ids at all, and its rows take their positions -- the very numbers
    /// its saved addresses were already using.
    #[test]
    fn rows_without_an_identity_take_their_position_as_one() {
        let legacy: Vec<EffectSlotState> = [EffectKind::Filter, EffectKind::Drive, EffectKind::Delay]
            .into_iter()
            .map(EffectSlotState::of_kind)
            .collect();
        assert!(legacy.iter().all(|slot| slot.id.is_unset()));
        let mut chain = DeviceChain::adopt(legacy);
        assert_eq!(ids(&chain), [0, 1, 2]);
        // And the counter clears them, so the next device is not handed an
        // identity a legacy address already names.
        assert_eq!(
            chain.push(EffectSlotState::of_kind(EffectKind::Reverb)),
            Some(DeviceId(3))
        );
    }

    #[test]
    fn a_saved_chain_round_trips_its_identities() {
        let mut chain = chain(&[EffectKind::Filter, EffectKind::Drive, EffectKind::Delay]);
        chain.remove(0);
        chain.push(EffectSlotState::of_kind(EffectKind::Reverb));
        let encoded = toml::to_string(&Wrapper { effects: chain.clone() }).expect("encodes");
        let decoded: Wrapper = toml::from_str(&encoded).expect("decodes");
        assert_eq!(decoded.effects, chain);
        assert_eq!(ids(&decoded.effects), [1, 2, 3]);
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Wrapper {
        effects: DeviceChain,
    }

    #[test]
    fn lanes_that_named_a_departed_device_go_with_it() {
        let mut chain = chain(&[EffectKind::Filter, EffectKind::Drive, EffectKind::Delay]);
        let drive = chain.id_at(1).expect("occupied");
        let filter = chain.id_at(0).expect("occupied");
        let mut lanes = vec![
            AutomationLane::new(ParamAddr::effect(SCOPE, filter, 1)),
            AutomationLane::new(ParamAddr::effect(SCOPE, drive, 1)),
            AutomationLane::new(ParamAddr::strip(SCOPE, 0)),
            // Another chain's device, which this edit is none of the business
            // of even if the two chains happened to mint the same number.
            AutomationLane::new(ParamAddr::effect(EffectTarget::Bus(0), drive, 1)),
        ];
        chain.remove(1);
        assert!(drop_lanes_for_device(&mut lanes, SCOPE, drive));
        let targets: Vec<ParamAddr> = lanes.iter().map(|lane| lane.target).collect();
        assert_eq!(
            targets,
            [
                ParamAddr::effect(SCOPE, filter, 1),
                ParamAddr::strip(SCOPE, 0),
                ParamAddr::effect(EffectTarget::Bus(0), drive, 1),
            ]
        );
        assert!(!drop_lanes_for_device(&mut lanes, SCOPE, drive));
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
