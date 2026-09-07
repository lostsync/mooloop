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
use crate::effect::{DeviceId, EffectParams, EffectSlotState};
use crate::mixer::EffectTarget;
use crate::modulation::{ParamAddr, ParamOwner};
use crate::MAX_EFFECTS_PER_CHANNEL;

/// How deeply containers may nest through the gestures the interface offers.
///
/// A limit on the *gesture*, not on the format: a deeper chain loads and is
/// reported by `integrity.rs` the way an over-long one is. Four, because
/// step 03 preallocates one dry buffer per open container and the price is
/// per-container rather than per-slot, so this bounds a real allocation
/// without bounding anything a musician is likely to reach for.
pub const MAX_CONTAINER_DEPTH: usize = 4;

/// The run of rows `slot` encloses, as `slot + 1 .. end`.
///
/// Empty when `slot` holds a leaf device. Clamped to the length of the chain,
/// so a malformed span reports what is actually there rather than a range
/// that would panic on indexing.
pub fn span_of(effects: &[EffectSlotState], slot: usize) -> std::ops::Range<usize> {
    let Some(EffectParams::Chain(chain)) = effects.get(slot).map(|effect| effect.params) else {
        return slot + 1..slot + 1;
    };
    let start = slot + 1;
    start..(start + chain.children as usize).min(effects.len())
}

/// How many containers enclose `slot`.
///
/// Zero for a top-level row. A container is not counted as enclosing itself.
pub fn depth_at(effects: &[EffectSlotState], slot: usize) -> usize {
    (0..slot.min(effects.len()))
        .filter(|outer| span_of(effects, *outer).contains(&slot))
        .count()
}

/// The innermost container enclosing `slot`, if any.
pub fn parent_of(effects: &[EffectSlotState], slot: usize) -> Option<usize> {
    (0..slot.min(effects.len())).rfind(|outer| span_of(effects, *outer).contains(&slot))
}

/// Why `effects` is not a well-formed chain, or `None` when it is.
///
/// Two invariants, and they are the whole correctness argument for holding a
/// container's children as a span rather than a `Vec`:
///
/// 1. **A span ends inside the chain.** `slot + 1 + children <= len`.
/// 2. **Spans nest.** For any two containers, one run contains the other or
///    they are disjoint. A straddling pair is unrepresentable in a chain any
///    edit here could produce, and is exactly what a hand-edited file could
///    write.
///
/// Depth is deliberately not checked: [`MAX_CONTAINER_DEPTH`] bounds what the
/// interface will build, not what the format may hold.
pub fn span_problem(effects: &[EffectSlotState]) -> Option<String> {
    let containers: Vec<(usize, usize)> = effects
        .iter()
        .enumerate()
        .filter_map(|(slot, effect)| match effect.params {
            EffectParams::Chain(chain) => Some((slot, chain.children as usize)),
            _ => None,
        })
        .collect();
    for (slot, children) in &containers {
        if slot + 1 + children > effects.len() {
            return Some(format!(
                "the container in slot {} encloses {children} devices, but the chain ends {} rows after it",
                slot + 1,
                effects.len() - slot - 1
            ));
        }
    }
    for (a, _) in &containers {
        for (b, _) in &containers {
            if a >= b {
                continue;
            }
            let outer = span_of(effects, *a);
            let inner = span_of(effects, *b);
            // `b` starts inside `a`'s run, so `b`'s run has to end inside it
            // too. Anything else is a straddle.
            if outer.contains(b) && inner.end > outer.end {
                return Some(format!(
                    "the containers in slots {} and {} overlap without one holding the other",
                    a + 1,
                    b + 1
                ));
            }
        }
    }
    None
}

/// The rows that move, or go, when the device in `slot` does: itself, and
/// everything inside it when it is a container.
///
/// Contiguous by construction, which is the property the whole representation
/// rests on -- a container's `children` counts every row in its run at every
/// depth, because the run is a stretch of one flat list.
pub fn run_of(effects: &[EffectSlotState], slot: usize) -> std::ops::Range<usize> {
    slot..span_of(effects, slot).end.max((slot + 1).min(effects.len()))
}

