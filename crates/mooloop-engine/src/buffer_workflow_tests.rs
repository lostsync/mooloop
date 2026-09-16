//! The Buffer workflow, end to end, as far as a test can carry it.
//!
//! `docs/FOCUS.md`'s step 2 sets the acceptance case and
//! `docs/plans/buffer-implementation/03-freeze-and-the-grid.md` closes on it:
//!
//! > generate or load sound, capture it continuously at a chosen insert
//! > point, sequence an audible jump/reverse/repeat transformation, show what
//! > the read head is doing, survive save and reload, and render the same
//! > result offline.
//!
//! Six of those seven are machine-checkable and are checked here. **"Show
//! what the read head is doing" is not**: the face draws it, and a test can
//! see the telemetry -- which `render::tests::a_buffer_publishes_a_picture_
//! for_its_face_to_draw` does -- but not whether a person can read it. Nor is
//! the judgement the step exists to make, which is whether this is materially
//! better than bouncing a sample and loading it again. Both want ears and a
//! screen, and `FOCUS.md` is explicit that listening is a step rather than a
//! formality.
//!
//! What these do settle is that there is something to listen *to*: the
//! transformation is audible rather than a no-op, the document carries it,
//! and the offline render is the same both times.

use crate::meters::{BufferMarks, DeviceTelemetry};
use crate::render::RenderState;
use crate::render_test_support::{peak_of, render_blocks, worst_difference, SAMPLE_RATE};
use mooloop_core::{
    AutomationLane, AutomationPoint, BufferParams, EffectKind, EffectParams, EffectSlotState,
    EffectTarget, NoteEvent, ParamAddr, ParamOwner, Project, ProjectChannel,
};

/// Put a Buffer on a channel's chain and make room for lanes.
fn with_buffer(mut channel: ProjectChannel, bars: u8) -> Project {
    channel.setup.channel.volume = 1.0;
    channel
        .setup
        .push_effect(EffectSlotState::new(EffectParams::Buffer(BufferParams {
            bars,
            ..Default::default()
        })))
        .expect("room for the insert");
    channel.setup.assign_device_ids();
    channel.normalize_automation();
    Project {
        channels: vec![channel],
        ..Project::default()
    }
}

/// Sixteen drum hits across one bar, into a two-bar ring.
///
/// **The pattern plays once**: `RenderState` runs the transport and nothing
/// here loops it, which is a fact about the fixture rather than about the
/// engine -- pattern wrapping belongs to the session layer. So every test
/// using this renders inside that first bar, and the ones that need audio
/// still playing after the ring is full use [`held_tone`] instead.
///
/// Hits rather than a tone because these are the tests about *where* the head
/// is: moving through a sustained note sounds like a sustained note.
fn drum_bar() -> Project {
    let mut channel = ProjectChannel::drum_synth(0, 1);
    for step in 0..16u32 {
        channel.notes[0].push(NoteEvent::new(step + 1, step, 24, 36, 127));
    }
    with_buffer(channel, 2)
}

/// One held synth note into a one-bar ring.
///
/// The freeze tests need two things at once that a drum pattern cannot give
/// them: a **full ring** at the moment of the freeze, and audio **still
/// playing** afterwards to compare against. A note held for the whole render
/// gives both, and a tone is also the signal that makes a rate change
/// legible -- half speed is an octave down.
fn held_tone() -> Project {
    let mut channel = ProjectChannel::poly_synth(0, 1);
    // Far longer than any render here, so the note never releases.
    channel.notes[0].push(NoteEvent::new(1, 0, 8_000, 48, 110));
    with_buffer(channel, 1)
}

/// Render, and read back what the Buffer was doing when it finished.
///
/// **A test that asserts an absence needs this.** "Freezing a loop is nearly
/// inaudible" passes just as well when the freeze never happened, which is
/// the shape `AGENTS.md` warns about -- a guard that stopped guarding. The
/// marks say the writer actually stopped, so the silence is the feature's
/// rather than the feature's absence.
fn render_with_marks(project: &Project, seconds: f32) -> (Vec<f32>, BufferMarks) {
    let telemetry = DeviceTelemetry::new();
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.attach_device_telemetry(telemetry.clone());
    render.play();
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut left = Vec::with_capacity(remaining);
    while remaining > 0 {
        let frames = remaining.min(128);
        render.process_once_block(frames);
        let master = render.master();
        left.extend_from_slice(&master.l[..frames]);
        remaining -= frames;
    }
    // Channel 0, stage 1: the first effect slot.
    (left, telemetry.read_buffer_marks(0, 1))
}

fn buffer_address(project: &Project, param: u32) -> ParamAddr {
    ParamAddr {
        scope: EffectTarget::Channel(0),
        owner: ParamOwner::Effect {
            device: project.channels[0].setup.effects[0].id,
        },
        param,
    }
}

