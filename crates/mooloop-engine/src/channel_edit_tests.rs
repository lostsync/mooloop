//! Channel edits as incremental commands: a removal, a move or an insertion
//! (a paste) reaches the graph as one `StructuralCommand` and leaves it as an
//! install of the edited project would (MOO-466).
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
use mooloop_dsp::{ChannelAudioSnapshot, SampleData};

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
        channel.setup.carried_modulation_mut().install(
            0,
            ModulatorParams::Lfo(ModLfoParams {
                rate_hz: 3.0,
                ..ModLfoParams::default()
            }),
        );
        for destination in [ParamAddr::effect(target, device, 0), ParamAddr::strip(target, 0)] {
            channel
                .setup
                .carried_modulation_mut()
                .add_route(ModRoute::to_slot(0, destination, 0.4, ModPolarity::Bipolar))
                .expect("room in the matrix");
        }
        project.lift_channel_modulation();
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

/// What channel `index` of `project` plays, as [`samples`] and the install
/// give it.
fn channel_audio(project: &Project, index: usize) -> ChannelAudioSnapshot {
    let channel = &project.channels[index];
    match channel.setup.source.sampler_state() {
        Some(state) => ChannelAudioSnapshot::for_sampler(Some(tone()), &state.slices, state.keys, Vec::new()),
        None => ChannelAudioSnapshot::default(),
    }
}

