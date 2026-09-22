//! Automation lane editing.
//!
//! The lane the roll is showing is `automation_target`; every edit here acts
//! on it, on the selected channel, in the current pattern.

use crate::session::Session;
use mooloop_core::{
    AutomationLane, AutomationPoint, EngineCommand, ParamAddr, PointId,
    MAX_AUTOMATION_LANES_PER_CHANNEL, TICKS_PER_STEP,
};

impl Session {
    /// The command addressing whichever lane is open, given its payload.
    fn lane_command(
        &self,
        make: impl FnOnce(u8, u8, ParamAddr) -> EngineCommand,
    ) -> Option<EngineCommand> {
        let target = self.automation_target.get()?;
        Some(make(
            self.current_pattern as u8,
            self.selected as u8,
            target,
        ))
    }

    /// Opens the lane at `index` of the destination list.
    ///
    /// The lane is created in the project as well as opened in the engine, so
    /// the picker's "already open" marks mean something before the first point
    /// is drawn. That also makes an empty open lane saved state, which is why
    /// opening one is an undoable edit.
    pub fn open_automation_lane(&mut self, index: i32) -> Option<EngineCommand> {
        let target = self
            .automation_destinations()
            .get(usize::try_from(index).ok()?)
            .map(|(target, _, _)| *target)?;
        self.open_automation_lane_at(target)
    }

    /// Opens the lane for `target`, by address rather than by picker position.
    ///
    /// Refuses when the pattern already holds
    /// [`MAX_AUTOMATION_LANES_PER_CHANNEL`] lanes and this target is not one
    /// of them. It used to set the target and report success either way, so
    /// the ninth pick left the editor showing that destination's name over a
    /// blank, inert canvas -- `automation_lane()` was `None`, so every point
    /// gesture returned `None` -- and wrote an undo entry whose `before` and
    /// `after` were identical. The engine refused the push too, so nothing
    /// diverged; it was simply silent.
    pub fn open_automation_lane_at(&mut self, target: ParamAddr) -> Option<EngineCommand> {
        let (pattern, channel) = (self.current_pattern, self.selected);
        if let Some(lanes) = self
            .channels
            .get_mut(channel)
            .and_then(|state| state.automation.get_mut(pattern))
        {
            if !lanes.iter().any(|lane| lane.target == target) {
                if lanes.len() >= MAX_AUTOMATION_LANES_PER_CHANNEL {
                    return None;
                }
                lanes.push(AutomationLane::new(target));
            }
        }
        self.automation_target.set(Some(target));
        self.automation_selected_point.set(None);
        Some(EngineCommand::OpenAutomationLane {
            pattern: pattern as u8,
            channel: channel as u8,
            target,
        })
    }

    /// Empties the open lane but leaves it open.
    pub fn clear_automation_lane(&mut self) -> Option<EngineCommand> {
        let command = self.lane_command(|pattern, channel, target| {
            EngineCommand::ClearAutomationLane {
                pattern,
                channel,
                target,
            }
        })?;
        self.automation_lane_mut()?.clear();
        self.automation_selected_point.set(None);
        Some(command)
    }

    /// Removes the open lane from the project entirely.
    pub fn close_automation_lane(&mut self) -> Option<EngineCommand> {
        let command = self.lane_command(|pattern, channel, target| {
            EngineCommand::RemoveAutomationLane {
                pattern,
                channel,
                target,
            }
        })?;
        let target = self.automation_target.get()?;
        let (pattern, channel) = (self.current_pattern, self.selected);
        if let Some(lanes) = self
            .channels
            .get_mut(channel)
            .and_then(|state| state.automation.get_mut(pattern))
        {
            lanes.retain(|lane| lane.target != target);
        }
        self.automation_target.set(None);
        self.automation_selected_point.set(None);
        Some(command)
    }

    /// The point nearest `(tick, value)` within `tolerance`, or `None`.
    ///
    /// Distance is measured in each axis's own tolerance rather than in
    /// screen units, so a lane that is short and wide does not become
    /// impossible to grab vertically.
    pub fn automation_point_at(&self, tick: i32, value: f32, tolerance: i32) -> Option<PointId> {
        const VALUE_TOLERANCE: f32 = 0.12;
        let lane = self.automation_lane()?;
        let tolerance = tolerance.max(1);
        lane.points()
            .iter()
            .filter(|point| (point.tick as i32 - tick).abs() <= tolerance)
            .filter(|point| (point.value - value).abs() <= VALUE_TOLERANCE)
            .min_by(|a, b| {
                let key = |point: &AutomationPoint| {
                    (point.tick as i32 - tick).abs() as f32 / tolerance as f32
                        + (point.value - value).abs() / VALUE_TOLERANCE
                };
                key(a).total_cmp(&key(b))
            })
            .map(|point| point.id)
    }

