//! Mixer and track edits. `docs/TERMINOLOGY.md` for which word means what.

use crate::session::Session;
use mooloop_core::{
    compile_bus_graph, is_legal_send, sanitize_route, would_create_cycle, AuxSend, BusSetup,
    EffectParams, EffectTarget, EngineCommand, SendTap, MAX_BUSES, MAX_LINEAR_GAIN,
};

/// A send is addressed by its track and its position in that track's own run,
/// which is the order it was authored in. Both arrive from the face as `i32`.
fn send_address(bus: i32, send: i32) -> Option<(usize, usize)> {
    Some((usize::try_from(bus).ok()?, usize::try_from(send).ok()?))
}

/// Why a routing edit was refused, for the status bar to say.
pub struct RoutingLoop {
    /// The bus that already feeds the one being re-routed.
    pub feeder: String,
}

impl Session {
    /// Points the device rack at a bus.
    pub fn select_bus(&mut self, bus: i32) -> Option<u8> {
        let bus = u8::try_from(bus).ok()?;
        if bus as usize >= self.buses.len() {
            return None;
        }
        self.effect_target = EffectTarget::Bus(bus);
        Some(bus)
    }

    /// Add a track, returning where it landed and its name.
    ///
    /// One `+`, one kind of thing: what a track *is* -- an ordinary track, a
    /// bus, a send -- is decided by what routes into it, not by which button
    /// made it. `docs/TERMINOLOGY.md` is the vocabulary.
    pub fn add_track(&mut self) -> Option<usize> {
        if self.buses.len() >= MAX_BUSES {
            return None;
        }
        let index = self.buses.len();
        self.buses.push(BusSetup::new(index));
        Some(index)
    }

    /// Remove the track at `index`, closing the gap.
    ///
    /// Refused on the master, which every route eventually reaches. The
    /// caller reinstalls the document, because everything that named a later
    /// track has to renumber and that is `Project::remove_track`'s walk --
    /// this side only says whether the gesture is allowed.
    pub fn can_remove_track(&self, index: usize) -> bool {
        index != mooloop_core::MASTER_BUS as usize && index < self.buses.len()
    }

    /// Rename a track. The gap `LOOSE_ENDS.md` has been carrying: the name
    /// has always saved and loaded and nothing could set it.
    ///
    /// An empty name is refused rather than stored, because a nameless column
    /// in a mixer is worse than a numbered one.
    pub fn rename_track(&mut self, index: i32, name: &str) -> bool {
        let name = name.trim();
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        let Some(setup) = self.buses.get_mut(index) else {
            return false;
        };
        if name.is_empty() || setup.bus.name == name {
            return false;
        }
        setup.bus.name = name.to_string();
        true
    }

    /// Flips a bus's mute.
    pub fn toggle_bus_mute(&mut self, bus: i32) -> Option<EngineCommand> {
        let index = usize::try_from(bus).ok()?;
        let setup = self.buses.get_mut(index)?;
        setup.bus.muted = !setup.bus.muted;
        Some(EngineCommand::SetBusMuted {
            bus: index as u8,
            muted: setup.bus.muted,
        })
    }

    /// Flips whether a track's *output* is analog-summed.
    ///
    /// Refused on the master, which feeds nothing: encoding there would put
    /// the mix into a sum nothing decodes. Nesting otherwise needs no special
    /// case -- a console-on track is a producer like any other and whatever
    /// it feeds decodes it.
    pub fn toggle_bus_console(&mut self, bus: i32) -> Option<EngineCommand> {
        let index = usize::try_from(bus).ok()?;
        if index == mooloop_core::MASTER_BUS as usize {
            return None;
        }
        let setup = self.buses.get_mut(index)?;
        setup.bus.console = !setup.bus.console;
        Some(EngineCommand::SetTrackConsole {
            bus: index as u8,
            enabled: setup.bus.console,
        })
    }

