//! The pump's per-tick displays: rack meters and traces, strip meters, the
//! compressor lamps and the sampler's playheads (MOO-261, MOO-258).
//!
//! Everything here moves every tick while a song plays, so none of it lives
//! on a row that also carries text. A whole-row `set_row_data` dirties every
//! binding on the face that reads the row, the layouts of its texts included,
//! and each of those re-measures its strings. These models are kept for the
//! window's life and written entry by entry, and only where a value moved
//! far enough to be drawn differently.
//!
//! The pump reads the engine; these methods only publish. That split is what
//! lets the tests hold the rule -- a tick with nothing moved writes nothing --
//! without an engine.

use super::*;

/// What a rack slot's device reads when nothing has been published for it:
/// the same resting values as `slot-meters()` in `main.slint`.
pub(crate) fn resting_slot_meters() -> EffectSlotMeters {
    EffectSlotMeters {
        input_left_db: METER_FLOOR_DB,
        input_right_db: METER_FLOOR_DB,
        output_left_db: METER_FLOOR_DB,
        output_right_db: METER_FLOOR_DB,
        detector_db: METER_FLOOR_DB,
        gain_reduction_db: 0.0,
        held_reduction_db: 0.0,
        buffer_collisions: 0,
        // -1 is "no head", which a fraction of a ring can never be.
        buffer_head: -1.0,
        buffer_write: 0.0,
        buffer_window_start: -1.0,
        buffer_window_end: -1.0,
        buffer_frozen: false,
        buffer_armed_freeze: 0,
        buffer_armed_gesture: false,
        buffer_position_bar: 1,
        buffer_position_beat: 1,
        buffer_position_tick: 0,
    }
}

/// A strip's meter at rest: `StripMeters.level()`'s answer past the end.
pub(crate) fn resting_strip_level() -> StripLevel {
    StripLevel {
        left_db: METER_FLOOR_DB,
        right_db: METER_FLOOR_DB,
        held_left_db: METER_FLOOR_DB,
        held_right_db: METER_FLOOR_DB,
        clipping: false,
    }
}

/// Whether `next` would be drawn differently from `previous`. Levels go
/// through the meters' quarter-decibel step, the dynamics readouts through
/// their half-decibel one, and everything else must be equal.
fn slot_meters_display_changed(previous: &EffectSlotMeters, next: &EffectSlotMeters) -> bool {
    meter_display_changed(previous.input_left_db, next.input_left_db)
        || meter_display_changed(previous.input_right_db, next.input_right_db)
        || meter_display_changed(previous.output_left_db, next.output_left_db)
        || meter_display_changed(previous.output_right_db, next.output_right_db)
        || dynamics_display_changed(previous.detector_db, next.detector_db)
        || dynamics_display_changed(previous.gain_reduction_db, next.gain_reduction_db)
        || dynamics_display_changed(previous.held_reduction_db, next.held_reduction_db)
        || previous.buffer_collisions != next.buffer_collisions
        || previous.buffer_head != next.buffer_head
        || previous.buffer_write != next.buffer_write
        || previous.buffer_window_start != next.buffer_window_start
        || previous.buffer_window_end != next.buffer_window_end
        || previous.buffer_frozen != next.buffer_frozen
        || previous.buffer_armed_freeze != next.buffer_armed_freeze
        || previous.buffer_armed_gesture != next.buffer_armed_gesture
        || previous.buffer_position_bar != next.buffer_position_bar
        || previous.buffer_position_beat != next.buffer_position_beat
        || previous.buffer_position_tick != next.buffer_position_tick
}

fn strip_level_display_changed(previous: &StripLevel, next: &StripLevel) -> bool {
    // The held level and the clip latch are stepped changes rather than a
    // continuous level, so they get their own reasons to repaint: throttling
    // them behind the level's own quantiser is how a peak marker comes to sit
    // one segment behind where the audio put it.
    meter_display_changed(previous.left_db, next.left_db)
        || meter_display_changed(previous.right_db, next.right_db)
        || meter_display_changed(previous.held_left_db, next.held_left_db)
        || meter_display_changed(previous.held_right_db, next.held_right_db)
        || previous.clipping != next.clipping
}

/// Bring a `VecModel` to `values`, touching only the entries that differ, or
/// resetting it when the length changed. Returns how many entries were
/// written, so zero means no binding moved.
pub(crate) fn write_values(model: &VecModel<f32>, values: &[f32]) -> usize {
    if model.row_count() != values.len() {
        model.set_vec(values.to_vec());
        return values.len().max(1);
    }
    let mut written = 0;
    for (index, value) in values.iter().enumerate() {
        if model.row_data(index) != Some(*value) {
            model.set_row_data(index, *value);
            written += 1;
        }
    }
    written
}

/// [`write_values`] into a trace model the pump made, which is always a
/// `VecModel`; anything else is left alone.
fn write_trace(model: &ModelRc<f32>, values: &[f32]) -> usize {
    model
        .as_any()
        .downcast_ref::<VecModel<f32>>()
        .map_or(0, |vec| write_values(vec, values))
}

/// What one publish wrote, for the tests' "nothing moved, nothing written".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DisplayWrites {
    /// `effect-slot-meters` / `StripMeters.levels` entries replaced.
    pub(crate) meters: usize,
    /// Trace (or lamp, or playhead) values written in place.
    pub(crate) values: usize,
}

#[cfg(test)]
impl DisplayWrites {
    pub(crate) fn add(&mut self, other: DisplayWrites) {
        self.meters += other.meters;
        self.values += other.values;
    }
}

