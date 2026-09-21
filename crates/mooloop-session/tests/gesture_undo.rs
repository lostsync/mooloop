//! What undo restores, and what it must leave alone.
//!
//! `docs/plans/gesture-undo/` step 04. Two rules bite only here, and both are
//! about a parameter whose *displayed* value is not its *stored* one.
//!
//! A knob under a modulation route or an automation lane is drawn at the
//! resolved position and saved at the base. Undo restores saved state, so it
//! works by construction -- as long as the snapshot is of the project and not
//! of what the face is showing. The trap is a handler that reads the face
//! back and writes what it read, and these are the tests that would catch it:
//! each asserts the base came back *and* that the thing resolving it is still
//! there, because a restore that took the route or the lane with it would be
//! a different and worse bug than the one it fixed.

use mooloop_core::{
    AutomationPoint, DeviceKind, EffectTarget, ModulatorKind, ParamAddr, STRIP_PARAM_VOLUME,
};
use mooloop_session::history::{Entry, History};
use mooloop_session::project::ProjectSnapshot;
use mooloop_session::session::{ArmedRoute, Session};
use std::collections::HashMap;

const BPM: i32 = 120;
const SWING: i32 = 50;

fn snapshot(session: &Session) -> ProjectSnapshot {
    ProjectSnapshot {
        project: session.project_snapshot(BPM, SWING),
        samples: HashMap::new(),
    }
}

fn install(session: &mut Session, snapshot: &ProjectSnapshot) {
    session.replace_project(&snapshot.project, &[]);
}

fn volume(session: &Session) -> f32 {
    session.channels[0].volume
}

fn set_volume(session: &mut Session, value: f32) {
    session
        .set_channel_volume(0, value)
        .expect("a channel's volume is a writable parameter");
}

/// The plain case, and the one the whole plan is for: a parameter edit that
/// reached the history comes back.
#[test]
fn a_device_parameter_undoes_to_what_it_was() {
    let mut session = Session::default();
    set_volume(&mut session, 0.25);
    let before = snapshot(&session);

    set_volume(&mut session, 0.75);
    assert!((volume(&session) - 0.75).abs() < 1e-6);

    install(&mut session, &before);
    assert!(
        (volume(&session) - 0.25).abs() < 1e-6,
        "undo did not restore the parameter"
    );
}

/// **A modulated parameter undoes to its base value, with the route intact.**
///
/// The route resolves the knob to somewhere else entirely, so a snapshot
/// taken from the face rather than the project would store the resolved
/// position -- and undoing would write the modulated value into the base,
/// moving the parameter to a place the user never put it.
#[test]
fn a_modulated_parameter_undoes_to_its_base_and_keeps_its_route() {
    let mut session = Session::default();
    session
        .add_modulation_source(ModulatorKind::Lfo)
        .expect("an empty rack has a free slot");
    session.toggle_modulation_assignment();
    let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
    let ArmedRoute::Added(_) = session.arm_modulation_route(destination, 0.4) else {
        panic!("the armed LFO did not author a route");
    };

    set_volume(&mut session, 0.25);
    let before = snapshot(&session);
    let routes_before = session.channels[0].modulation.routes.iter().flatten().count();
    assert_eq!(routes_before, 1, "the route this test needs is not there");

    set_volume(&mut session, 0.75);
    install(&mut session, &before);

    assert!(
        (volume(&session) - 0.25).abs() < 1e-6,
        "undo restored something other than the base value"
    );
    assert_eq!(
        session.channels[0].modulation.routes.iter().flatten().count(),
        routes_before,
        "undoing a parameter edit stranded the route driving it"
    );
}

/// **An automated parameter undoes to its base value, with the lane intact.**
///
/// Same shape as the route above and a different resolver: the lane decides
/// what the parameter reads while the transport runs, and the base is what
/// the document holds.
#[test]
fn an_automated_parameter_undoes_to_its_base_and_keeps_its_lane() {
    let mut session = Session::default();
    let target = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
    session
        .open_automation_lane_at(target)
        .expect("a strip parameter is automatable");
    session
        .automation_lane_mut()
        .expect("the lane just opened")
        .upsert(AutomationPoint::new(1, 0, 0.9));

    set_volume(&mut session, 0.25);
    let before = snapshot(&session);
    let points = session
        .automation_lane()
        .expect("the lane is open")
        .points()
        .len();
    assert_eq!(points, 1, "the lane this test needs has no point in it");

    set_volume(&mut session, 0.75);
    install(&mut session, &before);

    assert!(
        (volume(&session) - 0.25).abs() < 1e-6,
        "undo restored a resolved value rather than the base"
    );
    assert_eq!(
        session
            .automation_lane()
            .expect("the lane survived the install")
            .points()
            .len(),
        points,
        "undoing a parameter edit took the automation lane with it"
    );
}

/// **Parameter edits and structural edits share one history.**
///
/// They always did -- it is one list of project snapshots -- so what this
/// asserts is that interleaving them corrupts neither. The case is the one
/// that was already broken for a different reason: edit a parameter, add a
/// channel, edit a parameter, then walk back through all three.
#[test]
fn parameter_and_structural_edits_interleave_in_one_history() {
    let mut session = Session::default();
    let mut history: History<ProjectSnapshot> = History::default();

    let record = |history: &mut History<ProjectSnapshot>, before, after, label| {
        history.record(Entry {
            before,
            after,
            label,
            gesture: None,
        });
    };

    set_volume(&mut session, 0.2);
    let start = snapshot(&session);

    // One: a parameter.
    set_volume(&mut session, 0.5);
    let after_first = snapshot(&session);
    record(&mut history, start.clone(), after_first.clone(), "Volume");

    // Two: a structural edit, in the middle of the parameter ones.
    session
        .add_channel(DeviceKind::Sampler)
        .expect("an empty rack has room");
    let after_add = snapshot(&session);
    record(
        &mut history,
        after_first.clone(),
        after_add.clone(),
        "Add channel",
    );

    // Three: another parameter.
    set_volume(&mut session, 0.9);
    let after_second = snapshot(&session);
    record(&mut history, after_add.clone(), after_second, "Volume");

    let channels = session.channels.len();
    assert_eq!(channels, 2, "the structural edit did not land");

    for (step, (expect_volume, expect_channels)) in
        [(0.5, 2), (0.5, 1), (0.2, 1)].into_iter().enumerate()
    {
        let target = history
            .undo_target()
            .expect("three edits were recorded")
            .before
            .clone();
        install(&mut session, &target);
        history.commit_undo();
        assert!(
            (volume(&session) - expect_volume).abs() < 1e-6,
            "undo {step} left the volume at {}",
            volume(&session)
        );
        assert_eq!(
            session.channels.len(),
            expect_channels,
            "undo {step} left {} channels",
            session.channels.len()
        );
        if step == 2 {
            assert!(!history.can_undo(), "the history walked past its own start");
        }
    }
}
