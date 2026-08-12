//! Step-grid pattern data. Pure model; owns no audio or UI deps.
//!
//! A `Pattern` holds one row of steps per channel. Step resolution is fixed at
//! 16th notes for now (see [`crate::time`]); pattern length defaults to one
//! 4/4 bar (16 steps). Per-note melodic data (piano roll) lands in a later
//! phase — for now each step is a drum-style trigger.

/// Default number of steps per pattern (one 4/4 bar at 16th-note resolution).
pub const DEFAULT_STEPS: u16 = 16;

/// One step in the grid. `velocity` is carried in the model (MIDI-compatible,
/// 0..=127) even though the Phase 1 UI only toggles `on`; per-step velocity
/// editing arrives with the parameter lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub on: bool,
    pub velocity: u8,
}

impl Default for Step {
    fn default() -> Self {
        Self {
            on: false,
            velocity: 100,
        }
    }
}

impl Step {
    /// Toggle the on state, preserving velocity.
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

    /// Number of active steps in channel `index` (for UI feedback / sanity).
    pub fn count_active(&self, index: usize) -> usize {
        self.channel(index)
            .map(|c| c.steps.iter().filter(|s| s.on).count())
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
    }

    #[test]
    fn toggle_preserves_velocity() {
        let mut p = Pattern::new(1);
        p.channel_mut(0).unwrap().steps[3].velocity = 90;
        p.channel_mut(0).unwrap().steps[3] = p.channel(0).unwrap().steps[3].toggled();
        let s = p.channel(0).unwrap().steps[3];
        assert!(s.on);
        assert_eq!(s.velocity, 90);
    }
}