    /// Sets a bus's output level.
    ///
    /// The fader's throw reaches +6 dB and the engine's output stage accepts
    /// +12, the same as a channel's: clamping at unity left the top of every
    /// bus fader dead.
    pub fn set_bus_volume(&mut self, bus: i32, volume: f32) -> Option<EngineCommand> {
        let index = usize::try_from(bus).ok()?;
        let setup = self.buses.get_mut(index)?;
        setup.bus.volume = volume.clamp(0.0, MAX_LINEAR_GAIN);
        Some(EngineCommand::SetBusVolume {
            bus: index as u8,
            volume: setup.bus.volume,
        })
    }

    /// Sets a bus's pan position.
    pub fn set_bus_pan(&mut self, bus: i32, pan: f32) -> Option<EngineCommand> {
        let index = usize::try_from(bus).ok()?;
        let setup = self.buses.get_mut(index)?;
        setup.bus.pan = pan.clamp(-1.0, 1.0);
        Some(EngineCommand::SetBusPan {
            bus: index as u8,
            pan: setup.bus.pan,
        })
    }

    /// Re-routes a bus's output.
    ///
    /// `Err` is a refusal the user has to be told about. The picker greys out
    /// looping destinations already, but this is the boundary the engine's
    /// schedule rests on, so a graph that cannot be sorted is refused here as
    /// well rather than shipped.
    ///
    /// **Returns no command.** It used to hand back an
    /// `EngineCommand::InstallBusGraph` for the caller to send; the graph now
    /// travels with the sends that ride on it, which carry compensation rings
    /// and so must be prepared and reclaimed off the audio thread. The pump's
    /// `Session::sync_track_graph` derives and sends it, which is where the
    /// console accumulators and the audio edges already go.
    pub fn set_bus_output(&mut self, bus: i32, output: i32) -> Option<Result<(), RoutingLoop>> {
        let index = usize::try_from(bus).ok()?;
        let output = u8::try_from(output).ok()?;
        let output = sanitize_route(index as u8, output);
        self.buses.get(index)?;
        if would_create_cycle(&self.buses, index as u8, output) {
            return Some(Err(RoutingLoop {
                feeder: self.buses[output as usize].bus.name.clone(),
            }));
        }
        let previous = std::mem::replace(&mut self.buses[index].bus.output, output);
        if compile_bus_graph(&self.buses).is_none() {
            // Unreachable given the check above. Restore the visible graph
            // rather than letting the model and the audio diverge.
            self.buses[index].bus.output = previous;
            return None;
        }
        // Explicitly, because this used to happen by accident: the command
        // this handed back travelled through `apply_engine_message`, which
        // marks every non-transport command as an edit. Nothing is handed
        // back now, so a routing change would have stopped making the
        // document look unsaved.
        self.mark_dirty();
        Some(Ok(()))
    }

    /// Routes a send from `bus` to `target`, returning where it landed in
    /// that track's own run of sends.
    ///
    /// This is the whole of "creating a send": there is no send object to
    /// make and no track to create. `docs/TERMINOLOGY.md` -- what a track
    /// *is* is decided by what routes into it, so the track at the far end
    /// becomes an effects return by being sent to, and stops being one when
    /// the last send goes away.
    ///
    /// `Err` for the same reason [`Self::set_bus_output`] has one: a send
    /// into something that already reaches this track closes a loop, and it
    /// is refused rather than delayed.
    pub fn add_send(&mut self, bus: i32, target: i32) -> Option<Result<usize, RoutingLoop>> {
        let index = usize::try_from(bus).ok()?;
        let target = u8::try_from(target).ok()?;
        self.buses.get(index)?;
        self.buses.get(target as usize)?;
        if !is_legal_send(index as u8, target) {
            return None;
        }
        if would_create_cycle(&self.buses, index as u8, target) {
            return Some(Err(RoutingLoop {
                feeder: self.buses[target as usize].bus.name.clone(),
            }));
        }
        self.buses[index].sends.push(AuxSend::new(target));
        // Routing does not travel as a command, so the edit is marked here
        // rather than falling out of one. See `set_bus_output`.
        self.mark_dirty();
        Some(Ok(self.buses[index].sends.len() - 1))
    }

