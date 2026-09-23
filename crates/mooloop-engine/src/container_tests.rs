//! Containers, through the whole engine.
//!
//! Step 02 built the noun and gave it no sound of its own; step 03 gives it a
//! dry path and a mix. The two are tested together because the second is only
//! meaningful against the first: **at 100% wet a container is exactly its own
//! devices**, and every other statement about the control is a statement
//! about how far it has moved from there.
//!
//! A test that only checked a container's *own* output would pass on a device
//! that quietly reordered or dropped its children. These render whole
//! projects and compare the master, sample for sample.

use crate::meters::DeviceMeters;
use crate::render::RenderState;
use crate::render_test_support::{peak_of, render_blocks, worst_difference, SAMPLE_RATE};
use mooloop_core::{
    ContainerParams, EffectKind, EffectParams, EffectSlotState, NoteEvent, Project, ProjectChannel,
};

/// One drum channel hitting on the downbeat, through Filter, Drive and Delay.
///
/// Three devices rather than one because the claim is about a *run*: a
/// container around a single device could be transparent for the wrong
/// reason. Drive is in there deliberately -- it is the only kind that
/// declares a latency, so a container that disturbed the compensation plan
/// would show up as a shift rather than as a difference in timbre.
fn three_device_chain() -> Project {
    let mut channel = ProjectChannel::drum_synth(0, 1);
    channel.setup.channel.volume = 1.0;
    channel.notes[0].push(NoteEvent::new(1, 0, 96, 36, 127));
    for kind in [EffectKind::Filter, EffectKind::Drive, EffectKind::Delay] {
        channel
            .setup
            .push_effect(EffectSlotState::of_kind(kind))
            .expect("room");
    }
    Project {
        channels: vec![channel],
        ..Project::default()
    }
}

/// Wrap `run` of channel 0's chain in a container at `mix`.
fn wrap(project: &mut Project, run: std::ops::Range<usize>, mix: f32) {
    wrap_as(project, EffectKind::Chain, run, mix);
}

/// Wrap `run` of channel 0's chain in a container of `kind` at `mix`.
fn wrap_as(project: &mut Project, kind: EffectKind, run: std::ops::Range<usize>, mix: f32) {
    let setup = &mut project.channels[0].setup;
    let mut container = EffectSlotState::of_kind(kind);
    container
        .params
        .set(mooloop_core::CONTAINER_PARAM_MIX, mix)
        .expect("a container has a mix");
    mooloop_core::wrap_in_container(&mut setup.effects, &mut setup.next_device_id, run, container)
        .expect("wrapped");
}

/// **Step 02's acceptance case, as step 03 leaves it.** A chain with
/// containers in it renders identically to the same chain without them — at
/// any depth and any block size, *at full wet*.
///
/// It was written to hold at any mix, because step 02's container had no mix
/// to speak of. Step 03 gives it one, so the claim narrows to the end of the
/// range where it still has to be exactly true: a wet/dry that did not reduce
/// to identity at 100% would make every other statement about the control a
/// statement about nothing.
#[test]
fn a_container_at_full_wet_is_transparent_at_every_depth() {
    let bare = three_device_chain();
    for block in [64, 128, 512] {
        let reference = render_blocks(&bare, 0.5, block);
        assert!(
            reference.iter().any(|s| s.abs() > 1.0e-4),
            "the reference render was silent, so this proves nothing"
        );

        // One box around the middle device.
        let mut one = bare.clone();
        wrap(&mut one, 1..2, 1.0);
        assert_eq!(
            render_blocks(&one, 0.5, block),
            reference,
            "a container changed the render at block {block}"
        );

        // A box inside a box, around the whole run.
        let mut nested = bare.clone();
        wrap(&mut nested, 1..3, 1.0);
        wrap(&mut nested, 0..4, 1.0);
        assert_eq!(
            render_blocks(&nested, 0.5, block),
            reference,
            "nested containers changed the render at block {block}"
        );
    }
}