impl UiState {
    /// Rebuild the per-slot display models alongside the rack's rows, which
    /// is the only place the rows are rebuilt: an insert, a removal, a move,
    /// a fold, a container opening, undo or another chain all come through
    /// `sync_effects`. Every slot starts at rest and every trace empty, as
    /// the rows themselves used to, so no face ever shows a meter that was
    /// published for whatever device sat at its index before. The trace
    /// models themselves are kept and emptied, not replaced.
    pub(crate) fn realign_slot_displays(&self, count: usize) {
        self.effect_slot_meters
            .set_vec(vec![resting_slot_meters(); count]);
        while self.effect_slot_traces.row_count() > count {
            self.effect_slot_traces
                .remove(self.effect_slot_traces.row_count() - 1);
        }
        for slot in 0..self.effect_slot_traces.row_count() {
            if let Some(trace) = self.slot_trace(slot) {
                write_trace(&trace, &[]);
            }
        }
        while self.effect_slot_traces.row_count() < count {
            self.effect_slot_traces
                .push(ModelRc::from(Rc::new(VecModel::<f32>::default())));
        }
    }

    /// A slot's published meters, or its resting values.
    pub(crate) fn slot_meters(&self, slot: usize) -> EffectSlotMeters {
        self.effect_slot_meters
            .row_data(slot)
            .unwrap_or_else(resting_slot_meters)
    }

    fn slot_trace(&self, slot: usize) -> Option<ModelRc<f32>> {
        self.effect_slot_traces.row_data(slot)
    }

    /// Publish one slot's reading: its meters, if any of them would be drawn
    /// differently, and its trace in place (`None` for a slot drawing none,
    /// which empties a trace it no longer draws). Never writes the slot's
    /// `EffectSlotRow`.
    pub(crate) fn publish_slot_display(
        &self,
        slot: usize,
        meters: &EffectSlotMeters,
        trace: Option<&[f32]>,
    ) -> DisplayWrites {
        let mut writes = DisplayWrites::default();
        if let Some(previous) = self.effect_slot_meters.row_data(slot) {
            if slot_meters_display_changed(&previous, meters) {
                self.effect_slot_meters.set_row_data(slot, meters.clone());
                writes.meters += 1;
            }
        }
        if let Some(model) = self.slot_trace(slot) {
            writes.values += write_trace(&model, trace.unwrap_or(&[]));
        }
        writes
    }

    /// Publish one track's meter to `StripMeters.levels`, if it would be
    /// drawn differently. Never writes the track's `MixerStripRow`.
    pub(crate) fn publish_strip_level(&self, track: usize, level: &StripLevel) -> DisplayWrites {
        let mut writes = DisplayWrites::default();
        if let Some(previous) = self.strip_levels.row_data(track) {
            if strip_level_display_changed(&previous, level) {
                self.strip_levels.set_row_data(track, level.clone());
                writes.meters += 1;
            }
        }
        writes
    }

    /// Publish every track's gain reduction to `StripMeters.reduction-db`,
    /// in place.
    pub(crate) fn publish_strip_reductions(&self, reduction: &[f32]) -> DisplayWrites {
        DisplayWrites {
            meters: 0,
            values: write_values(&self.strip_reductions, reduction),
        }
    }

    /// Publish the sampler's voice positions, in place while the voice count
    /// holds, so the repeater drawing the lines keeps its instances.
    pub(crate) fn publish_playheads(&self, positions: &[f32]) -> DisplayWrites {
        DisplayWrites {
            meters: 0,
            values: write_values(&self.playhead_model, positions),
        }
    }
}

/// The greatest clip boundary -- a clip's start or its end, or 0 -- at or
/// before `position`: what `playlist-live-from` holds (MOO-261).
///
/// A clip lights while the playhead is inside it, `start <= position < end`.
/// That predicate gives the same answer at this tick as at `position`,
/// because no boundary lies strictly between them, and this tick moves only
/// when the playhead crosses an edge. The clips' borders read it rather than
/// `playlist-position-ticks`, which moves every pump tick and dirtied every
/// clip's binding each time.
pub(crate) fn playlist_live_from(
    clips: impl IntoIterator<Item = (i32, i32)>,
    position: i32,
) -> i32 {
    let mut from = 0;
    for (start, end) in clips {
        for boundary in [start, end] {
            if boundary <= position && boundary > from {
                from = boundary;
            }
        }
    }
    from
}

/// A playlist clip's `[start, end)` in ticks, as the markup computes
/// `clip-end-tick`.
pub(crate) fn clip_span(clip: &PlaylistClip) -> (i32, i32) {
    (
        clip.start_tick,
        clip.start_tick + clip.length_steps * TICKS_PER_STEP as i32,
    )
}

/// Move the playlist's playhead: `playlist-position-ticks` for the line, and
/// `playlist-live-from` for the clips' borders, the latter written only when
/// it changes. Every place that moves the playhead goes through here, so a
/// seek, a loop wrap or a stop re-evaluates it as forward motion does, and it
/// is always taken against the clips the window holds at that moment.
pub(crate) fn set_playlist_position(window: &MainWindow, ticks: i32) {
    window.set_playlist_position_ticks(ticks);
    let clips = window.get_playlist_clips();
    let from = playlist_live_from(clips.iter().map(|clip| clip_span(&clip)), ticks);
    if window.get_playlist_live_from() != from {
        window.set_playlist_live_from(from);
    }
}