/// Grow or shrink every container enclosing `slot` by `delta` rows.
///
/// The arithmetic behind every structural edit: rows appearing or vanishing
/// inside a box change that box's reach, and the reach of every box around
/// it, and nothing else in the project at all.
///
/// The enclosing set is read before anything is written, because resizing an
/// outer container moves the span that decides whether an inner one encloses
/// the same slot.
fn resize_enclosing(effects: &mut [EffectSlotState], slot: usize, delta: isize) {
    let enclosing: Vec<usize> = (0..slot.min(effects.len()))
        .filter(|outer| span_of(effects, *outer).contains(&slot))
        .collect();
    for outer in enclosing {
        if let EffectParams::Chain(chain) = &mut effects[outer].params {
            let grown = (chain.children as isize + delta).clamp(0, u8::MAX as isize);
            chain.children = grown as u8;
        }
    }
}

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

/// Move the device at `from` so that it lands at position `to`, taking its
/// run with it when it is a container. Returns whether anything moved.
///
/// `to` is where the device ends up, which is what the rack's drag reports
/// and what this has always meant; a container's *head* lands there and its
/// contents follow. Whether it lands inside another box is then simply where
/// that index falls, which is why dropping a container into itself needs no
/// refusal: after its own run is lifted out, none of the remaining positions
/// is inside it.
///
/// No address is touched. This used to return a permutation that every route,
/// every lane, the visible automation target, an in-flight save dialog and
/// the engine's mirror all had to run; a reorder is now invisible to all of
/// them.
pub fn move_effect(effects: &mut Vec<EffectSlotState>, from: usize, to: usize) -> bool {
    if from >= effects.len() || to >= effects.len() || from == to {
        return false;
    }
    let run = run_of(effects, from);
    let len = run.len();
    // Out of the boxes it was in, then into the boxes it lands in. Two
    // separate facts, and doing them in one pass is how a chain ends up
    // describing a shape it does not have.
    resize_enclosing(effects, from, -(len as isize));
    let moved: Vec<EffectSlotState> = effects.drain(run).collect();
    let at = to.min(effects.len());
    resize_enclosing(effects, at, len as isize);
    let tail = effects.split_off(at);
    effects.extend(moved);
    effects.extend(tail);
    true
}

/// Move the device at `from` -- and its whole run, when it is a container --
/// to be the first thing inside the container at `container`.
///
/// **Separate from [`move_effect`] for the reason
/// [`insert_into_container`] is separate from [`insert_effect`]**, and it is
/// the same ambiguity: the position just after a container's own row means
/// both "the first device inside it" and "the next device after it", and for
/// an *empty* container those are one integer. An index cannot say which, so
/// the gesture does.
///
/// This is the operation a drop lands on. Dragging a device onto a row that
/// is already inside a box put it in the box from the day the drag existed,
/// because `move_effect`'s index fell inside that box's span -- but emptying
/// a box left it with no rows to aim at and a span containing nothing, so
/// there was no index anywhere that meant "back inside this". The box became
/// a one-way door: unwrap or delete were the only ways out of it.
///
/// Returns `false` when `container` does not hold one, or when the run being
/// moved contains it -- a box cannot be put inside itself.
pub fn move_effect_into_container(
    effects: &mut Vec<EffectSlotState>,
    from: usize,
    container: usize,
) -> bool {
    if from >= effects.len() || container >= effects.len() || from == container {
        return false;
    }
    if !matches!(
        effects.get(container).map(|effect| effect.params),
        Some(EffectParams::Chain(_))
    ) {
        return false;
    }
    let run = run_of(effects, from);
    if run.contains(&container) {
        return false;
    }
    let len = run.len();
    resize_enclosing(effects, from, -(len as isize));
    let moved: Vec<EffectSlotState> = effects.drain(run.clone()).collect();
    // Lifting the run out from before the box slides the box back by its
    // length. Worked out here rather than by searching for the box again,
    // because the box may be one of several identical empty ones.
    let container = if run.start < container {
        container - len
    } else {
        container
    };
    // Every box *around* this one grows, and then this one does. The second
    // half is what `resize_enclosing` alone cannot do: for an empty box its
    // own span covers nothing, so it is not in its own enclosing set.
    resize_enclosing(effects, container, len as isize);
    if let EffectParams::Chain(chain) = &mut effects[container].params {
        chain.children = chain.children.saturating_add(len.min(u8::MAX as usize) as u8);
    }
    let at = container + 1;
    let tail = effects.split_off(at);
    effects.extend(moved);
    effects.extend(tail);
    true
}

