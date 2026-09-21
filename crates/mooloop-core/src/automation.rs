//! Clip automation: tick-addressed breakpoint lanes targeting a [`ParamAddr`].
//!
//! An automation lane is the direct-drawing counterpart to the modulator rack.
//! Both end up resolving the same destination through the same descriptor
//! table, so a lane never learns anything about the device it drives — it
//! stores normalized `0..1` breakpoints and the engine maps them through
//! [`crate::ParamDescriptor`] exactly like a knob position.
//!
//! Storage is reused rather than minted for the same reason note storage is
//! preallocated: lane edits are applied on the audio thread and must not
//! allocate or free. A lane's point vector is 12 KB, and the bank that holds
//! lanes is 256 patterns by 256 channels by eight, so preallocating one per
//! slot is 6 GiB and `docs/CAPACITY_POLICY.md` forbids it. Instead a vacated
//! slot keeps the vector it was given, and a slot that has never held a lane
//! takes one from [`LanePool`], which is refilled off the audio thread.

use crate::ParamAddr;

/// Lanes a single channel may open inside one pattern. The editor shows one at
/// a time; the surplus exists so switching the visible lane does not destroy
/// the automation behind it.
pub const MAX_AUTOMATION_LANES_PER_CHANNEL: usize = 8;

/// Breakpoints per lane. A point every sixty-fourth across the longest pattern
/// is 1024, so this is deliberately generous rather than a limit a drawn curve
/// is expected to reach.
pub const MAX_AUTOMATION_POINTS_PER_LANE: usize = 1024;

pub type PointId = u32;

/// One breakpoint. `value` is normalized `0..1` against the destination's
/// descriptor, never natural units — a lane must survive a descriptor's range
/// changing under it, and normalized is the form both the knob and the
/// modulation matrix already speak.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AutomationPoint {
    pub id: PointId,
    pub tick: u32,
    pub value: f32,
}

impl AutomationPoint {
    pub fn new(id: PointId, tick: u32, value: f32) -> Self {
        Self {
            id,
            tick,
            value: value.clamp(0.0, 1.0),
        }
    }
}

/// A destination plus the breakpoints drawn for it, ordered by tick.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AutomationLane {
    pub target: ParamAddr,
    points: Vec<AutomationPoint>,
    #[serde(default = "default_next_point_id")]
    next_point_id: PointId,
}

impl AutomationLane {
    /// The heap this lane owns. `points` is private, so the accounting the
    /// undo budget needs has to live beside it rather than reach in.
    pub fn heap_bytes(&self) -> usize {
        self.points.capacity() * std::mem::size_of::<AutomationPoint>()
    }
}

fn default_next_point_id() -> PointId {
    1
}

/// The address a vacant slot in a lane bank carries.
///
/// A lane bank is a fixed array of lanes whose open ones are a prefix, so
/// vacancy is really the prefix length; this exists so that a vacated slot
/// cannot answer a lookup for the destination it used to drive, and so that
/// two banks holding the same open lanes compare equal. `param` is `u32::MAX`,
/// which no descriptor id is ever assigned.
const VACANT_TARGET: ParamAddr = ParamAddr {
    scope: crate::EffectTarget::Bus(u8::MAX),
    owner: crate::ParamOwner::Strip,
    param: u32::MAX,
};

/// Spare point vectors, allocated off the audio thread and handed to a lane
/// slot that has never held one.
///
/// Opening a lane used to be `Vec::with_capacity(MAX_AUTOMATION_POINTS_PER_LANE)`
/// on the audio thread -- a 12 KB malloc inside the callback, against the rule
/// this module's header states. Closing one used to drop the same vector
/// there. Closing now keeps it in the slot, so the free is gone outright; the
/// malloc is gone for as long as this pool has anything in it.
///
/// Refilled by [`crate::Sequencer`]-style owners at project install, which
/// runs off the thread. When it is empty a lane still opens, by allocating as
/// it did before -- a lane the user asked for is never refused for want of a
/// spare, and `docs/CAPACITY_POLICY.md` is why. The standing follow-up is to
/// have the session supply storage with the command instead, the way
/// `StructuralCommand` already supplies a routing table.
#[derive(Debug, Default)]
pub struct LanePool {
    spare: Vec<Vec<AutomationPoint>>,
}

