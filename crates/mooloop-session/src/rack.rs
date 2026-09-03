//! Channel-rack edits: selection, mute, level, pan, bus, and source.

use crate::channel::ChannelState;
use crate::session::Session;
use mooloop_core::{
    DeviceKind, EffectTarget, EngineCommand, MAX_BUSES, MAX_CHANNELS, MAX_LINEAR_GAIN,
};

impl Session {
    /// Points the editor and the device rack at `channel`.
    ///
    /// Returns `None` when nothing moved. Re-clicking the selected channel is
    /// still meaningful when the rack is showing a bus: it points it back.
    pub fn select_channel(&mut self, channel: i32) -> Option<usize> {
        let channel = usize::try_from(channel).ok()?;
        if channel >= self.channels.len() {
            return None;
        }
        if channel == self.selected && self.effect_target == EffectTarget::Channel(channel as u8) {
            return None;
        }
        self.selected = channel;
        self.effect_target = EffectTarget::Channel(channel as u8);
        self.select_note(None);
        Some(channel)
    }

    /// Flips a channel's mute.
    pub fn toggle_channel_mute(&mut self, channel: i32) -> Option<EngineCommand> {
        let channel = usize::try_from(channel).ok()?;
        let state = self.channels.get_mut(channel)?;
        state.muted = !state.muted;
        Some(EngineCommand::SetChannelMuted {
            channel: channel as u8,
            muted: state.muted,
        })
    }

    /// Sets a channel's output level, clamped to the container's headroom.
    pub fn set_channel_volume(&mut self, channel: i32, volume: f32) -> Option<EngineCommand> {
        let channel = usize::try_from(channel).ok()?;
        let state = self.channels.get_mut(channel)?;
        state.volume = volume.clamp(0.0, MAX_LINEAR_GAIN);
        Some(EngineCommand::SetChannelVolume {
            channel: channel as u8,
            volume: state.volume,
        })
    }

    /// Sets a channel's pan position.
    pub fn set_channel_pan(&mut self, channel: i32, pan: f32) -> Option<EngineCommand> {
        let channel = usize::try_from(channel).ok()?;
        let state = self.channels.get_mut(channel)?;
        state.pan = pan.clamp(-1.0, 1.0);
        Some(EngineCommand::SetChannelPan {
            channel: channel as u8,
            pan: state.pan,
        })
    }

    /// Routes a channel to a mixer bus.
    pub fn set_channel_bus(&mut self, channel: i32, bus: i32) -> Option<EngineCommand> {
        let channel = usize::try_from(channel).ok()?;
        let bus = u8::try_from(bus).ok()?;
        if bus as usize >= MAX_BUSES {
            return None;
        }
        self.channels.get_mut(channel)?.bus = bus;
        Some(EngineCommand::SetChannelBus {
            channel: channel as u8,
            bus,
        })
    }

    /// Replaces the selected channel's generator, returning which channel
    /// changed. `None` when it is already that kind.
    pub fn change_selected_source(&mut self, source: DeviceKind) -> Option<usize> {
        let channel = self.selected;
        if self.channels[channel].kind == source {
            return None;
        }
        self.reset_channel_source(channel, source);
        self.select_note(None);
        Some(channel)
    }

    /// Appends a channel running `source` and selects it, or `None` if the
    /// rack is full.
    ///
    /// Its pattern banks are sized to the document rather than to one pattern,
    /// so switching patterns after adding a channel does not index past the
    /// end of it.
    pub fn add_channel(&mut self, source: DeviceKind) -> Option<usize> {
        if self.channels.len() >= MAX_CHANNELS {
            return None;
        }
        let index = self.channels.len();
        let patterns = self.pattern_lengths.len();
        let mut channel = ChannelState::new(index);
        channel.notes.resize_with(patterns, Vec::new);
        channel.automation.resize_with(patterns, Vec::new);
        self.channels.push(channel);
        self.reset_channel_source(index, source);
        self.selected = index;
        self.effect_target = EffectTarget::Channel(index as u8);
        self.select_note(None);
        Some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::TICKS_PER_STEP;

    /// Clicking the channel already selected is a no-op -- unless the device
    /// rack has wandered off to a bus, which is the case the guard exists for.
    #[test]
    fn reselecting_a_channel_only_counts_when_the_rack_is_on_a_bus() {
        let mut session = Session::default();
        session.add_channel(DeviceKind::Sampler);

        assert_eq!(session.select_channel(0), Some(0));
        assert_eq!(session.select_channel(0), None);

        session.effect_target = EffectTarget::Bus(1);
        assert_eq!(
            session.select_channel(0),
            Some(0),
            "the rack was on a bus, so re-clicking should point it back"
        );
        assert_eq!(session.effect_target, EffectTarget::Channel(0));

        assert_eq!(session.select_channel(9), None);
        assert_eq!(session.select_channel(-1), None);
    }

    /// A channel added after a second pattern exists still has a bank for it.
    #[test]
    fn a_new_channel_gets_a_bank_for_every_pattern() {
        let mut session = Session::default();
        session.add_pattern();
        session.add_pattern();

        let index = session.add_channel(DeviceKind::DrumSynth).expect("rack has room");

        assert_eq!(session.channels[index].notes.len(), 3);
        assert_eq!(session.channels[index].automation.len(), 3);
        assert_eq!(session.selected, index);
        assert_eq!(session.effect_target, EffectTarget::Channel(index as u8));
        assert_eq!(session.channels[index].kind, DeviceKind::DrumSynth);
    }

    /// Both gain stages share the container's headroom; a fader that reports
    /// a number the engine will not run is worse than no fader.
    #[test]
    fn level_and_pan_clamp_to_what_the_engine_accepts() {
        let mut session = Session::default();

        assert!(matches!(
            session.set_channel_volume(0, 100.0),
            Some(EngineCommand::SetChannelVolume { volume, .. }) if volume == MAX_LINEAR_GAIN
        ));
        assert!(matches!(
            session.set_channel_volume(0, -5.0),
            Some(EngineCommand::SetChannelVolume { volume, .. }) if volume == 0.0
        ));
        assert!(matches!(
            session.set_channel_pan(0, 9.0),
            Some(EngineCommand::SetChannelPan { pan, .. }) if pan == 1.0
        ));
        assert!(session.set_channel_volume(7, 0.5).is_none());
    }

    #[test]
    fn a_bus_outside_the_bank_is_refused() {
        let mut session = Session::default();
        assert!(session.set_channel_bus(0, 1).is_some());
        assert_eq!(session.channels[0].bus, 1);
        assert!(session.set_channel_bus(0, MAX_BUSES as i32).is_none());
        assert!(session.set_channel_bus(0, -1).is_none());
        assert_eq!(session.channels[0].bus, 1, "a refused bus was still applied");
    }

    /// Changing the generator drops the note selection with it, since the
    /// editor it belonged to is being replaced.
    #[test]
    fn changing_the_source_reports_only_a_real_change() {
        let mut session = Session::default();
        let note = session.channels[0].create_note(0, 0, TICKS_PER_STEP, 60);
        session.select_note(Some(note.id));

        assert_eq!(session.change_selected_source(DeviceKind::MonoSynth), Some(0));
        assert_eq!(session.channels[0].kind, DeviceKind::MonoSynth);
        assert_eq!(session.selected_note_id, None);

        assert_eq!(session.change_selected_source(DeviceKind::MonoSynth), None);
    }
}