/// Apply `edit` to `live` the way the handle does, `incoming` being the
/// project it produced: an inserted channel's fresh slot holds its audio.
fn reseat(live: &mut RenderState, edit: ChannelEdit, incoming: &Project) -> Box<ChannelReseat> {
    let bank = bank_after(&live.audio_bank(), edit).expect("seats in the bank");
    if let ChannelEdit::Inserted(at) = edit {
        if usize::from(at) < incoming.channels.len() {
            let audio = channel_audio(incoming, usize::from(at));
            bank[usize::from(at)].store((!audio.is_empty()).then(|| Arc::new(audio)));
        }
    }
    let reseat = ChannelReseat::new(edit, incoming, bank, input(), SAMPLE_RATE);
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
/// With the LFO routed on channel 4, whose route the removal of an earlier
/// channel renumbers. The install used to find that a changed setup and
/// rebuild the channel, cutting its voices (MOO-487); the song's set is not
/// strip content now, so the install carries it as the command does.
#[test]
fn removing_a_channel_renders_what_installing_the_result_renders() {
    for (solo, channel) in [(false, 1), (false, 2), (false, 4), (true, 1), (true, 4), (true, 5)] {
        let project = song(solo, true);
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
    let reseat = ChannelReseat::new(edit, &incoming, bank, input(), SAMPLE_RATE);
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

/// **A move sounds exactly like the install it replaces**, solo or not,
/// the routed channel included (MOO-487).
#[test]
fn moving_a_channel_renders_what_installing_the_result_renders() {
    for solo in [false, true] {
        for (from, to) in MOVES {
            let project = song(solo, true);
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

/// **The install a move used to be carries the routed channel** (MOO-487).
/// It used to rebuild it, cutting its notes: a channel's routes name its
/// seat, so a move renumbered them and the install found a changed setup.
/// The routes are the song's now, carried apart from the strips.
#[test]
fn the_install_carries_a_channel_whose_route_was_renumbered() {
    let project = song(false, true);
    let samples = samples(&project);
    let live = playing(&project, &samples);
    let routed = live.strip_identity(4);
    let mut incoming = project.clone();
    incoming.move_channel(0, 5).unwrap();

    let installed = install(live, &project, &incoming);

    assert_eq!(installed.strip_identity(3), routed, "the install rebuilt the routed channel");
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
    let reseat = ChannelReseat::new(edit, &incoming, bank, input(), SAMPLE_RATE);
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


/// The pastes the tests below make: a copy of `copied` landing at `at`, at
/// the top, in the middle, onto the routed channel's seat (4) and at the end.
const PASTES: [(usize, usize); 5] = [(0, 0), (3, 2), (4, 4), (5, 4), (2, 6)];

/// `project` with a copy of channel `copied` pasted at `at`, as the session's
/// paste builds it.
fn pasted(project: &Project, copied: usize, at: usize, change: impl FnOnce(&mut ProjectChannel)) -> Project {
    let mut incoming = project.clone();
    let mut copy = project.channels[copied].clone();
    change(&mut copy);
    assert_eq!(incoming.insert_channel(at, copy), Some(at));
    incoming
}

/// Paste into `live` the way the handle does.
fn paste(live: &mut RenderState, at: usize, incoming: &Project) -> Box<ChannelReseat> {
    reseat(live, ChannelEdit::Inserted(at as u8), incoming)
}

/// **A paste sounds exactly like the install it replaces**, solo or not,
/// the pasted channel included: it arrives built as the install builds it,
/// with its notes, lane, chain and audio, and plays from the same place,
/// the routed channel renumbered by it included (MOO-487).
#[test]
fn pasting_a_channel_renders_what_installing_the_result_renders() {
    for solo in [false, true] {
        for (copied, at) in PASTES {
            let project = song(solo, true);
            let samples = samples(&project);
            let incoming = pasted(&project, copied, at, |_| {});

            let mut by_command = playing(&project, &samples);
            let inserted = paste(&mut by_command, at, &incoming);
            assert!(inserted.applied(), "pasting {copied} at {at} was refused");
            assert!(!inserted.carries_departed(), "the paste kept its storage");
            let mut by_install = install(playing(&project, &samples), &project, &incoming);

            let blocks = (2 * SAMPLE_RATE as usize) / BLOCK;
            assert_same(
                &render(&mut by_command, blocks),
                &render(&mut by_install, blocks),
                &format!("pasting {copied} at {at} (solo {solo}): the command against the install"),
            );
        }
    }
}

/// **A paste is inaudible beyond the pasted channel**, the routed one
/// included. The song with everything a seat names -- the LFO routed on
/// channel 4, a lane on channel 5, a send, channel 4 soloed or not, a note
/// held on every channel across the edit -- plays the same after the
/// command as a copy that was never edited, the pasted channel muted and on
/// a track of its own.
#[test]
fn a_paste_changes_nothing_but_the_pasted_channel() {
    for solo in [false, true] {
        for (copied, at) in PASTES {
            let mut project = song(solo, true);
            project.ensure_tracks(4);
            let samples = samples(&project);
            let incoming = pasted(&project, copied, at, |copy| {
                copy.setup.channel.muted = true;
                copy.setup.channel.solo = false;
                copy.setup.channel.bus = 3;
            });

            let mut by_command = playing(&project, &samples);
            assert!(paste(&mut by_command, at, &incoming).applied());
            let mut untouched = playing(&project, &samples);

            let blocks = (2 * SAMPLE_RATE as usize) / BLOCK;
            assert_same(
                &render(&mut by_command, blocks),
                &render(&mut untouched, blocks),
                &format!("pasting {copied} at {at} (solo {solo}): the command against no edit"),
            );
        }
    }
}

/// **Every channel keeps its strip across a paste**, the routed one
/// included with its note still held -- the case an install gets wrong --
/// and the pasted channel has one of its own.
#[test]
fn a_paste_keeps_every_strip() {
    for (copied, at) in PASTES {
        let project = song(false, true);
        let samples = samples(&project);
        let mut live = playing(&project, &samples);
        let before: Vec<usize> =
            (0..project.channels.len()).map(|seat| live.strip_identity(seat)).collect();
        let incoming = pasted(&project, copied, at, |_| {});

        let inserted = paste(&mut live, at, &incoming);

        assert!(inserted.applied());
        assert_eq!(live.strip_count(), project.channels.len() + 1);
        let mut after: Vec<usize> = (0..live.strip_count()).map(|seat| live.strip_identity(seat)).collect();
        let arrived = after.remove(at);
        assert_eq!(after, before, "pasting at {at}: a strip was replaced or misplaced");
        assert!(!before.contains(&arrived), "pasting at {at}: the paste took another channel's strip");
        assert_eq!(
            live.channel_source(at).kind(),
            incoming.channels[at].setup.source.kind(),
            "pasting {copied} at {at}: the arrival plays another instrument"
        );
    }
}

/// **The callback allocates nothing, frees nothing and takes no lock**
/// applying a paste: the arrival was built on the control thread, and the
/// empty lanes it displaced go back in the payload.
#[test]
fn a_paste_allocates_nothing_on_the_callback() {
    let project = song(false, true);
    let samples = samples(&project);
    let mut live = playing(&project, &samples);
    let incoming = pasted(&project, 4, 1, |_| {});
    let edit = ChannelEdit::Inserted(1);
    let bank = bank_after(&live.audio_bank(), edit).unwrap();
    let audio = channel_audio(&incoming, 1);
    bank[1].store((!audio.is_empty()).then(|| Arc::new(audio)));
    let reseat = ChannelReseat::new(edit, &incoming, bank, input(), SAMPLE_RATE);
    let command = StructuralCommand::ReseatChannels { reseat };

    let locks = mooloop_core::lock_check::locks_taken();
    let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    let reclaimed = live.apply_structural(command);
    live.process_once_block(BLOCK);
    let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    let locked = mooloop_core::lock_check::locks_taken() - locks;

    assert_eq!(after, before, "the paste allocated or freed on the callback");
    assert_eq!(locked, 0, "the paste took a lock on the callback");
    match reclaimed {
        Some(StructuralReclaim::ChannelsReseated(reseat)) => {
            assert!(reseat.applied() && !reseat.carries_departed());
        }
        _ => panic!("the paste's payload did not come back"),
    }
}

/// **A paste that cannot land changes nothing** and its storage comes back
/// unused: a seat past the end, a song already full, or a payload built
/// from a song with another number of patterns.
#[test]
fn a_refused_paste_changes_nothing() {
    let project = song(false, true);
    let samples = samples(&project);
    let mut live = playing(&project, &samples);
    let before: Vec<usize> = (0..live.strip_count()).map(|seat| live.strip_identity(seat)).collect();
    let unchanged = |live: &RenderState, what: &str| {
        let after: Vec<usize> = (0..live.strip_count()).map(|seat| live.strip_identity(seat)).collect();
        assert_eq!(after, before, "{what} moved a strip");
    };

    // Past the end: the live song has six channels, so seat 7 is no seat.
    let mut beyond = pasted(&project, 0, 6, |_| {});
    beyond.insert_channel(7, project.channels[1].clone()).unwrap();
    let refused = paste(&mut live, 7, &beyond);
    assert!(!refused.applied(), "a paste past the end landed");
    assert!(refused.carries_departed(), "the refused paste lost its storage");
    unchanged(&live, "a paste past the end");

    // Another number of patterns than the song holds.
    let mut longer = pasted(&project, 0, 2, |_| {});
    longer.pattern_lengths.push(16);
    for channel in &mut longer.channels {
        channel.notes.push(Vec::new());
        channel.automation.push(Vec::new());
    }
    let refused = paste(&mut live, 2, &longer);
    assert!(!refused.applied(), "a paste built for two patterns landed in a song of one");
    assert!(refused.carries_departed());
    unchanged(&live, "a mismatched paste");
}

/// **A full song refuses a paste**, and the bank has no slot to give it.
#[test]
fn a_full_song_refuses_a_paste() {
    let mut full = Project::default();
    full.channels.clear();
    for index in 0..mooloop_core::MAX_CHANNELS {
        full.channels.push(ProjectChannel::drum_synth(index, 1));
    }
    full.assign_channel_ids();
    let mut live = playing(&full, &[]);
    assert_eq!(live.strip_count(), mooloop_core::MAX_CHANNELS);
    let identity = live.strip_identity(0);

    let mut incoming = full.clone();
    incoming.channels.pop();
    incoming.insert_channel(0, full.channels[0].clone()).unwrap();
    let edit = ChannelEdit::Inserted(0);
    let reseat = ChannelReseat::new(edit, &incoming, live.audio_bank(), input(), SAMPLE_RATE);
    match live.apply_structural(StructuralCommand::ReseatChannels { reseat }) {
        Some(StructuralReclaim::ChannelsReseated(reseat)) => {
            assert!(!reseat.applied(), "a full song took a paste");
            assert!(reseat.carries_departed());
        }
        _ => panic!("the paste's payload did not come back"),
    }
    assert_eq!(live.strip_count(), mooloop_core::MAX_CHANNELS);
    assert_eq!(live.strip_identity(0), identity);
}

/// **A pasted plugin instrument takes its own processor in its new seat**,
/// and only its own: the arrival is a placeholder keyed by the slot the
/// paste minted, as an install builds it, so the processor the session's
/// rack opens for that slot goes in, and one for the original's slot does
/// not. The original, moved up a seat, still answers to its own.
#[test]
fn a_pasted_plugin_channel_takes_its_own_processor_in_its_new_seat() {
    use mooloop_core::{ChannelSource, PluginSlotState};

    let mut project = song(false, false);
    let original = project.add_plugin_slot(PluginSlotState::new(crate::plugin_source_tests::fake_ref()));
    project.channels[2].setup.source = ChannelSource::Plugin(original);
    project.channels[2].setup.channel.kind = DeviceKind::Plugin;
    let samples = samples(&project);
    let mut live = playing(&project, &samples);

    let mut incoming = project.clone();
    let minted = incoming.add_plugin_slot(PluginSlotState::new(crate::plugin_source_tests::fake_ref()));
    let mut copy = project.channels[2].clone();
    copy.setup.source = ChannelSource::Plugin(minted);
    assert_eq!(incoming.insert_channel(0, copy), Some(0));
    assert!(paste(&mut live, 0, &incoming).applied());

    let host = |live: &mut RenderState, channel: u8, slot| {
        live.apply_structural(StructuralCommand::HostSourceProcessor {
            channel,
            slot,
            node: Some(crate::plugin_source_tests::fake()),
        })
    };
    assert!(
        matches!(host(&mut live, 0, original), Some(StructuralReclaim::HostedProcessor(_))),
        "the paste took the original's processor"
    );
    assert!(host(&mut live, 0, minted).is_none(), "the paste refused its own processor");
    assert!(host(&mut live, 3, original).is_none(), "the original refused its own processor");
}

/// **A pasted channel's own routes and lanes play as the install's do.** A
/// copy of the routed channel (4) or of the one with a lane (5), pasted where
/// no routed channel is renumbered -- so the install rebuilds nothing it
/// would otherwise carry -- arrives with its LFO driving its own effect and
/// strip, and its lane its own effect, at its new seat, solo or not.
#[test]
fn a_pasted_channels_routes_and_lanes_render_what_the_install_renders() {
    for solo in [false, true] {
        for (copied, at) in [(4, 5), (4, 6), (5, 6)] {
            let project = song(solo, true);
            let samples = samples(&project);
            let incoming = pasted(&project, copied, at, |_| {});

            let mut by_command = playing(&project, &samples);
            assert!(paste(&mut by_command, at, &incoming).applied());
            let mut by_install = install(playing(&project, &samples), &project, &incoming);

            let blocks = (2 * SAMPLE_RATE as usize) / BLOCK;
            assert_same(
                &render(&mut by_command, blocks),
                &render(&mut by_install, blocks),
                &format!("pasting routed {copied} at {at} (solo {solo}): the command against the install"),
            );
        }
    }
}
