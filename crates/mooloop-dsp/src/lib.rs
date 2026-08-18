//! DSP nodes: instruments, effects, and the buffers/events that connect them.
//!
//! - [`bus`]: the stereo buffers nodes process in place.
//! - [`event`]: sample-accurate event lists (VST3/CLAP-style).
//! - [`node`]: the `AudioNode` trait every DSP unit implements.
//! - [`sampler`]: the sample-playback instrument.
//! - [`drumsynth`]: the percussive synth (kick / snare / hat).
//! - [`monosynth`]: the three-oscillator mono synth.
//! - [`env`], [`osc`], [`filter`]: building blocks shared by the synths.
//!
//! The synths implement `AudioNode` but are not yet wired into channels or
//! the UI; that integration is a later step. Effects (Filter, Delay) arrive
//! in later phases and implement the same `AudioNode` trait.

pub mod bus;
pub mod drumsynth;
pub mod env;
pub mod event;
pub mod filter;
pub mod monosynth;
pub mod node;
pub mod osc;
pub mod sampler;

pub use bus::{pan_gains, StereoBus, MAX_BLOCK_SIZE};
pub use drumsynth::DrumSynth;
pub use event::{Event, EventList, TimedEvent};
pub use monosynth::MonoSynth;
pub use node::{AudioNode, ProcessContext};
pub use sampler::{SampleData, Sampler};
