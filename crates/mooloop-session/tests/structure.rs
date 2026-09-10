//! What a structural edit must not silently break.
//!
//! `docs/FOCUS.md` records the bug these exist for: routes and automation
//! lanes named their destination by slot and their channel by index, so any
//! structural edit re-aimed them at whatever slid into the seat.
//!
//! Since `docs/plans/containers/01`, a route and a lane name a `DeviceId`,
//! so a reorder or an insert is not an event either of them can observe.
//! These tests therefore assert something stronger than "the permutation ran
//! correctly": that the addresses **do not change at all**, which is the only
//! way to tell that apart from a remap that happens to be right. A channel is
//! still a position, and `ChannelEdit` still has to run over one.

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

        let destination = filter_param(&session, channel);
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
    let filter = filter_param(&session, 0);
    session
        .open_automation_lane_at(filter)
        .expect("channel 0's filter exists");
    session
}

/// The address of the first parameter of `channel`'s filter, by the filter's
/// identity rather than by where it currently sits.
fn filter_param(session: &Session, channel: usize) -> ParamAddr {
    device_param(session, channel, EffectKind::Filter)
}

fn device_param(session: &Session, channel: usize, kind: EffectKind) -> ParamAddr {
    let device = session.channels[channel]
        .effects
        .iter()
        .find(|effect| effect.kind() == kind)
        .expect("the chain holds one")
        .id;
    ParamAddr::effect(
        EffectTarget::Channel(channel as u8),
        device,
        kind.descriptors()[0].id,
    )
}

/// A device-scoped address as the pair a structural edit used to be able to
/// move, and now cannot.
type SlotParam = (DeviceId, u32);

/// Every route and lane on `channel`, by the device and parameter they name.
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
        mooloop_core::ParamOwner::Effect { device } => Some((device, address.param)),
        _ => None,
    }
}

/// Reordering the chain moves the device. Nothing else moves -- and that is
/// what is asserted: the addresses before and after are the same values, not
/// merely values that still resolve to the same device.
#[test]
fn reordering_effects_leaves_every_address_untouched() {
    let mut session = session_with_routes();
    let before = addresses(&session, 0);
    let shown_before = session.automation_target.get();
    let filter = filter_param(&session, 0);

    // Filter moves from slot 1 to slot 0.
    session.move_effect_to(1, 0).expect("both slots occupied");

    assert_eq!(
        addresses(&session, 0),
        before,
        "a reorder changed an address that names a device"
    );
    assert_eq!(
        session.automation_target.get(),
        shown_before,
        "the open lane's address moved under a reorder"
    );
    // And it still names the filter, which is now in slot 0.
    assert_eq!(
        mooloop_core::slot_of(&session.channels[0].effects, filter),
        Some(0)
    );

    // An insert above is equally invisible.
    session.insert_effect_at(EffectKind::Gate, 0).expect("room");
    assert_eq!(addresses(&session, 0), before, "an insert moved an address");
    assert_eq!(
        mooloop_core::slot_of(&session.channels[0].effects, filter),
        Some(1),
        "the filter did not shift down for the inserted gate"
    );
}

/// The property stated where a musician would notice it: the *saved bytes* of
/// a project's routes and lanes are identical across a reorder and an insert.
///
/// This is the assertion the slot scheme could not pass by construction, and
/// it is what the whole step buys -- an edit to the rack is no longer an edit
/// to the document's addresses.
#[test]
fn a_reorder_does_not_change_one_byte_of_the_saved_addresses() {
    fn saved_addresses(session: &Session) -> String {
        let project = session.project_snapshot(120, 50);
        let mut out = String::new();
        for channel in &project.channels {
            for route in channel.setup.modulation.routes.iter().flatten() {
                out.push_str(&format!("{:?}\n", route.destination));
            }
            for lanes in &channel.automation {
                for lane in lanes {
                    out.push_str(&format!("{:?}\n", lane.target));
                }
            }
        }
        out
    }

    let mut session = session_with_routes();
    let before = saved_addresses(&session);
    assert!(!before.is_empty(), "the fixture wrote no addresses at all");

    session.move_effect_to(1, 0).expect("both slots occupied");
    session.insert_effect_at(EffectKind::Gate, 0).expect("room");
    session.move_effect_to(0, 2).expect("in range");

    assert_eq!(saved_addresses(&session), before);
}

