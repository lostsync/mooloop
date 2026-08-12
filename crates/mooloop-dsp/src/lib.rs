//! DSP nodes: instruments, effects, and helpers.
//!
//! Phase 1 ships the `Device` trait and a `Sampler` instrument. Effects
//! (Filter, Delay) and the DrumSynth / MonoSynth arrive in later phases and
//! implement the same `Device` trait.

pub mod device;
pub mod sampler;

pub use device::{Device, ProcessContext};
pub use sampler::{SampleData, Sampler};
