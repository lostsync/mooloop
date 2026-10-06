//! Changing a channel's device leaves what was aimed at the old one inert,
//! and kept (MOO-135).
//!
//! A knob learned on the sampler's Cutoff (id 12) must not start moving the
//! drum synth's own id 12 when the channel becomes a drum synth, and it must
//! move Cutoff again when the channel is switched back. The engine's half --
//! routes and lanes -- is `render.rs`'s
//! `a_route_and_a_lane_made_on_one_device_are_inert_on_another_until_it_returns`.
//!
//! An inert lane still takes one of its pattern's lane slots, so the lane
//! menu lists it as missing and it can be removed (MOO-270).

use mooloop_core::control::{ControlBinding, ControlSource, ControlTarget};
use mooloop_core::{
    DeviceKind, EffectTarget, MidiChannelFilter, MidiKind, MidiMessage, MidiPortFilter,
    MidiPortId, MidiPortInfo, ParamAddr, SAMPLER_PARAM_FILTER_CUTOFF,
    MAX_AUTOMATION_LANES_PER_CHANNEL,
};
use mooloop_session::session::Session;

const CUTOFF: u32 = SAMPLER_PARAM_FILTER_CUTOFF;

fn cc(value: u8) -> MidiMessage {
    MidiMessage {
        offset: 0,
        port: MidiPortId(0),
        channel: 0,
        kind: MidiKind::ControlChange {
            controller: 7,
            value,
        },
    }
}

#[test]
fn a_binding_made_on_one_device_moves_nothing_on_another_and_comes_back() {
    let ports = vec![MidiPortInfo {
        id: MidiPortId(0),
        name: "desk".to_owned(),
    }];
    let mut session = Session::default();
    session.change_selected_source(DeviceKind::Sampler);
    let selected = session.selected;
    assert!(
        DeviceKind::DrumSynth.descriptor(CUTOFF).is_some(),
        "the premise: the drum synth has an id 12 of its own"
    );
    let cutoff = session.selected_source_address(CUTOFF).unwrap();
    assert_eq!(
        cutoff,
        ParamAddr::source(EffectTarget::Channel(selected as u8), DeviceKind::Sampler, CUTOFF)
    );
    session.control_map.bind(ControlBinding::new(
        ControlSource::Cc {
            port: MidiPortFilter::Any,
            channel: MidiChannelFilter::Omni,
            controller: 7,
        },
        ControlTarget::Param(session.param_key(cutoff).expect("the channel has an id")),
    ));
    session.resolve_control_map(&ports);
    assert!(session.param_descriptor(cutoff).is_some());

    // To the drum synth. The binding is still there, still named, and
    // names nothing this device has.
    session.change_selected_source(DeviceKind::DrumSynth);
    assert_eq!(session.control_map.bindings.len(), 1);
    assert_eq!(session.param_descriptor(cutoff), None);
    assert_eq!(session.modulation_destination(cutoff), None);
    let before = session.channels[selected].generator_params();
    session.resolve_control_map(&ports);
    for value in [0, 64, 127, 3] {
        session.apply_control_input(&cc(value), &ports, false);
    }
    assert_eq!(
        session.channels[selected].generator_params(),
        before,
        "the sampler's Cutoff binding moved the drum synth"
    );
    assert_eq!(
        session.control_target_label(&session.control_map.bindings[0].target),
        "Unavailable parameter"
    );

    // And back: it moves Cutoff again.
    session.change_selected_source(DeviceKind::Sampler);
    session.resolve_control_map(&ports);
    assert!(session.param_descriptor(cutoff).is_some());
    let at_rest = session.param_normalized(cutoff).unwrap();
    // Pickup: catch the control where Cutoff is, then take it to the far end.
    let catch = (at_rest * 127.0).round() as u8;
    let far = if at_rest > 0.5 { 0 } else { 127 };
    session.apply_control_input(&cc(catch), &ports, false);
    session.apply_control_input(&cc(far), &ports, false);
    let moved = session.param_normalized(cutoff).unwrap();
    assert!(
        (moved - at_rest).abs() > 0.4,
        "the binding did not come back: Cutoff stayed at {at_rest} ({moved})"
    );
}

