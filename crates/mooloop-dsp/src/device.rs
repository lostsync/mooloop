//! The `Device` trait — the common interface every audio node implements.
//!
//! Output is **planar stereo**: separate left and right buffers, matching the
//! per-port model used by JACK. Devices add into the buffers (they do not
//! overwrite) so several devices can sum into the same channel strip.

/// Per-block context handed to every `Device::process` call. The data here is
/// valid only for the duration of the call and must not be retained.
#[derive(Debug, Clone, Copy)]
pub struct ProcessContext {
    /// Sample rate in Hz, fixed for the lifetime of the audio client.
    pub sample_rate: u32,
    /// Number of frames in this block (same length as each output buffer).
    pub frames: usize,
}

/// A realtime audio node.
///
/// ## Realtime safety contract
///
/// `process` and `note_on`/`note_off` run on the JACK realtime thread.
/// Implementations MUST NOT:
/// - allocate or free memory,
/// - take any lock that could be contended by a non-RT thread,
/// - perform I/O or syscalls,
/// - block or wait.
///
/// Parameter changes arrive via lock-free structures owned by the device; the
/// trait exposes only the realtime surface.
pub trait Device {
    /// Render `ctx.frames` samples into the planar stereo output buffers.
    /// Buffers are pre-zeroed by the engine; devices **add** into them.
    fn process(&mut self, ctx: ProcessContext, out_l: &mut [f32], out_r: &mut [f32]);

    /// Schedule a note-on at `sample_offset` samples into the upcoming block.
    /// `note` is a MIDI note number (0..=127), `velocity` is 0..=127.
    /// Default no-op so effects can ignore note events.
    fn note_on(&mut self, _sample_offset: usize, _note: u8, _velocity: u8) {}

    /// Schedule a note-off at `sample_offset` samples into the upcoming block.
    fn note_off(&mut self, _sample_offset: usize, _note: u8) {}
}
