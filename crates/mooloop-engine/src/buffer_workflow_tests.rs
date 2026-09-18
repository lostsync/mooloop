//! The Buffer workflow, end to end, as far as a test can carry it.
//!
//! `docs/FOCUS.md`'s step 2 sets the acceptance case and
//! `docs/plans/archive/buffer-implementation/03-freeze-and-the-grid.md` closes on it:
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
    EffectTarget, NoteEvent, ParamAddr, ParamOwner, Project, ProjectChannel, TICKS_PER_STEP,
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
///
/// **The ring is two bars and only the first is ever written**, because that
/// is all the render lasts. `Position` addresses the ring in its own
/// coordinates at the default full span, so the written half is `0.0..0.5`
/// and anything above it is silence the writer never reached.
fn drum_bar() -> Project {
    let mut channel = ProjectChannel::drum_synth(0, 1);
    // `NoteEvent::new` takes a start *tick*. Passing the step index put all
    // sixteen hits in the first sixteen ticks -- 4 000 frames -- and left the
    // rest of the bar silent, which every test here was quietly relying on
    // not mattering.
    for step in 0..16u32 {
        channel.notes[0].push(NoteEvent::new(
            step + 1,
            step * TICKS_PER_STEP,
            TICKS_PER_STEP,
            36,
            127,
        ));
    }
    with_buffer(channel, 2)
}

/// One held synth note into a one-bar ring.
///
/// The freeze tests need two things at once that a drum pattern cannot give
/// them: a **full ring** at the moment of the freeze, and audio **still
/// playing** afterwards to compare against. A note held for the whole render
/// gives both. The ring is one bar and so is the default `Quant Start`, so a
/// freeze or a gesture held from the top lands on the 2 s line with the ring
/// exactly full.
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

/// A lane value that denormalizes to `index` on one of the Buffer's stepped
/// grid parameters, whose range is the whole `ModTimeDivision` table.
fn grid(index: f32) -> f32 {
    index / mooloop_core::MOD_TIME_DIVISION_TOP
}

/// Frames per tick in these fixtures: 120 BPM, 96 PPQ, 48 kHz.
const FRAMES_PER_TICK: usize = 250;

/// A `Position` lane that rests at its default for the first half-bar and
/// then plays the first half-bar **backward, at unity**, over the second.
///
/// Resting at `1.0` -- the default -- is resting: `Position` is heard only
/// while it moves, so the first half of the render is the input untouched.
/// From tick 193 it runs `0.25 -> 0.0`, which on a two-bar ring is frames
/// 48 000 down to 0 over 47 750 frames: the half-bar that was just written,
/// read back at very nearly unit speed. Unity matters -- a ramp faster than
/// `MAX_SWEEP_RATE` is treated as a string of edits and cut, not swept -- and
/// so does the direction: the drum bar repeats every sixteenth, so a
/// *forward* replay of it could be indistinguishable from the live signal.
///
/// The one-tick step from `1.0` to `0.25` is a jump, and is heard as one.
const REVERSE_SWEEP: [(u32, f32); 4] = [(0, 1.0), (192, 1.0), (193, 0.25), (384, 0.0)];

