//! Step-grid pattern data. Pure model; owns no audio or UI deps.
//!
//! A `Pattern` holds one row of steps per channel. Step resolution is fixed at
//! 16th notes for now (see [`crate::time`]); pattern length defaults to one
//! 4/4 bar (16 steps) and may be changed per pattern. Each channel row stores
//! one pitched, velocity-sensitive note slot per step.

/// Default number of steps per pattern (one 4/4 bar at 16th-note resolution).
pub const DEFAULT_STEPS: u16 = 16;

/// Maximum pattern length. The realtime sequencer pre-allocates this many
/// steps for every channel in every pattern so length edits never allocate.
pub const MAX_PATTERN_STEPS: u16 = 256;

/// One monophonic note slot in the grid. Pitch and velocity use MIDI-compatible
/// ranges and are preserved while the slot is inactive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub on: bool,
    /// MIDI note number. The sampler interprets this relative to its root note.
    pub note: u8,
    pub velocity: u8,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            on: false,
            note: 60,
            velocity: 100,
        }
    }
}

impl Step {
    /// Toggle the on state, preserving pitch and velocity.
    pub fn toggled(self) -> Self {
        Self {
            on: !self.on,
            ..self
        }
    }
}

/// One channel's row inside a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPattern {
    pub steps: Vec<Step>,
}

impl ChannelPattern {
    pub fn new(num_steps: usize) -> Self {
        Self {
            steps: vec![Step::default(); num_steps],
        }
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// A step-sequencer pattern. Channels are indexed in registration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    pub length_steps: u16,
    pub channels: Vec<ChannelPattern>,
}

impl Pattern {
    /// New pattern with `num_channels` channels, each `DEFAULT_STEPS` long.
    pub fn new(num_channels: usize) -> Self {
        Self::with_steps(num_channels, DEFAULT_STEPS as usize)
    }

    pub fn with_steps(num_channels: usize, num_steps: usize) -> Self {
        let length_steps = num_steps.min(u16::MAX as usize) as u16;
        Self {
            length_steps,
            channels: (0..num_channels)
                .map(|_| ChannelPattern::new(num_steps))
                .collect(),
        }
    }

    pub fn channel(&self, index: usize) -> Option<&ChannelPattern> {
        self.channels.get(index)
    }

    pub fn channel_mut(&mut self, index: usize) -> Option<&mut ChannelPattern> {
        self.channels.get_mut(index)
    }

    /// Change the logical playback length without resizing channel storage.
    /// Shortening a pattern therefore preserves steps that are currently past
    /// the end, so extending it again is non-destructive.
    pub fn set_length_steps(&mut self, length_steps: usize) {
        let capacity = self
            .channels
            .iter()
            .map(ChannelPattern::len)
            .min()
            .unwrap_or(1)
            .min(u16::MAX as usize)
            .max(1);
        self.length_steps = length_steps.clamp(1, capacity) as u16;
    }

    /// Number of active steps in channel `index` (for UI feedback / sanity).
    pub fn count_active(&self, index: usize) -> usize {
        self.channel(index)
            .map(|c| {
                c.steps
                    .iter()
                    .take(self.length_steps as usize)
                    .filter(|s| s.on)
                    .count()
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_defaults() {
        let p = Pattern::new(2);
        assert_eq!(p.length_steps, 16);
        assert_eq!(p.channels.len(), 2);
        assert_eq!(p.channel(0).unwrap().len(), 16);
        assert!(p.channel(0).unwrap().steps.iter().all(|s| !s.on));
        assert!(p
            .channel(0)
            .unwrap()
            .steps
            .iter()
            .all(|s| s.note == 60 && s.velocity == 100));
    }

    #[test]
    fn toggle_preserves_note_data() {
        let mut p = Pattern::new(1);
        p.channel_mut(0).unwrap().steps[3].note = 48;
        p.channel_mut(0).unwrap().steps[3].velocity = 90;
        p.channel_mut(0).unwrap().steps[3] = p.channel(0).unwrap().steps[3].toggled();
        let s = p.channel(0).unwrap().steps[3];
        assert!(s.on);
        assert_eq!(s.note, 48);
        assert_eq!(s.velocity, 90);
    }

    #[test]
    fn length_changes_are_bounded_and_non_destructive() {
        let mut p = Pattern::with_steps(1, 32);
        p.channel_mut(0).unwrap().steps[23].on = true;

        p.set_length_steps(12);
        assert_eq!(p.length_steps, 12);
        assert_eq!(p.count_active(0), 0, "hidden steps are not active");

        p.set_length_steps(24);
        assert_eq!(p.length_steps, 24);
        assert!(
            p.channel(0).unwrap().steps[23].on,
            "hidden data is preserved"
        );

        p.set_length_steps(0);
        assert_eq!(p.length_steps, 1);
        p.set_length_steps(usize::MAX);
        assert_eq!(p.length_steps, 32);
    }
}