    /// Clamps a breakpoint into the lane's drawable range.
    fn lane_point(&self, id: PointId, tick: i32, value: f32) -> AutomationPoint {
        let length_ticks = self.pattern_lengths[self.current_pattern] as u32 * TICKS_PER_STEP;
        AutomationPoint::new(
            id,
            (tick.max(0) as u32).min(length_ticks),
            value.clamp(0.0, 1.0),
        )
    }

    /// Adds a breakpoint to the open lane, returning its id.
    pub fn create_automation_point(
        &mut self,
        tick: i32,
        value: f32,
    ) -> Option<(PointId, EngineCommand)> {
        let target = self.automation_target.get()?;
        let (pattern, channel) = (self.current_pattern, self.selected);
        let id = self.automation_lane_mut()?.allocate_id();
        let point = self.lane_point(id, tick, value);
        if !self.automation_lane_mut()?.upsert(point) {
            return None;
        }
        self.automation_selected_point.set(Some(id));
        Some((
            id,
            EngineCommand::UpsertAutomationPoint {
                pattern: pattern as u8,
                channel: channel as u8,
                target,
                point,
            },
        ))
    }

    /// Moves an existing breakpoint.
    pub fn move_automation_point(
        &mut self,
        id: PointId,
        tick: i32,
        value: f32,
    ) -> Option<EngineCommand> {
        let target = self.automation_target.get()?;
        let (pattern, channel) = (self.current_pattern, self.selected);
        if !self
            .automation_lane()?
            .points()
            .iter()
            .any(|point| point.id == id)
        {
            return None;
        }
        let point = self.lane_point(id, tick, value);
        self.automation_lane_mut()?.upsert(point);
        self.automation_selected_point.set(Some(id));
        Some(EngineCommand::UpsertAutomationPoint {
            pattern: pattern as u8,
            channel: channel as u8,
            target,
            point,
        })
    }

    /// Deletes a breakpoint.
    pub fn remove_automation_point(&mut self, id: PointId) -> Option<EngineCommand> {
        let target = self.automation_target.get()?;
        let (pattern, channel) = (self.current_pattern, self.selected);
        self.automation_lane_mut()?.remove(id)?;
        if self.automation_selected_point.get() == Some(id) {
            self.automation_selected_point.set(None);
        }
        Some(EngineCommand::RemoveAutomationPoint {
            pattern: pattern as u8,
            channel: channel as u8,
            target,
            id,
        })
    }
}

#[cfg(test)]
mod tests {

    /// The ninth lane is refused rather than half-opened. It used to set the
    /// target and report success whatever happened, so the pick left the
    /// editor showing that destination's name over a blank canvas that no
    /// point gesture could touch -- and recorded an undo entry whose two
    /// snapshots were identical.
    ///
    /// `CAPACITY_POLICY.md` asks for a boundary test on any cap over a
    /// user-created collection; there was none for this one.
    #[test]
    fn the_lane_ceiling_refuses_rather_than_opening_nothing() {
        let mut session = Session::default();
        let target = |param: u32| {
            ParamAddr::effect(mooloop_core::EffectTarget::Channel(0), mooloop_core::DeviceId(0), param)
        };

        for param in 0..MAX_AUTOMATION_LANES_PER_CHANNEL as u32 {
            assert!(
                session.open_automation_lane_at(target(param)).is_some(),
                "lane {param} is within the ceiling"
            );
        }

        // One past. Nothing is opened, and the shown target is left where it
        // was rather than pointing at a lane that does not exist.
        let shown = session.automation_target.get();
        assert!(session
            .open_automation_lane_at(target(MAX_AUTOMATION_LANES_PER_CHANNEL as u32))
            .is_none());
        assert_eq!(session.automation_target.get(), shown);

        // A target already open is still reachable at the ceiling: it is not
        // a new lane, so the cap does not apply to it.
        assert!(session.open_automation_lane_at(target(0)).is_some());
        assert_eq!(session.automation_target.get(), Some(target(0)));
    }
    use super::*;

    fn with_lane() -> Session {
        let mut session = Session::default();
        session
            .open_automation_lane(0)
            .expect("every channel has at least one automatable destination");
        session
    }

