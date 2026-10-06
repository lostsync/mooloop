//! What switching a channel's instrument away and back gives the user
//! (MOO-192).
//!
//! The session used to keep one parameter block per device kind, with a
//! comment saying the others were kept "so swapping a device out and back
//! returns to the patch that was there". They never did: a source change
//! resets the kind it switches *to*, and every install rebuilds the channel
//! from a document that holds only the current kind. This pins that, so
//! collapsing the blocks into one is seen to change nothing.

use mooloop_core::DeviceKind;
use mooloop_session::session::Session;

const KINDS: [DeviceKind; 8] = [
    DeviceKind::Sampler,
    DeviceKind::DrumSynth,
    DeviceKind::MonoSynth,
    DeviceKind::PolySynth,
    DeviceKind::MlM1,
    DeviceKind::MlP8,
    DeviceKind::Ds01,
    DeviceKind::AuxIn,
];

/// Some parameter of `kind` moved off its default, and what it now reads.
fn edit(session: &mut Session, kind: DeviceKind) {
    let descriptor = kind
        .descriptors()
        .iter()
        .find(|descriptor| descriptor.max > descriptor.min)
        .expect("every kind has a parameter with a range");
    let value = descriptor.min + (descriptor.max - descriptor.min) * 0.37;
    let selected = session.selected;
    session.channels[selected]
        .set_generator_param(descriptor.id, value)
        .expect("the current kind takes its own parameter");
    assert_ne!(
        session.channels[selected].generator_params(),
        kind.default_generator_params(),
        "{kind:?}: the edit changed nothing, so this test would prove nothing"
    );
}

#[test]
fn switching_away_and_back_returns_the_device_at_its_defaults() {
    for kind in KINDS {
        for other in KINDS.into_iter().filter(|other| *other != kind) {
            let mut session = Session::default();
            session.change_selected_source(kind);
            edit(&mut session, kind);
            session.change_selected_source(other);
            session.change_selected_source(kind);
            assert_eq!(
                session.channels[session.selected].generator_params(),
                kind.default_generator_params(),
                "{kind:?} -> {other:?} -> {kind:?}"
            );
        }
    }
}

/// An install (undo, load, every project edit) rebuilds the channel from
/// the document, which carries only the running kind: nothing else survives
/// it either.
#[test]
fn an_install_keeps_only_the_running_kind() {
    let mut session = Session::default();
    session.change_selected_source(DeviceKind::MonoSynth);
    edit(&mut session, DeviceKind::MonoSynth);
    let edited = session.channels[session.selected].generator_params();
    let project = session.project_snapshot(120, 50);
    let samples = vec![None; project.channels.len()];
    session.replace_project(&project, &samples);
    assert_eq!(session.channels[session.selected].generator_params(), edited);
}

/// MOO-313: a plugin instrument is a device with an identity of its own,
/// minted from the channel's `next_device_id` each time one is set, so two in
/// a row never share one; a native source has none. The identity survives a
/// snapshot and an install, which is the path undo and every project edit
/// take.
#[test]
fn each_plugin_source_gets_its_own_device_id_and_a_native_one_gets_none() {
    use mooloop_core::DeviceId;
    let mut session = Session::default();
    session.reset_channel_source(0, DeviceKind::Plugin);
    let first = session.channels[0].source_device;
    session.reset_channel_source(0, DeviceKind::Plugin);
    let second = session.channels[0].source_device;
    assert!(first.is_assigned() && second.is_assigned(), "{first:?} {second:?}");
    assert_ne!(first, second, "a second plugin source reused the first's id");

    let project = session.project_snapshot(120, 50);
    assert_eq!(project.channels[0].setup.source_device, second);
    let samples = vec![None; project.channels.len()];
    session.replace_project(&project, &samples);
    assert_eq!(session.channels[0].source_device, second, "an install lost the id");

    for kind in KINDS {
        session.reset_channel_source(0, DeviceKind::Plugin);
        session.reset_channel_source(0, kind);
        assert_eq!(session.channels[0].source_device, DeviceId::UNASSIGNED, "{kind:?}");
    }
}

/// Replacing a plugin instrument lets go of its lanes and routes, as
/// deleting an effect does -- Adam's recommended answer to the open Question
/// on MOO-312, `Session::forget_replaced_source_device`. A lane on the native
/// generator is not the instrument's and stays (MOO-135).
#[test]
fn replacing_a_plugin_instrument_forgets_its_lanes_and_routes() {
    use mooloop_core::{
        AutomationLane, EffectTarget, ModLfoParams, ModPolarity, ModRoute, ModulatorParams,
        ParamAddr,
    };
    let here = EffectTarget::Channel(0);
    for next in [DeviceKind::Plugin, DeviceKind::MonoSynth] {
        let mut session = Session::default();
        session.reset_channel_source(0, DeviceKind::Plugin);
        let instrument = session.channels[0].source_device;
        let on_instrument = ParamAddr::plugin_param(here, instrument, 7);
        let on_generator =
            ParamAddr::source(here, DeviceKind::Sampler, mooloop_core::SAMPLER_PARAM_DRIVE);
        let home = session.channels[0].id;
        let lfo = session.modulation.add_module(
            ModulatorParams::Lfo(ModLfoParams::default()),
            mooloop_core::InputSource::None,
            home,
            "Channel 1",
        );
        for destination in [on_instrument, on_generator] {
            session
                .modulation
                .routes
                .push(ModRoute::from_module(lfo, destination, 0.5, ModPolarity::Bipolar));
        }
        for destination in [on_instrument, on_generator] {
            session.channels[0].automation[0].push(AutomationLane::new(destination));
        }

        session.reset_channel_source(0, next);
        let channel = &session.channels[0];
        let routes: Vec<ParamAddr> =
            session.modulation.routes.iter().map(|route| route.destination).collect();
        let lanes: Vec<ParamAddr> = channel.automation[0].iter().map(|lane| lane.target).collect();
        assert_eq!(routes, [on_generator], "{next:?}: routes");
        assert_eq!(lanes, [on_generator], "{next:?}: lanes");
    }
}
