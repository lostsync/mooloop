//! Channel edits as incremental commands: a removal or a move reaches the
//! graph as one `StructuralCommand` and leaves it as an install of the edited
//! project would (MOO-466).
//!
//! The acceptance case is parity, in-process: the same live state, edited
//! once by the command and once by the install path the executor runs
//! (prepare, carry, swap), renders the same samples afterwards. Beside it:
//! every other strip is the same strip, the callback allocates nothing, and
//! a refused edit hands its payload back untouched.

use std::sync::Arc;

use mooloop_core::{
    AutomationLane, AutomationPoint, AuxSend, ChannelEdit, DeviceKind, EffectKind, EffectSlotState, EffectTarget,
    ModLfoParams, ModPolarity, ModRoute, ModulatorParams, NoteEvent, ParamAddr, Project,
    ProjectChannel,
};
use mooloop_dsp::SampleData;

use crate::render::{bank_after, ChannelReseat, RenderState};
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

/// Six channels of different kinds on two tracks with a send between them,
/// notes held across the edit, tails on the channels after the removed one,
/// a lane on one of them, and -- when `routed` -- an LFO routed on another,
/// and when `solo` channel 4 soloed, so every seat-keyed structure the
/// removal renumbers is carrying something.
fn song(solo: bool, routed: bool) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.ensure_tracks(3);
    let kinds = [
        DeviceKind::Ds01,
        DeviceKind::MlM1,
        DeviceKind::Sampler,
        DeviceKind::PolySynth,
        DeviceKind::MlP8,
        DeviceKind::DrumSynth,
    ];
    for (index, kind) in kinds.into_iter().enumerate() {
        let mut channel = match kind {
            DeviceKind::Ds01 => ProjectChannel::ds01(index, 1),
            DeviceKind::MlM1 => ProjectChannel::mlm1(index, 1),
            DeviceKind::Sampler => ProjectChannel::sampler(index, 1),
            DeviceKind::PolySynth => ProjectChannel::poly_synth(index, 1),
            DeviceKind::MlP8 => ProjectChannel::mlp8(index, 1),
            _ => ProjectChannel::drum_synth(index, 1),
        };
        channel.setup.channel.volume = 0.4;
        channel.setup.channel.bus = 1 + (index % 2) as u8;
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
    project.channels[4].setup.channel.solo = solo;
    project.buses[1].sends.push(AuxSend::new(2));
    project.buses[2]
        .push_effect(EffectSlotState::of_kind(EffectKind::Reverb))
        .expect("room in the track's chain");
    for (index, kind) in [(3, EffectKind::Filter), (4, EffectKind::Delay), (4, EffectKind::Reverb), (5, EffectKind::Eq)] {
        project.channels[index]
            .setup
            .push_effect(EffectSlotState::of_kind(kind))
            .expect("room in the chain");
    }
    // An LFO on channel 4, to its first effect and its strip.
    if routed {
        let channel = &mut project.channels[4];
        let target = EffectTarget::Channel(4);
        let device = channel.setup.effects[0].id;
        channel.setup.modulation.install(
            0,
            ModulatorParams::Lfo(ModLfoParams {
                rate_hz: 3.0,
                ..ModLfoParams::default()
            }),
        );
        for destination in [ParamAddr::effect(target, device, 0), ParamAddr::strip(target, 0)] {
            channel
                .setup
                .modulation
                .add_route(ModRoute::to_slot(0, destination, 0.4, ModPolarity::Bipolar))
                .expect("room in the matrix");
        }
    }
    // A lane on channel 5's first effect.
    {
        let channel = &mut project.channels[5];
        let device = channel.setup.effects[0].id;
        let mut lane = AutomationLane::new(ParamAddr::effect(EffectTarget::Channel(5), device, 0));
        lane.reserve_points();
        lane.reset_points([
            AutomationPoint::new(1, 0, 0.1),
            AutomationPoint::new(2, 192, 0.9),
            AutomationPoint::new(3, 383, 0.2),
        ]);
        channel.automation[0].push(lane);
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

/// A state playing `project`, run far enough that every channel is sounding.
fn playing(project: &Project, samples: &[Option<Arc<SampleData>>]) -> RenderState {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, samples);
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

/// Apply `edit` to `live` the way the handle does, `incoming` being the
/// project it produced.
fn reseat(live: &mut RenderState, edit: ChannelEdit, incoming: &Project) -> Box<ChannelReseat> {
    let bank = bank_after(&live.audio_bank(), edit).expect("seats in the bank");
    let reseat = ChannelReseat::new(edit, incoming, bank, input());
    match live.apply_structural(StructuralCommand::ReseatChannels { reseat }) {
        Some(StructuralReclaim::ChannelsReseated(reseat)) => reseat,
        _ => panic!("a channel edit comes back as ChannelsReseated"),
    }
}

/// Remove `channel` from `live` the way the handle does.
fn remove(live: &mut RenderState, channel: usize, incoming: &Project) -> Box<ChannelReseat> {
    reseat(live, ChannelEdit::Removed(channel as u8), incoming)
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

/// **A removal sounds exactly like the install it replaces.** The same
/// playing song, one copy edited by the command and the other by prepare,
/// carry and swap, renders the same samples for two seconds afterwards.
///
/// Without modulation routes, because a route names its channel's seat: the
/// install finds a renumbered route a changed setup and rebuilds that
/// channel, cutting its voices, where the command keeps it sounding. The
/// next test holds that channel to a song with no edit at all instead.
#[test]
fn removing_a_channel_renders_what_installing_the_result_renders() {
    for (solo, channel) in [(false, 1), (false, 2), (false, 4), (true, 1), (true, 4), (true, 5)] {
        let project = song(solo, false);
        let samples = samples(&project);
        let mut incoming = project.clone();
        incoming.remove_channel(channel).expect("a channel to remove");

        let mut by_command = playing(&project, &samples);
        let removal = remove(&mut by_command, channel, &incoming);
        assert!(removal.applied(), "channel {channel} was refused");
        assert!(removal.carries_departed());
        let mut by_install = install(playing(&project, &samples), &project, &incoming);

        let blocks = (2 * SAMPLE_RATE as usize) / BLOCK;
        assert_same(
            &render(&mut by_command, blocks),
            &render(&mut by_install, blocks),
            &format!("removing channel {channel} (solo {solo}): the command against the install"),
        );
    }
}

/// **A removal is inaudible beyond the channel it removes.** The song with
/// everything a seat names -- a modulation route, a lane, a send, notes held
/// across the edit -- plays the same after the command as a copy that was
/// never edited, the removed channel muted in both from the start.
#[test]
fn a_removal_changes_nothing_but_the_removed_channel() {
    for channel in [0, 1, 3] {
        let mut project = song(false, true);
        project.channels[channel].setup.channel.muted = true;
        let samples = samples(&project);
        let mut incoming = project.clone();
        incoming.remove_channel(channel).expect("a channel to remove");

        let mut by_command = playing(&project, &samples);
        assert!(remove(&mut by_command, channel, &incoming).applied());
        let mut untouched = playing(&project, &samples);

        let blocks = (2 * SAMPLE_RATE as usize) / BLOCK;
        assert_same(
            &render(&mut by_command, blocks),
            &render(&mut untouched, blocks),
            &format!("removing muted channel {channel}: the command against no edit"),
        );
    }
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

/// **Every other channel keeps its strip**, moved to its new seat: the same
/// box, so its voices, tails and rings are the ones that were sounding.
#[test]
fn a_removal_keeps_every_other_strip() {
    let project = song(false, true);
    let samples = samples(&project);
    let mut live = playing(&project, &samples);
    let before: Vec<usize> = (0..project.channels.len()).map(|seat| live.strip_identity(seat)).collect();
    let mut incoming = project.clone();
    incoming.remove_channel(1).unwrap();

    let removal = remove(&mut live, 1, &incoming);

    assert!(removal.applied());
    assert_eq!(live.strip_count(), project.channels.len() - 1);
    let after: Vec<usize> = (0..live.strip_count()).map(|seat| live.strip_identity(seat)).collect();
    let expected: Vec<usize> = before.iter().enumerate().filter(|&(seat, _)| seat != 1).map(|(_, id)| *id).collect();
    assert_eq!(after, expected, "a surviving channel's strip was replaced");
}

/// **The callback allocates nothing, frees nothing and takes no lock**
/// applying a removal: what leaves goes back in the payload.
#[test]
fn a_removal_allocates_nothing_on_the_callback() {
    assert!(
        mooloop_core::lock_check::counting(),
        "a build without debug assertions counts no locks, so the lock half would pass unchecked"
    );
    let project = song(false, true);
    let samples = samples(&project);
    let mut live = playing(&project, &samples);
    let mut incoming = project.clone();
    incoming.remove_channel(3).unwrap();
    let edit = ChannelEdit::Removed(3);
    let bank = bank_after(&live.audio_bank(), edit).unwrap();
    let reseat = ChannelReseat::new(edit, &incoming, bank, input());
    let command = StructuralCommand::ReseatChannels { reseat };

    let locks = mooloop_core::lock_check::locks_taken();
    let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    let reclaimed = live.apply_structural(command);
    live.process_once_block(BLOCK);
    let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    let locked = mooloop_core::lock_check::locks_taken() - locks;

    assert_eq!(after, before, "the removal allocated or freed on the callback");
    assert_eq!(locked, 0, "the removal took a lock on the callback");
    match reclaimed {
        Some(StructuralReclaim::ChannelsReseated(removal)) => {
            assert!(removal.applied() && removal.carries_departed());
        }
        _ => panic!("the departed channel did not come back in the payload"),
    }
}

/// **A removal that cannot land changes nothing**: past the last channel,
/// or the last channel left, the payload comes back as it went.
#[test]
fn a_refused_removal_changes_nothing() {
    let mut one = Project::default();
    one.channels.truncate(1);
    one.assign_channel_ids();
    let mut live = playing(&one, &[]);
    let identity = live.strip_identity(0);
    for channel in [0, 5] {
        let removal = remove(&mut live, channel, &one);
        assert!(!removal.applied(), "channel {channel} of a one-channel song was removed");
        assert!(!removal.carries_departed());
        assert_eq!(live.strip_count(), 1);
        assert_eq!(live.strip_identity(0), identity);
    }
}

/// **A channel added after a removal takes the vacated seat** with storage
/// of its own, and plays: nothing of the departed channel is reused.
#[test]
fn a_channel_added_after_a_removal_plays_in_the_vacated_seat() {
    let project = song(false, true);
    let samples = samples(&project);
    let mut live = playing(&project, &samples);
    let mut incoming = project.clone();
    incoming.remove_channel(0).unwrap();
    let departed = live.strip_identity(0);
    // Held until the comparison: freed, the departed strip's address is the
    // allocator's to hand straight to the arriving one, and the identity
    // check would compare addresses rather than strips.
    let removal = remove(&mut live, 0, &incoming);
    assert!(removal.carries_departed());

    let seat = live.strip_count();
    let storage = RenderState::build_channel(live.audio_bank()[seat].clone(), DeviceKind::PolySynth, SAMPLE_RATE);
    let returned = live.apply_structural(StructuralCommand::AddChannel { storage });

    assert!(returned.is_none(), "the arriving storage came straight back");
    assert_eq!(live.strip_count(), seat + 1);
    assert_ne!(live.strip_identity(seat), departed);
    assert_eq!(live.channel_source(seat).kind(), DeviceKind::PolySynth);
    drop(removal);
}


/// The moves the tests below make: forwards and back, by one seat and across
/// the song, past the routed channel (4) and onto it.
const MOVES: [(usize, usize); 6] = [(0, 5), (5, 0), (1, 2), (4, 1), (2, 4), (3, 4)];

/// Move `from` to `to` in `live` the way the handle does.
fn move_to(live: &mut RenderState, from: usize, to: usize, incoming: &Project) -> Box<ChannelReseat> {
    reseat(
        live,
        ChannelEdit::Moved {
            from: from as u8,
            to: to as u8,
        },
        incoming,
    )
}

/// **A move sounds exactly like the install it replaces**, solo or not.
/// Without routes, for the reason the removal's twin gives: the install
/// rebuilds a channel whose route was renumbered.
#[test]
fn moving_a_channel_renders_what_installing_the_result_renders() {
    for solo in [false, true] {
        for (from, to) in MOVES {
            let project = song(solo, false);
            let samples = samples(&project);
            let mut incoming = project.clone();
            incoming.move_channel(from, to).expect("a channel to move");

            let mut by_command = playing(&project, &samples);
            let moved = move_to(&mut by_command, from, to, &incoming);
            assert!(moved.applied(), "{from} -> {to} was refused");
            assert!(!moved.carries_departed(), "a move took a channel out");
            let mut by_install = install(playing(&project, &samples), &project, &incoming);

            let blocks = (2 * SAMPLE_RATE as usize) / BLOCK;
            assert_same(
                &render(&mut by_command, blocks),
                &render(&mut by_install, blocks),
                &format!("moving {from} -> {to} (solo {solo}): the command against the install"),
            );
        }
    }
}

/// **A move is inaudible.** The song with everything a seat names -- the
/// LFO routed on channel 4, a lane on channel 5, a send, channel 4 soloed or
/// not, a note held on every channel across the edit -- plays the same after
/// the command as a copy that was never edited. The moved channel has a track
/// to itself, so every track sums its channels in the same order either way
/// and the comparison can be bit for bit.
#[test]
fn a_move_changes_nothing_audible() {
    for solo in [false, true] {
        for (from, to) in MOVES {
            let mut project = song(solo, true);
            project.ensure_tracks(4);
            project.channels[from].setup.channel.bus = 3;
            let samples = samples(&project);
            let mut incoming = project.clone();
            incoming.move_channel(from, to).expect("a channel to move");

            let mut by_command = playing(&project, &samples);
            assert!(move_to(&mut by_command, from, to, &incoming).applied());
            let mut untouched = playing(&project, &samples);

            let blocks = (2 * SAMPLE_RATE as usize) / BLOCK;
            assert_same(
                &render(&mut by_command, blocks),
                &render(&mut untouched, blocks),
                &format!("moving {from} -> {to} (solo {solo}): the command against no edit"),
            );
        }
    }
}

/// **Every channel keeps its strip across a move**, the routed one included
/// with its note still held: the same boxes, reordered as the channels were.
#[test]
fn a_move_keeps_every_strip() {
    for (from, to) in MOVES {
        let project = song(false, true);
        let samples = samples(&project);
        let mut live = playing(&project, &samples);
        let mut expected: Vec<usize> =
            (0..project.channels.len()).map(|seat| live.strip_identity(seat)).collect();
        let lifted = expected.remove(from);
        expected.insert(to, lifted);
        let mut incoming = project.clone();
        incoming.move_channel(from, to).unwrap();

        assert!(move_to(&mut live, from, to, &incoming).applied());

        let after: Vec<usize> = (0..live.strip_count()).map(|seat| live.strip_identity(seat)).collect();
        assert_eq!(after, expected, "{from} -> {to}: a strip was replaced or misplaced");
    }
}

/// **The install a move used to be rebuilds the routed channel** where the
/// command keeps it: the reason the test above holds the command to a song
/// with no edit rather than to the install. When this starts failing, the
/// install has learned to carry a renumbered route, and the routed song can
/// join the install parity test.
#[test]
fn the_install_still_rebuilds_a_channel_whose_route_was_renumbered() {
    let project = song(false, true);
    let samples = samples(&project);
    let live = playing(&project, &samples);
    let routed = live.strip_identity(4);
    let mut incoming = project.clone();
    incoming.move_channel(0, 5).unwrap();

    let installed = install(live, &project, &incoming);

    assert_ne!(installed.strip_identity(3), routed, "the install carried the routed channel");
}

/// **The callback allocates nothing, frees nothing and takes no lock**
/// applying a move.
#[test]
fn a_move_allocates_nothing_on_the_callback() {
    let project = song(false, true);
    let samples = samples(&project);
    let mut live = playing(&project, &samples);
    let mut incoming = project.clone();
    let edit = incoming.move_channel(5, 0).unwrap();
    let bank = bank_after(&live.audio_bank(), edit).unwrap();
    let reseat = ChannelReseat::new(edit, &incoming, bank, input());
    let command = StructuralCommand::ReseatChannels { reseat };

    let locks = mooloop_core::lock_check::locks_taken();
    let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    let reclaimed = live.apply_structural(command);
    live.process_once_block(BLOCK);
    let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    let locked = mooloop_core::lock_check::locks_taken() - locks;

    assert_eq!(after, before, "the move allocated or freed on the callback");
    assert_eq!(locked, 0, "the move took a lock on the callback");
    match reclaimed {
        Some(StructuralReclaim::ChannelsReseated(reseat)) => assert!(reseat.applied()),
        _ => panic!("the move's payload did not come back"),
    }
}

/// **A move that cannot land changes nothing**: onto itself, or from or to
/// a seat past the last channel.
#[test]
fn a_refused_move_changes_nothing() {
    let project = song(false, true);
    let samples = samples(&project);
    let mut live = playing(&project, &samples);
    let before: Vec<usize> = (0..live.strip_count()).map(|seat| live.strip_identity(seat)).collect();
    for (from, to) in [(2, 2), (6, 0), (0, 6)] {
        let moved = move_to(&mut live, from, to, &project);
        assert!(!moved.applied(), "{from} -> {to} landed");
        let after: Vec<usize> = (0..live.strip_count()).map(|seat| live.strip_identity(seat)).collect();
        assert_eq!(after, before, "{from} -> {to} moved a strip");
    }
}
