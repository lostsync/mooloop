//! Containers, through the whole engine.
//!
//! `docs/plans/containers/02-the-container-is-a-device.md` asks for one thing
//! and it is a negative: **a container that is not yet doing anything must not
//! be doing anything.** Step 02 builds the noun -- a device that holds a run
//! of devices, saves, loads, bypasses and takes an identity -- and gives it no
//! sound of its own, so that step 03's mix has a null to be measured against.
//!
//! A test that only checked the container's *own* output would pass on a
//! device that quietly reordered or dropped its children. These render whole
//! projects and compare the master, sample for sample, against the same
//! devices with no boxes around them at all.

use crate::render::RenderState;
use mooloop_core::{
    ChainParams, EffectKind, EffectParams, EffectSlotState, NoteEvent, Project, ProjectChannel,
};

const SAMPLE_RATE: u32 = 48_000;

/// Render `seconds` of a project in fixed `block` frames; master left.
fn render_blocks(project: &Project, seconds: f32, block: usize) -> Vec<f32> {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut out = Vec::new();
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    while remaining > 0 {
        let frames = remaining.min(block);
        render.process_once_block(frames);
        out.extend_from_slice(&render.master().l[..frames]);
        remaining -= frames;
    }
    out
}

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
    let setup = &mut project.channels[0].setup;
    let mut container = EffectSlotState::of_kind(EffectKind::Chain);
    if let EffectParams::Chain(chain) = &mut container.params {
        chain.mix = mix;
    }
    mooloop_core::wrap_in_container(&mut setup.effects, &mut setup.next_device_id, run, container)
        .expect("wrapped");
}

/// **The step 02 acceptance case.** A chain with containers in it renders
/// identically to the same chain with the containers removed -- at any depth,
/// at any mix, and at any block size.
///
/// The mix is varied on purpose even though step 02 does not read it: a
/// container whose mix already did something would make step 03's "at 100%
/// wet it nulls" test unfalsifiable, because there would be nothing it could
/// null *against*.
#[test]
fn a_container_that_is_not_doing_anything_is_not_doing_anything() {
    let bare = three_device_chain();
    for block in [64, 128, 512] {
        let reference = render_blocks(&bare, 0.5, block);
        assert!(
            reference.iter().any(|s| s.abs() > 1.0e-4),
            "the reference render was silent, so this proves nothing"
        );

        for mix in [0.0_f32, 0.5, 1.0] {
            // One box around the middle device.
            let mut one = bare.clone();
            wrap(&mut one, 1..2, mix);
            assert_eq!(
                render_blocks(&one, 0.5, block),
                reference,
                "a container at mix {mix} changed the render at block {block}"
            );

            // A box inside a box, around the whole run.
            let mut nested = bare.clone();
            wrap(&mut nested, 1..3, mix);
            wrap(&mut nested, 0..4, mix);
            assert_eq!(
                render_blocks(&nested, 0.5, block),
                reference,
                "nested containers at mix {mix} changed the render at block {block}"
            );
        }
    }
}

/// A bypassed container is transparent for the same reason an active one is,
/// and stays time-transparent: `chain_latency` counts the children, and a
/// container declares nothing of its own, so nothing about the compensation
/// plan moves when a box is added or bypassed.
#[test]
fn a_bypassed_container_does_not_move_the_chain_in_time() {
    let bare = three_device_chain();
    let reference = render_blocks(&bare, 0.5, 128);

    let mut boxed = bare.clone();
    wrap(&mut boxed, 0..3, 1.0);
    boxed.channels[0].setup.effects[0].bypassed = true;
    assert_eq!(render_blocks(&boxed, 0.5, 128), reference);

    // And the declared latency of the chain is the same number either way,
    // which is the fact the render above depends on.
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

/// A project written before containers existed has no `children` and no
/// `mix`, and a container written by a reader that has both must decode with
/// the defaults rather than failing the load -- `PROJECT_FORMAT.md`'s
/// defaulted-field rule, checked at the one place a new params variant could
/// break it.
#[test]
fn a_container_decodes_from_a_manifest_that_names_neither_field() {
    let params: EffectParams =
        toml::from_str("type = \"chain\"\n[state]\n").expect("an empty state decodes");
    assert_eq!(params, EffectParams::Chain(ChainParams::default()));
    assert_eq!(params.kind(), EffectKind::Chain);
}
