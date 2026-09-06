//! What a structural edit must not silently break.
//!
//! The bug these exist for: routes and automation lanes named their
//! destination by slot and their channel by index, so any structural edit
//! re-aimed them at whatever slid into the seat. A rack row now carries a
//! durable `DeviceId` minted when the device is added, and every saved
//! address names *that*, so a reorder is no longer an addressing event at
//! all -- these assert that a route, a lane and the open lane come out of an
//! edit meaning the same knob, and that a removal is the one edit that still
//! reaches them.

use mooloop_core::{
    DeviceId, DeviceKind, EffectKind, EffectTarget, ModPolarity, ModRoute, ModulatorKind,
    ParamAddr, Project, TICKS_PER_STEP,
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

        let filter = session.device_at(EffectTarget::Channel(channel as u8), 1).expect("a filter");
        let destination = filter_param(channel as u8, filter);
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
    let filter = session
        .device_at(EffectTarget::Channel(0), 1)
        .expect("a filter");
    session
        .open_automation_lane_at(filter_param(0, filter))
        .expect("channel 0's filter exists");
    session
}

/// The address of `device`'s first filter parameter on `channel`.
fn filter_param(channel: u8, device: DeviceId) -> ParamAddr {
    let descriptors = EffectKind::Filter.descriptors();
    ParamAddr::effect(EffectTarget::Channel(channel), device, descriptors[0].id)
}

/// A device-scoped address as the pair a saved route or lane actually holds.
type DeviceParam = (DeviceId, u32);

/// Every route and lane on `channel`, by the device and parameter they name.
fn addresses(session: &Session, channel: usize) -> (Vec<DeviceParam>, Vec<DeviceParam>) {
    let routes = session.channels[channel]
        .modulation
        .routes
        .iter()
        .flatten()
        .filter_map(|route| device_and_param(route.destination))
        .collect();
    let lanes = session.channels[channel].automation[0]
        .iter()
        .filter_map(|lane| device_and_param(lane.target))
        .collect();
    (routes, lanes)
}

fn device_and_param(address: ParamAddr) -> Option<DeviceParam> {
    match address.owner {
        mooloop_core::ParamOwner::Effect { device } => Some((device, address.param)),
        _ => None,
    }
}

/// Reordering the chain moves the device and moves *nothing else*. The route,
/// the lane and the open lane all named the device, so the edit is not an
/// addressing event: every address comes out of it byte for byte what it went
/// in as, still resolving to the filter.
#[test]
fn reordering_effects_leaves_every_address_alone() {
    let mut session = session_with_routes();
    let target = EffectTarget::Channel(0);
    let filter = session.device_at(target, 1).expect("a filter in slot 1");
    let before = addresses(&session, 0);
    let shown_before = session.automation_target.get();
    assert_eq!(before.0, vec![(filter, before.1[0].1)]);

    // Filter moves from slot 1 to slot 0.
    session.move_effect_to(1, 0).expect("both slots occupied");

    assert_eq!(
        addresses(&session, 0),
        before,
        "a reorder rewrote an address that named a device"
    );
    assert_eq!(
        session.automation_target.get(),
        shown_before,
        "the open lane was re-aimed by a reorder"
    );
    assert_eq!(
        session.device_slot(target, filter),
        Some(0),
        "the filter is where the drag put it"
    );
}

/// The other half of the same property, and the one position-as-identity got
/// wrong most often: inserting above a device renumbers its row and changes
/// nothing about what names it.
#[test]
fn inserting_above_a_device_does_not_touch_what_names_it() {
    let mut session = session_with_routes();
    let target = EffectTarget::Channel(0);
    let filter = session.device_at(target, 1).expect("a filter in slot 1");
    let before = addresses(&session, 0);

    session.insert_effect_at(EffectKind::Drive, 0).expect("room");

    assert_eq!(session.device_slot(target, filter), Some(2));
    assert_eq!(
        addresses(&session, 0),
        before,
        "an insert re-aimed a route or a lane"
    );
    // And the newcomer did not inherit anything: the identity it was minted
    // is one nothing has ever named.
    let inserted = session.device_at(target, 0).expect("the drive");
    assert!(!before.0.iter().any(|(device, _)| *device == inserted));
}

/// Removing the device a route names must drop that route, not leave it
/// pointing at whatever moves up into the slot.
#[test]
fn removing_an_effect_drops_what_named_it_and_leaves_the_rest_alone() {
    let mut session = session_with_routes();
    let target = EffectTarget::Channel(0);
    let delay = session.device_at(target, 0).expect("a delay in slot 0");
    let filter = session.device_at(target, 1).expect("a filter in slot 1");
    // A second route, aimed at the delay, so there is something that must go
    // alongside the one that must stay.
    let delay_param = ParamAddr::effect(target, delay, EffectKind::Delay.descriptors()[0].id);
    session.channels[0]
        .modulation
        .add_route(ModRoute::to_slot(0, delay_param, 0.25, ModPolarity::Bipolar))
        .expect("the matrix has room");

    // Drop the delay, so the filter slides from slot 1 to slot 0.
    session.remove_effect_at(0).expect("slot 0 is occupied");

    let (routes, lanes) = addresses(&session, 0);
    assert_eq!(
        routes,
        vec![(filter, lanes[0].1)],
        "the route naming the removed device survived, or the survivor moved"
    );
    assert_eq!(lanes, vec![(filter, routes[0].1)]);
    assert_eq!(session.device_slot(target, filter), Some(0));
    assert_eq!(session.device_slot(target, delay), None);
}

/// A bus chain can be automated from any channel's clip, so a bus-side edit
/// has to run over every channel rather than the selected one.
#[test]
fn a_bus_chain_removal_reaches_every_channels_lanes() {
    let mut session = Session::default();
    session.add_channel(DeviceKind::Sampler);
    session.select_bus(1).expect("bus 1 exists");
    session.insert_effect_at(EffectKind::Delay, 0).expect("room");
    session.insert_effect_at(EffectKind::Filter, 1).expect("room");
    let bus = EffectTarget::Bus(1);
    let filter = session.device_at(bus, 1).expect("a filter");

    let bus_filter = ParamAddr::effect(bus, filter, EffectKind::Filter.descriptors()[0].id);
    for channel in [0usize, 1] {
        session.selected = channel;
        session.automation_target.set(Some(bus_filter));
        session
            .open_automation_lane_at(bus_filter)
            .expect("the destination exists");
    }

    // A reorder is nothing to any of them.
    session.move_effect_to(1, 0).expect("both slots occupied");
    for channel in [0usize, 1] {
        let lanes: Vec<_> = session.channels[channel].automation[0]
            .iter()
            .filter_map(|lane| device_and_param(lane.target))
            .collect();
        assert_eq!(
            lanes.first().map(|(device, _)| *device),
            Some(filter),
            "channel {channel}'s lane on the bus chain was re-aimed by a reorder"
        );
    }

    // The removal is, and it has to reach every channel: a bus chain can be
    // automated from any clip.
    session.remove_effect_at(0).expect("the filter is in slot 0");
    for channel in [0usize, 1] {
        assert!(
            session.channels[channel].automation[0]
                .iter()
                .all(|lane| device_and_param(lane.target).is_none()),
            "channel {channel} kept a lane on a device that left the bus chain"
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