/// The frame the sweep starts at.
const SWEEP_START: usize = 192 * FRAMES_PER_TICK;

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
///
/// It also pins where the difference is. `Position` is heard only while it
/// moves, so the half-bar it rests in has to be the input **exactly**, and the
/// half-bar it sweeps has to be sound rather than the silence of a part of the
/// ring the writer never reached -- which is what the lane this test first
/// had was reading, from a ring coordinate that no longer meant "live".
#[test]
fn a_position_lane_makes_an_audible_difference() {
    let plain = drum_bar();
    let mut moved = plain.clone();
    draw(
        &mut moved,
        mooloop_core::BUFFER_PARAM_POSITION,
        &REVERSE_SWEEP,
    );

    let before = render_blocks(&plain, 2.0, 128);
    let after = render_blocks(&moved, 2.0, 128);

    assert!(
        peak_of(&before) > 0.001,
        "the channel has to make sound before the Buffer can do anything to it"
    );
    let resting = ..SWEEP_START - 1_000;
    assert_eq!(
        worst_difference(&before[resting], &after[resting]),
        0.0,
        "a Position resting at its default was heard: it has to be live"
    );
    // Past the jump and its crossfades, into the sweep proper.
    let sweeping = SWEEP_START + 2 * FRAMES_PER_TICK..before.len().min(after.len());
    assert!(
        peak_of(&after[sweeping.clone()]) > 0.001,
        "the sweep is silent: the head is reading history nobody wrote"
    );
    assert!(
        worst_difference(&before[sweeping.clone()], &after[sweeping]) > 0.01,
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
/// defect. `buffer_device::tests::freezing_leaves_a_loop_running` pins what
/// the claim rests on -- a frozen ring plays forward at unity from its oldest
/// frame, which is the input delayed by exactly one ring, and so is the input
/// itself whenever the input repeats at that length. The end-to-end version
/// wants a looping transport, and until then it wants ears.
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

/// **Reversing a frozen ring is audible, and it is the ring backward**, which
/// is the transformation the freeze exists to enable: once the writer has
/// stopped, the history is a sample, and playing it backward is something no
/// arrangement of the live signal can produce.
///
/// The reference is the same freeze *without* the gesture, not the live
/// signal. A frozen ring going round forward already differs from the live
/// tone by its phase (see the test above), so "differs from live" would pass
/// on a REVERSE that did nothing.
///
/// Both lanes are held from the top and both land on the 2 s bar line, the
/// gesture outranking the frozen ring the moment it does.
#[test]
fn a_frozen_buffer_played_in_reverse_is_the_ring_backward() {
    let plain = held_tone();
    let mut frozen = plain.clone();
    draw(&mut frozen, mooloop_core::BUFFER_PARAM_FREEZE, &[(0, 1.0)]);
    let mut reversed = frozen.clone();
    draw(
        &mut reversed,
        mooloop_core::BUFFER_PARAM_REVERSE,
        &[(0, 1.0)],
    );

    let live = render_blocks(&plain, 3.0, 128);
    let (forward, forward_marks) = render_with_marks(&frozen, 3.0);
    let (backward, marks) = render_with_marks(&reversed, 3.0);
    assert!(
        forward_marks.frozen,
        "the reference freeze has to have landed"
    );
    assert!(
        marks.frozen && marks.head.is_some(),
        "the freeze has to have landed and a head has to be playing, or the \
         comparison below is between two things that did nothing"
    );
    assert!(!marks.armed_gesture, "and the reverse is not still waiting");

    let line = SAMPLE_RATE as usize * 2;
    // Past the crossfade out of live.
    let span = line + 1_000..live.len().min(backward.len());
    assert!(
        peak_of(&live[span.clone()]) > 0.01,
        "the note has to still be sounding after the freeze, or there is \
         nothing to compare the frozen playback against"
    );
    assert!(
        worst_difference(&forward[span.clone()], &backward[span]) > 0.01,
        "a frozen buffer held in REVERSE sounded the same as the frozen buffer"
    );

    // And it is the ring *backward*: the frames after the line are the frames
    // before it, in the opposite order. A lag of a frame or two is where the
    // head starts relative to the newest frame, which this does not pin.
    let length = SAMPLE_RATE as usize / 2;
    let start = line + 1_000;
    let (lag, worst) = (0..8)
        .map(|lag| {
            let worst = (0..length)
                .map(|k| (backward[start + k] - live[2 * line - 1 - start - k - lag]).abs())
                .fold(0.0f32, f32::max);
            (lag, worst)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .expect("at least one lag");
    assert!(
        worst < 1e-4,
        "half a second of the reversed playback is not the ring read backward \
         at any lag under 8 frames (best: lag {lag}, worst difference {worst})"
    );
}

/// A thirty-second stutter, quantized to the quarter, held from just before
/// beat 2 to just after beat 3 of the drum bar.
///
/// Three lanes, and each is one of the gesture's own settings: the gate, its
/// length and the grid it starts on. A thirty-second is 3 000 frames, which is
/// half the drum bar's spacing, so a live signal cannot repeat at that period
/// and a stutter must.
fn draw_stutter(project: &mut Project) {
    draw(
        project,
        mooloop_core::BUFFER_PARAM_STUTTER_LENGTH,
        &[(0, grid(16.0))],
    );
    draw(
        project,
        mooloop_core::BUFFER_PARAM_QUANT_START,
        &[(0, grid(7.0))],
    );
    draw(
        project,
        mooloop_core::BUFFER_PARAM_STUTTER,
        &[(0, 0.0), (95, 0.0), (96, 1.0), (240, 1.0), (241, 0.0)],
    );
}

const THIRTY_SECOND: usize = 3_000;

/// **It survives save and reload, and renders the same offline.**
///
/// Sample for sample: the document has to carry the lanes, the device's own
/// parameters and its identity, and the offline path has to be the same
/// engine. A difference here is a project that sounds different the second
/// time it is opened, which is the failure a musician cannot work around.
///
/// **Equality is an absence**, so the stutter is first proved to have fired:
/// the render differs from the unstuttered bar, and inside the held span it
/// repeats at exactly the stutter's length, which the drums alone do not.
#[test]
fn the_whole_thing_survives_a_round_trip_and_renders_the_same() {
    let temp = tempfile::tempdir().expect("a temp dir");
    let path = temp.path().join("buffered.mooloop");

    let plain = drum_bar();
    let mut project = plain.clone();
    draw_stutter(&mut project);

    let unstuttered = render_blocks(&plain, 2.0, 128);
    let before = render_blocks(&project, 2.0, 128);
    assert!(peak_of(&before) > 0.001, "the fixture has to make sound");
    assert!(
        worst_difference(&unstuttered, &before) > 0.01,
        "the stutter lanes changed nothing"
    );
    // Beat 2 is frame 24 000 and the release is just after frame 60 000; one
    // repeat in, and one repeat short of the release.
    let held = 24_000 + THIRTY_SECOND + 1_000..60_000 - THIRTY_SECOND;
    assert!(
        peak_of(&before[held.clone()]) > 0.001,
        "the stutter is repeating silence, which proves nothing"
    );
    let repeats = |render: &[f32]| {
        held.clone()
            .map(|t| (render[t] - render[t + THIRTY_SECOND]).abs())
            .fold(0.0f32, f32::max)
    };
    assert!(
        repeats(&before) < 1e-6,
        "the held span does not repeat at the stutter length: {}",
        repeats(&before)
    );
    assert!(
        repeats(&unstuttered) > 0.01,
        "the drums alone repeat at a thirty-second, so the check above is \
         not evidence of a stutter"
    );

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
///
/// The document drives both a moving `Position` and a quantized gesture, and
/// the first assertion is that it did something at all -- two renders of an
/// inert device agree trivially.
#[test]
fn two_offline_renders_of_one_document_agree() {
    let plain = drum_bar();
    let mut project = plain.clone();
    draw(
        &mut project,
        mooloop_core::BUFFER_PARAM_POSITION,
        &REVERSE_SWEEP,
    );
    draw_stutter(&mut project);

    let first = render_blocks(&project, 2.0, 128);
    assert!(
        worst_difference(&render_blocks(&plain, 2.0, 128), &first) > 0.01,
        "the lanes changed nothing, so agreement below proves nothing"
    );
    let second = render_blocks(&project, 2.0, 128);
    assert_eq!(worst_difference(&first, &second), 0.0);

    // And across block sizes, which is where a device that carried per-block
    // state would show up. The buffer's own suite asserts this of the device;
    // this asserts it of the whole graph with lanes driving it.
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
///
/// **A Delay, not a Drive.** A memoryless shaper commutes with replay -- a
/// sample distorted and then read back is the sample read back and then
/// distorted -- so it could only ever show the choice through crossfade
/// blends. A delay does not commute with a backward sweep: in front, the
/// history holds the echoes and they play backward as pre-echoes; behind, the
/// echoes follow the reversed hits. And while the Buffer is live it is a wire,
/// so until the sweep starts the two chains have to be the same signal.
#[test]
fn where_the_insert_sits_decides_what_it_captures() {
    let mut early = drum_bar();
    // A delay in front of the Buffer, so the history holds the echoes.
    early.channels[0]
        .setup
        .effects
        .insert(0, EffectSlotState::of_kind(EffectKind::Delay));
    early.channels[0].setup.assign_device_ids();
    let mut late = early.clone();
    // ...and the same delay behind it, so the history holds the dry hits and
    // the echoes are applied to whatever the head plays.
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
        lane.reset_points(
            REVERSE_SWEEP
                .iter()
                .enumerate()
                .map(|(index, (tick, value))| {
                    AutomationPoint::new(index as u32 + 1, *tick, *value)
                }),
        );
        project.channels[0].automation[0].clear();
        project.channels[0].automation[0].push(lane);
    }

    let a = render_blocks(&early, 2.0, 128);
    let b = render_blocks(&late, 2.0, 128);
    let resting = ..SWEEP_START - 1_000;
    assert!(
        worst_difference(&a[resting], &b[resting]) < 1e-6,
        "the chains differ while the Buffer is a wire, so a difference later \
         would not be the capture point's"
    );
    let sweeping = SWEEP_START..a.len().min(b.len());
    assert!(
        worst_difference(&a[sweeping.clone()], &b[sweeping]) > 0.001,
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