/// The single-row moves that turn `before` into `after`.
///
/// The engine mirrors a reorder with one `MoveEffect` per row, which was
/// enough while a reorder moved one row. A container moves its whole run, so
/// the model now has to say *how* rather than just that something moved.
///
/// An insertion sort over identities rather than a permutation: for each
/// position in turn, find the device that belongs there and move it there.
/// At most one move per row, on a gesture made by hand, and correct for any
/// rearrangement rather than for the ones a container happens to produce.
/// Each pair is `(from, to)` in the same remove-then-insert sense
/// [`move_effect`] uses, applied in order.
pub fn move_sequence(before: &[DeviceId], after: &[DeviceId]) -> Vec<(u8, u8)> {
    let mut working: Vec<DeviceId> = before.to_vec();
    let mut moves = Vec::new();
    for (target, device) in after.iter().enumerate() {
        let Some(from) = working.iter().position(|id| id == device) else {
            continue;
        };
        if from == target {
            continue;
        }
        let (Ok(from_index), Ok(to_index)) = (u8::try_from(from), u8::try_from(target)) else {
            continue;
        };
        let device = working.remove(from);
        working.insert(target, device);
        moves.push((from_index, to_index));
    }
    moves
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
    // Every container whose run `at` falls inside gains a row. Landing on a
    // run's end boundary is landing *after* the container, not in it, which
    // is what makes "insert before slot N" mean the same thing at every
    // depth.
    resize_enclosing(effects, at, 1);
    effects.insert(at, effect.with_id(mint_device_id(next_id)));
    Some(at)
}

/// Insert `effect` as the first device inside the container at `container`,
/// minting it an identity. Returns the slot it landed in, or `None` when
/// `container` does not hold one or the chain is full.
///
/// **Separate from [`insert_effect`] on purpose.** The position just after a
/// container's own row means two different things -- "the first device inside
/// it" and "the next device after it" -- and for an *empty* container those
/// are the same index, so no index can express both. Rather than pick a
/// winner and make the other unreachable, the two are different operations:
/// an index says "before this row", and this says "into this box".
///
/// Getting that wrong is not a near miss. An earlier cut resolved the
/// ambiguity inside `resize_enclosing`, which made the position *count* as
/// inside for the purpose of growing spans while `span_of` still said it was
/// outside -- so wrapping the row after an empty box quietly pulled an
/// unrelated device into it.
pub fn insert_into_container(
    effects: &mut Vec<EffectSlotState>,
    next_id: &mut u32,
    container: usize,
    effect: EffectSlotState,
) -> Option<usize> {
    if !matches!(
        effects.get(container).map(|effect| effect.params),
        Some(EffectParams::Chain(_))
    ) {
        return None;
    }
    if effects.len() >= MAX_EFFECTS_PER_CHANNEL {
        return None;
    }
    let at = container + 1;
    // The box itself and every box around it, rather than only the ones whose
    // span already covers `at` -- which for an empty box is none of them.
    resize_enclosing(effects, container, 1);
    if let EffectParams::Chain(chain) = &mut effects[container].params {
        chain.children = chain.children.saturating_add(1);
    }
    effects.insert(at, effect.with_id(mint_device_id(next_id)));
    Some(at)
}

/// Remove the device at `at`, and its whole run when it is a container.
/// Returns what went, in rack order, container first. `None` when there is
/// nothing there.
///
/// **A box is deleted with its contents.** That is what "bypasses as a unit,
/// saves as one preset" implies about deletion too; emptying the box first is
/// the user's business, and step 04 gives them an unwrap gesture so it is one
/// click rather than N drags.
///
/// The returned states' `id`s are what the caller drops routes and lanes by.
pub fn remove_effect(
    effects: &mut Vec<EffectSlotState>,
    at: usize,
) -> Option<Vec<EffectSlotState>> {
    if at >= effects.len() {
        return None;
    }
    let run = run_of(effects, at);
    resize_enclosing(effects, at, -(run.len() as isize));
    Some(effects.drain(run).collect())
}

