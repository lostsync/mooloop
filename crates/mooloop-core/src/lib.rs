//! Core data model and message types shared between the engine and the UI.
//!
//! No dependency on audio or UI code so this can be linked freely from both the
//! realtime and GUI threads.

pub mod bridge;
pub mod channel;
pub mod pattern;
pub mod sampler;
pub mod time;

pub use bridge::{EngineCommand, EngineEvent};
pub use channel::{Channel, DeviceKind, MAX_CHANNELS, MAX_PATTERNS};
pub use pattern::{ChannelPattern, Pattern, Step, DEFAULT_STEPS};
pub use sampler::{clamp01, LoopMode, SamplerParams};
pub use time::{ticks_per_sample, Ppq, Samples, Ticks};