    /// Removes one of `bus`'s sends. The ones after it move up, which is why
    /// the engine is told through the plan rather than by index.
    pub fn remove_send(&mut self, bus: i32, send: i32) -> bool {
        let (Ok(index), Ok(send)) = (usize::try_from(bus), usize::try_from(send)) else {
            return false;
        };
        let Some(setup) = self.buses.get_mut(index) else {
            return false;
        };
        if send >= setup.sends.len() {
            return false;
        }
        setup.sends.remove(send);
        self.mark_dirty();
        true
    }

    /// Sets a send's level. The same range a fader has: a send is a gain
    /// stage too, and one that could not reach unity would be a trim.
    pub fn set_send_level(&mut self, bus: i32, send: i32, level: f32) -> Option<EngineCommand> {
        let (index, send) = send_address(bus, send)?;
        let entry = self.buses.get_mut(index)?.sends.get_mut(send)?;
        entry.level = level.clamp(0.0, MAX_LINEAR_GAIN);
        Some(EngineCommand::SetSendLevel {
            producer: EffectTarget::Bus(index as u8),
            index: send as u8,
            level: entry.level,
        })
    }

    /// Switches a send on or off, which is not the same as turning it down.
    pub fn set_send_enabled(&mut self, bus: i32, send: i32, enabled: bool) -> Option<EngineCommand> {
        let (index, send) = send_address(bus, send)?;
        let entry = self.buses.get_mut(index)?.sends.get_mut(send)?;
        entry.enabled = enabled;
        Some(EngineCommand::SetSendEnabled {
            producer: EffectTarget::Bus(index as u8),
            index: send as u8,
            enabled,
        })
    }

    /// Moves a send between the pre-fader and post-fader taps.
    pub fn set_send_tap(&mut self, bus: i32, send: i32, tap: SendTap) -> Option<EngineCommand> {
        let (index, send) = send_address(bus, send)?;
        let entry = self.buses.get_mut(index)?.sends.get_mut(send)?;
        entry.tap = tap;
        Some(EngineCommand::SetSendTap {
            producer: EffectTarget::Bus(index as u8),
            index: send as u8,
            tap,
        })
    }

    /// Turns an EQ slot's spectrum analyzer on or off.
    ///
    /// Returns the target and slot for the telemetry subscription; the
    /// analyzer is a view of the audio, not part of it, so it never reaches
    /// the command ring.
    pub fn set_eq_analyzer(&mut self, slot: i32, enabled: bool) -> Option<(EffectTarget, u8)> {
        let target = self.effect_target;
        let slot = usize::try_from(slot).ok()?;
        let effect = self.effect_chain_mut()?.get_mut(slot)?;
        let EffectParams::Eq(params) = &mut effect.params else {
            return None;
        };
        params.analyzer_enabled = enabled;
        Some((target, slot as u8))
    }
}

