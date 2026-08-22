//! DSP nodes: instruments, effects, and the buffers/events that connect them.
//!
//! - [`bus`]: the stereo buffers nodes process in place.
//! - [`event`]: sample-accurate event lists (VST3/CLAP-style).
//! - [`node`]: the `AudioNode` trait every DSP unit implements.
//! - [`sampler`]: the sample-playback instrument.
//! - [`drumsynth`]: the percussive synth (kick / snare / hat).
//! - [`monosynth`]: the three-oscillator mono synth.
//! - [`effects`]: chainable effects that run after a channel's generator
//!   (see `docs/EFFECTS_PLAN.md` and `docs/MODULATION_PLAN.md`).
//! - [`env`], [`osc`], [`lfo`], [`filter`], [`shaper`], [`smooth`]: building
//!   blocks shared by the synths and effects.
//!
//! The synths implement `AudioNode` but are not yet wired into channels or
//! the UI; that integration is a later step. Effects implement the same
//! `AudioNode` trait, processing the bus in place after the generator.

pub mod bus;
pub mod delayline;
pub mod drumsynth;
pub mod dynamics;
pub mod effects;
pub mod env;
pub mod event;
pub mod filter;
pub mod lfo;
pub mod monosynth;
pub mod node;
pub mod osc;
pub mod sampler;
pub mod shaper;
pub mod smooth;

pub use bus::{balance_gains, pan_gains, StereoBus, MAX_BLOCK_SIZE};
pub use delayline::{DelayLine, ReadHead};
pub use drumsynth::DrumSynth;
pub use effects::{
    build_effect, BitcrushEffect, CompressorEffect, DelayEffect, DriveEffect, FilterEffect,
    GateEffect, LimiterEffect,
};
pub use event::{Event, EventList, TimedEvent};
pub use monosynth::MonoSynth;
pub use node::{AudioNode, ProcessContext};
pub use sampler::{SampleData, Sampler};
