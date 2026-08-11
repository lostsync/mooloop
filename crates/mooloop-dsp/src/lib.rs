//! DSP nodes: instruments, effects, and helpers.
//!
//! Phase 0 ships only the `Device` trait definition and a `Metronome` click
//! generator used to prove the audio path works end-to-end. Real instruments
//! (Sampler, DrumSynth, MonoSynth) and effects (Filter, Delay) arrive in
//! later phases and implement the same `Device` trait.

pub mod device;
pub mod metronome;

pub use device::{Device, ProcessContext};
pub use metronome::Metronome;
