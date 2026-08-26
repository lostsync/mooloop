//! The `AudioNode` trait — the common interface every DSP unit implements.
//!
//! This is deliberately shaped like the modern plugin APIs (VST3's
//! `IAudioProcessor`, CLAP's `process`, LV2's `run`): each call hands the
//! node an audio buffer to work on, a list of sample-timed input events, and
//! an optional output event list. A future plugin-hosting layer can map CLAP
//! or LV2 plugins onto this trait one-to-one.
//!
//! ## Processing model
//!
//! Nodes process **in place** on a [`StereoBus`]:
//!
//! - **Instruments** assume the bus has been cleared for the block and add
//!   their rendered audio into it.
//! - **Effects** read the bus and modify it in place (input == output).
//!
//! The engine owns all buses and decides what each node's buffer means —
//! today every channel strip is `instrument -> [effects] -> gain/pan ->
//! master`. Future routing (send/return buses, sidechain inputs) hangs off
//! the engine's bus management, not a node retaining someone else's storage.
//! A future process-buffer view will provide auxiliary inputs such as a
//! sidechain for the duration of each call, exactly like plugin port groups.
//!
//! ## Realtime safety contract
//!
//! `process` runs on the JACK realtime thread. Implementations MUST NOT:
//! - allocate or free memory,
//! - take any lock that could be contended by a non-RT thread,
//! - perform I/O or syscalls,
//! - block or wait.
//!
//! Parameter changes arrive via the command queue / lock-free structures
//! owned by the node; the trait exposes only the realtime surface.

use crate::bus::StereoBus;
use crate::event::EventList;

/// Per-block context handed to every `AudioNode::process` call. Valid only
/// for the duration of the call; must not be retained.
#[derive(Debug, Clone, Copy)]
pub struct ProcessContext {
    /// Sample rate in Hz, fixed for the lifetime of the audio client.
    pub sample_rate: u32,
    /// Number of frames in this block (the active region of the bus).
    pub frames: usize,
    /// Transport state for this block. Nodes that sync to the host
    /// (tempo-synced delays, LFOs) should read these rather than guessing.
    pub playing: bool,
    pub bpm: f64,
    /// Transport position in ticks at the start of this block.
    pub position_ticks: f64,
    /// Absolute transport position in frames at the start of this block.
    /// Ground truth for tempo-synced nodes (delays, LFOs): unlike tick
    /// position it never accumulates float error.
    pub position_frames: u64,
}

/// A realtime audio node (instrument or effect).
pub trait AudioNode {
    /// Processing latency of the active path in base-rate frames. The value is
    /// queried while a graph is prepared, never from a hot inner sample loop.
    /// Nodes with internal parallel paths must align them before reporting the
    /// resulting external latency.
    fn latency_frames(&self) -> u32 {
        0
    }

    /// Frames by which the generic host should delay its dry branch before it
    /// mixes it with this node. Most latency-producing effects return their
    /// processing latency here. A wet-only device such as convolution reverb
    /// can retain an intentional delayed return without moving the channel's
    /// dry signal and neighbouring tracks with it.
    fn dry_path_latency_frames(&self) -> u32 {
        self.latency_frames()
    }

    /// Number of times a retained-audio read head has been overtaken by its
    /// writer and force-returned to live. Only the buffer device reports a
    /// nonzero value; the host publishes it as display telemetry so forced
    /// returns are observable without logging from the audio thread.
    fn buffer_collisions(&self) -> u64 {
        0
    }

    /// Process one block in place on `bus`. `events_in` is sorted by sample
    /// offset; nodes that respond to events must split rendering at those
    /// offsets. `events_out` is provided when the engine wants events back
    /// (metronomes, arpeggiators, MIDI-generating effects); most nodes
    /// ignore it.
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        events_out: Option<&mut EventList>,
    );
}
