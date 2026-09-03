//! The wire between the session and the audio engine.
//!
//! UI callbacks all run on one thread, but boxed structural edits and POD
//! commands used to enter separate relay queues and lose their relative
//! order. Everything below shares one queue so that ordering survives, and
//! the typed senders keep the convenient `.send(...)` shape the callers use.

use crate::channel::ChannelState;
use crate::project::ProjectEdit;
use mooloop_core::{DeviceKind, EffectTarget, EngineCommand, SliceMap};
use mooloop_dsp::SampleData;
use crate::session::Session;
use mooloop_engine::{EngineHandle, StructuralCommand};
use std::sync::Arc;

/// UI callbacks all run on one thread, but boxed structural edits and POD
/// commands used to enter separate relay queues and lose their relative
/// order. These typed senders share one queue while preserving the convenient
/// `.send(...)` call shape used by the callback wiring below.
///
/// The width is `StructuralCommand`'s, and `EngineCommand` (`bridge.rs`
/// documents what sets that) is the runner-up. This queue is drained on the
/// UI thread into the preallocated ring, so evening the variants out with a
/// `Box` would trade a fixed stack copy for an allocation per command and
/// cost the `Copy` the wiring relies on.
#[allow(clippy::large_enum_variant)]
pub enum PendingEngineMessage {
    Command(EngineCommand),
    ResizeBuffers {
        bpm: f64,
    },
    Structural(StructuralCommand),
    /// Adding a channel allocates its strip, event list and control-output
    /// buffer, so it is structural rather than POD. The pump expands it: the
    /// engine handle owns the sample slot the new strip needs.
    AddChannel {
        channel: usize,
        source: DeviceKind,
    },
    ProjectEdit(ProjectEdit),
    Audio(AudioAction),
    Telemetry(TelemetryAction),
    /// Linear preview gain. A plain value rather than a command because the
    /// engine reads it from a shared cell, live, while a preview plays.
    PreviewGain(f32),
}

/// Display subscriptions are handled by the pump, which exclusively owns the
/// engine handle. They observe a device's signal; they are not audio-thread
/// commands and never become modulation routes.
pub enum TelemetryAction {
    SetEffectSpectrumEnabled {
        target: EffectTarget,
        slot: u8,
        enabled: bool,
    },
}

/// One requested change from the Audio preferences page. These reach
/// `EngineHandle` directly rather than through `EngineCommand`: they are
/// non-realtime JACK API calls (port connect/disconnect, buffer resize), not
/// realtime-thread state, but `handle` still only lives inside the pump.
pub enum AudioAction {
    /// Apply settings loaded from disk at startup, before the user has
    /// touched the Audio page.
    ApplyPersisted(mooloop_engine::AudioConfig),
    /// Re-read the live JACK graph and driver status.
    RefreshTargets,
    SelectOutput {
        port_l: String,
        port_r: String,
    },
    SelectBufferSize(u32),
    SetAutoReconnect(bool),
}

#[derive(Clone)]
pub struct EngineCommandSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl EngineCommandSender {
    pub fn send(&self, command: EngineCommand) -> bool {
        self.0.send(PendingEngineMessage::Command(command)).is_ok()
    }

    pub fn resize_buffers(&self, bpm: f64) -> bool {
        self.0
            .send(PendingEngineMessage::ResizeBuffers { bpm })
            .is_ok()
    }
}

#[derive(Clone)]
pub struct StructuralCommandSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl StructuralCommandSender {
    pub fn send(&self, command: StructuralCommand) -> bool {
        self.0
            .send(PendingEngineMessage::Structural(command))
            .is_ok()
    }

    pub fn add_channel(&self, channel: usize, source: DeviceKind) -> bool {
        self.0
            .send(PendingEngineMessage::AddChannel { channel, source })
            .is_ok()
    }
}

#[derive(Clone)]
pub struct ProjectEditSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl ProjectEditSender {
    pub fn send(&self, edit: ProjectEdit) -> bool {
        self.0.send(PendingEngineMessage::ProjectEdit(edit)).is_ok()
    }
}

#[derive(Clone)]
pub struct AudioActionSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl AudioActionSender {
    pub fn send(&self, action: AudioAction) -> bool {
        self.0.send(PendingEngineMessage::Audio(action)).is_ok()
    }
}

