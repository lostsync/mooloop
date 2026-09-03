//! What a structural edit must not silently break.
//!
//! `docs/FOCUS.md` records the bug these exist for: routes and automation
//! lanes named their destination by slot and their channel by index, so any
//! structural edit re-aimed them at whatever slid into the seat.
//! `mooloop_core::structure` states each edit once as a permutation; what had
//! never been asserted is that the *session* runs that permutation over
//! everything it holds that names a position.

use mooloop_core::{
    DeviceKind, EffectKind, EffectTarget, ModPolarity, ModRoute, ModulatorKind, ParamAddr, Project,
    TICKS_PER_STEP,
};
use mooloop_session::session::Session;

/// Two channels, each with a delay then a filter, an LFO routed at the
/// filter's first parameter, and an automation lane on the same address.
fn session_with_routes() -> Session {
    let mut session = Session::default();
    session.add_channel(DeviceKind::Sampler);

    for channel in [0usize, 1] {
        session.selected = channel;
        session.effect_target = EffectTarget::Channel(channel as u8);
        session.insert_effect_at(EffectKind::Delay, 0).expect("room");
        session.insert_effect_at(EffectKind::Filter, 1).expect("room");
        session
            .add_modulation_source(ModulatorKind::Lfo)
            .expect("an empty rack has a free slot");

        let destination = filter_param(channel as u8, 1);
        session.channels[channel]
            .modulation
            .add_route(ModRoute::to_slot(0, destination, 0.5, ModPolarity::Bipolar))
            .expect("the matrix is empty");
        session.automation_target.set(Some(destination));
        session
            .open_automation_lane_at(destination)
            .expect("the destination exists");
    }
    // Leave the roll showing channel 0's lane: `automation_target` is a
    // single open lane, not one per channel, so which one it names matters.
    session.selected = 0;
    session.effect_target = EffectTarget::Channel(0);
    session
        .open_automation_lane_at(filter_param(0, 1))
        .expect("channel 0's filter exists");
    session
}

/// The address of the filter's first parameter in `slot` on `channel`.
fn filter_param(channel: u8, slot: u8) -> ParamAddr {
    let descriptors = EffectKind::Filter.descriptors();
    ParamAddr::effect(
        EffectTarget::Channel(channel),
        slot,
        descriptors[0].id,
    )
}

/// A device-scoped address as the pair that a structural edit can move.
type SlotParam = (u8, u32);

/// Every route and lane on `channel`, by the slot and parameter they name.
fn addresses(session: &Session, channel: usize) -> (Vec<SlotParam>, Vec<SlotParam>) {
    let routes = session.channels[channel]
        .modulation
        .routes
        .iter()
        .flatten()
        .filter_map(|route| slot_and_param(route.destination))
        .collect();
    let lanes = session.channels[channel].automation[0]
        .iter()
        .filter_map(|lane| slot_and_param(lane.target))
        .collect();
    (routes, lanes)
}

fn slot_and_param(address: ParamAddr) -> Option<SlotParam> {
    match address.owner {
        mooloop_core::ParamOwner::Effect { slot } => Some((slot, address.param)),
        _ => None,
    }
}

/// Reordering the chain moves the device; the route and the lane have to
/// follow the *device*, not stay on the slot number it used to occupy.
#[test]
fn reordering_effects_carries_routes_and_lanes_with_the_device() {
    let mut session = session_with_routes();
    let (routes_before, lanes_before) = addresses(&session, 0);
    assert_eq!(routes_before, vec![(1, lanes_before[0].1)]);

    // Filter moves from slot 1 to slot 0.
    session.move_effect_to(1, 0).expect("both slots occupied");

    let (routes, lanes) = addresses(&session, 0);
    assert_eq!(
        routes,
        vec![(0, routes_before[0].1)],
        "the route stayed on the old slot number"
    );
    assert_eq!(
        lanes,
        vec![(0, lanes_before[0].1)],
        "the automation lane stayed on the old slot number"
    );
    assert_eq!(
        session.automation_target.get().and_then(slot_and_param),
        Some((0, lanes_before[0].1)),
        "the open lane is still showing the parameter it was showing"
    );
}

/// Removing the device a route names must drop that route, not leave it
/// pointing at whatever moves up into the slot.
#[test]
fn removing_an_effect_drops_what_named_it_and_renumbers_the_rest() {
    let mut session = session_with_routes();
    // A second route, aimed at the delay in slot 0, so there is something
    // that must survive alongside the one that must not.
    let delay_param = ParamAddr::effect(
        EffectTarget::Channel(0),
        0,
        EffectKind::Delay.descriptors()[0].id,
    );
    session.channels[0]
        .modulation
        .add_route(ModRoute::to_slot(0, delay_param, 0.25, ModPolarity::Bipolar))
        .expect("the matrix has room");

    // Drop the delay, so the filter slides from slot 1 to slot 0.
    session.remove_effect_at(0).expect("slot 0 is occupied");

    let (routes, lanes) = addresses(&session, 0);
    assert_eq!(
        routes.len(),
        1,
        "the route naming the removed device survived: {routes:?}"
    );
    assert_eq!(routes[0].0, 0, "the surviving route was not renumbered");
    assert_eq!(lanes, vec![(0, routes[0].1)]);
}