/// Draw a lane on one Buffer parameter.
fn draw(project: &mut Project, param: u32, points: &[(u32, f32)]) {
    let target = buffer_address(project, param);
    let mut lane = AutomationLane::new(target);
    lane.reserve_points();
    lane.reset_points(
        points
            .iter()
            .enumerate()
            .map(|(index, (tick, value))| AutomationPoint::new(index as u32 + 1, *tick, *value)),
    );
    project.channels[0].automation[0].push(lane);
}

/// **A sequenced transformation is audible**, which is the first thing the
/// acceptance case asks and the one a wired-up-but-inert device would fail.
///
/// The comparison is against the same project with the Buffer bypassed rather
/// than against silence: a Buffer doing nothing is a wire, so "it made sound"
/// proves only that the drum synth works.
#[test]
fn a_position_lane_makes_an_audible_difference() {
    let plain = drum_bar();
    let mut moved = plain.clone();
    // Sweep the head back through the retained history across the bar, then
    // hold it there. Live is 1.0 and the old end is 0.0.
    draw(
        &mut moved,
        mooloop_core::BUFFER_PARAM_POSITION,
        &[(0, 1.0), (48, 0.6), (96, 0.6)],
    );

    let before = render_blocks(&plain, 2.0, 128);
    let after = render_blocks(&moved, 2.0, 128);

    assert!(
        peak_of(&before) > 0.001,
        "the channel has to make sound before the Buffer can do anything to it"
    );
    assert!(
        worst_difference(&before, &after) > 0.01,
        "a Position lane changed nothing: the device is wired up and inert"
    );
}

/// **A freeze latches the history and the writer stops**, end to end: the
/// lane crosses the threshold, the request waits for the bar line, and what
/// comes out afterwards is the retained history rather than the input.
///
/// **What this deliberately does not assert is that it is *inaudible*.** That
/// claim -- the headline one, that pressing Freeze sounds like almost nothing
/// happened -- is only true when the material is periodic at the buffer
/// length, and nothing at this level can produce such material:
/// `RenderState` runs the transport but does not loop the pattern, so a
/// fixture is either one pass of a pattern or a held note whose period has
/// no relation to the ring's. A held tone frozen mid-cycle differs from the
/// live one by more than either amplitude, which is phase rather than a
/// defect. `buffer_device::tests::freezing_a_periodic_loop_is_inaudible`
/// makes that claim against a ring holding a whole number of cycles, which is
/// the only place it can honestly be made today. The end-to-end version wants
/// a looping transport, and until then it wants ears.
#[test]
fn a_freeze_latches_the_history_and_stops_the_writer() {
    let plain = held_tone();
    let mut frozen = plain.clone();
    draw(&mut frozen, mooloop_core::BUFFER_PARAM_FREEZE, &[(0, 1.0)]);

    // The ring is one bar and the grid is one bar, so the freeze lands at the
    // 2 s line with the ring exactly full.
    let before = render_blocks(&plain, 3.0, 128);
    let (after, marks) = render_with_marks(&frozen, 3.0);
    assert!(
        marks.frozen,
        "the writer has to have actually stopped, or every claim below is \
         about a device that did nothing"
    );

    let frozen_span = (SAMPLE_RATE as usize * 2)..before.len().min(after.len());
    assert!(
        peak_of(&before[frozen_span.clone()]) > 0.01,
        "the note has to still be sounding, or there is nothing to compare"
    );
    assert!(
        peak_of(&after[frozen_span]) > 0.01,
        "the latched history has to be playing: silence here is a head reading \
         a part of the ring the writer never reached"
    );
}

/// **Freeze plus a Rate change is audible**, which is the transformation the
/// freeze exists to enable: once the writer has stopped, `Rate` is the time
/// base, and running the latched history at half speed is something no
/// arrangement of the live signal can produce.
#[test]
fn a_frozen_buffer_played_at_another_rate_is_audible() {
    let plain = held_tone();
    let mut stretched = plain.clone();
    draw(&mut stretched, mooloop_core::BUFFER_PARAM_FREEZE, &[(0, 1.0)]);
    // 0.5 normalized over -4..4 is a rate of zero; 0.5625 is half speed
    // forward, which on a held note is an octave down -- audible in a way
    // that needs no ears to confirm.
    draw(
        &mut stretched,
        mooloop_core::BUFFER_PARAM_RATE,
        &[(0, 0.5625)],
    );

    let before = render_blocks(&plain, 3.0, 128);
    let (after, marks) = render_with_marks(&stretched, 3.0);
    assert!(marks.frozen, "the freeze has to have landed first");
    // Only the frozen span is worth comparing: before the bar line both
    // renders are the same live signal by construction.
    let frozen_span = (SAMPLE_RATE as usize * 2)..before.len().min(after.len());
    assert!(
        peak_of(&before[frozen_span.clone()]) > 0.01,
        "the note has to still be sounding after the freeze, or there is \
         nothing to compare the frozen playback against"
    );
    assert!(
        worst_difference(&before[frozen_span.clone()], &after[frozen_span]) > 0.01,
        "a frozen buffer at half speed sounded the same as the live signal"
    );
}

