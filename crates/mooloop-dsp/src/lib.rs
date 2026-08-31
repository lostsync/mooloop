//! DSP nodes: instruments, effects, and the buffers/events that connect them.
//!
//! - [`bus`]: the stereo buffers nodes process in place.
//! - [`event`]: sample-accurate event lists (VST3/CLAP-style).
//! - [`node`]: the `AudioNode` trait every DSP unit implements.
//! - [`sampler`]: the sample-playback instrument.
//! - [`drumsynth`]: the percussive synth (kick / snare / hat).
//! - [`monosynth`]: the three-oscillator mono synth.
//! - [`ml1`]: the ML-1, built around its filter and its note
//!   behaviour rather than around being the poly synth with one voice.
//! - [`polysynth`]: the three-oscillator poly synth.
//! - [`effects`]: chainable effects that run after a channel's generator
//!   (see `docs/EFFECTS_PLAN.md` and `docs/MODULATION_PLAN.md`).
//! - [`align`]: the dry-path latency delay the engine's effect container
//!   blends against.
//! - [`env`], [`osc`], [`lfo`], [`filter`], [`biquad`], [`scale`],
//!   [`shaper`], [`smooth`]: building blocks shared by the synths and
//!   effects.
//!
//! The synths implement `AudioNode` but are not yet wired into channels or
//! the UI; that integration is a later step. Effects implement the same
//! `AudioNode` trait, processing the bus in place after the generator.

pub mod align;
pub mod analysis;
pub mod biquad;
pub mod buffer_device;
pub mod bus;
pub mod delayline;
pub mod drumsynth;
pub mod dynamics;
pub mod effects;
pub mod env;
pub mod event;
pub mod filter;
pub mod heldnotes;
pub mod lfo;
pub mod modulator;
pub mod ml1;
pub mod monosynth;
pub mod node;
pub mod osc;
pub mod polysynth;
pub mod sample_analysis;
pub mod sampler;
pub mod scale;
pub mod shaper;
pub mod smooth;
mod synth_voice;

pub use align::DryAlign;
pub use analysis::{SpectrumAnalyzer, SPECTRUM_BINS};
pub use buffer_device::{buffer_allocation_key, BufferDevice, TimedBufferEvent};
pub use modulator::{ModulatorRack, NoteGateEvents, CONTROL_RATE_FRAMES};
pub use bus::{balance_gains, pan_gains, StereoBus, MAX_BLOCK_SIZE};
pub use delayline::{DelayLine, ReadHead};
pub use drumsynth::DrumSynth;
pub use effects::{
    build_effect, build_effect_at_tempo, BitcrushEffect, CompressorEffect, DelayEffect,
    DriveEffect, FilterEffect, GateEffect, LimiterEffect, ModulationEffect, ReverbEffect,
};
pub use event::{Event, EventList, TimedEvent};
pub use ml1::Ml1;
pub use monosynth::MonoSynth;
pub use mooloop_core::{BufferDuration, BufferEvent, BufferParams};
pub use node::{AudioNode, ProcessContext};
pub use polysynth::PolySynth;
pub use sampler::{SampleData, Sampler};
