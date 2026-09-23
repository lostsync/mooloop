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
use crate::render_test_support::{render_blocks, SAMPLE_RATE};
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

/// **`containers/07`'s acceptance case.** A layer lands running its rows in
/// series, exactly as a chain does, so it has to *be* a chain: the same run,
/// wrapped in a `Layer` and in a `Chain` at the same mix, renders sample for
/// sample the same at three block sizes.
///
/// Drive is in the run for the reason step 02 gives -- it is the only kind
/// that declares a latency, so a layer that sized its dry path differently
/// would comb at the partial mix rather than pass. That is also why the mix
/// is not only 1.0: at full wet the dry path is never heard.
///
/// This is the thing `containers/08`'s "a layer of one branch is a chain" is
/// identical *to*. When 08 teaches the engine to split, a layer of three
/// leaves stops being serial and this case gives way to that one.
#[test]
fn a_layer_running_in_series_is_a_chain() {
    let bare = three_device_chain();
    for mix in [1.0, 0.6] {
        let mut chain = bare.clone();
        wrap_as(&mut chain, EffectKind::Chain, 0..3, mix);
        let mut layer = bare.clone();
        wrap_as(&mut layer, EffectKind::Layer, 0..3, mix);
        for block in [64, 128, 512] {
            let reference = render_blocks(&chain, 0.5, block);
            assert!(
                reference.iter().any(|s| s.abs() > 1.0e-4),
                "the reference render was silent, so this proves nothing"
            );
            assert_eq!(
                render_blocks(&layer, 0.5, block),
                reference,
                "a layer at mix {mix} was not its chain at block {block}"
            );
        }
    }
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
