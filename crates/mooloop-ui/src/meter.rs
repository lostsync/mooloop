/// Bottom of every meter's scale. Shared so the mixer's strips start at the
/// same floor the master meter uses.
pub(crate) use mooloop_core::gain::MIN_DB;
use mooloop_core::gain::linear_to_db;
/// IEC 60268-18 digital peak fall rate: 20 dB in 1.7 s. Attack is
/// instantaneous and the peak hold is 1 s; those were already standard.
const DECAY_DB_PER_SECOND: f32 = 20.0 / 1.7;
const HOLD_SECONDS: f32 = 1.0;

#[derive(Clone, Copy, Debug)]
pub(crate) struct MeterReading {
    pub level_db: f32,
    pub held_db: f32,
    pub clipping: bool,
}

#[derive(Debug)]
pub(crate) struct MeterBallistics {
    level_db: f32,
    held_db: f32,
    hold_remaining: f32,
    clipped: bool,
}

impl Default for MeterBallistics {
    fn default() -> Self {
        Self {
            level_db: MIN_DB,
            held_db: MIN_DB,
            hold_remaining: 0.0,
            clipped: false,
        }
    }
}

impl MeterBallistics {
    pub(crate) fn update(&mut self, linear_peak: f32, elapsed_seconds: f32) -> MeterReading {
        let elapsed = elapsed_seconds.max(0.0);
        let incoming_db = linear_to_db(linear_peak);

        self.level_db = if incoming_db >= self.level_db {
            incoming_db
        } else {
            (self.level_db - DECAY_DB_PER_SECOND * elapsed).max(incoming_db)
        };

        if incoming_db >= self.held_db {
            self.held_db = incoming_db;
            self.hold_remaining = HOLD_SECONDS;
        } else if self.hold_remaining > elapsed {
            self.hold_remaining -= elapsed;
        } else {
            let release_elapsed = elapsed - self.hold_remaining;
            self.hold_remaining = 0.0;
            self.held_db =
                (self.held_db - DECAY_DB_PER_SECOND * release_elapsed).max(self.level_db);
        }

        self.clipped |= linear_peak >= 1.0;

        MeterReading {
            level_db: self.level_db,
            held_db: self.held_db,
            clipping: self.clipped,
        }
    }

    /// Release the clip latch.
    ///
    /// The latch has no timer on purpose. A clip light that puts itself out
    /// is a light that is off by the time anyone looks at the meter, which is
    /// the whole reason to latch one; `ClipIndicator` has said "Click to
    /// clear" since it was written, and this is the half that makes that
    /// true. The peak hold above is the one that releases on its own, because
    /// it is a reading rather than an alarm.
    pub(crate) fn clear_clip(&mut self) {
        self.clipped = false;
    }

    /// Forget everything, because this meter is now showing a different
    /// track.
    ///
    /// A strip's ballistics are keyed by *index*, and removing a track shifts
    /// every later one down. Old track 4 becomes track 3 and inherits track
    /// 3's state -- which for the level and the hold is a smear that washes
    /// out in under two seconds, and for the clip latch is permanent, because
    /// [`Self::clear_clip`] is the only thing that releases one and it is
    /// reached by a click. So a track it never earned would show a clip light
    /// for the rest of the session.
    ///
    /// Resetting is the right direction for an alarm: losing a lit lamp is an
    /// inconvenience, and showing one nobody earned is the meter lying.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The latch outlives everything except a reset, which is what a strip
    /// showing a different track needs.
    #[test]
    fn a_reset_meter_has_forgotten_the_clip_and_the_hold() {
        let mut meter = MeterBallistics::default();
        let clipped = meter.update(1.5, 0.01);
        assert!(clipped.clipping);
        assert!(clipped.held_db > MIN_DB);

        // Silence for long enough that the level and the hold have decayed as
        // far as they are going to, and the latch is still lit -- which is the
        // whole design, and the reason an inherited one never goes out.
        for _ in 0..200 {
            meter.update(0.0, 0.05);
        }
        let quiet = meter.update(0.0, 0.05);
        assert!(quiet.clipping, "the latch released on its own");

        meter.reset();
        let fresh = meter.update(0.0, 0.01);
        assert!(!fresh.clipping, "a reset meter is still latched");
        assert_eq!(fresh.held_db, MIN_DB, "a reset meter still holds a peak");
        assert_eq!(fresh.level_db, MIN_DB, "a reset meter still shows a level");
    }

    #[test]
    fn attacks_instantly_and_falls_twenty_db_in_one_point_seven_seconds() {
        let mut meter = MeterBallistics::default();
        assert_eq!(meter.update(1.0, 0.01).level_db, 0.0);
        // Half the standard's fall time, half the fall.
        let reading = meter.update(0.0, 0.85);
        assert!((reading.level_db - -10.0).abs() < 0.01);
    }

    #[test]
    fn holds_peak_then_releases_it_while_the_clip_latch_stays_lit() {
        let mut meter = MeterBallistics::default();
        meter.update(1.1, 0.01);
        let held = meter.update(0.0, 0.75);
        assert!(held.held_db > 0.0);
        assert!(held.clipping);

        let released = meter.update(0.0, 0.5);
        assert!(released.held_db < held.held_db);
        // Silence for a minute does not put a clip light out.
        assert!(meter.update(0.0, 60.0).clipping);
    }

    #[test]
    fn the_clip_latch_is_released_only_by_clearing_it() {
        let mut meter = MeterBallistics::default();
        assert!(meter.update(1.0, 0.01).clipping);
        meter.clear_clip();
        assert!(!meter.update(0.0, 0.01).clipping);
        // And it re-arms: clearing is not disabling.
        assert!(meter.update(2.0, 0.01).clipping);
    }
}