#[derive(Clone)]
pub struct TelemetryActionSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl TelemetryActionSender {
    pub fn send(&self, action: TelemetryAction) -> bool {
        self.0.send(PendingEngineMessage::Telemetry(action)).is_ok()
    }
}

#[derive(Clone)]
pub struct PreviewSender(pub std::sync::mpsc::Sender<PendingEngineMessage>);

impl PreviewSender {
    pub fn send_gain(&self, gain: f32) -> bool {
        self.0.send(PendingEngineMessage::PreviewGain(gain)).is_ok()
    }
}

/// A channel's audio, on its way to the pump.
///
/// Neither half can ride the command ring: `EngineCommand` is `Copy` and
/// unboxed by design, and both of these live in `ArcSwap` slots the pump
/// exclusively owns. Same route the built-in sample reset already takes.
///
/// Both are always sent together because they are one fact: after a commit
/// the published buffer and the map that indexes it change at the same
/// instant, and delivering one without the other would leave the voice
/// reading markers that name frames in a buffer it no longer holds.
pub struct ChannelAudio {
    pub channel: usize,
    pub sample: Option<Arc<SampleData>>,
    pub slices: Option<Arc<SliceMap>>,
}

#[derive(Clone)]
pub struct ChannelAudioSender(pub std::sync::mpsc::Sender<ChannelAudio>);

pub fn publish_channel_audio_to(tx: &ChannelAudioSender, channel: usize, state: &ChannelState) {
    let _ = tx.0.send(ChannelAudio {
        channel,
        sample: state.published_sample().cloned(),
        slices: (!state.slices.is_empty()).then(|| Arc::new(state.slices.clone())),
    });
}

/// Where the transport is, in the terms the position readout shows.
pub struct TransportPosition {
    /// Step under the playhead, wrapped into the current pattern.
    pub step: i32,
    /// Position along the arrangement, or `None` in pattern mode.
    pub playlist_ticks: Option<i32>,
    pub bar: i32,
    pub beat: i32,
    pub tick: i32,
}

impl Session {
    /// Applies one queued message that needs nothing but the engine handle.
    ///
    /// Returns whether the document just became dirty, which the caller turns
    /// into a title refresh -- once per drain rather than once per message.
    ///
    /// `ProjectEdit` and `Audio` are not handled here: both end in something
    /// the user sees, so both stay with the layer that can show it.
    pub fn apply_engine_message(
        &mut self,
        handle: &mut EngineHandle,
        message: PendingEngineMessage,
    ) -> bool {
        match message {
            PendingEngineMessage::Command(command) => {
                // Transport is not an edit: starting playback must not make
                // an untouched document look unsaved.
                let edits = !matches!(
                    command,
                    EngineCommand::Play | EngineCommand::Pause | EngineCommand::Stop
                );
                handle.send(command);
                edits && self.became_dirty()
            }
            PendingEngineMessage::PreviewGain(gain) => {
                handle.set_preview_gain(gain);
                false
            }
            PendingEngineMessage::ResizeBuffers { bpm } => {
                // Each replacement allocates its ring on the pump thread; the
                // ordered realtime queue then swaps the ready node at a block
                // boundary.
                for (target, slot, params) in self.buffer_effects() {
                    let _ = handle.replace_buffer(target, slot, params, params, bpm);
                }
                false
            }
            PendingEngineMessage::AddChannel { channel, source } => {
                handle.add_channel(channel, source);
                self.became_dirty()
            }
            PendingEngineMessage::Structural(command) => {
                handle.send_structural(command);
                // Any structural change is an unsaved edit.
                self.became_dirty()
            }
            PendingEngineMessage::Telemetry(TelemetryAction::SetEffectSpectrumEnabled {
                target,
                slot,
                enabled,
            }) => {
                handle.set_effect_spectrum_enabled(target, slot, enabled);
                false
            }
            // Both of these end in something the user sees; the view keeps
            // them. `apply_engine_message` is only reached for the rest.
            PendingEngineMessage::ProjectEdit(_) | PendingEngineMessage::Audio(_) => false,
        }
    }

    /// Marks the document edited, reporting whether that was news.
    fn became_dirty(&mut self) -> bool {
        let was_clean = !self.dirty;
        self.mark_dirty();
        was_clean
    }

