//! DSP nodes: instruments, effects, and the buffers/events that connect them.
//!
//! - [`bus`]: the stereo buffers nodes process in place.
//! - [`event`]: sample-accurate event lists (VST3/CLAP-style).
//! - [`node`]: the `AudioNode` trait every DSP unit implements.
//! - [`sampler`]: the sample-playback instrument.
//! - [`drumsynth`]: the percussive synth (kick / snare / hat).
//! - [`monosynth`]: the three-oscillator mono synth.
//! - [`mlm1`]: the ML-M1, built around its filter and its note
//!   behaviour rather than around being the poly synth with one voice.
//! - [`polysynth`]: the three-oscillator poly synth.
//! - [`mlp8`]: the ML-P8, eight voices around a three-oscillator network
//!   rather than three oscillators layered.
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
pub mod commit;
pub mod bus;
pub mod delayline;
pub mod drumsynth;
pub mod dynamics;
pub mod effects;
pub mod env;
pub mod event;
pub mod filter;
pub mod interpolate;
pub mod heldnotes;
pub mod lfo;
pub mod modulator;
pub mod mlm1;
pub mod mlp8;
pub mod monosynth;
pub mod node;
pub mod osc;
pub mod polysynth;
pub mod sample_analysis;
pub mod sampler;
pub mod scale;
pub mod shaper;
pub mod smooth;
pub mod stretch;

mod synth_voice;

pub use align::DryAlign;
pub use stretch::{render_stretched, StretchPool, StretchReader, StretchRender, Stretcher};
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
pub use mlm1::MlM1;
pub use mlp8::MlP8;
pub use monosynth::MonoSynth;
pub use mooloop_core::{BufferDuration, BufferEvent, BufferParams};
pub use node::{AudioNode, DynamicsFrame, ProcessContext};
pub use polysynth::PolySynth;
pub use sampler::{SampleData, Sampler};
