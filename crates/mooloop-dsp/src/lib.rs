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
//!   (see `docs/archive/EFFECTS_PLAN.md` and `docs/MODULATION.md`).
//! - [`align`]: the dry-path latency delay the engine's effect container
//!   blends against.
//! - [`output_guard`]: the master's last stage -- the non-finite scrub and
//!   the 0 dBFS safety limiter every block passes through on its way out.
//! - [`env`], [`osc`], [`lfo`], [`filter`], [`biquad`], [`scale`],
//!   [`shaper`], [`smooth`]: building blocks shared by the synths and
//!   effects.
//! - [`testkit`]: the measurement kit every DSP test measures through, at
//!   the four sample rates in [`testkit::RATES`].
//!
//! Every node here implements `AudioNode`. A generator writes the bus; an
//! effect reads and modifies it in place, after the generator.

pub mod align;
pub mod analysis;
pub mod aux_in;
pub mod biquad;
pub mod buffer_device;
pub mod commit;
pub mod console;
pub mod bus;
pub mod delayline;
pub mod drumsynth;
pub mod ds01;
pub mod dynamics;
pub mod effects;
pub mod env;
pub mod event;
pub mod filter;
pub mod harmonics;
pub mod interpolate;
pub mod heldnotes;
pub mod lfo;
pub mod modulator;
pub mod mlm1;
pub mod mlp8;
pub mod monosynth;
pub mod node;
pub mod osc;
pub mod output_guard;
pub mod polysynth;
pub mod preamp;
pub mod sample_analysis;
pub mod sampler;
pub mod scale;
pub mod shaper;
pub mod smooth;
pub mod strip;
pub mod stretch;
#[cfg(test)]
mod stretch_cost;
pub mod taps;
pub mod testkit;
pub mod glide;
pub mod voice_filter;

mod synth_voice;

pub use align::IntegerDelay;
pub use aux_in::AuxIn;
pub use taps::AudioTaps;
pub use stretch::{render_stretched, StretchPool, StretchReader, StretchRender, Stretcher};
pub use analysis::{SpectrumAnalyzer, SPECTRUM_BINS};
pub use buffer_device::{
    buffer_allocation_key, BufferDevice, BufferDisplay, TimedBufferEvent, WAVEFORM_BINS,
};
pub use modulator::{ModulatorRack, NoteGateEvents, CONTROL_RATE_FRAMES};
pub use output_guard::{GuardReport, OutputGuard, OUTPUT_CEILING};
pub use bus::{balance_gains, pan_gains, StereoBus, MAX_BLOCK_SIZE};
pub use delayline::{DelayLine, ReadHead};
pub use drumsynth::DrumSynth;
pub use ds01::Ds01;
pub use effects::{
    build_effect, build_effect_at_tempo, BitcrushEffect, CompressorEffect, DelayEffect,
    DriveEffect, FilterEffect, GateEffect, LimiterEffect, ModulationEffect, ReverbEffect,
};
pub use event::{Event, EventList, TimedEvent};
pub use mlm1::MlM1;
pub use mlp8::MlP8;
pub use monosynth::MonoSynth;
pub use mooloop_core::{BufferDuration, BufferEvent, BufferParams};
pub use node::{
    feedback_tail_frames, AudioNode, ControlCurve, CurveKind, Discontinuity, DynamicsFrame,
    HostedParam, ProcessContext,
    SourceNode, MAX_CONTROL_TICKS_PER_BLOCK, REST_EPSILON, SILENCE_PEAK,
};
pub use polysynth::PolySynth;
pub use sampler::{ChannelAudioSnapshot, RetiredAudio, SampleData, Sampler};
