//! Track edits as incremental commands: adding, removing or moving a track
//! reaches the graph as one `StructuralCommand` and leaves it as an install
//! of the edited project would (MOO-466).
//!
//! The acceptance case is parity, in-process: the same playing song, edited
//! once by the command and once by the install path the executor runs
//! (prepare, carry, swap), renders the same samples afterwards and holds the
//! same mixer derivations -- compensation, console, solo, sends, the track
//! graph. Beside it: a song made with the edit from the start sounds the
//! same, every surviving strip is the same strip, the callback allocates
//! nothing, and a refused edit hands its payload back untouched.

use std::sync::Arc;

use mooloop_core::{
    AutomationLane, AutomationPoint, AuxSend, DeviceKind, EffectKind, EffectSlotState, EffectTarget,
    ModLfoParams, ModPolarity, ModRoute, ModulatorParams, NoteEvent, ParamAddr, Project,
    ProjectChannel, TrackEdit, MAX_BUSES,
};
use mooloop_dsp::SampleData;

use crate::render::{project_latency, RenderState, TrackReseat};
use crate::render_test_support::SAMPLE_RATE;
use crate::{carry_plan, InputState, StructuralCommand, StructuralReclaim};

const BLOCK: usize = 256;

fn tone() -> Arc<SampleData> {
    Arc::new(SampleData {
        frames: (0..SAMPLE_RATE as usize / 2)
            .map(|frame| {
                let s = (frame as f32 * 220.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin();
                [s * 0.5, s * 0.4]
            })
            .collect(),
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    })
}

/// Six channels over four tracks and the master, with everything a track
/// seat names carrying something:
///
/// - track 1 has a lookahead limiter (a latent insert, so the master and the
///   send owe compensation) and a send to track 3;
/// - track 2 has a reverb (a tail), feeds track 4 rather than the master, and
///   is console-encoded, so track 4 holds an accumulator;
/// - track 3 is console-encoded into the master;
/// - track 4 has a delay (a tail) whose first knob a lane on channel 0 drives
///   and, when `routed`, an LFO on channel 1 modulates;
/// - track 2 soloed when `solo`;
/// - a note held on every channel across the edit.
fn song(solo: bool, routed: bool) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.ensure_tracks(5);
    let kinds = [
        DeviceKind::Ds01,
        DeviceKind::MlM1,
        DeviceKind::Sampler,
        DeviceKind::PolySynth,
        DeviceKind::MlP8,
        DeviceKind::DrumSynth,
    ];
    let feeds = [1u8, 2, 3, 4, 2, 0];
    for (index, kind) in kinds.into_iter().enumerate() {
        let mut channel = match kind {
            DeviceKind::Ds01 => ProjectChannel::ds01(index, 1),
            DeviceKind::MlM1 => ProjectChannel::mlm1(index, 1),
            DeviceKind::Sampler => ProjectChannel::sampler(index, 1),
            DeviceKind::PolySynth => ProjectChannel::poly_synth(index, 1),
            DeviceKind::MlP8 => ProjectChannel::mlp8(index, 1),
            _ => ProjectChannel::drum_synth(index, 1),
        };
        channel.setup.channel.volume = 0.3;
        channel.setup.channel.bus = feeds[index];
        for (step, pitch) in [(0u32, 48u8), (4, 55), (8, 60), (12, 67)] {
            channel.notes[0].push(NoteEvent::new(
                step + 1,
                step * 24,
                96 + 24,
                pitch + index as u8,
                110,
            ));
        }
        // One note held from the top of the bar to its end, across the edit.
        channel.notes[0].push(NoteEvent::new(20, 0, 16 * 24, 40 + index as u8, 90));
        project.channels.push(channel);
    }
    project.assign_channel_ids();
    project.buses[1]
        .push_effect(EffectSlotState::of_kind(EffectKind::Limiter))
        .expect("room in track 1's chain");
    project.buses[1].sends.push(AuxSend::new(3));
    project.buses[2]
        .push_effect(EffectSlotState::of_kind(EffectKind::Reverb))
        .expect("room in track 2's chain");
    project.buses[2].bus.output = 4;
    project.buses[2].bus.console = true;
    project.buses[2].bus.solo = solo;
    project.buses[3].bus.console = true;
    let delay = project.buses[4]
        .push_effect(EffectSlotState::of_kind(EffectKind::Delay))
        .expect("room in track 4's chain");
    let on_delay = ParamAddr::effect(EffectTarget::Bus(4), delay, 0);
    {
        let mut lane = AutomationLane::new(on_delay);
        lane.reserve_points();
        lane.reset_points([
            AutomationPoint::new(1, 0, 0.1),
            AutomationPoint::new(2, 192, 0.9),
            AutomationPoint::new(3, 383, 0.2),
        ]);
        project.channels[0].automation[0].push(lane);
    }
    if routed {
        let rack = project.channels[1].setup.carried_modulation_mut();
        rack.install(
            0,
            ModulatorParams::Lfo(ModLfoParams {
                rate_hz: 3.0,
                ..ModLfoParams::default()
            }),
        );
        rack.add_route(ModRoute::to_slot(0, on_delay, 0.4, ModPolarity::Bipolar))
            .expect("room in the matrix");
        project.lift_channel_modulation();
    }
    project
}

