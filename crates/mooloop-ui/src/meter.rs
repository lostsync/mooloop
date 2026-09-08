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
}

#[cfg(test)]
mod tests {
    use super::*;

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
