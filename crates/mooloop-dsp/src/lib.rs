//! DSP nodes: instruments, effects, and the buffers/events that connect them.
//!
//! - [`bus`]: the stereo buffers nodes process in place.
//! - [`event`]: sample-accurate event lists (VST3/CLAP-style).
//! - [`node`]: the `AudioNode` trait every DSP unit implements.
//! - [`sampler`]: the sample-playback instrument.
//!
//! Effects (Filter, Delay) and the DrumSynth / MonoSynth arrive in later
//! phases and implement the same `AudioNode` trait.

pub mod bus;
pub mod event;
pub mod node;
pub mod sampler;

pub use bus::{pan_gains, StereoBus, MAX_BLOCK_SIZE};
pub use event::{Event, EventList, TimedEvent};
pub use node::{AudioNode, ProcessContext};
pub use sampler::{SampleData, Sampler};
