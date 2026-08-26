//! DSP nodes: instruments, effects, and the buffers/events that connect them.
//!
//! - [`bus`]: the stereo buffers nodes process in place.
//! - [`event`]: sample-accurate event lists (VST3/CLAP-style).
//! - [`node`]: the `AudioNode` trait every DSP unit implements.
//! - [`sampler`]: the sample-playback instrument.
//! - [`drumsynth`]: the percussive synth (kick / snare / hat).
//! - [`monosynth`]: the three-oscillator mono synth.
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
pub mod lfo;
pub mod modulator;
pub mod monosynth;
pub mod node;
pub mod osc;
pub mod polysynth;
pub mod sampler;
pub mod scale;
pub mod shaper;
pub mod smooth;

pub use align::DryAlign;
pub use analysis::{SpectrumAnalyzer, SPECTRUM_BINS};
pub use buffer_device::{buffer_allocation_key, BufferDevice, TimedBufferEvent};
pub use modulator::{ModulatorRack, CONTROL_RATE_FRAMES};
pub use bus::{balance_gains, pan_gains, StereoBus, MAX_BLOCK_SIZE};
pub use delayline::{DelayLine, ReadHead};
pub use drumsynth::DrumSynth;
pub use effects::{
    build_effect, build_effect_at_tempo, generate_room_ir, BitcrushEffect, CompressorEffect,
    DelayEffect, DriveEffect, FilterEffect, GateEffect, LimiterEffect, ModulationEffect,
    PreparedIr, ReverbEffect, StereoIr, CONVOLUTION_BLOCK_FRAMES,
};
pub use event::{Event, EventList, TimedEvent};
pub use monosynth::MonoSynth;
pub use mooloop_core::{BufferDuration, BufferEvent, BufferParams};
pub use node::{AudioNode, ProcessContext};
pub use polysynth::PolySynth;
pub use sampler::{SampleData, Sampler};