/// **Bypassing a box bypasses the run**, and costs exactly the frames that
/// run declares.
///
/// Two claims, and the second is the one that is easy to lose. Skipping the
/// devices must be audible — otherwise the control does nothing — and it must
/// not move the channel in time, because the mixer's plan sums a chain's
/// declared latency including bypassed rows and a bypass that shortened the
/// path would leave every other channel over-compensated against this one.
/// The same argument `chain_latency` records for a bypassed device, one level
/// out.
#[test]
fn bypassing_a_container_skips_its_run_without_moving_it_in_time() {
    let bare = three_device_chain();
    let active = render_blocks(&bare, 0.5, 128);

    let mut boxed = bare.clone();
    wrap(&mut boxed, 0..3, 1.0);
    boxed.channels[0].setup.effects[0].bypassed = true;
    let bypassed = render_blocks(&boxed, 0.5, 128);

    let difference: f32 = active
        .iter()
        .zip(&bypassed)
        .map(|(a, b)| (a - b) * (a - b))
        .sum();
    let energy: f32 = active.iter().map(|s| s * s).sum();
    assert!(
        difference > energy * 1.0e-6,
        "bypassing the box left its run running"
    );

    // The declared latency of the chain is the same number either way, which
    // is what keeps the channel where it was.
    assert_eq!(
        mooloop_core::chain_latency(&boxed.channels[0].setup.effects),
        mooloop_core::chain_latency(&bare.channels[0].setup.effects),
    );
}