/// Insert `rows` -- a whole run -- before slot `at`, minting each an identity.
///
/// The paste half of [`remove_effect`]'s "a box goes with its contents": a
/// clipboard holds a run, and a run arrives all at once or not at all.
///
/// `rows` must be a well-formed run in its own right, which is what stops a
/// straddle entering a chain that was fine before. A run lifted by
/// [`run_of`] always is; one read off disk may not be, which is why this
/// checks rather than trusts.
///
/// Returns where the run landed. `None` when `rows` is empty, malformed, or
/// will not fit.
pub fn insert_run(
    effects: &mut Vec<EffectSlotState>,
    next_id: &mut u32,
    at: usize,
    rows: &[EffectSlotState],
) -> Option<usize> {
    if rows.is_empty() || span_problem(rows).is_some() {
        return None;
    }
    if effects.len() + rows.len() > MAX_EFFECTS_PER_CHANNEL {
        return None;
    }
    let at = at.min(effects.len());
    // One resize for the whole run, for the reason `replace_run` gives about
    // doing this arithmetic once: the boxes around `at` gain every row that
    // arrives, and counting them one at a time means re-deriving the
    // enclosing set against a chain that is already moving.
    resize_enclosing(effects, at, rows.len() as isize);
    for (offset, row) in rows.iter().enumerate() {
        effects.insert(at + offset, row.with_id(mint_device_id(next_id)));
    }
    Some(at)
}

/// Replace the run at `at` with `rows`, minting each an identity.
///
/// Returns what went, in rack order. `None` when `at` names nothing or the
/// chain has no room for the exchange.
///
/// The reason this is one function rather than a remove and an insert at the
/// call site: the boxes *around* `at` lose the run that left and gain the run
/// that arrived, and those are two different numbers. Doing it in two steps
/// means writing that arithmetic twice, and getting it wrong in the second
/// place is a container whose `children` no longer describes the rows it
/// holds -- which the invariants call a straddle and the integrity pass
/// flattens.
pub fn replace_run(
    effects: &mut Vec<EffectSlotState>,
    next_id: &mut u32,
    at: usize,
    rows: &[EffectSlotState],
) -> Option<Vec<EffectSlotState>> {
    if at >= effects.len() {
        return None;
    }
    let run = run_of(effects, at);
    if effects.len() - run.len() + rows.len() > MAX_EFFECTS_PER_CHANNEL {
        return None;
    }
    resize_enclosing(effects, at, -(run.len() as isize));
    let removed: Vec<EffectSlotState> = effects.drain(run).collect();
    resize_enclosing(effects, at, rows.len() as isize);
    for (offset, row) in rows.iter().enumerate() {
        effects.insert(at + offset, row.with_id(mint_device_id(next_id)));
    }
    Some(removed)
}

/// Take the container in `at` out of the chain, leaving its children where
/// they are. Returns whether it was a container at all.
///
/// The escape hatch that makes [`remove_effect`]'s "a box goes with its
/// contents" safe to have.
pub fn unwrap_container(effects: &mut Vec<EffectSlotState>, at: usize) -> bool {
    if !matches!(
        effects.get(at).map(|effect| effect.params),
        Some(EffectParams::Chain(_))
    ) {
        return false;
    }
    // The children stay, so every enclosing container loses exactly the one
    // row the container itself occupied.
    resize_enclosing(effects, at, -1);
    effects.remove(at);
    true
}