impl LanePool {
    /// Spares kept for lanes drawn between two installs. Each is 12 KB, so
    /// this is 384 KB held against an editing session's worth of new lanes.
    pub const SPARE_LANES: usize = MAX_AUTOMATION_LANES_PER_CHANNEL * 4;

    pub fn new() -> Self {
        let mut pool = Self {
            spare: Vec::with_capacity(Self::SPARE_LANES),
        };
        pool.refill();
        pool
    }

    /// Top the pool back up to [`Self::SPARE_LANES`]. Allocates, so it is for
    /// the install path and never for the callback.
    pub fn refill(&mut self) {
        while self.spare.len() < Self::SPARE_LANES {
            self.spare
                .push(Vec::with_capacity(MAX_AUTOMATION_POINTS_PER_LANE));
        }
    }

    pub fn spare_lanes(&self) -> usize {
        self.spare.len()
    }

    /// Storage for a lane that is opening. Allocation-free while the pool has
    /// a spare; see the type's own note for why it allocates rather than
    /// refusing when it does not.
    fn take(&mut self) -> Vec<AutomationPoint> {
        self.spare
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(MAX_AUTOMATION_POINTS_PER_LANE))
    }
}

impl AutomationLane {
    /// A slot no destination has claimed. It owns no heap until it is opened,
    /// which is what makes a bank of them affordable.
    pub fn vacant() -> Self {
        Self {
            target: VACANT_TARGET,
            points: Vec::new(),
            next_point_id: 1,
        }
    }

    /// Whether this slot drives a destination. The bank's prefix length is the
    /// authority; this answers the same question for one lane in hand.
    pub fn is_open(&self) -> bool {
        self.target != VACANT_TARGET
    }

    /// Claim a vacant slot for `target`, taking point storage from `pool` only
    /// when the slot has none of its own. Runs on the audio thread.
    ///
    /// A slot that has held a lane before still has that lane's vector, empty
    /// and at full capacity, so reopening one costs nothing at all. A slot
    /// holding a short vector -- one restored by a decode that had fewer
    /// points than the ceiling -- keeps it rather than swapping it out, since
    /// dropping it would be a free on the callback; `upsert` then refuses past
    /// the shorter capacity, which is its documented behaviour already.
    pub(crate) fn claim(&mut self, target: ParamAddr, pool: &mut LanePool) {
        self.points.clear();
        if self.points.capacity() == 0 {
            self.points = pool.take();
        }
        self.target = target;
        self.next_point_id = 1;
    }

    /// Close this slot, keeping its point storage for the next lane that lands
    /// in it. No free, which is the whole point: this runs on the callback.
    pub(crate) fn vacate(&mut self) {
        self.points.clear();
        self.target = VACANT_TARGET;
        self.next_point_id = 1;
    }

    pub fn new(target: ParamAddr) -> Self {
        Self {
            target,
            points: Vec::with_capacity(MAX_AUTOMATION_POINTS_PER_LANE),
            next_point_id: 1,
        }
    }

