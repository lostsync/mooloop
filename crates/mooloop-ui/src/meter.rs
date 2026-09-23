/// Bottom of every meter's scale. Shared so the mixer's strips start at the
/// same floor the master meter uses.
pub(crate) use mooloop_core::gain::MIN_DB;
use mooloop_core::gain::{linear_to_db, MAX_DB};
/// IEC 60268-18 digital peak fall rate: 20 dB in 1.7 s. Attack is
/// instantaneous and the peak hold is 1 s; those were already standard.
const DECAY_DB_PER_SECOND: f32 = 20.0 / 1.7;
const HOLD_SECONDS: f32 = 1.0;

/// The fall rates Preferences > Appearance offers, in dB per second.
///
/// Index 1 is the IEC rate above -- what every meter here did before the
/// preference existed, and what it still does unless somebody says otherwise.
/// The others are two stops slower and one faster: fast enough to read the
/// shape of a transient, or slow enough to read a level from across the room.
pub(crate) const FALLOFF_DB_PER_SECOND: [f32; 4] = [30.0, DECAY_DB_PER_SECOND, 6.0, 3.0];

/// The rate an option index means, for the metering pump.
pub(crate) fn falloff_db_per_second(index: i32) -> f32 {
    FALLOFF_DB_PER_SECOND[index.clamp(0, FALLOFF_DB_PER_SECOND.len() as i32 - 1) as usize]
}

