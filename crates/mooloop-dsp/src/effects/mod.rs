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
    use crate::bus::StereoBus;
    use crate::event::EventList;
    use crate::node::{ProcessContext, SILENCE_PEAK};
    use mooloop_core::{BitcrushParams, BitcrushStyle, EffectKind, EffectParams};

    const SAMPLE_RATE: u32 = 48_000;
    const BLOCK: usize = 512;

    fn context(frames: usize) -> ProcessContext {
        ProcessContext {
            sample_rate: SAMPLE_RATE,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// Deterministic broadband noise at -6 dBFS. Broadband because a tone
    /// leaves most of an EQ's or a reverb's structure untouched, and loud
    /// because a gate has to open before its release means anything.
    fn fill_burst(bus: &mut StereoBus, frames: usize, state: &mut u32) {
        for index in 0..frames {
            let mut next = || {
                *state ^= *state << 13;
                *state ^= *state >> 17;
                *state ^= *state << 5;
                ((*state >> 8) as f32 / 8_388_608.0 - 1.0) * 0.5
            };
            bus.l[index] = next();
            bus.r[index] = next();
        }
    }

    /// The host's own decision, written once so the test and `EffectChain`
    /// cannot drift: a slot may be skipped when its input is silent and
    /// either the node's state has settled or it has been fed silence for
    /// longer than the tail it declared.
    fn may_skip(node: &dyn AudioNode, silent_frames: u32) -> bool {
        silent_frames > 0 && (node.is_at_rest() || silent_frames > node.tail_frames())
    }

    /// The contract step 02 rests on, checked against each device rather than
    /// derived from it: **once a node says it can be left alone, leaving it
    /// alone is inaudible.** A device that under-reports here is a chopped
    /// reverb tail, which is the one way this whole mechanism can be heard.
    ///
    /// Ten seconds is the cap. It is not an assertion about any device's
    /// tail -- a delay at full feedback honestly reports minutes -- only a
    /// bound on how long this test will run before it accepts that a device
    /// with default settings has decided to stay awake.
    #[test]
    fn every_effect_kind_is_silent_once_it_says_it_can_be_skipped() {
        const CAP_BLOCKS: usize = 10 * SAMPLE_RATE as usize / BLOCK;
        // The one device that makes sound out of a silent input by design,
        // and so declines the whole mechanism. Named here so removing the
        // decision from `buffer_device.rs` fails this test rather than
        // quietly starting to skip a playing buffer.
        let never_sleeps = [EffectKind::Buffer];

        for kind in EffectKind::ALL {
            let mut node = build_effect_at_tempo(kind.default_params(), SAMPLE_RATE, 120.0);
            let mut bus = StereoBus::with_capacity(BLOCK);
            let events = EventList::empty();
            let mut state = 0x1234_5678;

            // Half a second of noise, so every ring in every device is
            // carrying something when the input stops.
            for _ in 0..(SAMPLE_RATE as usize / 2 / BLOCK) {
                fill_burst(&mut bus, BLOCK, &mut state);
                node.process(&context(BLOCK), &mut bus, &events, None);
            }

            let mut silent_frames = 0u32;
            let mut slept_at = None;
            for block in 0..CAP_BLOCKS {
                if may_skip(node.as_ref(), silent_frames) {
                    slept_at = Some(block);
                    break;
                }
                bus.clear(BLOCK);
                node.process(&context(BLOCK), &mut bus, &events, None);
                silent_frames = silent_frames.saturating_add(BLOCK as u32);
            }

            let Some(slept_at) = slept_at else {
                assert!(
                    never_sleeps.contains(&kind),
                    "{kind:?} never reported itself skippable in ten seconds of silence,                      and is not one of the devices that deliberately never does"
                );
                continue;
            };
            assert!(
                !never_sleeps.contains(&kind),
                "{kind:?} is declared as a device that never sleeps, but reported                  itself skippable after {slept_at} blocks"
            );

            // Keep processing past the point the host would have stopped. If
            // anything was still coming, this is where it shows up.
            for block in 0..64 {
                bus.clear(BLOCK);
                node.process(&context(BLOCK), &mut bus, &events, None);
                let (left, right) = bus.peak(BLOCK);
                assert!(
                    left.max(right) <= SILENCE_PEAK,
                    "{kind:?} said it could be skipped after {slept_at} blocks of                      silence, then produced {} on block {block} after that",
                    left.max(right)
                );
            }
        }
    }

    /// The exception the crusher's `is_at_rest` is written around, held here
    /// so it cannot be simplified away. TPDF dither spans a whole quantiser
    /// step, so it lands on `+/-step` about half the time whatever the input
    /// was; at four bits that is -18 dBFS of hiss on a channel that has
    /// stopped playing, and it is the device's noise floor rather than a tail.
    #[test]
    fn a_dithering_crusher_never_reports_rest_while_its_wet_is_heard() {
        let params = BitcrushParams {
            bits: 4.0,
            style: BitcrushStyle::Dither,
            mix: 1.0,
            ..BitcrushParams::default()
        };
        let mut node = build_effect(EffectParams::Bitcrush(params), SAMPLE_RATE);
        let mut bus = StereoBus::with_capacity(BLOCK);
        let events = EventList::empty();

        let mut loudest = 0.0f32;
        for _ in 0..16 {
            bus.clear(BLOCK);
            node.process(&context(BLOCK), &mut bus, &events, None);
            let (left, right) = bus.peak(BLOCK);
            loudest = loudest.max(left.max(right));
            assert!(
                !may_skip(node.as_ref(), BLOCK as u32),
                "a dithering crusher reported itself skippable"
            );
        }
        assert!(
            loudest > SILENCE_PEAK,
            "the premise of this test is that dither makes noise out of silence;              it produced at most {loudest}"
        );
    }

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