/// **Inert lanes that fill a pattern's slots are listed as missing and can be
/// removed, and undo brings a removed one back** (MOO-270).
///
/// Before, a channel switched from the sampler with eight sampler lanes in a
/// pattern could open no drum-synth lane there, while the lane menu listed
/// none of the eight: "no room", with nothing visible taking it. The cap
/// stays; what changed is that the eight can be seen and removed.
#[test]
fn inert_lanes_that_fill_the_slots_are_listed_and_can_be_removed() {
    const BPM: i32 = 120;
    const SWING: i32 = 50;
    let mut session = Session::default();
    session.change_selected_source(DeviceKind::Sampler);
    let selected = session.selected;
    let scope = EffectTarget::Channel(selected as u8);

    // Cutoff first, so a row can be checked by name, then seven more.
    let others = DeviceKind::Sampler
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.id)
        .filter(|&id| id != CUTOFF);
    let sampler: Vec<ParamAddr> = std::iter::once(CUTOFF)
        .chain(others)
        .take(MAX_AUTOMATION_LANES_PER_CHANNEL)
        .map(|id| session.selected_source_address(id).unwrap())
        .collect();
    assert_eq!(
        sampler.len(),
        MAX_AUTOMATION_LANES_PER_CHANNEL,
        "the premise: the sampler has eight parameters"
    );
    for &address in &sampler {
        session.open_automation_lane_at(address).expect("within the ceiling");
    }
    session.create_automation_point(96, 0.25).expect("the last lane is open");
    let with_point = *sampler.last().unwrap();
    assert!(
        session.inert_source_lanes().is_empty(),
        "a lane on the device the channel runs is not inert"
    );

    // To the drum synth. The eight are kept and take every slot.
    session.change_selected_source(DeviceKind::DrumSynth);
    let drum = session
        .selected_source_address(DeviceKind::DrumSynth.descriptors()[0].id)
        .unwrap();
    assert!(
        session.open_automation_lane_at(drum).is_none(),
        "the premise: the inert lanes fill the pattern"
    );
    let inert = session.inert_source_lanes();
    assert_eq!(
        inert.iter().map(|lane| lane.address).collect::<Vec<_>>(),
        sampler,
        "every inert lane is listed, in the pattern's order"
    );
    let cutoff = inert
        .iter()
        .find(|lane| lane.address.param == CUTOFF)
        .expect("the sampler's Cutoff lane is among them");
    assert_eq!(cutoff.device, DeviceKind::Sampler.label());
    assert_eq!(cutoff.name, DeviceKind::Sampler.descriptor(CUTOFF).unwrap().name);

    // Picking the missing row reopens the lane, and Remove lane takes it
    // out, which frees a slot for the device the channel runs.
    let before = session.project_snapshot(BPM, SWING);
    session
        .open_automation_lane_at(with_point)
        .expect("an existing inert lane reopens");
    assert_eq!(session.automation_lane().map(|lane| lane.points().len()), Some(1));
    session.close_automation_lane().expect("the reopened lane closes");
    assert_eq!(session.inert_source_lanes().len(), MAX_AUTOMATION_LANES_PER_CHANNEL - 1);
    assert!(!session.channels[selected].automation[0]
        .iter()
        .any(|lane| lane.target == with_point));
    assert!(
        session.open_automation_lane_at(with_point).is_none(),
        "a removed inert lane was made again: a new lane must name the device the channel runs"
    );
    session
        .open_automation_lane_at(drum)
        .expect("the removal left a slot for the drum synth");
    session.close_automation_lane().expect("open");

    // Undo is the snapshot taken before the removal, one step: the lane is
    // back, point and all, still inert and still listed.
    session.replace_project(&before, &[]);
    assert_eq!(
        session.inert_source_lanes().iter().map(|lane| lane.address).collect::<Vec<_>>(),
        sampler
    );
    let restored = session.channels[selected].automation[0]
        .iter()
        .find(|lane| lane.target == with_point)
        .expect("undo brought the removed lane back");
    assert_eq!(restored.points().len(), 1);
    assert_eq!(restored.target.scope, scope);

    // And on switching back, none of them is missing.
    session.change_selected_source(DeviceKind::Sampler);
    assert!(session.inert_source_lanes().is_empty());
}
