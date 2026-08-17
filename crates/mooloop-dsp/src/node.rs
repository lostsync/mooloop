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
//! the engine's bus management, not this trait: a node needing an external
//! signal (e.g. a sidechain compressor) will receive a bus reference from
//! the engine at construction, exactly like LV2's sidechain port groups.
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
