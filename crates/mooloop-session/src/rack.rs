//! Channel-rack edits: selection, mute, solo, level, pan, bus, and source.

use crate::channel::ChannelState;
use crate::session::Session;
use mooloop_core::{
    DeviceKind, EffectTarget, EngineCommand, ProjectColor, MAX_BUSES, MAX_CHANNELS,
    MAX_LINEAR_GAIN,
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

    /// Renames a channel. `true` when the name actually changed.
    ///
    /// The same gap `rename_track` closed, one level down: a channel's name
    /// saved, loaded, and drew on its rack plate, and nothing could set it.
    ///
    /// An empty name is refused rather than stored, because the rack plate is
    /// the only thing that identifies a row and a blank one identifies
    /// nothing. A pattern may go nameless -- there the number is right beside
    /// it -- which is why `rename_pattern` accepts what this rejects.
    pub fn rename_channel(&mut self, channel: i32, name: &str) -> bool {
        let name = name.trim();
        let Ok(channel) = usize::try_from(channel) else {
            return false;
        };
        let Some(state) = self.channels.get_mut(channel) else {
            return false;
        };
        if name.is_empty() || state.name == name {
            return false;
        }
        state.name = name.to_string();
        true
    }

    /// Gives a channel a colour, or takes its colour away with `None`.
    ///
    /// Unlike a name, a colour has no default worth deriving and no reason to
    /// be refused: a blank name would leave a rack row identifying nothing,
    /// where a channel with no colour is simply the ordinary case. Returns
    /// whether anything changed, so re-choosing the colour a channel already
    /// has does not dirty the document.
    pub fn set_channel_color(&mut self, channel: i32, color: Option<ProjectColor>) -> bool {
        let Ok(channel) = usize::try_from(channel) else {
            return false;
        };
        let Some(state) = self.channels.get_mut(channel) else {
            return false;
        };
        if state.color == color {
            return false;
        }
        state.color = color;
        true
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

    /// Flips a channel's solo.
    ///
    /// **Solo in place**, the same ruling a track's follows: what it silences
    /// is derived by the pump's `sync_channel_solo`, not here, because it is
    /// a property of the whole bank rather than of this channel. This only
    /// says which button is lit, which is why it hands back no command.
    ///
    /// Nothing is refused. A track's solo is refused on the master, which
    /// every track reaches and soloing would silence nothing; no channel has
    /// that standing, so soloing the only channel in a song is simply a solo
    /// that silences nothing, and dropping it changes nothing back.
    pub fn toggle_channel_solo(&mut self, channel: i32) -> bool {
        let Ok(channel) = usize::try_from(channel) else {
            return false;
        };
        let Some(state) = self.channels.get_mut(channel) else {
            return false;
        };
        state.solo = !state.solo;
        // Routing-shaped state does not travel as a command, so the edit is
        // marked here rather than falling out of one -- the same reason
        // `toggle_track_solo` marks its own, and the reason a plain
        // `dirty = true` at the call site would be wrong: `mark_dirty` also
        // bumps the revision everything else reads.
        self.mark_dirty();
        true
    }

    /// Which channels a solo is silencing, for the pump to send and for the
    /// rack rows to dim. Derived from the bank on each ask, for the reason
    /// [`Self::toggle_channel_solo`] hands back no command.
    ///
    /// The whole plan rather than one channel's answer, because both callers
    /// want every row: the plan is what `sync_channel_solo` diffs, and asking
    /// per row would re-derive the bank once per row.
    pub fn channel_solo_silenced(&self) -> [bool; MAX_CHANNELS] {
        mooloop_core::channel::solo_silenced(self.channels.iter().map(|state| state.solo))
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
        // The session's half of the rule `Project::insert_channel` keeps: a
        // channel that joins the bank is minted on the way in, so no two ever
        // wear one id.
        channel.id = mooloop_core::mint_channel_id(&mut self.next_channel_id);
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

    /// The generator's preset label belongs to the device wearing it, not to
    /// the channel's seat: swapping in a different source drops it, and a
    /// channel that was never loaded from a preset never claims one.
    #[test]
    fn a_generators_preset_label_dies_with_its_device() {
        let mut session = Session::default();
        session.add_channel(DeviceKind::Ds01);
        session.select_channel(0);

        session.set_source_preset_name(0, "Deep House Kick");
        assert_eq!(session.source_preset_name(0), Some("Deep House Kick"));
        assert_eq!(session.source_preset_name(1), None);

        // A new device on the same channel did not come from that preset.
        session.change_selected_source(DeviceKind::MlP8);
        assert_eq!(session.source_preset_name(0), None);

        // Nor does a channel added into a seat that once wore one.
        session.set_source_preset_name(1, "Rimshot");
        session.select_channel(1);
        session.change_selected_source(DeviceKind::Sampler);
        assert_eq!(session.source_preset_name(1), None);
    }

    /// **A name the user typed survives a change of source device.**
    ///
    /// Live from 2026-09-09, when renaming shipped, to 2026-09-13: a channel
    /// called "Kick" switched from the sampler to the DS-01 came back called
    /// "DS-01 1", because the source change re-derived the name from the
    /// index unconditionally. That was correct while every name was derived
    /// and became a gesture that discards user content the day one could be
    /// typed.
    #[test]
    fn a_named_channel_keeps_its_name_when_its_device_changes() {
        let mut session = Session::default();
        session.select_channel(0);
        assert!(session.rename_channel(0, "Kick"));

        session.change_selected_source(DeviceKind::Ds01);

        assert_eq!(session.channels[0].name, "Kick", "the source change ate the name");
        assert_eq!(session.channels[0].kind, DeviceKind::Ds01, "the device did not change");
    }

    /// The other half of the same rule: a channel still wearing the *outgoing*
    /// device's default name was never named by anybody, so the name follows
    /// the device. Without this the rack would fill up with rows called
    /// "Sampler 1" running a DS-01.
    #[test]
    fn an_unnamed_channel_still_follows_its_device() {
        let mut session = Session::default();
        session.select_channel(0);
        assert_eq!(session.channels[0].name, DeviceKind::Sampler.default_channel_name(0));

        session.change_selected_source(DeviceKind::Ds01);

        assert_eq!(session.channels[0].name, DeviceKind::Ds01.default_channel_name(0));
    }

    /// A colour is content the same way a name is, and it is content the
    /// channel keeps: a source change is not a statement about it either.
    #[test]
    fn a_channel_takes_a_colour_and_keeps_it_across_a_source_change() {
        let mut session = Session::default();
        let green = ProjectColor::new(0x84, 0xCC, 0x16);
        session.select_channel(0);

        assert!(session.set_channel_color(0, Some(green)));
        assert_eq!(session.channels[0].color, Some(green));
        assert!(!session.set_channel_color(0, Some(green)), "re-choosing is not an edit");

        session.change_selected_source(DeviceKind::MlP8);
        assert_eq!(session.channels[0].color, Some(green), "the source change ate the colour");

        assert!(session.set_channel_color(0, None), "clearing a colour was refused");
        assert_eq!(session.channels[0].color, None);
        assert!(!session.set_channel_color(99, Some(green)), "a missing channel was coloured");
    }

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

    /// Renaming, from the side that stores the name. The UI half of this --
    /// that the field keeps showing the name after the edit rather than the
    /// keystrokes -- is `NameField`'s, and is what made a rename look broken
    /// even where the store was correct.
    #[test]
    fn a_channel_takes_a_name_and_refuses_a_blank_one() {
        let mut session = Session::default();
        assert!(session.rename_channel(0, "  Kick  "), "a real name was refused");
        assert_eq!(session.channels[0].name, "Kick", "the name was not trimmed");

        assert!(!session.rename_channel(0, "Kick"), "an unchanged name reported a change");
        assert!(!session.rename_channel(0, "   "), "a blank name was stored");
        assert_eq!(session.channels[0].name, "Kick", "a refused name was still applied");

        assert!(!session.rename_channel(-1, "Snare"));
        assert!(!session.rename_channel(session.channels.len() as i32, "Snare"));
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