/// A bus chain can be automated from any channel's clip, so a bus-side edit
/// has to run over every channel rather than the selected one.
#[test]
fn a_bus_chain_edit_retargets_every_channels_lanes() {
    let mut session = Session::default();
    session.add_channel(DeviceKind::Sampler);
    session.select_bus(1).expect("bus 1 exists");
    session.insert_effect_at(EffectKind::Delay, 0).expect("room");
    session.insert_effect_at(EffectKind::Filter, 1).expect("room");

    let bus_filter = ParamAddr::effect(
        EffectTarget::Bus(1),
        1,
        EffectKind::Filter.descriptors()[0].id,
    );
    for channel in [0usize, 1] {
        session.selected = channel;
        session.automation_target.set(Some(bus_filter));
        session
            .open_automation_lane_at(bus_filter)
            .expect("the destination exists");
    }

    session.move_effect_to(1, 0).expect("both slots occupied");

    for channel in [0usize, 1] {
        let lanes: Vec<_> = session.channels[channel].automation[0]
            .iter()
            .filter_map(|lane| slot_and_param(lane.target))
            .collect();
        assert_eq!(
            lanes.first().map(|(slot, _)| *slot),
            Some(0),
            "channel {channel}'s lane on the bus chain was not retargeted"
        );
    }
}

/// Installing a document is what a channel delete ultimately does, and the
/// selection has to land on something that exists afterwards.
#[test]
fn installing_a_shorter_document_leaves_the_selection_somewhere_real() {
    let mut session = Session::default();
    session.add_channel(DeviceKind::Sampler);
    session.add_channel(DeviceKind::Sampler);
    let note = session.channels[2].create_note(0, 0, TICKS_PER_STEP, 60);
    session.select_note(Some(note.id));
    assert_eq!(session.selected, 2);

    // Drop the last channel, the way a delete does.
    let mut project = session.project_snapshot(120, 50);
    project.remove_channel(2).expect("channel 2 exists");
    project.selected_channel = 2usize.min(project.channels.len() - 1) as u8;
    let samples = vec![None; project.channels.len()];
    session.replace_project(&project, &samples);

    assert_eq!(session.channels.len(), 2);
    assert!(
        session.selected < session.channels.len(),
        "the selection points past the end of the rack"
    );
    assert_eq!(
        session.effect_target,
        EffectTarget::Channel(session.selected as u8),
        "the device rack is pointed at a channel that is not selected"
    );
    assert_eq!(
        session.selected_note_id, None,
        "a note selection survived the document it belonged to"
    );
    assert!(session.selected_note_ids.is_empty());
}

/// Open, edit, save, reopen: `mooloop-project` tests the format, but nothing
/// tested that the session puts back what it took out.
#[test]
fn a_document_round_trips_through_the_session() {
    let temp = tempfile::tempdir().expect("a temp dir");
    let path = temp.path().join("round-trip.mooloop");

    let mut session = session_with_routes();
    session.channels[0].name = "Kick".into();
    session.set_pattern_length(8);
    session.toggle_step(0, 3).expect("cell 3 exists");
    session.set_channel_volume(0, 0.42).expect("channel 0");

    let saved = session.project_snapshot(137, 62);
    mooloop_project::save_song(&path, &saved, mooloop_project::AssetMode::Referenced)
        .expect("the document saves");

    let report = mooloop_project::load_bundle(&path).expect("the document reopens");
    let mooloop_project::LoadedDocument::Song(reloaded) = report.document else {
        panic!("a song came back as something else");
    };

    let mut reopened = Session::default();
    let samples = vec![None; reloaded.channels.len()];
    reopened.replace_project(&reloaded, &samples);

    assert_eq!(reopened.channels.len(), session.channels.len());
    assert_eq!(reopened.channels[0].name, "Kick");
    assert_eq!(reopened.pattern_lengths[0], 8);
    assert!((reopened.channels[0].volume - 0.42).abs() < 1.0e-6);
    assert_eq!(
        reopened.channels[0].notes[0].len(),
        1,
        "the note written into the step grid did not come back"
    );
    assert_eq!(
        reopened.channels[0].notes[0][0].start_tick,
        3 * TICKS_PER_STEP
    );
    assert_eq!(
        addresses(&reopened, 0),
        addresses(&session, 0),
        "routes or automation lanes changed address across a save and reload"
    );

    // The second snapshot is what the first one said, which is the property a
    // round trip actually has to have.
    let resaved = reopened.project_snapshot(137, 62);
    assert_eq!(resaved.channels.len(), saved.channels.len());
    assert_eq!(resaved.pattern_lengths, saved.pattern_lengths);
    assert_eq!(resaved.bpm, saved.bpm);
    assert_eq!(resaved.swing_percent, saved.swing_percent);
}

/// A default document is the one every new session starts from; if it does not
/// survive its own round trip, nothing else will.
#[test]
fn the_starting_document_round_trips() {
    let temp = tempfile::tempdir().expect("a temp dir");
    let path = temp.path().join("starter.mooloop");
    let session = Session::default();
    let saved = session.project_snapshot(120, 50);

    mooloop_project::save_song(&path, &saved, mooloop_project::AssetMode::Referenced)
        .expect("the starter saves");
    let report = mooloop_project::load_bundle(&path).expect("the starter reopens");
    let mooloop_project::LoadedDocument::Song(reloaded) = report.document else {
        panic!("a song came back as something else");
    };
    assert_eq!(reloaded.channels.len(), saved.channels.len());
    assert_eq!(reloaded.pattern_lengths, saved.pattern_lengths);
    assert_eq!(Project::default().ppq, reloaded.ppq);
}
