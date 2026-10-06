//! The Assign gesture reaches every knob in the song (song modulation step
//! 03): one module, armed once, routed by the ordinary drag to a knob on two
//! different channels, a track's insert, a track's fader and the master's.
//!
//! "Moves" is checked the way the knobs see it: the route is in the set the
//! engine is sent, filed under the chain it lands on, and with the module's
//! output read off the engine the knob draws an offset. That the engine then
//! moves a track's insert and fader under such a route is
//! `song_modulation_tests.rs`'s, in the engine crate.

use mooloop_core::{
    DeviceKind, EffectKind, EffectTarget, InputSource, ModulatorKind, ParamAddr,
    FILTER_PARAM_CUTOFF_HZ, MASTER_BUS, STRIP_DESCRIPTORS, STRIP_PARAM_PAN, STRIP_PARAM_VOLUME,
};
use mooloop_session::session::{ArmedRoute, Session};

/// Two channels and a second track, with an LFO added on the first channel
/// and armed.
fn armed_session() -> Session {
    let mut session = Session::default();
    session.add_channel(DeviceKind::Sampler).expect("room for a channel");
    session.add_track().expect("room for a track");
    session.selected = 0;
    session.effect_target = EffectTarget::Channel(0);
    assert!(session.add_modulation_source(ModulatorKind::Lfo));
    assert!(session.toggle_modulation_assignment().is_some(), "the LFO did not arm");
    session
}

/// A filter on `target`'s chain, and the address of its cutoff.
fn filter_on(session: &mut Session, target: EffectTarget) -> ParamAddr {
    session.effect_target = target;
    session.insert_effect_at(EffectKind::Filter, 0).expect("room");
    let chain = session.effect_chain().expect("the chain is there");
    ParamAddr::effect(target, chain[0].id, FILTER_PARAM_CUTOFF_HZ)
}

/// Assign `destination`, then check that the set the engine is sent files
/// the route under its chain and that the knob draws the module's output.
fn assign_and_check(session: &mut Session, destination: ParamAddr) {
    let ArmedRoute::Added(route) = session.arm_modulation_route(destination, 0.5) else {
        panic!("{destination:?} refused the armed LFO");
    };
    assert_eq!(route.destination, destination);

    let plan = session.modulation_plan();
    assert!(
        plan.chain_routes(destination.scope)
            .iter()
            .any(|route| route.destination == destination),
        "the route onto {destination:?} is not in the set the engine runs"
    );
    session.modulation_sent = plan;
    session.modulation_levels.borrow_mut().modules = vec![0.4];
    let policy = session.modulation_policy(destination).expect("a destination");
    let offset = session.live_offset(destination, &policy);
    assert!(offset > 0.0, "the knob on {destination:?} draws no offset: {offset}");
    assert_eq!(session.route_count(destination), 1);
}

#[test]
fn one_module_reaches_a_knob_on_two_channels() {
    let mut session = armed_session();
    let mine = filter_on(&mut session, EffectTarget::Channel(0));
    let theirs = filter_on(&mut session, EffectTarget::Channel(1));
    assign_and_check(&mut session, mine);
    assign_and_check(&mut session, theirs);
    assert_eq!(session.modulation.routes.len(), 2);
}

#[test]
fn one_module_reaches_a_track_insert() {
    let mut session = armed_session();
    let insert = filter_on(&mut session, EffectTarget::Bus(1));
    assign_and_check(&mut session, insert);
}

#[test]
fn one_module_reaches_a_track_fader() {
    let mut session = armed_session();
    assign_and_check(&mut session, ParamAddr::strip(EffectTarget::Bus(1), STRIP_PARAM_VOLUME));
}

#[test]
fn one_module_reaches_the_master() {
    let mut session = armed_session();
    let master = EffectTarget::Bus(MASTER_BUS);
    let insert = filter_on(&mut session, master);
    assign_and_check(&mut session, insert);
    assign_and_check(&mut session, ParamAddr::strip(master, STRIP_PARAM_VOLUME));
    assign_and_check(&mut session, ParamAddr::strip(master, STRIP_PARAM_PAN));
}

/// Arming names the module, not a slot of the selected channel, so moving
/// to another channel keeps the gesture armed and a second drag on the same
/// knob retunes the route rather than adding a twin.
#[test]
fn arming_survives_a_channel_change_and_a_second_drag_retunes() {
    let mut session = armed_session();
    let theirs = filter_on(&mut session, EffectTarget::Channel(1));
    session.select_channel(1).expect("channel 1 is there");
    assign_and_check(&mut session, theirs);
    let ArmedRoute::Added(route) = session.arm_modulation_route(theirs, -0.25) else {
        panic!("the second drag did not retune");
    };
    assert_eq!(route.depth, -0.25);
    assert_eq!(session.modulation.routes.len(), 1);
}

/// The depths the faces draw for the armed source follow the song's routes
/// on any chain.
#[test]
fn the_armed_depths_follow_routes_on_any_chain() {
    let mut session = armed_session();
    let fader = ParamAddr::strip(EffectTarget::Bus(1), STRIP_PARAM_VOLUME);
    let _ = session.arm_modulation_route(fader, 0.3);
    let depths = session.destination_depths(
        session.modulation_armed.get(),
        &STRIP_DESCRIPTORS,
        |param| ParamAddr::strip(EffectTarget::Bus(1), param),
    );
    assert!((depths[STRIP_PARAM_VOLUME as usize] - 0.3).abs() < 1e-6, "{depths:?}");
}

/// The four kinds that hear notes list None and then every channel; Math
/// lists None and then every other module. Each writes the module's input.
#[test]
fn the_input_picker_offers_every_channel_and_every_module() {
    let mut session = armed_session();
    assert!(session.add_modulation_source(ModulatorKind::Envelope));
    assert!(session.add_modulation_source(ModulatorKind::Math));
    let ids: Vec<_> = session.modulation.modules.iter().map(|module| module.id).collect();
    let (lfo, envelope, math) = (ids[0], ids[1], ids[2]);
    let other = session.channels[1].id;

    for id in [lfo, envelope] {
        let options = session.module_input_options(id);
        let inputs: Vec<_> = options.iter().map(|(input, _)| *input).collect();
        assert_eq!(
            inputs,
            vec![
                InputSource::None,
                InputSource::ChannelNotes(session.channels[0].id),
                InputSource::ChannelNotes(other),
            ]
        );
        assert!(session.set_module_input(id, InputSource::ChannelNotes(other)));
        assert_eq!(session.module_input_choice(id).0, 2);
        assert!(!session.set_module_input(id, InputSource::Module(math)), "a module is not a gate");
    }

    let options = session.module_input_options(math);
    let inputs: Vec<_> = options.iter().map(|(input, _)| *input).collect();
    assert_eq!(
        inputs,
        vec![InputSource::None, InputSource::Module(lfo), InputSource::Module(envelope)]
    );
    assert!(session.set_module_input(math, InputSource::Module(envelope)));
    assert_eq!(session.module_input_choice(math), (2, "READS THIS TICK"));
    assert!(!session.set_module_input(math, InputSource::Module(math)), "it read itself");
    assert!(!session.set_module_input(math, InputSource::ChannelNotes(other)));
    assert!(session.set_module_input(math, InputSource::None));
}