fn samples(project: &Project) -> Vec<Option<Arc<SampleData>>> {
    project
        .channels
        .iter()
        .map(|channel| (channel.setup.source.kind() == DeviceKind::Sampler).then(tone))
        .collect()
}

/// A state playing `project`, run far enough that every channel and every
/// tail is sounding.
fn playing(project: &Project) -> RenderState {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &samples(project));
    render.play();
    for _ in 0..40 {
        render.process_once_block(BLOCK);
    }
    render
}

fn input() -> InputState {
    InputState {
        record_armed: false,
        midi_routing: Vec::new(),
        audio_input: Vec::new(),
        monitor: Vec::new(),
    }
}

/// The payload the handle builds for `edit`, `incoming` being the project it
/// produced, with the compensation an install would derive.
fn payload(edit: TrackEdit, incoming: &Project) -> Box<TrackReseat> {
    TrackReseat::new(edit, incoming, input(), &project_latency(incoming), SAMPLE_RATE)
}

/// Apply `edit` to `live` the way the handle does.
fn reseat(live: &mut RenderState, edit: TrackEdit, incoming: &Project) -> Box<TrackReseat> {
    match live.apply_structural(StructuralCommand::ReseatTracks {
        reseat: payload(edit, incoming),
    }) {
        Some(StructuralReclaim::TracksReseated(reseat)) => reseat,
        _ => panic!("a track edit comes back as TracksReseated"),
    }
}

/// Install `incoming` over `live` the way the executor does for an edit.
fn install(live: RenderState, live_project: &Project, incoming: &Project) -> RenderState {
    let mut live = live;
    let mut fresh = RenderState::from_project(SAMPLE_RATE, incoming, &samples(incoming));
    fresh.adopt_performance_state(&live);
    fresh.carry_strips_from(&mut live, &carry_plan(live_project, incoming));
    fresh
}

fn render(render: &mut RenderState, blocks: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(blocks * BLOCK * 2);
    for _ in 0..blocks {
        render.process_once_block(BLOCK);
        let master = render.master();
        out.extend_from_slice(&master.l[..BLOCK]);
        out.extend_from_slice(&master.r[..BLOCK]);
    }
    out
}

fn assert_same(edited: &[f32], reference: &[f32], what: &str) {
    assert!(
        reference.iter().any(|sample| sample.abs() > 1e-3),
        "{what}: the song is silent, so nothing is compared"
    );
    let first = edited
        .iter()
        .zip(reference)
        .position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(first, None, "{what}: parted at sample {first:?}");
}

/// Every edit the parity tests make, with the project it produces from
/// `project`: each track removed, each kind of move, and a track added.
fn edits(project: &Project) -> Vec<(TrackEdit, Project)> {
    let mut edits = Vec::new();
    for track in 1..=4 {
        let mut incoming = project.clone();
        incoming.remove_track(track).expect("a track to remove");
        edits.push((TrackEdit::Removed(track as u8), incoming));
    }
    for (from, to) in [(1, 4), (4, 1), (2, 3), (3, 2), (1, 2)] {
        let mut incoming = project.clone();
        let edit = incoming.move_track(from, to).expect("a track to move");
        edits.push((edit, incoming));
    }
    let mut incoming = project.clone();
    let at = incoming.add_track().expect("room for a track");
    edits.push((TrackEdit::Inserted(at as u8), incoming));
    edits
}

