//! Chainable effect nodes. Effects implement `AudioNode` like instruments
//! do, but read and modify the bus in place (see `node.rs`'s processing
//! model) and stay ignorant of channel-strip concepts (gain/pan/mute) so the
//! same node can later run on a master or send bus without changes.
//!
//! Every effect takes its parameters as sample-timed `Event::ParamValue`
//! values in **natural units** (Hz, bits, linear gain). The non-realtime side
//! converts from normalized knob positions through the descriptor tables in
//! `mooloop_core::effect`; nodes never see a curve. See
//! `docs/MODULATION_PLAN.md` for why the split falls there.

mod bitcrush;
mod delay;
mod drive;
mod dynamics;
mod filter;

pub use bitcrush::BitcrushEffect;
pub use delay::DelayEffect;
pub use drive::DriveEffect;
pub use dynamics::{CompressorEffect, GateEffect, LimiterEffect};
pub use filter::FilterEffect;

use mooloop_core::EffectParams;

use crate::node::AudioNode;

/// Construct the DSP node for a parameter set.
///
/// This allocates, so it belongs on the non-realtime side: the engine calls
/// it at project load and the GUI calls it when the user adds an effect,
/// shipping the box through the ordered control stream. Keeping one
/// constructor means a new effect kind is wired in exactly once.
pub fn build_effect(params: EffectParams, sample_rate: u32) -> Box<dyn AudioNode + Send> {
    match params {
        EffectParams::Filter(p) => Box::new(FilterEffect::new(p, sample_rate)),
        EffectParams::Drive(p) => Box::new(DriveEffect::new(p, sample_rate)),
        EffectParams::Bitcrush(p) => Box::new(BitcrushEffect::new(p)),
        EffectParams::Delay(p) => Box::new(DelayEffect::new(p, sample_rate)),
        EffectParams::Gate(p) => Box::new(GateEffect::new(p, sample_rate)),
        EffectParams::Compressor(p) => Box::new(CompressorEffect::new(p, sample_rate)),
        EffectParams::Limiter(p) => Box::new(LimiterEffect::new(p, sample_rate)),
    }
}
