//! Core data model and message types shared between the engine and the UI.
//!
//! This crate has no dependency on audio or UI code so it can be linked freely
//! from both realtime and GUI threads.

pub mod bridge;
pub mod time;

pub use bridge::{EngineCommand, EngineEvent};
pub use time::{ticks_per_sample, Ppq, Samples, Ticks};
