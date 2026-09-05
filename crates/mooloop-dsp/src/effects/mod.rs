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
mod eq;
mod filter;
mod modulation;
mod plate;
mod reverb;

pub use bitcrush::BitcrushEffect;
pub use delay::DelayEffect;
pub use drive::DriveEffect;
pub use dynamics::{CompressorEffect, GateEffect, LimiterEffect};
pub use eq::EqEffect;
pub use filter::FilterEffect;
pub use modulation::ModulationEffect;
pub use plate::PlateEffect;
pub use reverb::ReverbEffect;

use mooloop_core::EffectParams;

use crate::node::AudioNode;

/// Construct the DSP node for a parameter set.
///
/// This allocates, so it belongs on the non-realtime side: the engine calls
/// it at project load and the GUI calls it when the user adds an effect,
/// shipping the box through the ordered control stream. Keeping one
/// constructor means a new effect kind is wired in exactly once.
pub fn build_effect(params: EffectParams, sample_rate: u32) -> Box<dyn AudioNode + Send> {
    build_effect_at_tempo(params, sample_rate, 120.0)
}

/// Construct an effect node using the current transport tempo for devices
/// whose preallocated state is beat-relative. This remains a control-plane
/// operation: callers must never invoke it from the audio callback.
pub fn build_effect_at_tempo(
    params: EffectParams,
    sample_rate: u32,
    bpm: f64,
) -> Box<dyn AudioNode + Send> {
    match params {
        EffectParams::Eq(p) => Box::new(EqEffect::new(p, sample_rate)),
        EffectParams::Modulation(p) => Box::new(ModulationEffect::new(p, sample_rate)),
        EffectParams::Filter(p) => Box::new(FilterEffect::new(p, sample_rate)),
        EffectParams::Drive(p) => Box::new(DriveEffect::new(p, sample_rate)),
        EffectParams::Bitcrush(p) => Box::new(BitcrushEffect::new(p)),
        EffectParams::Delay(mut p) => {
            if p.tempo_sync {
                p.time_ms = p.time_division.time_ms(bpm);
            }
            Box::new(DelayEffect::new(p, sample_rate))
        }
        EffectParams::Reverb(p) => Box::new(ReverbEffect::new(p, sample_rate)),
        EffectParams::Plate(p) => Box::new(PlateEffect::new(p, sample_rate)),
        EffectParams::Gate(p) => Box::new(GateEffect::new(p, sample_rate)),
        EffectParams::Compressor(p) => Box::new(CompressorEffect::new(p, sample_rate)),
        EffectParams::Limiter(p) => Box::new(LimiterEffect::new(p, sample_rate)),
        EffectParams::Buffer(p) => Box::new(crate::BufferDevice::new(p, sample_rate, bpm)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::EffectKind;

    /// A device's latency is written down twice: as a declaration in
    /// `mooloop-core`, which the control thread reads to size a compensation
    /// delay before any node exists, and on the running node itself. Two
    /// numbers for one fact is how they come to disagree, and a disagreement
    /// here is silent — the delay would be built to the wrong length and the
    /// misalignment it was meant to remove would move instead.
    ///
    /// This is the one crate that can see both, so this is where they are held
    /// together. Two sample rates, because a device whose latency depended on
    /// the rate could not be declared statically at all and this is where that
    /// would be found out.
    #[test]
    fn every_effect_kinds_declared_latency_matches_its_node() {
        for kind in EffectKind::ALL {
            for sample_rate in [44_100, 48_000] {
                let node = build_effect_at_tempo(kind.default_params(), sample_rate, 120.0);
                assert_eq!(
                    node.latency_frames(),
                    kind.latency_frames(),
                    "{kind:?} at {sample_rate} Hz: node reports {}, core declares {}",
                    node.latency_frames(),
                    kind.latency_frames()
                );
                // The container's bypass path delays the signal through the
                // ring it sized from the *dry path* length, while the latency
                // plan sums the declared `latency_frames`. They are the same
                // number for every device today, and a device that broke that
                // would make a bypassed slot cost a different number of frames
                // than the plan compensated for. Asserted rather than assumed,
                // because the symptom would be a misalignment nothing reports.
                assert_eq!(
                    node.dry_path_latency_frames(),
                    node.latency_frames(),
                    "{kind:?} declares a dry-path latency apart from its own; \
                     the bypass path and the latency plan would disagree"
                );
            }
        }
    }
}
