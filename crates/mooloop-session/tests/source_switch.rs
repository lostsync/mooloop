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