    /// The fader and the pan can be picked like any device parameter, and a
    /// lane on them reads in their own units. Shaped against the tree where
    /// the engine played strip lanes but the picker offered no row for one,
    /// and `automation_descriptor` answered `None` for a strip lane -- so one
    /// loaded from a file played, and its readout was blank.
    #[test]
    fn the_channel_fader_and_pan_are_automation_destinations() {
        use mooloop_core::{ParamOwner, STRIP_PARAM_PAN, STRIP_PARAM_VOLUME};
        let mut session = Session::default();
        let channel = mooloop_core::EffectTarget::Channel(0);
        let destinations = session.automation_destinations();
        for param in [STRIP_PARAM_VOLUME, STRIP_PARAM_PAN] {
            let index = destinations
                .iter()
                .position(|(target, _, _)| *target == ParamAddr::strip(channel, param))
                .expect("the strip is in the picker");
            assert_eq!(destinations[index].1, "Channel strip");
            session
                .open_automation_lane(index as i32)
                .expect("a strip lane opens like any other");
            assert_eq!(
                session.automation_descriptor().map(|descriptor| descriptor.id),
                Some(param),
                "the open strip lane has no descriptor to read its values with"
            );
        }
        // Every generator and chain row comes before them: the strip is after
        // the chain in the signal path.
        let first_strip = destinations
            .iter()
            .position(|(target, _, _)| target.owner == ParamOwner::Strip)
            .unwrap();
        assert!(destinations[..first_strip]
            .iter()
            .all(|(target, _, _)| target.scope == channel && target.owner != ParamOwner::Strip));
    }

    /// Opening a lane creates it in the project, so the picker's marks mean
    /// something before any point is drawn -- and re-opening it does not add
    /// a second one.
    #[test]
    fn opening_a_lane_creates_it_once() {
        let mut session = with_lane();
        let target = session.automation_target.get().expect("a lane is open");

        assert_eq!(session.channels[0].automation[0].len(), 1);
        session.open_automation_lane(0).expect("still valid");
        assert_eq!(session.channels[0].automation[0].len(), 1);
        assert_eq!(session.channels[0].automation[0][0].target, target);

        assert!(session.open_automation_lane(-1).is_none());
        assert!(session.open_automation_lane(9_999).is_none());
    }

    /// Clearing empties the lane; closing takes it out of the document. The
    /// difference is the whole reason there are two commands.
    #[test]
    fn clearing_keeps_the_lane_and_closing_does_not() {
        let mut session = with_lane();
        session.create_automation_point(0, 0.5).expect("lane is open");
        assert_eq!(session.automation_lane().expect("open").points().len(), 1);

        session.clear_automation_lane().expect("lane is open");
        assert_eq!(session.automation_lane().expect("open").points().len(), 0);
        assert_eq!(session.channels[0].automation[0].len(), 1);

        session.close_automation_lane().expect("lane is open");
        assert!(session.channels[0].automation[0].is_empty());
        assert!(session.automation_target.get().is_none());

        // With nothing open, every lane edit is a no-op rather than a panic.
        assert!(session.clear_automation_lane().is_none());
        assert!(session.close_automation_lane().is_none());
        assert!(session.create_automation_point(0, 0.5).is_none());
        assert!(session.remove_automation_point(1).is_none());
    }

    /// A point dragged past either end of the lane is clamped, not dropped.
    #[test]
    fn points_are_clamped_into_the_lane() {
        let mut session = with_lane();
        let length = session.pattern_lengths[0] as u32 * TICKS_PER_STEP;

        let (id, _) = session
            .create_automation_point(i32::MAX, 9.0)
            .expect("lane is open");
        let point = session.automation_lane().expect("open").points()[0];
        assert_eq!((point.tick, point.value), (length, 1.0));

        session.move_automation_point(id, -50, -9.0).expect("exists");
        let point = session.automation_lane().expect("open").points()[0];
        assert_eq!((point.tick, point.value), (0, 0.0));

        assert!(
            session.move_automation_point(id + 100, 0, 0.5).is_none(),
            "a point that is not there was moved"
        );
    }

    /// Grabbing measures each axis against its own tolerance, so a lane that
    /// is short and wide stays grabbable vertically.
    #[test]
    fn the_nearest_point_wins_in_both_axes() {
        let mut session = with_lane();
        let (low, _) = session.create_automation_point(100, 0.20).expect("open");
        let (high, _) = session.create_automation_point(100, 0.40).expect("open");

        assert_eq!(session.automation_point_at(100, 0.21, 20), Some(low));
        assert_eq!(session.automation_point_at(100, 0.39, 20), Some(high));
        // Outside the value tolerance, nothing is grabbed however close the
        // tick is.
        assert_eq!(session.automation_point_at(100, 0.9, 20), None);
        // Outside the tick tolerance, likewise.
        assert_eq!(session.automation_point_at(500, 0.20, 20), None);
    }

    /// Removing the selected point unselects it; removing another leaves the
    /// selection alone.
    #[test]
    fn removing_a_point_only_clears_the_selection_when_it_was_selected() {
        let mut session = with_lane();
        let (first, _) = session.create_automation_point(10, 0.5).expect("open");
        let (second, _) = session.create_automation_point(200, 0.5).expect("open");
        assert_eq!(session.automation_selected_point.get(), Some(second));

        session.remove_automation_point(first).expect("exists");
        assert_eq!(session.automation_selected_point.get(), Some(second));

        session.remove_automation_point(second).expect("exists");
        assert_eq!(session.automation_selected_point.get(), None);
        assert!(session.remove_automation_point(second).is_none());
    }
}