    pub fn points(&self) -> &[AutomationPoint] {
        &self.points
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Preallocated point storage. Exposed so a caller can tell "the lane is
    /// full" from "the edit was rejected for some other reason".
    pub fn capacity(&self) -> usize {
        self.points.capacity()
    }

    /// Restore the preallocated point storage that `Clone` and serde drop.
    ///
    /// `upsert` measures fullness against *capacity*, because the audio
    /// thread must never reallocate -- and `Vec::clone` allocates exactly
    /// `len`. So a lane that has been through `replace_project` or a decode
    /// has `capacity == len` and refuses the next point for ever, silently:
    /// the click returns `None` and nothing is reported. Dragging an
    /// existing point still works, because that path removes before it
    /// inserts, which is what makes it read as "this lane is finished"
    /// rather than as a fault.
    ///
    /// Called where a lane becomes editable, never on the audio thread.
    /// Deliberately not a manual `Clone`: preallocating on every clone would
    /// put 12 KB per lane into every undo snapshot, against the budget
    /// `edit_cost.rs` measures.
    pub fn reserve_points(&mut self) {
        let have = self.points.capacity();
        if have < MAX_AUTOMATION_POINTS_PER_LANE {
            self.points
                .reserve_exact(MAX_AUTOMATION_POINTS_PER_LANE - have);
        }
    }

    /// Allocate an id that no live point in this lane holds. Kept on the lane
    /// so a point id is meaningful without also naming a channel and pattern.
    pub fn allocate_id(&mut self) -> PointId {
        let id = self.next_point_id;
        self.next_point_id = self.next_point_id.wrapping_add(1).max(1);
        id
    }

    /// Insert or replace a point by stable id, keeping tick order. Returns
    /// false when preallocated storage is full.
    pub fn upsert(&mut self, point: AutomationPoint) -> bool {
        if let Some(index) = self
            .points
            .iter()
            .position(|existing| existing.id == point.id)
        {
            self.points.remove(index);
        } else if self.points.len() == self.points.capacity() {
            return false;
        }
        self.next_point_id = self.next_point_id.max(point.id.wrapping_add(1)).max(1);
        let index = self
            .points
            .binary_search_by_key(&(point.tick, point.id), |existing| {
                (existing.tick, existing.id)
            })
            .unwrap_or_else(|index| index);
        self.points.insert(index, point);
        true
    }

    /// Rebuild the lane from `points`, restoring tick order and moving the id
    /// allocator past everything kept. For repair passes, which have to edit
    /// ticks and ids together and so cannot go through `upsert` one point at a
    /// time; ordinary edits still should. Anything past the preallocated
    /// capacity is dropped, because the realtime side cannot address it.
    pub fn reset_points(&mut self, points: impl IntoIterator<Item = AutomationPoint>) {
        let capacity = self.points.capacity();
        self.points.clear();
        self.points.extend(points.into_iter().take(capacity));
        self.points.sort_by_key(|point| (point.tick, point.id));
        self.next_point_id = self
            .points
            .iter()
            .map(|point| point.id)
            .max()
            .unwrap_or(0)
            .wrapping_add(1)
            .max(1);
    }

    pub fn remove(&mut self, id: PointId) -> Option<AutomationPoint> {
        let index = self.points.iter().position(|point| point.id == id)?;
        Some(self.points.remove(index))
    }

    pub fn clear(&mut self) {
        self.points.clear();
    }

    /// The lane's value at `tick`, linearly interpolated between neighbours
    /// and held flat outside the outermost pair. `None` only when the lane has
    /// no points at all, which is what tells the engine to leave the knob
    /// alone rather than force it to zero.
    pub fn value_at(&self, tick: f64) -> Option<f32> {
        let first = self.points.first()?;
        let last = self.points.last()?;
        if tick <= first.tick as f64 {
            return Some(first.value);
        }
        if tick >= last.tick as f64 {
            return Some(last.value);
        }
        // Points are tick-ordered, so the first point at or past `tick` and
        // its predecessor bracket it.
        let index = self
            .points
            .partition_point(|point| (point.tick as f64) <= tick);
        let after = &self.points[index.min(self.points.len() - 1)];
        let before = &self.points[index.saturating_sub(1)];
        let span = after.tick as f64 - before.tick as f64;
        if span <= 0.0 {
            return Some(after.value);
        }
        let t = ((tick - before.tick as f64) / span) as f32;
        Some(before.value + (after.value - before.value) * t.clamp(0.0, 1.0))
    }
}

#[cfg(test)]
mod tests {

    /// **A cloned lane is a full lane.** `upsert` measures fullness against
    /// capacity, because the audio thread must not reallocate -- and
    /// `Vec::clone` allocates exactly `len`. So every lane that had been
    /// through `Session::replace_project` (project load, undo, redo, and
    /// every `ProjectEdit`) refused its next point for ever, silently: the
    /// click returned `None` and nothing was reported. Opening a saved song
    /// froze every lane in it at its saved point count.
    ///
    /// Nothing caught it because no test built a lane by cloning one, and
    /// the engine was frozen in step -- `Pattern::set_lanes` moved the same
    /// short vectors in -- so the two sides agreed.
    #[test]
    fn a_cloned_lane_still_takes_its_full_complement_of_points() {
        let mut lane = lane();
        for tick in 0..3u32 {
            let id = lane.allocate_id();
            assert!(lane.upsert(AutomationPoint::new(id, tick, 0.5)));
        }

        let mut cloned = lane.clone();
        assert_eq!(cloned.points().len(), 3);
        cloned.reserve_points();

        // The whole remaining complement, not just one more.
        for tick in 3..MAX_AUTOMATION_POINTS_PER_LANE as u32 {
            let id = cloned.allocate_id();
            assert!(
                cloned.upsert(AutomationPoint::new(id, tick, 0.5)),
                "a restored lane must accept point {tick}"
            );
        }
        assert_eq!(cloned.points().len(), MAX_AUTOMATION_POINTS_PER_LANE);
        // And still refuses past the ceiling, which is what the guard is for.
        let id = cloned.allocate_id();
        assert!(!cloned.upsert(AutomationPoint::new(id, 9_999, 0.5)));
    }
    use super::*;
    use crate::EffectTarget;