impl Session {
    /// Make sure the bank has at least `count` tracks, for tests written when
    /// a session came with seventeen. A track is now made, not found.
    #[cfg(test)]
    pub(crate) fn ensure_tracks(&mut self, count: usize) {
        while self.buses.len() < count {
            self.add_track().expect("room for a track");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{EffectKind, MASTER_BUS};

    /// The rename this crate already had and nothing exercised. A blank name
    /// is refused here where `rename_pattern` accepts one: a mixer column is
    /// identified by its name alone, and a pattern has its number beside it.
    #[test]
    fn a_track_takes_a_name_and_refuses_a_blank_one() {
        let mut session = Session::default();
        session.ensure_tracks(2);

        assert!(session.rename_track(1, "  Drum Bus  "), "a real name was refused");
        assert_eq!(session.buses[1].bus.name, "Drum Bus", "the name was not trimmed");

        assert!(!session.rename_track(1, "Drum Bus"), "an unchanged name reported a change");
        assert!(!session.rename_track(1, "  "), "a blank name was stored");
        assert_eq!(session.buses[1].bus.name, "Drum Bus", "a refused name was still applied");

        assert!(!session.rename_track(-1, "Nope"));
        assert!(!session.rename_track(session.buses.len() as i32, "Nope"));
    }

    /// A bus fader that stops at unity leaves its top half dead; both gain
    /// stages share the container's headroom.
    #[test]
    fn a_bus_fader_reaches_the_containers_headroom() {
        let mut session = Session::default();
        session.ensure_tracks(2);
        assert!(matches!(
            session.set_bus_volume(1, 100.0),
            Some(EngineCommand::SetBusVolume { volume, .. }) if volume == MAX_LINEAR_GAIN
        ));
        assert!(matches!(
            session.set_bus_pan(1, -9.0),
            Some(EngineCommand::SetBusPan { pan, .. }) if pan == -1.0
        ));
        assert!(session.set_bus_volume(9_999, 0.5).is_none());
    }

    /// The engine's schedule is a topological sort; a graph with a loop in it
    /// cannot be sorted, so the edit is refused with something to say.
    #[test]
    fn a_routing_loop_is_refused_by_name() {
        let mut session = Session::default();
        session.ensure_tracks(3);
        session.buses[1].bus.name = "Drum Bus".into();

        // Send bus 2 into bus 1, then try to close the loop the other way.
        assert!(matches!(session.set_bus_output(2, 1), Some(Ok(_))));
        let refusal = session.set_bus_output(1, 2).expect("bus 1 exists");
        let Err(RoutingLoop { feeder }) = refusal else {
            panic!("a loop was accepted");
        };
        assert_eq!(feeder, session.buses[2].bus.name);
        assert_eq!(
            session.buses[1].bus.output, MASTER_BUS,
            "the refused edge was applied anyway"
        );
    }

    /// The same refusal, reached through a **send** rather than an output.
    ///
    /// Not a duplicate of the test above: `add_send` builds its own
    /// `RoutingLoop` rather than sharing `set_bus_output`'s, so the two could
    /// name different tracks and only one of them would be caught. They are
    /// one rule -- `would_create_cycle` -- and the step doc promises the send
    /// menu greys exactly what the output picker greys, with the same
    /// sentence, so the sentence is asserted rather than assumed.
    #[test]
    fn a_send_that_would_loop_is_refused_by_name() {
        let mut session = Session::default();
        session.ensure_tracks(3);
        session.buses[2].bus.name = "Reverb".into();

        // Track 2 already feeds track 1, so a send from 1 back into 2 closes
        // the loop -- and it is the *send* that has to notice.
        assert!(matches!(session.set_bus_output(2, 1), Some(Ok(_))));
        let refusal = session.add_send(1, 2).expect("both tracks exist");
        let Err(RoutingLoop { feeder }) = refusal else {
            panic!("a send closed a loop and was accepted");
        };
        assert_eq!(feeder, session.buses[2].bus.name, "the refusal named the wrong track");
        assert!(
            session.buses[1].sends.is_empty(),
            "the refused send was pushed anyway"
        );
    }

    /// A track cannot send to itself, which is the degenerate loop and the
    /// one a user reaches first by clicking their own row.
    #[test]
    fn a_track_cannot_send_to_itself() {
        let mut session = Session::default();
        session.ensure_tracks(3);
        assert!(session.add_send(1, 1).is_none(), "a self-send was routed");
        assert!(session.buses[1].sends.is_empty());
        assert!(
            !session.allowed_destinations(1)[1],
            "the picker offered the track its own row"
        );
    }

    #[test]
    fn selecting_a_bus_points_the_rack_at_it() {
        let mut session = Session::default();
        session.ensure_tracks(4);
        assert_eq!(session.select_bus(3), Some(3));
        assert_eq!(session.effect_target, EffectTarget::Bus(3));
        assert_eq!(session.select_bus(-1), None);
        assert_eq!(session.select_bus(9_999), None);
        assert_eq!(session.effect_target, EffectTarget::Bus(3));
    }

    /// The analyzer toggle belongs to an EQ; a slot holding anything else
    /// must not be reinterpreted.
    #[test]
    fn the_analyzer_toggle_refuses_a_slot_that_is_not_an_eq() {
        let mut session = Session::default();
        session.ensure_tracks(2);
        session.select_bus(1);
        session.insert_effect_at(EffectKind::Delay, 0);
        assert!(session.set_eq_analyzer(0, true).is_none());

        session.insert_effect_at(EffectKind::Eq, 1);
        assert_eq!(
            session.set_eq_analyzer(1, true),
            Some((EffectTarget::Bus(1), 1))
        );
    }
}