/// Wrap `run` in a new container, minting it an identity. Returns the slot
/// the container landed in, or `None` when the range is empty, out of range,
/// or would split a container's run.
///
/// The gesture the interface actually offers: a container is far more often
/// made around devices that already exist than inserted empty.
pub fn wrap_in_container(
    effects: &mut Vec<EffectSlotState>,
    next_id: &mut u32,
    run: std::ops::Range<usize>,
    container: EffectSlotState,
) -> Option<usize> {
    if run.is_empty() || run.end > effects.len() || effects.len() >= MAX_EFFECTS_PER_CHANNEL {
        return None;
    }
    // The selection has to be a whole number of complete runs with one
    // parent between them. Walking it run by run is the check: landing
    // anywhere but exactly on the end means the range cuts a container in
    // half, and a straddle is the one shape this representation cannot
    // describe.
    let parent = parent_of(effects, run.start);
    let mut slot = run.start;
    while slot < run.end {
        if parent_of(effects, slot) != parent {
            return None;
        }
        slot = run_of(effects, slot).end;
    }
    if slot != run.end {
        return None;
    }
    let mut container = container;
    if let EffectParams::Chain(chain) = &mut container.params {
        chain.children = u8::try_from(run.len()).ok()?;
    } else {
        return None;
    }
    // The rows do not move, so nothing enclosing them changes reach except by
    // the one row the container itself adds.
    resize_enclosing(effects, run.start, 1);
    effects.insert(run.start, container.with_id(mint_device_id(next_id)));
    Some(run.start)
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

        remove_effect(&mut effects, 0).expect("removed");
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
        let removed = removed[0].id;
        assert!(drop_lanes_for_device(&mut lanes, SCOPE, removed));
        assert_eq!(
            lanes.iter().map(|lane| lane.target).collect::<Vec<_>>(),
            [before[0], before[2], before[3]]
        );
        // Another chain's device of the same number is none of this edit's
        // business, and neither is the strip.
        assert!(!drop_lanes_for_device(
            &mut lanes,
            EffectTarget::Bus(0),
            removed
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

    /// The engine mirrors a reorder one row at a time, so a run that moves
    /// has to be spelled out. Applying the sequence to the before-order has
    /// to give the after-order exactly, whatever moved.
    #[test]
    fn a_move_sequence_reproduces_the_order_it_was_derived_from() {
        let apply = |before: &[u32], moves: &[(u8, u8)]| {
            let mut working: Vec<u32> = before.to_vec();
            for (from, to) in moves {
                let device = working.remove(*from as usize);
                working.insert(*to as usize, device);
            }
            working
        };
        let ids = |raw: &[u32]| raw.iter().map(|id| DeviceId(*id)).collect::<Vec<_>>();

        for (before, after) in [
            (vec![0u32, 1, 2, 3], vec![0u32, 1, 2, 3]),
            (vec![0, 1, 2, 3], vec![3, 0, 1, 2]),
            // A container and its two children moved past a leaf: the case a
            // single `MoveEffect` could not express.
            (vec![0, 1, 2, 3], vec![3, 0, 1, 2]),
            (vec![0, 1, 2, 3, 4], vec![4, 1, 2, 0, 3]),
            (vec![0, 1, 2], vec![2, 1, 0]),
        ] {
            let moves = move_sequence(&ids(&before), &ids(&after));
            assert_eq!(apply(&before, &moves), after, "from {before:?} to {after:?}");
        }
    }

    // --- Containers ---------------------------------------------------

    fn container() -> EffectSlotState {
        EffectSlotState::of_kind(EffectKind::Chain)
    }

    /// The chain as a shape: each row's kind, indented by how many containers
    /// enclose it. What a well-formed chain looks like at a glance, and the
    /// only thing these tests need to compare.
    fn shape(effects: &[EffectSlotState]) -> Vec<(usize, EffectKind)> {
        (0..effects.len())
            .map(|slot| (depth_at(effects, slot), effects[slot].kind()))
            .collect()
    }

    /// An empty container is not a dead end.
    ///
    /// Inserting a container and then filling it is the obvious way to make
    /// one -- it is what the rail's insert menu offers, and the only route
    /// available on a chain with no devices to wrap yet. An empty box has an
    /// empty span, so no *index* is inside it; `insert_into_container` names
    /// the box instead, which is why it exists.
    #[test]
    fn a_device_can_be_inserted_into_an_empty_container() {
        let mut effects = Vec::new();
        let mut next = 0;
        insert_effect(&mut effects, &mut next, 0, container()).expect("room");
        assert_eq!(span_of(&effects, 0), 1..1, "a fresh box holds nothing");

        insert_into_container(
            &mut effects,
            &mut next,
            0,
            EffectSlotState::of_kind(EffectKind::Filter),
        )
        .expect("room");
        assert_eq!(
            shape(&effects),
            [(0, EffectKind::Chain), (1, EffectKind::Filter)],
            "the filter landed beside the box rather than in it"
        );

        // A second one joins it at the head of the run.
        insert_into_container(
            &mut effects,
            &mut next,
            0,
            EffectSlotState::of_kind(EffectKind::Drive),
        )
        .expect("room");
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Chain),
                (1, EffectKind::Drive),
                (1, EffectKind::Filter)
            ]
        );
        assert_eq!(span_problem(&effects), None);

        // And it grows every box around it, not just the one named.
        let mut next = effects.len() as u32;
        wrap_in_container(&mut effects, &mut next, 0..3, container()).expect("outer");
        insert_into_container(
            &mut effects,
            &mut next,
            1,
            EffectSlotState::of_kind(EffectKind::Gate),
        )
        .expect("room");
        assert_eq!(depth_at(&effects, 2), 2, "the gate is inside both boxes");
        assert_eq!(span_problem(&effects), None);

        // A leaf is not a box, so there is nothing to insert into.
        assert!(insert_into_container(
            &mut effects,
            &mut next,
            2,
            EffectSlotState::of_kind(EffectKind::Delay)
        )
        .is_none());
    }

    /// Emptying a box must not seal it.
    ///
    /// The case Adam hit: wrap a device, drag it out, and the box is left
    /// with a span covering no index -- so under `move_effect` alone there is
    /// no `to` anywhere on the chain that puts anything back in. The box
    /// could only be refilled from its own `+`, or thrown away.
    #[test]
    fn a_device_can_be_dragged_back_into_a_box_it_was_dragged_out_of() {
        let mut effects = Vec::new();
        let mut next = 0;
        insert_effect(
            &mut effects,
            &mut next,
            0,
            EffectSlotState::of_kind(EffectKind::Filter),
        )
        .expect("room");
        wrap_in_container(&mut effects, &mut next, 0..1, container()).expect("wrapped");
        assert_eq!(
            shape(&effects),
            [(0, EffectKind::Chain), (1, EffectKind::Filter)],
            "the filter did not start inside the box"
        );

        // Out: the filter lands after the box, which empties it.
        assert!(move_effect(&mut effects, 1, 0));
        assert_eq!(
            shape(&effects),
            [(0, EffectKind::Filter), (0, EffectKind::Chain)],
            "dragging the filter out did not empty the box"
        );

        // And back in. There is no index that expresses this -- the box's
        // span is empty -- so the drop names the box.
        assert!(move_effect_into_container(&mut effects, 0, 1));
        assert_eq!(
            shape(&effects),
            [(0, EffectKind::Chain), (1, EffectKind::Filter)],
            "the filter did not go back inside the box"
        );
        assert_eq!(span_problem(&effects), None);
    }

    /// Moving a run into a box counts every row of it, and the boxes around
    /// that box count them too.
    #[test]
    fn a_whole_run_moves_into_a_box_and_every_enclosing_span_grows() {
        let mut effects = Vec::new();
        let mut next = 0;
        // An empty outer box, an empty inner box beside it, then a container
        // holding a filter to move as a run.
        insert_effect(&mut effects, &mut next, 0, container()).expect("room");
        insert_into_container(&mut effects, &mut next, 0, container()).expect("room");
        insert_effect(
            &mut effects,
            &mut next,
            2,
            EffectSlotState::of_kind(EffectKind::Filter),
        )
        .expect("room");
        insert_effect(
            &mut effects,
            &mut next,
            3,
            EffectSlotState::of_kind(EffectKind::Drive),
        )
        .expect("room");
        wrap_in_container(&mut effects, &mut next, 3..4, container()).expect("wrapped");
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Chain),
                (1, EffectKind::Chain),
                (0, EffectKind::Filter),
                (0, EffectKind::Chain),
                (1, EffectKind::Drive),
            ],
            "the fixture is not the shape the test is about"
        );

        // The drive's box, two rows, into the empty inner box at 1.
        assert!(move_effect_into_container(&mut effects, 3, 1));
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Chain),
                (1, EffectKind::Chain),
                (2, EffectKind::Chain),
                (3, EffectKind::Drive),
                (0, EffectKind::Filter),
            ],
            "the run did not land two levels in"
        );
        assert_eq!(span_problem(&effects), None);
    }

    /// A box cannot be put inside itself.
    #[test]
    fn a_container_refuses_to_be_dropped_into_its_own_run() {
        let mut effects = Vec::new();
        let mut next = 0;
        insert_effect(
            &mut effects,
            &mut next,
            0,
            EffectSlotState::of_kind(EffectKind::Filter),
        )
        .expect("room");
        wrap_in_container(&mut effects, &mut next, 0..1, container()).expect("wrapped");
        insert_into_container(&mut effects, &mut next, 0, container()).expect("room");
        let before = shape(&effects);

        assert!(
            !move_effect_into_container(&mut effects, 0, 1),
            "the outer box was allowed inside a box it contains"
        );
        assert_eq!(before, shape(&effects), "the refused move still edited the chain");
        assert_eq!(span_problem(&effects), None);
    }

    /// The position just after an empty box is *outside* it, which is what
    /// leaves room to put anything after a container at all.
    ///
    /// An earlier cut made that position count as inside for the purpose of
    /// growing spans, while `span_of` still said it was outside -- so this
    /// wrap silently pulled the filter two boxes deep having been in none.
    #[test]
    fn the_row_after_an_empty_container_is_not_inside_it() {
        let mut effects = Vec::new();
        let mut next = 0;
        insert_effect(&mut effects, &mut next, 0, container()).expect("room");
        insert_effect(
            &mut effects,
            &mut next,
            1,
            EffectSlotState::of_kind(EffectKind::Filter),
        )
        .expect("room");
        assert_eq!(depth_at(&effects, 1), 0, "the filter fell into the box");

        wrap_in_container(&mut effects, &mut next, 1..2, container()).expect("wrapped");
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Chain),
                (0, EffectKind::Chain),
                (1, EffectKind::Filter),
            ],
            "wrapping the row after an empty box changed what that box holds"
        );
        assert_eq!(span_problem(&effects), None);
    }

    /// Wrapping does not move anything. It adds one row and gives it a reach.
    #[test]
    fn wrapping_a_run_leaves_every_device_where_it_was() {
        let mut effects = chain(&[EffectKind::Filter, EffectKind::Drive, EffectKind::Delay]);
        let mut next = effects.len() as u32;
        let ids: Vec<DeviceId> = effects.iter().map(|effect| effect.id).collect();

        let at = wrap_in_container(&mut effects, &mut next, 1..3, container()).expect("wrapped");
        assert_eq!(at, 1);
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Filter),
                (0, EffectKind::Chain),
                (1, EffectKind::Drive),
                (1, EffectKind::Delay),
            ]
        );
        // Every original device is still on the chain, in order, wearing the
        // identity it had.
        assert_eq!(device_slot(&effects, ids[0]), Some(0));
        assert_eq!(device_slot(&effects, ids[1]), Some(2));
        assert_eq!(device_slot(&effects, ids[2]), Some(3));
        assert_eq!(span_problem(&effects), None);
    }

    /// A selection that would cut a container's run in half is refused rather
    /// than producing a straddle nothing downstream could interpret.
    #[test]
    fn wrapping_half_of_a_run_is_refused() {
        let mut effects = chain(&[EffectKind::Filter, EffectKind::Drive, EffectKind::Delay]);
        let mut next = effects.len() as u32;
        wrap_in_container(&mut effects, &mut next, 1..3, container()).expect("wrapped");
        let before = shape(&effects);
        // Slots 1..3 are the container and its first child: half a run.
        assert!(wrap_in_container(&mut effects, &mut next, 1..3, container()).is_none());
        // And 2..3 is one child of two, which is the same cut from inside.
        assert!(wrap_in_container(&mut effects, &mut next, 2..4, container()).is_some());
        assert_ne!(shape(&effects), before);
    }

    /// Inserting inside a box grows it, and grows every box around it.
    /// Inserting at a run's end boundary lands after the container.
    #[test]
    fn an_insert_inside_a_container_grows_every_box_around_it() {
        let mut effects = chain(&[EffectKind::Filter, EffectKind::Drive]);
        let mut next = effects.len() as u32;
        wrap_in_container(&mut effects, &mut next, 0..2, container()).expect("outer");
        wrap_in_container(&mut effects, &mut next, 1..3, container()).expect("inner");
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Chain),
                (1, EffectKind::Chain),
                (2, EffectKind::Filter),
                (2, EffectKind::Drive),
            ]
        );

        insert_effect(&mut effects, &mut next, 3, EffectSlotState::of_kind(EffectKind::Gate));
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Chain),
                (1, EffectKind::Chain),
                (2, EffectKind::Filter),
                (2, EffectKind::Gate),
                (2, EffectKind::Drive),
            ],
            "the gate did not land inside both boxes"
        );
        assert_eq!(span_problem(&effects), None);

        // Past the end of every run is outside every run.
        insert_effect(&mut effects, &mut next, 5, EffectSlotState::of_kind(EffectKind::Limiter));
        assert_eq!(depth_at(&effects, 5), 0, "the limiter was swallowed");
        assert_eq!(span_problem(&effects), None);
    }

    /// A box is removed with its contents, and the boxes around it close up
    /// by the whole run rather than by one row.
    #[test]
    fn removing_a_container_takes_its_run_and_shrinks_its_parent() {
        let mut effects = chain(&[
            EffectKind::Filter,
            EffectKind::Drive,
            EffectKind::Delay,
            EffectKind::Gate,
        ]);
        let mut next = effects.len() as u32;
        wrap_in_container(&mut effects, &mut next, 1..3, container()).expect("inner");
        wrap_in_container(&mut effects, &mut next, 0..5, container()).expect("outer");
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Chain),
                (1, EffectKind::Filter),
                (1, EffectKind::Chain),
                (2, EffectKind::Drive),
                (2, EffectKind::Delay),
                (1, EffectKind::Gate),
            ]
        );

        let removed = remove_effect(&mut effects, 2).expect("removed");
        assert_eq!(
            removed.iter().map(EffectSlotState::kind).collect::<Vec<_>>(),
            [EffectKind::Chain, EffectKind::Drive, EffectKind::Delay],
            "the box came out without its contents"
        );
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Chain),
                (1, EffectKind::Filter),
                (1, EffectKind::Gate),
            ]
        );
        assert_eq!(span_problem(&effects), None);
    }

    /// Unwrapping is the escape hatch that makes deleting-with-contents safe.
    #[test]
    fn unwrapping_keeps_the_children_and_costs_the_parent_one_row() {
        let mut effects = chain(&[EffectKind::Filter, EffectKind::Drive, EffectKind::Delay]);
        let mut next = effects.len() as u32;
        wrap_in_container(&mut effects, &mut next, 1..3, container()).expect("inner");
        wrap_in_container(&mut effects, &mut next, 0..4, container()).expect("outer");

        assert!(unwrap_container(&mut effects, 2));
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Chain),
                (1, EffectKind::Filter),
                (1, EffectKind::Drive),
                (1, EffectKind::Delay),
            ]
        );
        assert_eq!(span_problem(&effects), None);
        assert!(!unwrap_container(&mut effects, 1), "a leaf is not a box");
    }

    /// A container moves as a unit, out of what it was in and into what it
    /// lands in.
    #[test]
    fn moving_a_container_carries_its_run_and_reparents_it() {
        let mut effects = chain(&[
            EffectKind::Filter,
            EffectKind::Drive,
            EffectKind::Delay,
            EffectKind::Gate,
        ]);
        let mut next = effects.len() as u32;
        wrap_in_container(&mut effects, &mut next, 0..2, container()).expect("box");
        // Chain, Filter, Drive, Delay, Gate. The box lands at index 1, which
        // is after the delay once its own three rows are lifted out.
        assert!(move_effect(&mut effects, 0, 1));
        assert_eq!(
            shape(&effects),
            [
                (0, EffectKind::Delay),
                (0, EffectKind::Chain),
                (1, EffectKind::Filter),
                (1, EffectKind::Drive),
                (0, EffectKind::Gate),
            ],
            "the box did not take its contents to its new home"
        );
        assert_eq!(span_problem(&effects), None);

        // Dropping a leaf into the run puts it in the box.
        assert!(move_effect(&mut effects, 4, 2));
        assert_eq!(depth_at(&effects, 2), 1, "the gate did not go into the box");
        assert_eq!(span_problem(&effects), None);
    }

    /// A hand-edited file is a real input, so the two invariants are reported
    /// rather than asserted.
    #[test]
    fn a_malformed_span_is_reported_rather_than_trusted() {
        let mut effects = chain(&[EffectKind::Chain, EffectKind::Filter]);
        if let EffectParams::Chain(chain) = &mut effects[0].params {
            chain.children = 9;
        }
        assert!(span_problem(&effects)
            .is_some_and(|problem| problem.contains("the chain ends")));

        // Two boxes that overlap without one holding the other.
        let mut effects = chain(&[
            EffectKind::Chain,
            EffectKind::Chain,
            EffectKind::Filter,
            EffectKind::Drive,
        ]);
        if let EffectParams::Chain(chain) = &mut effects[0].params {
            chain.children = 2;
        }
        if let EffectParams::Chain(chain) = &mut effects[1].params {
            chain.children = 2;
        }
        assert!(span_problem(&effects).is_some_and(|problem| problem.contains("overlap")));
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