/// The rows the preference offers, labelled with the rate itself.
///
/// Derived from the table rather than written beside it. A hand-typed
/// `["30 dB/s", "12 dB/s", ...]` in the markup would be the fault `AGENTS.md`
/// opens on -- a number spelled in Rust and again in Slint, with nothing able
/// to notice when one of them moved -- and the label here is *only* the
/// number, so there is nothing else for it to say.
pub(crate) fn falloff_options() -> Vec<String> {
    FALLOFF_DB_PER_SECOND
        .iter()
        .map(|rate| format!("{rate:.0} dB/s"))
        .collect()
}

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
    /// `decay_db_per_second` is the tuned fall rate, which the metering pump
    /// reads from the preference once a tick and hands to every meter --
    /// rather than each meter holding a copy that would have to be kept in
    /// step when the preference changes.
    pub(crate) fn update(
        &mut self,
        linear_peak: f32,
        elapsed_seconds: f32,
        decay_db_per_second: f32,
    ) -> MeterReading {
        let elapsed = elapsed_seconds.max(0.0);
        // A non-finite peak is a device that blew up -- the engine reads a NaN
        // sample as an infinite one (`StereoBus::peak`). `linear_to_db` puts
        // both at the floor, which is silence: the one reading such a bus
        // must not get. It reads as the top of the scale instead (MOO-93).
        let incoming_db = if linear_peak.is_finite() {
            linear_to_db(linear_peak)
        } else {
            MAX_DB
        };
        let decay = decay_db_per_second.max(0.0);

        self.level_db = if incoming_db >= self.level_db {
            incoming_db
        } else {
            (self.level_db - decay * elapsed).max(incoming_db)
        };

        if incoming_db >= self.held_db {
            self.held_db = incoming_db;
            self.hold_remaining = HOLD_SECONDS;
        } else if self.hold_remaining > elapsed {
            self.hold_remaining -= elapsed;
        } else {
            let release_elapsed = elapsed - self.hold_remaining;
            self.hold_remaining = 0.0;
            self.held_db = (self.held_db - decay * release_elapsed).max(self.level_db);
        }

        self.clipped |= linear_peak >= 1.0 || linear_peak.is_nan();

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
        let clipped = meter.update(1.5, 0.01, DECAY_DB_PER_SECOND);
        assert!(clipped.clipping);
        assert!(clipped.held_db > MIN_DB);

        // Silence for long enough that the level and the hold have decayed as
        // far as they are going to, and the latch is still lit -- which is the
        // whole design, and the reason an inherited one never goes out.
        for _ in 0..200 {
            meter.update(0.0, 0.05, DECAY_DB_PER_SECOND);
        }
        let quiet = meter.update(0.0, 0.05, DECAY_DB_PER_SECOND);
        assert!(quiet.clipping, "the latch released on its own");

        meter.reset();
        let fresh = meter.update(0.0, 0.01, DECAY_DB_PER_SECOND);
        assert!(!fresh.clipping, "a reset meter is still latched");
        assert_eq!(fresh.held_db, MIN_DB, "a reset meter still holds a peak");
        assert_eq!(fresh.level_db, MIN_DB, "a reset meter still shows a level");
    }

    #[test]
    fn attacks_instantly_and_falls_twenty_db_in_one_point_seven_seconds() {
        let mut meter = MeterBallistics::default();
        assert_eq!(meter.update(1.0, 0.01, DECAY_DB_PER_SECOND).level_db, 0.0);
        // Half the standard's fall time, half the fall.
        let reading = meter.update(0.0, 0.85, DECAY_DB_PER_SECOND);
        assert!((reading.level_db - -10.0).abs() < 0.01);
    }

    #[test]
    fn holds_peak_then_releases_it_while_the_clip_latch_stays_lit() {
        let mut meter = MeterBallistics::default();
        meter.update(1.1, 0.01, DECAY_DB_PER_SECOND);
        let held = meter.update(0.0, 0.75, DECAY_DB_PER_SECOND);
        assert!(held.held_db > 0.0);
        assert!(held.clipping);

        let released = meter.update(0.0, 0.5, DECAY_DB_PER_SECOND);
        assert!(released.held_db < held.held_db);
        // Silence for a minute does not put a clip light out.
        assert!(meter.update(0.0, 60.0, DECAY_DB_PER_SECOND).clipping);
    }

    /// The preference reaches the fall, and the labels name the rates.
    ///
    /// Worth a test rather than a reading of the code: the rate is handed in
    /// per tick now, so nothing stops a caller passing a constant and leaving
    /// the option inert -- the "convincing but inert control" this codebase
    /// keeps a rule about. A slower setting must fall less far in the same
    /// time, and the row the user reads must be the number that is used.
    #[test]
    fn the_falloff_preference_changes_the_fall_and_says_which_rate_it_is() {
        let fall_after = |rate: f32| {
            let mut meter = MeterBallistics::default();
            meter.update(1.0, 0.01, rate);
            meter.update(0.0, 1.0, rate).level_db
        };
        let fast = fall_after(falloff_db_per_second(0));
        let standard = fall_after(falloff_db_per_second(1));
        let slowest = fall_after(falloff_db_per_second(3));
        assert!(fast < standard, "the fast setting must fall further");
        assert!(slowest > standard, "the slowest setting must fall less far");

        // A second of fall at N dB/s is N dB down, which is what the label on
        // the row promises the user.
        assert!((standard - -FALLOFF_DB_PER_SECOND[1]).abs() < 0.01);
        assert_eq!(falloff_options()[0], "30 dB/s");
        assert_eq!(
            falloff_options().len(),
            FALLOFF_DB_PER_SECOND.len(),
            "every rate needs a row to pick it with"
        );
        // Out-of-range indices come from a hand-edited settings file.
        assert_eq!(falloff_db_per_second(-3), FALLOFF_DB_PER_SECOND[0]);
        assert_eq!(falloff_db_per_second(99), FALLOFF_DB_PER_SECOND[3]);
    }

    /// Shaped against the unfixed meter, which read an infinite peak as
    /// `MIN_DB` -- silence -- and did not latch a NaN at all, because
    /// `NaN >= 1.0` is false.
    #[test]
    fn a_non_finite_peak_reads_full_scale_and_latches_the_clip() {
        for peak in [f32::INFINITY, f32::NAN] {
            let mut meter = MeterBallistics::default();
            let reading = meter.update(peak, 0.01, DECAY_DB_PER_SECOND);
            assert_eq!(reading.level_db, MAX_DB, "{peak} metered as {}", reading.level_db);
            assert!(reading.clipping, "{peak} did not light the clip latch");
        }
    }

    #[test]
    fn the_clip_latch_is_released_only_by_clearing_it() {
        let mut meter = MeterBallistics::default();
        assert!(meter.update(1.0, 0.01, DECAY_DB_PER_SECOND).clipping);
        meter.clear_clip();
        assert!(!meter.update(0.0, 0.01, DECAY_DB_PER_SECOND).clipping);
        // And it re-arms: clearing is not disabling.
        assert!(meter.update(2.0, 0.01, DECAY_DB_PER_SECOND).clipping);
    }
}
