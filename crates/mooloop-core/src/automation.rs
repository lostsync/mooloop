//! Clip automation: tick-addressed breakpoint lanes targeting a [`ParamAddr`].
//!
//! An automation lane is the direct-drawing counterpart to the modulator rack.
//! Both end up resolving the same destination through the same descriptor
//! table, so a lane never learns anything about the device it drives — it
//! stores normalized `0..1` breakpoints and the engine maps them through
//! [`crate::ParamDescriptor`] exactly like a knob position.
//!
//! Storage is preallocated for the same reason note storage is: lane edits are
//! applied on the audio thread and must not allocate.

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

fn default_next_point_id() -> PointId {
    1
}

impl AutomationLane {
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
    use super::*;
    use crate::EffectTarget;

    fn lane() -> AutomationLane {
        AutomationLane::new(ParamAddr::effect(EffectTarget::Channel(0), 0, 3))
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
