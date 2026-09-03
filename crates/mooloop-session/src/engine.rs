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
use mooloop_engine::StructuralCommand;
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
