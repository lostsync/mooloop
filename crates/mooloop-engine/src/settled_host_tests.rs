//! A settled insert takes its block whole, and sounds exactly as the
//! per-sample blend did (MOO-260).
//!
//! MOO-108 ramps every host control of an insert -- input trim, wet/dry,
//! output trim and the bypass crossfade -- per sample, and ran that blend on
//! every slot of every block whether or not anything was moving. MOO-260
//! sends a settled slot down a cheaper path (`LeafPath::Unity` at full wet
//! and unity trims, `LeafPath::Still` otherwise). These tests hold that path
//! to the per-sample one **in the same process**: each case renders twice,
//! once as it ships and once with `force_host_reference` sending every slot
//! down the per-sample path, and compares every sample.
//!
//! Not against stored hashes. A hash taken on one machine is a claim about
//! that machine's floating point, and one pinned on x86 broke macOS CI on
//! 2026-09-25; a comparison made in one process is a claim about the code.

use mooloop_core::{
    DelayParams, DriveParams, EffectParams, EffectSlotState, EffectTarget, EngineCommand,
    FilterMode, FilterParams, MonoSynthParams, NoteEvent, Project, ProjectChannel, DEFAULT_STEPS,
};

use crate::render::{force_host_reference, RenderState};
use crate::render_test_support::SAMPLE_RATE;

const FX: EffectTarget = EffectTarget::Channel(0);

/// A saw held for a bar, through four inserts that between them cover every
/// way a settled slot can stand:
///
/// 0. a low-pass at full wet and unity trims (`Unity`);
/// 1. a Drive, which declares latency and so has a dry ring, at 60% wet
///    with its input trimmed (`Still`, with the ring);
/// 2. a band-pass at full wet with its output trimmed (`Still`);
/// 3. a delay at full wet and unity trims (`Unity`, with a tail).
fn project() -> Project {
    let params = MonoSynthParams {
        attack: 0.005,
        sustain: 1.0,
        ..MonoSynthParams::default()
    };
    let mut channel = ProjectChannel::mono_synth_with_params(0, 1, params);
    channel.setup.channel.volume = 1.0;
    channel.notes[0].push(NoteEvent::new(1, 0, 384, 45, 127));
    let filter = |mode, cutoff_hz| {
        EffectParams::Filter(FilterParams {
            cutoff_hz,
            resonance: 0.3,
            mode,
            ..FilterParams::default()
        })
    };
    let mut low = EffectSlotState::new(filter(FilterMode::LowPass, 900.0));
    low.wet_dry = 1.0;
    let mut drive = EffectSlotState::new(EffectParams::Drive(DriveParams::default()));
    drive.wet_dry = 0.6;
    drive.input_trim = 0.8;
    let mut band = EffectSlotState::new(filter(FilterMode::BandPass, 1_200.0));
    band.output_trim = 1.4;
    let delay = EffectSlotState::new(EffectParams::Delay(DelayParams {
        time_ms: 90.0,
        ..DelayParams::default()
    }));
    for effect in [low, drive, band, delay] {
        channel.setup.push_effect(effect).expect("room");
    }
    Project {
        channels: vec![channel],
        pattern_lengths: vec![DEFAULT_STEPS],
        ..Project::default()
    }
}

/// One step of a script: render `frames`, then apply `commands`.
type Step = (usize, Vec<EngineCommand>);

/// Render the script, as shipped or on the per-sample reference, and return
/// the master with the output guard's limiter off, so it is the mix itself.
fn render(script: &[Step], reference: bool) -> (Vec<f32>, Vec<f32>) {
    force_host_reference(reference);
    let mut render = RenderState::from_project(SAMPLE_RATE, &project(), &[]);
    render.unlimit_output();
    render.play();
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for (frames, commands) in script {
        let mut remaining = *frames;
        // Odd block sizes, so ramps start and finish inside blocks as often
        // as on their edges.
        let mut sizes = [128usize, 61, 256, 7, 190, 1, 333].into_iter().cycle();
        while remaining > 0 {
            let block = sizes.next().expect("cycles").min(remaining);
            render.process_once_block(block);
            left.extend_from_slice(&render.master().l[..block]);
            right.extend_from_slice(&render.master().r[..block]);
            remaining -= block;
        }
        for command in commands {
            render.apply_command(*command);
        }
    }
    force_host_reference(false);
    (left, right)
}

/// The two renders agree sample for sample. Compared as `f32`s, so `-0.0`
/// equals `0.0` (the one difference `LeafPath::Unity` documents), and bit
/// for bit everywhere else.
fn assert_same(what: &str, script: &[Step]) {
    let shipped = render(script, false);
    let reference = render(script, true);
    let peak = reference.0.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    assert!(
        peak > 0.05,
        "{what}: the material has to be sounding, peak {peak}"
    );
    for (side, (a, b)) in [
        ("left", (&shipped.0, &reference.0)),
        ("right", (&shipped.1, &reference.1)),
    ] {
        assert_eq!(a.len(), b.len());
        for (frame, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            let same = if *x == 0.0 {
                *y == 0.0
            } else {
                x.to_bits() == y.to_bits()
            };
            assert!(
                same,
                "{what}: {side} frame {frame}: {x:e} as shipped, {y:e} per sample"
            );
        }
    }
}

fn bypass(slot: u8, bypassed: bool) -> EngineCommand {
    EngineCommand::SetEffectBypassed {
        target: FX,
        slot,
        bypassed,
    }
}

fn wet(slot: u8, wet_dry: f32) -> EngineCommand {
    EngineCommand::SetEffectWetDry {
        target: FX,
        slot,
        wet_dry,
    }
}

fn input_trim(slot: u8, input_trim: f32) -> EngineCommand {
    EngineCommand::SetEffectInputTrim {
        target: FX,
        slot,
        input_trim,
    }
}

fn output_trim(slot: u8, output_trim: f32) -> EngineCommand {
    EngineCommand::SetEffectOutputTrim {
        target: FX,
        slot,
        output_trim,
    }
}

#[test]
fn a_settled_chain_sounds_as_the_per_sample_blend_did() {
    assert_same("a still chain", &[(24_000, vec![])]);
}

#[test]
fn toggling_bypass_and_wet_mid_block_sounds_as_the_per_sample_blend_did() {
    // Every change lands between two odd-sized blocks, so the ramps it
    // starts end inside a later block; the slot leaves the settled path on
    // the block the change lands on and rejoins it the block after its
    // ramps arrive.
    assert_same(
        "bypass and wet moved",
        &[
            (6_001, vec![bypass(0, true)]),
            // Back before the fade has landed: out of the settled path and
            // back into it without ever reaching out-of-path.
            (97, vec![bypass(0, false)]),
            (4_003, vec![wet(3, 0.5), wet(1, 1.0)]),
            (2_111, vec![wet(3, 1.0), bypass(2, true)]),
            // Past the whole bypass fade, then back.
            (9_000, vec![bypass(2, false), wet(1, 0.0)]),
            (5_555, vec![bypass(1, true), wet(0, 0.25)]),
            (6_000, vec![]),
        ],
    );
}

#[test]
fn moving_the_trims_sounds_as_the_per_sample_blend_did() {
    assert_same(
        "trims moved",
        &[
            (5_003, vec![input_trim(0, 0.5), output_trim(3, 1.7)]),
            (3_001, vec![input_trim(0, 1.0), output_trim(2, 1.0)]),
            // Two moves one block apart, the second while the first ramps.
            (129, vec![input_trim(1, 1.0)]),
            (61, vec![input_trim(1, 0.3), output_trim(3, 1.0)]),
            (8_000, vec![]),
        ],
    );
}