/// **Every track edit sounds exactly like the install it replaces**, solo
/// or not: the same playing song, one copy edited by the command and the
/// other by prepare, carry and swap, renders the same samples for two
/// seconds afterwards -- tails, the latent limiter's compensation, the send,
/// both console sums and the lane on the delay included.
///
/// Without the LFO route on track 4's delay: a route names its track's seat,
/// so the install finds a renumbered route a changed setup and rebuilds the
/// routing channel, cutting its voices, where the command keeps it sounding.
/// The test after next holds that to a song made with the edit instead.
#[test]
fn a_track_edit_renders_what_installing_the_result_renders() {
    for solo in [false, true] {
        let project = song(solo, false);
        for (edit, incoming) in edits(&project) {
            let mut by_command = playing(&project);
            let reseat = reseat(&mut by_command, edit, &incoming);
            assert!(reseat.applied(), "{edit:?} was refused");
            let mut by_install = install(playing(&project), &project, &incoming);

            assert_eq!(
                by_command.mixer_derivations(),
                by_install.mixer_derivations(),
                "{edit:?} (solo {solo}): the mixer the command left against the install's"
            );
            let blocks = (2 * SAMPLE_RATE as usize) / BLOCK;
            assert_same(
                &render(&mut by_command, blocks),
                &render(&mut by_install, blocks),
                &format!("{edit:?} (solo {solo}): the command against the install"),
            );
        }
    }
}

/// **The song really owes compensation and console sums**, so the parity
/// above compares something: the limiter's track arrives late, the send and
/// the other paths into the master wait for it, and both console-encoded
/// tracks' destinations hold an accumulator.
#[test]
fn the_parity_song_exercises_compensation_console_and_solo() {
    let project = song(true, false);
    let plan = project_latency(&project);
    let waits = (0..5).filter(|&track| plan.bus(track) > 0).count()
        + (0..project.channels.len()).filter(|&channel| plan.channel(channel) > 0).count();
    assert!(waits > 0, "nothing waits for the limiter");
    assert_eq!(playing(&project).console_sums_held(), 2, "track 4 and the master hold no accumulator");
    let silenced = mooloop_core::mixer::solo_silenced(&project.buses);
    assert!(silenced.iter().take(5).any(|&s| s), "the solo silences nothing");
}

/// **A track edit is inaudible beyond the track it is about.** A song made
/// with the edit from the start -- the LFO routed on the moved delay, the
/// lane on it, the send, the console sums, the latent limiter, track 2
/// soloed or not, a note held on every channel -- plays the same after the
/// command as the edited song does. A removed or added track is an empty
/// one, so the song without it is the same song.
#[test]
fn a_track_edit_matches_a_song_made_with_it() {
    for solo in [false, true] {
        let project = song(solo, true);
        let mut cases: Vec<(Project, TrackEdit, Project)> = Vec::new();
        for (from, to) in [(1, 4), (4, 1), (2, 3), (4, 2)] {
            let mut incoming = project.clone();
            let edit = incoming.move_track(from, to).expect("a track to move");
            cases.push((project.clone(), edit, incoming));
        }
        for seat in [1, 3] {
            // An empty track at `seat`, removed.
            let mut with_empty = project.clone();
            let added = with_empty.add_track().expect("room for a track");
            with_empty.move_track(added, seat).expect("the empty track moves");
            let mut incoming = with_empty.clone();
            incoming.remove_track(seat).expect("the empty track goes");
            cases.push((with_empty, TrackEdit::Removed(seat as u8), incoming));
        }
        let mut incoming = project.clone();
        let at = incoming.add_track().expect("room for a track");
        cases.push((project.clone(), TrackEdit::Inserted(at as u8), incoming));

        for (before, edit, incoming) in cases {
            let mut by_command = playing(&before);
            assert!(reseat(&mut by_command, edit, &incoming).applied(), "{edit:?} was refused");
            let mut made_so = playing(&incoming);

            let blocks = (2 * SAMPLE_RATE as usize) / BLOCK;
            assert_same(
                &render(&mut by_command, blocks),
                &render(&mut made_so, blocks),
                &format!("{edit:?} (solo {solo}): the command against a song made with the edit"),
            );
        }
    }
}

/// **Every surviving track keeps its strip**, in its new seat: the same
/// summing buffer, so its chain, tails and fader state are the ones that were
/// sounding. A removed track's strip comes back in the payload; an added
/// one's leaves it.
#[test]
fn a_track_edit_keeps_every_other_strip() {
    let project = song(false, true);
    for (edit, incoming) in edits(&project) {
        let mut live = playing(&project);
        let mut expected: Vec<usize> = (0..live.track_count()).map(|seat| live.track_identity(seat)).collect();
        let reseat = reseat(&mut live, edit, &incoming);
        assert!(reseat.applied(), "{edit:?} was refused");
        let after: Vec<usize> = (0..live.track_count()).map(|seat| live.track_identity(seat)).collect();
        match edit {
            TrackEdit::Removed(at) => {
                expected.remove(usize::from(at));
                assert!(reseat.carries_strip(), "{edit:?}: the removed strip did not come back");
                assert_eq!(after, expected, "{edit:?}: a strip was replaced or misplaced");
            }
            TrackEdit::Moved { from, to } => {
                let lifted = expected.remove(usize::from(from));
                expected.insert(usize::from(to), lifted);
                assert!(!reseat.carries_strip());
                assert_eq!(after, expected, "{edit:?}: a strip was replaced or misplaced");
            }
            TrackEdit::Inserted(at) => {
                assert!(!reseat.carries_strip(), "the added strip did not land");
                let mut kept = after.clone();
                kept.remove(usize::from(at));
                assert_eq!(kept, expected, "{edit:?}: a strip was replaced or misplaced");
            }
        }
        assert_eq!(live.track_count(), incoming.buses.len());
    }
}