    /// Every buffer device in the document, channel chains then bus chains.
    fn buffer_effects(&self) -> Vec<(EffectTarget, u8, mooloop_core::BufferParams)> {
        let channels = self.channels.iter().enumerate().flat_map(|(channel, state)| {
            state.effects.iter().enumerate().filter_map(move |(slot, effect)| {
                effect
                    .params
                    .buffer()
                    .copied()
                    .map(|params| (EffectTarget::Channel(channel as u8), slot as u8, params))
            })
        });
        let buses = self.buses.iter().enumerate().flat_map(|(bus, state)| {
            state.effects.iter().enumerate().filter_map(move |(slot, effect)| {
                effect
                    .params
                    .buffer()
                    .copied()
                    .map(|params| (EffectTarget::Bus(bus as u8), slot as u8, params))
            })
        });
        channels.chain(buses).collect()
    }

    /// Where a transport tick lands, in the readout's terms.
    ///
    /// In song mode the position is along the arrangement; in pattern mode it
    /// wraps inside the pattern on screen, which is why the two cannot share
    /// one modulus.
    pub fn transport_position(&self, tick: u64) -> TransportPosition {
        let length = self.pattern_lengths[self.current_pattern] as u64;
        let ticks_per_step = (mooloop_core::Ppq::DEFAULT.ticks_per_beat() / 4) as u64;
        let ticks_per_beat = mooloop_core::Ppq::DEFAULT.ticks_per_beat() as u64;
        let (position_ticks, playlist_ticks) = if self.song_mode {
            let position = tick % u64::from(self.song_length_ticks());
            (position, Some(position as i32))
        } else {
            (tick % (length * ticks_per_step), None)
        };
        let ticks_per_bar = u64::from(mooloop_core::TICKS_PER_BAR);
        let tick_in_bar = position_ticks % ticks_per_bar;
        TransportPosition {
            step: ((tick / ticks_per_step) % length) as i32,
            playlist_ticks,
            bar: (position_ticks / ticks_per_bar) as i32 + 1,
            beat: (tick_in_bar / ticks_per_beat) as i32 + 1,
            tick: (tick_in_bar % ticks_per_beat) as i32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{PatternPlacement, TICKS_PER_BAR, TICKS_PER_STEP};

    /// Pattern mode wraps inside the pattern on screen; song mode runs along
    /// the arrangement. The two cannot share one modulus, which is the whole
    /// reason this is not a single expression.
    #[test]
    fn the_readout_wraps_by_pattern_or_by_song_depending_on_the_transport() {
        let mut session = Session::default();
        let pattern_ticks = session.pattern_lengths[0] as u32 * TICKS_PER_STEP;

        // One tick past the end of the pattern is the top of it again.
        let wrapped = session.transport_position(u64::from(pattern_ticks));
        assert_eq!(wrapped.step, 0);
        assert_eq!(wrapped.bar, 1);
        assert_eq!(wrapped.playlist_ticks, None);

        // Two clips back to back, so the song is twice the pattern.
        session.song_mode = true;
        session.playlist = vec![
            PatternPlacement::new(0, 0),
            PatternPlacement::new(0, pattern_ticks),
        ];
        let along = session.transport_position(u64::from(pattern_ticks));
        assert_eq!(
            along.playlist_ticks,
            Some(pattern_ticks as i32),
            "song mode wrapped inside the pattern instead of along the song"
        );
        assert_eq!(along.bar, (pattern_ticks / TICKS_PER_BAR) as i32 + 1);
    }

    /// Bar, beat and tick are one-based where the readout shows them and
    /// zero-based where it does not, which is easy to get backwards.
    #[test]
    fn the_position_readout_counts_bars_and_beats_from_one() {
        let session = Session::default();
        let at_start = session.transport_position(0);
        assert_eq!((at_start.bar, at_start.beat, at_start.tick), (1, 1, 0));

        let ticks_per_beat = u64::from(TICKS_PER_BAR) / 4;
        let second_beat = session.transport_position(ticks_per_beat + 3);
        assert_eq!(
            (second_beat.bar, second_beat.beat, second_beat.tick),
            (1, 2, 3)
        );
    }
}