/// Removing the device a route names must drop that route. Everything else is
/// left exactly where it was -- there is no renumbering to do.
#[test]
fn removing_an_effect_drops_what_named_it_and_disturbs_nothing_else() {
    let mut session = session_with_routes();
    // A second route, aimed at the delay, so there is something that must
    // survive alongside the one that must not.
    let delay_param = device_param(&session, 0, EffectKind::Delay);
    let filter_address = filter_param(&session, 0);
    session.channels[0]
        .modulation
        .add_route(ModRoute::to_slot(0, delay_param, 0.25, ModPolarity::Bipolar))
        .expect("the matrix has room");

    // Drop the *filter*, which is what both the original route and the lane
    // name, and which sits above the delay.
    let slot = mooloop_core::slot_of(&session.channels[0].effects, filter_address)
        .expect("the filter is on the chain");
    session.remove_effect_at(slot).expect("the slot is occupied");

    let (routes, lanes) = addresses(&session, 0);
    assert_eq!(
        routes,
        vec![(delay_param.device().expect("an effect address"), delay_param.param)],
        "the route naming the removed device survived, or the delay's did not: {routes:?}"
    );
    assert!(
        lanes.is_empty(),
        "the lane on the removed device outlived it: {lanes:?}"
    );
    assert_eq!(
        session.automation_target.get(),
        None,
        "the roll is still showing a lane whose device is gone"
    );
}

/// A bus chain can be automated from any channel's clip. Under the slot
/// scheme a bus-side edit had to walk every channel to renumber their lanes;
/// now it has to walk them only to *drop* lanes on a removed device, and a
/// reorder must leave all of them alone.
#[test]
fn a_bus_chain_reorder_leaves_every_channels_lanes_alone() {
    let mut session = Session::default();
    session.add_channel(DeviceKind::Sampler);
    session.add_track().expect("room for a track");
    session.select_bus(1).expect("track 1 exists");
    session.insert_effect_at(EffectKind::Delay, 0).expect("room");
    let filter = session
        .insert_effect_at(EffectKind::Filter, 1)
        .expect("room")
        .device;

    let bus_filter = ParamAddr::effect(
        EffectTarget::Bus(1),
        filter,
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
            .map(|lane| lane.target)
            .collect();
        assert_eq!(
            lanes,
            vec![bus_filter],
            "channel {channel}'s lane on the bus chain was disturbed by a reorder"
        );
    }

    // Removing it does reach every channel.
    session.remove_effect_at(0).expect("the filter is in slot 0");
    for channel in [0usize, 1] {
        assert!(
            session.channels[channel].automation[0].is_empty(),
            "channel {channel} kept a lane on a device that is gone"
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

/// The session half of a channel edit, which `Project::rescope_after` cannot
/// reach because none of it is in the document.
///
/// Six things here are keyed by a channel index or by an `EffectTarget`
/// holding one, and all six were wrong after *any* structural channel edit
/// until `Session::rescope_after` existed -- an insert and a delete have been
/// mis-keying them for as long as they have existed. The reorder is only the
/// first edit that makes it visible, because it is the first one performed
/// while looking at the device the labels belong to.
#[test]
fn the_session_follows_a_channel_move_too() {
    let mut session = Session::default();
    session.add_channel(DeviceKind::Sampler);
    session.add_channel(DeviceKind::Sampler);
    session.add_channel(DeviceKind::Sampler);

    let third = EffectTarget::Channel(3);
    let device = DeviceId(7);
    session.effect_target = third;
    session.selected_device = Some((third, device));
    session.selected_source = Some(third);
    session
        .automation_target
        .set(Some(ParamAddr::strip(third, mooloop_core::STRIP_PARAM_VOLUME)));
    session.set_effect_preset_name(third, device, "Wide Plate");
    session.set_source_preset_name(3, "Deep Kick");
    // A bus-scoped label, which must be left exactly where it is: a bus
    // exists independently of which channels feed it.
    let bus = EffectTarget::Bus(2);
    session.set_effect_preset_name(bus, device, "Glue");

    session.rescope_after(mooloop_core::ChannelEdit::Moved { from: 3, to: 0 });

    let first = EffectTarget::Channel(0);
    assert_eq!(session.selected_device, Some((first, device)));
    assert_eq!(session.selected_source, Some(first));
    assert_eq!(
        session.automation_target.get(),
        Some(ParamAddr::strip(first, mooloop_core::STRIP_PARAM_VOLUME)),
        "the open automation lane stayed on the seat rather than the channel"
    );
    assert_eq!(
        session.effect_preset_name(first, device),
        Some("Wide Plate"),
        "a rack row's preset label did not follow its channel"
    );
    assert_eq!(session.effect_preset_name(third, device), None);
    assert_eq!(session.source_preset_name(0), Some("Deep Kick"));
    assert_eq!(session.source_preset_name(3), None);
    assert_eq!(session.effect_preset_name(bus, device), Some("Glue"));

    // A removal is the same walk with a different answer: what named the
    // departed channel is dropped rather than moved.
    session.rescope_after(mooloop_core::ChannelEdit::Removed(0));
    assert_eq!(session.selected_device, None);
    assert_eq!(session.selected_source, None);
    assert_eq!(session.automation_target.get(), None);
    assert_eq!(session.effect_preset_name(first, device), None);
    assert_eq!(session.source_preset_name(0), None);
    assert_eq!(
        session.effect_preset_name(bus, device),
        Some("Glue"),
        "a bus label was dropped by a channel removal"
    );
}