    fn lane() -> AutomationLane {
        AutomationLane::new(ParamAddr::effect(EffectTarget::Channel(0), crate::DeviceId(0), 3))
    }

    #[test]
    fn an_empty_lane_has_no_opinion() {
        assert_eq!(lane().value_at(0.0), None);
    }

    #[test]
    fn points_stay_tick_ordered_and_replace_by_id() {
        let mut lane = lane();
        assert!(lane.upsert(AutomationPoint::new(2, 48, 1.0)));
        assert!(lane.upsert(AutomationPoint::new(1, 0, 0.0)));
        assert_eq!(
            lane.points().iter().map(|p| p.id).collect::<Vec<_>>(),
            [1, 2]
        );

        assert!(lane.upsert(AutomationPoint::new(2, 12, 0.5)));
        assert_eq!(
            lane.points().iter().map(|p| p.tick).collect::<Vec<_>>(),
            [0, 12]
        );
    }

    #[test]
    fn a_single_point_holds_across_the_whole_pattern() {
        let mut lane = lane();
        lane.upsert(AutomationPoint::new(1, 48, 0.25));
        assert_eq!(lane.value_at(0.0), Some(0.25));
        assert_eq!(lane.value_at(48.0), Some(0.25));
        assert_eq!(lane.value_at(999.0), Some(0.25));
    }

    #[test]
    fn values_interpolate_between_neighbours_and_hold_outside_them() {
        let mut lane = lane();
        lane.upsert(AutomationPoint::new(1, 0, 0.0));
        lane.upsert(AutomationPoint::new(2, 96, 1.0));

        assert_eq!(lane.value_at(-5.0), Some(0.0));
        assert_eq!(lane.value_at(0.0), Some(0.0));
        assert!((lane.value_at(48.0).unwrap() - 0.5).abs() < 1e-6);
        assert!((lane.value_at(24.0).unwrap() - 0.25).abs() < 1e-6);
        assert_eq!(lane.value_at(96.0), Some(1.0));
        assert_eq!(lane.value_at(500.0), Some(1.0));
    }

    #[test]
    fn two_points_on_the_same_tick_step_rather_than_divide_by_zero() {
        let mut lane = lane();
        lane.upsert(AutomationPoint::new(1, 0, 0.0));
        lane.upsert(AutomationPoint::new(2, 48, 0.2));
        lane.upsert(AutomationPoint::new(3, 48, 0.9));
        lane.upsert(AutomationPoint::new(4, 96, 1.0));
        // Landing exactly on the pair reads the later value; the step is
        // instantaneous rather than an infinite slope.
        assert_eq!(lane.value_at(48.0), Some(0.9));
        assert!(lane.value_at(47.0).unwrap() < 0.2);
    }

    #[test]
    fn values_are_clamped_on_the_way_in() {
        let mut lane = lane();
        lane.upsert(AutomationPoint::new(1, 0, -3.0));
        lane.upsert(AutomationPoint::new(2, 10, 7.0));
        assert_eq!(lane.value_at(0.0), Some(0.0));
        assert_eq!(lane.value_at(10.0), Some(1.0));
    }

    #[test]
    fn allocate_id_never_collides_with_a_deserialized_point() {
        let mut lane = lane();
        lane.upsert(AutomationPoint::new(40, 0, 0.5));
        let id = lane.allocate_id();
        assert!(lane.points().iter().all(|point| point.id != id));
    }

    #[test]
    fn storage_is_bounded_and_refuses_rather_than_reallocating() {
        let mut lane = lane();
        for id in 0..MAX_AUTOMATION_POINTS_PER_LANE {
            assert!(lane.upsert(AutomationPoint::new(id as u32 + 1, id as u32, 0.5)));
        }
        let capacity = lane.capacity();
        assert!(!lane.upsert(AutomationPoint::new(9_999, 0, 0.5)));
        assert_eq!(lane.capacity(), capacity);
        // Replacing an existing id still works at capacity.
        assert!(lane.upsert(AutomationPoint::new(1, 0, 0.25)));
    }
}