/// **It survives save and reload, and renders the same offline.**
///
/// Sample for sample: the document has to carry the lane, the device's own
/// parameters and its identity, and the offline path has to be the same
/// engine. A difference here is a project that sounds different the second
/// time it is opened, which is the failure a musician cannot work around.
#[test]
fn the_whole_thing_survives_a_round_trip_and_renders_the_same() {
    let temp = tempfile::tempdir().expect("a temp dir");
    let path = temp.path().join("buffered.mooloop");

    let mut project = drum_bar();
    draw(
        &mut project,
        mooloop_core::BUFFER_PARAM_POSITION,
        &[(0, 1.0), (48, 0.6), (96, 0.6)],
    );
    draw(
        &mut project,
        mooloop_core::BUFFER_PARAM_LENGTH,
        &[(0, 13.0 / 20.0)],
    );
    draw(&mut project, mooloop_core::BUFFER_PARAM_LOOP, &[(48, 1.0)]);

    let before = render_blocks(&project, 2.0, 128);
    assert!(peak_of(&before) > 0.001, "the fixture has to make sound");

    mooloop_project::save_song(&path, &project, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let mooloop_project::LoadedDocument::Song(reloaded) = mooloop_project::load_bundle(&path)
        .expect("it reopens")
        .document
    else {
        panic!("a song came back as something else");
    };

    assert_eq!(
        reloaded.channels[0].setup.effects[0].kind(),
        EffectKind::Buffer,
        "the insert has to still be a Buffer"
    );
    assert_eq!(
        reloaded.channels[0].automation[0].len(),
        3,
        "all three lanes have to survive: one dropped is a transformation that \
         quietly stops happening"
    );

    let after = render_blocks(&reloaded, 2.0, 128);
    assert_eq!(
        worst_difference(&before, &after),
        0.0,
        "the reopened project rendered differently"
    );
}

/// **The offline render is deterministic.** Two runs of the same document
/// have to agree, or "render the same result offline" cannot be asserted of
/// anything.
#[test]
fn two_offline_renders_of_one_document_agree() {
    let mut project = drum_bar();
    draw(
        &mut project,
        mooloop_core::BUFFER_PARAM_POSITION,
        &[(0, 1.0), (48, 0.6), (96, 0.6)],
    );
    draw(
        &mut project,
        mooloop_core::BUFFER_PARAM_FREEZE,
        &[(96, 1.0)],
    );

    let first = render_blocks(&project, 2.0, 128);
    let second = render_blocks(&project, 2.0, 128);
    assert_eq!(worst_difference(&first, &second), 0.0);

    // And across block sizes, which is where a device that carried per-block
    // state would show up. The buffer's own suite asserts this of the device;
    // this asserts it of the whole graph with a lane driving it.
    let chunked = render_blocks(&project, 2.0, 512);
    let shared = first.len().min(chunked.len());
    assert!(
        worst_difference(&first[..shared], &chunked[..shared]) < 1e-5,
        "the render depends on the block size"
    );
}

/// The capture point is a *choice*: a Buffer later in the chain retains what
/// the devices before it did, and that has to be audible in the difference.
///
/// This is the "capture it continuously at a chosen insert point" half, and
/// it is the one that makes the device an insert rather than a recorder.
#[test]
fn where_the_insert_sits_decides_what_it_captures() {
    let mut early = drum_bar();
    // A drive in front of the Buffer, so the history holds distorted audio.
    early.channels[0]
        .setup
        .effects
        .insert(0, EffectSlotState::of_kind(EffectKind::Drive));
    early.channels[0].setup.assign_device_ids();
    let mut late = early.clone();
    // ...and the same drive behind it, so the history holds clean audio and
    // the distortion is applied to whatever the head plays.
    late.channels[0].setup.effects.swap(0, 1);
    late.channels[0].setup.assign_device_ids();

    for project in [&mut early, &mut late] {
        let device = project.channels[0]
            .setup
            .effects
            .iter()
            .find(|slot| slot.kind() == EffectKind::Buffer)
            .expect("the buffer is still there")
            .id;
        let mut lane = AutomationLane::new(ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Effect { device },
            param: mooloop_core::BUFFER_PARAM_POSITION,
        });
        lane.reserve_points();
        lane.reset_points([
            AutomationPoint::new(1, 0, 1.0),
            AutomationPoint::new(2, 48, 0.6),
        ]);
        project.channels[0].automation[0].clear();
        project.channels[0].automation[0].push(lane);
    }

    let a = render_blocks(&early, 2.0, 128);
    let b = render_blocks(&late, 2.0, 128);
    assert!(
        worst_difference(&a, &b) > 0.001,
        "the insert position made no difference, so it is not an insert"
    );
}

/// A guard on the fixture rather than on the device: if the sample rate this
/// suite renders at ever stops being the one the beat arithmetic above
/// assumes, every "audible difference" here becomes a coincidence.
#[test]
fn the_fixture_renders_at_the_rate_its_arithmetic_assumes() {
    assert_eq!(SAMPLE_RATE, 48_000);
    let project = drum_bar();
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    render.play();
    let report = render.process_block(128);
    assert!(report.peak_l >= 0.0, "a block renders at all");
}
