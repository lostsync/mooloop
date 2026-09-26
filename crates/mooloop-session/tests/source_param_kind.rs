//! Changing a channel's device leaves what was aimed at the old one inert,
//! and kept (MOO-135).
//!
//! A knob learned on the sampler's Cutoff (id 12) must not start moving the
//! drum synth's own id 12 when the channel becomes a drum synth, and it must
//! move Cutoff again when the channel is switched back. The engine's half --
//! routes and lanes -- is `render.rs`'s
//! `a_route_and_a_lane_made_on_one_device_are_inert_on_another_until_it_returns`.

use mooloop_core::control::{ControlBinding, ControlSource, ControlTarget};
use mooloop_core::{
    DeviceKind, EffectTarget, MidiChannelFilter, MidiKind, MidiMessage, MidiPortFilter,
    MidiPortId, MidiPortInfo, ParamAddr, SAMPLER_PARAM_FILTER_CUTOFF,
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
    assert_eq!(session.channel_modulation_destination(cutoff), None);
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