/// **The song's modulation set is the incoming project's**: a route to a
/// moved track's device follows it, and one to a removed track's goes.
#[test]
fn a_track_edit_rescopes_every_route_to_a_track() {
    let project = song(false, true);
    for (edit, incoming) in edits(&project) {
        let mut live = playing(&project);
        assert!(reseat(&mut live, edit, &incoming).applied(), "{edit:?} was refused");
        assert_eq!(
            *live.song_modulation().plan(),
            crate::SongModulator::compile(&incoming),
            "{edit:?}: the set is not the incoming project's"
        );
    }
}

/// **The callback allocates nothing, frees nothing and takes no lock**
/// applying any track edit: what leaves goes back in the payload.
#[test]
fn a_track_edit_allocates_nothing_on_the_callback() {
    assert!(
        mooloop_core::lock_check::counting(),
        "a build without debug assertions counts no locks, so the lock half would pass unchecked"
    );
    let project = song(true, true);
    for (edit, incoming) in edits(&project) {
        let mut live = playing(&project);
        let command = StructuralCommand::ReseatTracks {
            reseat: payload(edit, &incoming),
        };

        let locks = mooloop_core::lock_check::locks_taken();
        let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        let reclaimed = live.apply_structural(command);
        live.process_once_block(BLOCK);
        let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        let locked = mooloop_core::lock_check::locks_taken() - locks;

        assert_eq!(after, before, "{edit:?} allocated or freed on the callback");
        assert_eq!(locked, 0, "{edit:?} took a lock on the callback");
        match reclaimed {
            Some(StructuralReclaim::TracksReseated(reseat)) => {
                assert!(reseat.applied());
                assert_eq!(
                    reseat.carries_strip(),
                    matches!(edit, TrackEdit::Removed(_)),
                    "{edit:?}: the payload came back holding the wrong strips"
                );
            }
            _ => panic!("{edit:?}: the payload did not come back"),
        }
    }
}

/// **A track edit that cannot land changes nothing**: the master's seat
/// either way, a seat past the last track, a move onto itself, and an
/// addition to a full bank. The payload comes back as it went.
#[test]
fn a_refused_track_edit_changes_nothing() {
    let project = song(false, true);
    let mut live = playing(&project);
    let before: Vec<usize> = (0..live.track_count()).map(|seat| live.track_identity(seat)).collect();
    let derived = live.mixer_derivations();
    let mut grown = project.clone();
    grown.add_track().unwrap();
    for (edit, incoming) in [
        (TrackEdit::Removed(0), &project),
        (TrackEdit::Removed(5), &project),
        (TrackEdit::Moved { from: 0, to: 2 }, &project),
        (TrackEdit::Moved { from: 2, to: 0 }, &project),
        (TrackEdit::Moved { from: 2, to: 2 }, &project),
        (TrackEdit::Moved { from: 1, to: 5 }, &project),
        (TrackEdit::Inserted(0), &grown),
        (TrackEdit::Inserted(6), &grown),
    ] {
        let reseat = reseat(&mut live, edit, incoming);
        assert!(!reseat.applied(), "{edit:?} landed");
        assert_eq!(
            reseat.carries_strip(),
            edit == TrackEdit::Inserted(0),
            "{edit:?}: the payload did not come back as it went"
        );
        let after: Vec<usize> = (0..live.track_count()).map(|seat| live.track_identity(seat)).collect();
        assert_eq!(after, before, "{edit:?} moved a strip");
        assert_eq!(live.mixer_derivations(), derived, "{edit:?} changed the mixer");
    }

    let mut full = project.clone();
    full.ensure_tracks(MAX_BUSES);
    let mut live = playing(&full);
    let mut over = full.clone();
    over.buses.push(over.buses[1].clone());
    let reseat = reseat(&mut live, TrackEdit::Inserted(MAX_BUSES as u8), &over);
    assert!(!reseat.applied(), "a full bank took another track");
    assert_eq!(live.track_count(), MAX_BUSES);
}