/// A container survives a save and a reload with its run intact, and the
/// project still renders the same.
#[test]
fn a_container_round_trips_through_the_document() {
    let temp = tempfile::tempdir().expect("a temp dir");
    let path = temp.path().join("boxed.mooloop");

    let mut boxed = three_device_chain();
    wrap(&mut boxed, 1..3, 0.4);
    wrap(&mut boxed, 0..4, 0.8);
    let before = render_blocks(&boxed, 0.5, 128);

    mooloop_project::save_song(&path, &boxed, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let mooloop_project::LoadedDocument::Song(reloaded) =
        mooloop_project::load_bundle(&path).expect("it reopens").document
    else {
        panic!("a song came back as something else");
    };

    let shape: Vec<(usize, EffectKind)> = (0..reloaded.channels[0].setup.effects.len())
        .map(|slot| {
            (
                mooloop_core::depth_at(&reloaded.channels[0].setup.effects, slot),
                reloaded.channels[0].setup.effects[slot].kind(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            (0, EffectKind::Chain),
            (1, EffectKind::Filter),
            (1, EffectKind::Chain),
            (2, EffectKind::Drive),
            (2, EffectKind::Delay),
        ],
        "the nesting did not survive the round trip"
    );
    assert_eq!(mooloop_core::span_problem(&reloaded.channels[0].setup.effects), None);
    assert_eq!(render_blocks(&reloaded, 0.5, 128), before);
}

/// One drum channel hitting on the downbeat, through `kinds` in order, the
/// rows named in `bypassed` switched out.
fn drum_through(kinds: &[EffectKind], bypassed: &[usize]) -> Project {
    let mut channel = ProjectChannel::drum_synth(0, 1);
    channel.setup.channel.volume = 0.5;
    channel.notes[0].push(NoteEvent::new(1, 0, 96, 36, 127));
    for kind in kinds {
        channel
            .setup
            .push_effect(EffectSlotState::of_kind(*kind))
            .expect("room");
    }
    for row in bypassed {
        channel.setup.effects[*row].bypassed = true;
    }
    Project {
        channels: vec![channel],
        ..Project::default()
    }
}

/// `a + b`, sample for sample: what two renders summed at the master would
/// be if the engine had summed them earlier.
fn summed(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

/// **`containers/08`'s first acceptance case.** A layer of one branch is a
/// chain: the same run, as the one branch of a `Layer` and as a `Chain` at
/// the same mix, renders sample for sample the same at three block sizes.
///
/// Twice over, because a branch is a run: once with the branch a lone Drive,
/// and once with it a whole chain of Filter → Drive → Delay. Drive is in both
/// for the reason step 02 gives -- it is the only kind that declares a
/// latency, so a layer that sized its dry path differently from a chain's
/// would comb at the partial mix rather than pass.
///
/// It replaces 07's `a_layer_running_in_series_is_a_chain`, which this step
/// makes untrue for any layer of more than one row.
#[test]
fn a_layer_of_one_branch_is_a_chain() {
    for mix in [1.0, 0.6] {
        // A lone Drive, boxed each way.
        let mut chain = drum_through(&[EffectKind::Drive], &[]);
        wrap_as(&mut chain, EffectKind::Chain, 0..1, mix);
        let mut layer = drum_through(&[EffectKind::Drive], &[]);
        wrap_as(&mut layer, EffectKind::Layer, 0..1, mix);
        // A three-device run: a layer holding one chain, against the chain.
        let mut long_chain = three_device_chain();
        wrap_as(&mut long_chain, EffectKind::Chain, 0..3, mix);
        let mut long_layer = three_device_chain();
        wrap_as(&mut long_layer, EffectKind::Chain, 0..3, 1.0);
        wrap_as(&mut long_layer, EffectKind::Layer, 0..4, mix);
        for block in [64, 128, 512] {
            for (what, chain, layer) in [
                ("a lone Drive", &chain, &layer),
                ("Filter, Drive and Delay", &long_chain, &long_layer),
            ] {
                let reference = render_blocks(chain, 0.5, block);
                assert!(
                    reference.iter().any(|s| s.abs() > 1.0e-4),
                    "the reference render was silent, so this proves nothing"
                );
                assert_eq!(
                    render_blocks(layer, 0.5, block),
                    reference,
                    "a layer of one branch ({what}) at mix {mix} was not its chain \
                     at block {block}"
                );
            }
        }
    }
}

/// **The second.** Two branches doing the same thing to the same input are
/// that thing *doubled* -- not "about 6 dB louder", but every sample exactly
/// twice what one branch gives. Branches sum at unity, and any other gain
/// would be a decision about level this step has no business making.
///
/// Exact because doubling is: a factor of two moves only the exponent, so
/// every linear stage downstream of the layer (the strip, the master) gives
/// exactly twice what it gave before -- for every normal float. The one
/// place it is not is the subnormal range, where a multiply has fewer bits
/// to round to; the assertion says how that is allowed for.
#[test]
fn two_identical_branches_are_exactly_six_db() {
    let mut one = drum_through(&[EffectKind::Filter], &[]);
    wrap_as(&mut one, EffectKind::Layer, 0..1, 1.0);
    let mut two = drum_through(&[EffectKind::Filter, EffectKind::Filter], &[]);
    wrap_as(&mut two, EffectKind::Layer, 0..2, 1.0);
    for block in [64, 512] {
        let single = render_blocks(&one, 0.5, block);
        assert!(
            single.iter().any(|s| s.abs() > 1.0e-4),
            "the reference render was silent, so this proves nothing"
        );
        let doubled: Vec<f32> = single.iter().map(|s| 2.0 * s).collect();
        let rendered = render_blocks(&two, 0.5, block);
        // Exact wherever the sample is a normal float. Down in the subnormal
        // range -- the filter's tail, around 1e-39 -- the strip's gain rounds
        // `2a` and `a` to fewer significant bits than each other, so the two
        // can differ in their last one. Measured: two samples of 24,000,
        // at 2.18e-39. Those are still required to be subnormal on both sides.
        let differing: Vec<usize> = (0..rendered.len())
            .filter(|i| {
                let (got, want) = (rendered[*i], doubled[*i]);
                if want.is_normal() || got.is_normal() {
                    got != want
                } else {
                    (got - want).abs() >= f32::MIN_POSITIVE
                }
            })
            .collect();
        assert!(
            differing.is_empty(),
            "two identical branches were not one branch doubled at block {block}: \
             {} samples differ, first at {:?} ({:?} against {:?}), last at {:?}",
            differing.len(),
            differing.first(),
            differing.first().map(|i| rendered[*i]),
            differing.first().map(|i| doubled[*i]),
            differing.last(),
        );
    }
}

/// **The most audible thing this step can get wrong.** A Drive in one branch
/// and nothing in the other -- a bypassed Filter, which passes its input and
/// declares nothing -- fed the same drum. Drive's oversampler runs 15 frames
/// late, so an unaligned sum is the dry drum against itself 15 frames apart:
/// a comb filter across the whole spectrum.
///
/// Asserted against the aligned reference, built from two renders the engine
/// already makes: the Drive alone, and the drum through a *bypassed* box
/// around that Drive, which is exactly the input delayed by the Drive's
/// latency. The layer must be their sum. The unaligned sum -- the Drive plus
/// the bare drum -- is checked to be far away, so the test can tell the two
/// apart.
#[test]
fn a_branch_that_declares_latency_does_not_comb_against_one_that_does_not() {
    let mut layered = drum_through(&[EffectKind::Drive, EffectKind::Filter], &[1]);
    wrap_as(&mut layered, EffectKind::Layer, 0..2, 1.0);
    assert_eq!(
        mooloop_core::chain_latency(&layered.channels[0].setup.effects),
        EffectKind::Drive.latency_frames(),
        "the layer declares its longest branch"
    );

    let driven = render_blocks(&drum_through(&[EffectKind::Drive], &[]), 0.5, 128);
    let mut late = drum_through(&[EffectKind::Drive], &[]);
    wrap(&mut late, 0..1, 1.0);
    late.channels[0].setup.effects[0].bypassed = true;
    let aligned = summed(&driven, &render_blocks(&late, 0.5, 128));
    let combed = summed(&driven, &render_blocks(&drum_through(&[], &[]), 0.5, 128));

    let rendered = render_blocks(&layered, 0.5, 128);
    let scale = peak_of(&aligned);
    assert!(scale > 1.0e-3, "the reference render was silent, so this proves nothing");
    assert!(
        worst_difference(&aligned, &combed) > scale * 1.0e-2,
        "aligned and combed are indistinguishable, so this proves nothing"
    );
    assert!(
        worst_difference(&rendered, &aligned) <= scale * 1.0e-6,
        "the layer is not its branches aligned and summed (worst {} against a peak of {scale})",
        worst_difference(&rendered, &aligned)
    );
}

/// **A bypassed layer is its input**, delayed by the latency the layer
/// declares -- its longest branch -- and so does not move the channel in time.
///
/// Written against the order that has bitten this plan once: step 03 shipped
/// a bypassed container whose generic bypass arm ran first and let its run
/// play. For a layer that failure is every branch still summing, and a layer
/// of a Drive and a pass-through would then come out as roughly twice the
/// input rather than the input.
#[test]
fn a_bypassed_layer_is_its_input() {
    let mut layered = drum_through(&[EffectKind::Drive, EffectKind::Filter], &[1]);
    wrap_as(&mut layered, EffectKind::Layer, 0..2, 1.0);
    layered.channels[0].setup.effects[0].bypassed = true;

    // The drum through a bypassed box around one Drive: the input, 15 frames
    // late.
    let mut late = drum_through(&[EffectKind::Drive], &[]);
    wrap(&mut late, 0..1, 1.0);
    late.channels[0].setup.effects[0].bypassed = true;

    for block in [64, 512] {
        let reference = render_blocks(&late, 0.5, block);
        assert!(
            reference.iter().any(|s| s.abs() > 1.0e-4),
            "the reference render was silent, so this proves nothing"
        );
        assert_eq!(
            render_blocks(&layered, 0.5, block),
            reference,
            "a bypassed layer was not its input at block {block}"
        );
    }
    assert_eq!(
        mooloop_core::chain_latency(&layered.channels[0].setup.effects),
        mooloop_core::chain_latency(&late.channels[0].setup.effects),
        "bypassing the layer moved the channel in time"
    );
}

/// A layer inside a chain inside a layer, down to `MAX_CONTAINER_DEPTH`:
///
/// ```text
/// Layer                 depth 0
///   Chain               depth 1
///     Layer             depth 2
///       Chain           depth 3
///         Drive
///       Filter (off)
///   Filter (off)
/// ```
///
/// Each layer is a Drive-carrying branch beside a pass-through, so the answer
/// is the Drive plus the input twice, both copies aligned to the Drive. The
/// branch buffers are indexed by *depth*, and two layers open at once is the
/// case that catches them being indexed by anything else: the inner layer
/// overwriting the outer one's input would lose one of the two copies.
#[test]
fn a_layer_inside_a_chain_inside_a_layer() {
    let mut nested = drum_through(
        &[EffectKind::Drive, EffectKind::Filter, EffectKind::Filter],
        &[1, 2],
    );
    wrap_as(&mut nested, EffectKind::Chain, 0..1, 1.0);
    wrap_as(&mut nested, EffectKind::Layer, 0..3, 1.0);
    wrap_as(&mut nested, EffectKind::Chain, 0..4, 1.0);
    wrap_as(&mut nested, EffectKind::Layer, 0..6, 1.0);
    let effects = &nested.channels[0].setup.effects;
    let shape: Vec<(usize, EffectKind)> = (0..effects.len())
        .map(|slot| (mooloop_core::depth_at(effects, slot), effects[slot].kind()))
        .collect();
    assert_eq!(
        shape,
        [
            (0, EffectKind::Layer),
            (1, EffectKind::Chain),
            (2, EffectKind::Layer),
            (3, EffectKind::Chain),
            (4, EffectKind::Drive),
            (3, EffectKind::Filter),
            (1, EffectKind::Filter),
        ],
        "the nesting is not the one this test is about"
    );
    assert_eq!(
        mooloop_core::depth_at(effects, 4),
        mooloop_core::MAX_CONTAINER_DEPTH,
        "the Drive is not at the depth cap, so the cap is not exercised"
    );

    let driven = render_blocks(&drum_through(&[EffectKind::Drive], &[]), 0.5, 128);
    let mut late = drum_through(&[EffectKind::Drive], &[]);
    wrap(&mut late, 0..1, 1.0);
    late.channels[0].setup.effects[0].bypassed = true;
    let late = render_blocks(&late, 0.5, 128);
    let expected = summed(&summed(&driven, &late), &late);

    let rendered = render_blocks(&nested, 0.5, 128);
    let scale = peak_of(&expected);
    assert!(scale > 1.0e-3, "the reference render was silent, so this proves nothing");
    assert!(
        worst_difference(&rendered, &expected) <= scale * 1.0e-6,
        "the nested layers are not the Drive plus the input twice (worst {} against {scale})",
        worst_difference(&rendered, &expected)
    );
}

/// **A layer allocates nothing on the callback**, measured with the
/// `CountingAllocator` rather than reasoned: the nested layers above,
/// rendered block after block, and a branch's ring swapped in through the
/// structural path the control thread uses.
///
/// The branch buffers and every ring are allocated when the project is
/// built, which is the control thread's work; what is measured is the block.
#[test]
fn a_layer_allocates_nothing_on_the_callback() {
    let mut nested = drum_through(
        &[EffectKind::Drive, EffectKind::Filter, EffectKind::Filter],
        &[1, 2],
    );
    wrap_as(&mut nested, EffectKind::Chain, 0..1, 1.0);
    wrap_as(&mut nested, EffectKind::Layer, 0..3, 0.7);
    wrap_as(&mut nested, EffectKind::Chain, 0..4, 1.0);
    wrap_as(&mut nested, EffectKind::Layer, 0..6, 0.5);

    let mut render = RenderState::from_project(SAMPLE_RATE, &nested, &[]);
    render.play();
    // Warm: first touches of anything lazily initialised are not the layer's.
    for _ in 0..4 {
        render.process_once_block(256);
    }
    // Built here, on what stands in for the control thread.
    let ring = crate::StructuralCommand::SetBranchAlign {
        target: mooloop_core::EffectTarget::Channel(0),
        slot: 6,
        align: mooloop_dsp::IntegerDelay::new(7).map(Box::new),
    };

    let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    for _ in 0..16 {
        render.process_once_block(256);
    }
    let displaced = render.apply_structural(ring);
    for _ in 0..4 {
        render.process_once_block(256);
    }
    let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    drop(displaced);

    assert_eq!(
        after, before,
        "a layer allocated or freed on the thread that would be the callback"
    );
}

/// A layer survives a save and a reload as a layer, inside a chain, with its
/// run intact -- `layer` is a new `type` tag and the document has to carry it
/// both ways -- and the project still renders the same.
#[test]
fn a_layer_round_trips_through_the_document() {
    let temp = tempfile::tempdir().expect("a temp dir");
    let path = temp.path().join("layered.mooloop");

    let mut layered = three_device_chain();
    wrap_as(&mut layered, EffectKind::Layer, 1..3, 0.4);
    wrap_as(&mut layered, EffectKind::Chain, 0..4, 0.8);
    let before = render_blocks(&layered, 0.5, 128);

    mooloop_project::save_song(&path, &layered, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let mooloop_project::LoadedDocument::Song(reloaded) =
        mooloop_project::load_bundle(&path).expect("it reopens").document
    else {
        panic!("a song came back as something else");
    };

    let effects = &reloaded.channels[0].setup.effects;
    let shape: Vec<(usize, EffectKind)> = (0..effects.len())
        .map(|slot| (mooloop_core::depth_at(effects, slot), effects[slot].kind()))
        .collect();
    assert_eq!(
        shape,
        [
            (0, EffectKind::Chain),
            (1, EffectKind::Filter),
            (1, EffectKind::Layer),
            (2, EffectKind::Drive),
            (2, EffectKind::Delay),
        ],
        "the layer did not survive the round trip as a layer"
    );
    assert_eq!(
        effects[2].params,
        layered.channels[0].setup.effects[2].params,
        "the layer's mix and span came back different"
    );
    assert_eq!(mooloop_core::span_problem(effects), None);
    assert_eq!(render_blocks(&reloaded, 0.5, 128), before);
}

/// **The step 03 acceptance case, first half.** A container at 100% wet is
/// the same audio as its devices with no box around them.
///
/// This is the one that had to be built before the mix existed, and it is
/// what makes every claim below meaningful: a wet/dry that did not reduce to
/// identity at full wet would make "50% is halfway between" a statement about
/// nothing in particular.
#[test]
fn a_container_at_full_wet_is_its_devices() {
    let bare = three_device_chain();
    for block in [64, 128, 512] {
        let reference = render_blocks(&bare, 0.5, block);
        let mut boxed = bare.clone();
        wrap(&mut boxed, 0..3, 1.0);
        assert_eq!(
            render_blocks(&boxed, 0.5, block),
            reference,
            "a container at full wet was not its own run at block {block}"
        );
    }
}

/// **The step 03 acceptance case, second half.** A container at 0% mix is the
/// signal that went into it, delayed by exactly what its run declares.
///
/// The delay is the point. The run holds a Drive, which is the only kind that
/// declares a latency, so a dry path that was *not* aligned would arrive
/// early and the two would comb rather than cancel. Compared against the same
/// project with the run bypassed, which the compensation work already made
/// time transparent — so the two agree only if the container's dry path is
/// aligned the same way a bypassed device's is.
#[test]
fn a_container_at_zero_mix_is_its_input_delayed_by_its_run() {
    let bare = three_device_chain();

    let mut dry = bare.clone();
    wrap(&mut dry, 0..3, 0.0);

    let mut bypassed = bare.clone();
    wrap(&mut bypassed, 0..3, 1.0);
    bypassed.channels[0].setup.effects[0].bypassed = true;

    for block in [64, 128, 512] {
        assert_eq!(
            render_blocks(&dry, 0.5, block),
            render_blocks(&bypassed, 0.5, block),
            "a container at zero mix did not equal its own run bypassed, at block {block}"
        );
    }
}

/// The mix does something, and it does it monotonically. Not a claim about a
/// particular curve — the crossfade is equal-power and
/// `docs/GAIN_STRUCTURE.md` records why — only that the control is connected
/// and points the right way.
#[test]
fn the_mix_moves_the_sound_between_the_two_ends() {
    let bare = three_device_chain();

    let energy = |mix: f32| {
        let mut boxed = bare.clone();
        wrap(&mut boxed, 0..3, mix);
        render_blocks(&boxed, 0.5, 128)
            .iter()
            .map(|s| s * s)
            .sum::<f32>()
    };

    let wet = energy(1.0);
    let half = energy(0.5);
    let dry = energy(0.0);
    assert!(wet > 0.0 && dry > 0.0, "both ends were silent");
    assert!(
        (half - wet).abs() > wet * 1.0e-4 && (half - dry).abs() > dry * 1.0e-4,
        "50% landed on one of the ends: dry {dry}, half {half}, wet {wet}"
    );
}

/// A container's mix is a blend across its *run*, which is the thing that has
/// no equivalent today: a Drive and a Delay blended together, rather than
/// each blended against its own input in turn.
///
/// The wet-FX gap the brief opens with, stated as a difference rather than as
/// an absence: wrapping two devices at 50% is not the same sound as setting
/// each of them to 50%, and it is the first one a musician means.
#[test]
fn a_run_blended_once_is_not_two_devices_blended_in_turn() {
    let mut per_device = three_device_chain();
    for slot in 1..3 {
        per_device.channels[0].setup.effects[slot].wet_dry = 0.5;
    }

    let mut per_run = three_device_chain();
    wrap(&mut per_run, 1..3, 0.5);

    let a = render_blocks(&per_device, 0.5, 128);
    let b = render_blocks(&per_run, 0.5, 128);
    let difference: f32 = a.iter().zip(&b).map(|(a, b)| (a - b) * (a - b)).sum();
    let energy: f32 = a.iter().map(|s| s * s).sum();
    assert!(
        difference > energy * 1.0e-6,
        "blending the run and blending each device came out the same, \
         so the container is not doing what it is for"
    );
}

/// Nesting composes: an inner box blends its run, and the outer box blends
/// *that* against what went into the outer box.
#[test]
fn a_nested_container_blends_what_the_inner_one_produced() {
    let bare = three_device_chain();

    // Inner box at full wet is a no-op, so the outer box sees the same run it
    // would have seen without it -- which makes this equal to the outer box
    // alone. Anything else means the inner blend leaked.
    let mut nested = bare.clone();
    wrap(&mut nested, 1..3, 1.0);
    wrap(&mut nested, 0..4, 0.5);

    let mut flat = bare.clone();
    wrap(&mut flat, 0..3, 0.5);

    assert_eq!(render_blocks(&nested, 0.5, 128), render_blocks(&flat, 0.5, 128));
}

/// A project written before containers existed has no `children` and no
/// `mix`, and a container written by a reader that has both must decode with
/// the defaults rather than failing the load -- `PROJECT_FORMAT.md`'s
/// defaulted-field rule, checked at the one place a new params variant could
/// break it.
#[test]
fn a_container_decodes_from_a_manifest_that_names_neither_field() {
    let params: EffectParams =
        toml::from_str("type = \"chain\"\n[state]\n").expect("an empty state decodes");
    assert_eq!(params, EffectParams::Chain(ContainerParams::default()));
    assert_eq!(params.kind(), EffectKind::Chain);
}

/// **A container's OUT reads the end of its run, not its input.**
///
/// `DeviceOutputRail` is drawn past everything the box holds, precisely so it
/// reads as the box's output -- and until 2026-09-13 the engine published the
/// peak taken *going in* to both of the container's meter cells and then
/// `continue`d, so a run that boosted 12 dB or blended half its dry back read
/// as no change at all.
///
/// The cells are `fetch_max` peak holds, which is what makes it worth a test
/// rather than a glance: writing the input into the OUT cell does not merely
/// report the wrong figure, it *survives* a correct later write for any run
/// that attenuates. So the assertion here is the attenuating direction.
#[test]
fn a_containers_out_meter_reads_the_end_of_its_run() {
    let mut project = three_device_chain();
    // Take most of the level out inside the box, so its two cells cannot be
    // mistaken for each other.
    project.channels[0].setup.effects[0].params =
        EffectParams::Filter(mooloop_core::FilterParams {
            cutoff_hz: 100.0,
            resonance: 0.0,
            mode: mooloop_core::FilterMode::LowPass,
            ..mooloop_core::FilterParams::default()
        });
    // The filter alone, not the whole chain: Drive is in `three_device_chain`
    // and it *boosts*, so a box around all three reads hotter coming out than
    // going in -- which is a true reading and the wrong one to assert on. The
    // attenuating direction is the one the `fetch_max` hold would hide.
    wrap(&mut project, 0..1, 1.0);

    let meters = DeviceMeters::new();
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    render.attach_device_meters(meters.clone());
    render.play();
    render.process_block(1024);

    // Stage `slot + 1`, and the container took slot 0 when `wrap` inserted it.
    let (container_in, container_out) = meters.take(0, 1);
    assert!(
        container_in.0 > 0.001,
        "the box has to be hearing something for this to mean anything, got {container_in:?}"
    );
    // Strictly less, with no margin, because the margin is not what is being
    // tested: the unfixed engine wrote *the same peak* into both cells, so
    // the two were bit-identical. Any difference in the attenuating direction
    // is the whole result, and a threshold would only make the test fragile
    // about how much a 100 Hz lowpass takes off one drum hit.
    assert!(
        container_out.0 < container_in.0,
        "the OUT cell must read past the run's filter, and the unfixed engine \
         made these equal: in {container_in:?}, out {container_out:?}"
    );
}
