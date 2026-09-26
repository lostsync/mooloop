//! A mono CLAP effect in a chain (MOO-266): the in-repo test gain in its
//! three mono layouts, run by `ClapProcessor`.
//!
//! The rule is a TRS cable into a TS jack, with no setting: a mono input is
//! fed `(L + R) / 2`, the sum at -6 dB, so a centred signal passes at unity
//! and a hard-panned one comes through 6 dB under the side it was on; a mono
//! output is copied to both sides. `(L + R) * 0.707` was the alternative,
//! and it raises a centred signal by 3 dB, which is the "twice as loud"
//! Adam ruled out on the issue.
//!
//! Every instance is created on the test's own thread, its CLAP main
//! thread, and processed on a thread spawned for it, as in
//! `plugin_host_tests.rs`.

use std::collections::BTreeMap;

use mooloop_core::{
    EffectKind, EffectParams, EffectSlotState, PluginFormat, PluginRef, PluginSlotState,
};
use mooloop_dsp::{EventList, ProcessContext, StereoBus};
use mooloop_plugin_host::clap::{ClapInstance, MAX_FRAMES};
use mooloop_plugin_host::{AudioConfig, HostedInstance, Lifeline};
use mooloop_test_plugin as test_plugin;

use crate::plugin_host_tests::{drum_loop, gain_state, render_hosted, test_plugin_path, worst_difference};
use crate::render_test_support::SAMPLE_RATE;

fn gain_ref(id: &str) -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: id.to_owned(),
        name: "Test Gain".to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    }
}

/// The test gain with `id`'s ports, at 0 dB and no latency.
fn open(id: &str) -> ClapInstance {
    ClapInstance::open(
        &test_plugin_path(),
        &gain_ref(id),
        &gain_state(0.0, 0),
        AudioConfig {
            sample_rate: SAMPLE_RATE,
            max_frames: MAX_FRAMES,
        },
    )
    .unwrap_or_else(|error| panic!("{id} opens: {error:?}"))
}

/// What `id` makes of one block of a constant `left` and `right`: the last
/// frame of each side.
fn through(id: &str, left: f32, right: f32) -> (f32, f32) {
    let mut instance = open(id);
    assert!(instance.fits_effect(), "{id} is refused from a chain");
    let life = Lifeline::new();
    let mut node = instance.build_processor(life.tie()).expect("a processor");
    let out = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let frames = 64;
                let mut bus = StereoBus::with_capacity(frames);
                bus.l.fill(left);
                bus.r.fill(right);
                let ctx = ProcessContext {
                    sample_rate: SAMPLE_RATE,
                    frames,
                    playing: true,
                    bpm: 120.0,
                    position_ticks: 0.0,
                    position_frames: 0,
                };
                node.process(&ctx, &mut bus, &EventList::empty(), None);
                (bus.l[frames - 1], bus.r[frames - 1])
            })
            .join()
            .expect("the audio thread did not panic")
    });
    assert!(life.is_alone(), "the processor outlived its thread");
    assert!(!instance.failed(), "{id} failed");
    out
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

/// **Mono in, mono out: a centred signal passes at unity, not twice as
/// loud, and a hard-left one comes out 6 dB down on both sides.**
#[test]
fn a_mono_effect_passes_a_centred_signal_at_unity_and_a_hard_panned_one_6_db_down() {
    let (left, right) = through(test_plugin::GAIN_MONO_ID, 0.5, 0.5);
    assert!(close(left, 0.5) && close(right, 0.5), "L = R = 0.5 came out {left}, {right}");
    let (left, right) = through(test_plugin::GAIN_MONO_ID, 0.5, 0.0);
    assert!(close(left, 0.25) && close(right, 0.25), "hard left 0.5 came out {left}, {right}");
    let db = 20.0 * (left / 0.5).log10();
    assert!((db + 6.02).abs() < 0.01, "hard left is {db:.2} dB, not -6");
}

/// **Mono in, stereo out** (a mono-in reverb, say): both of the plugin's
/// outputs hear the sum at -6 dB.
#[test]
fn a_mono_in_stereo_out_effect_hears_the_sum_on_both_sides() {
    let (left, right) = through(test_plugin::GAIN_MONO_IN_ID, 0.5, 0.5);
    assert!(close(left, 0.5) && close(right, 0.5), "L = R = 0.5 came out {left}, {right}");
    let (left, right) = through(test_plugin::GAIN_MONO_IN_ID, 0.5, 0.1);
    assert!(close(left, 0.3) && close(right, 0.3), "0.5 and 0.1 came out {left}, {right}");
}

/// **Stereo in, mono out**: the plugin hears both sides as they are, and
/// its one output (the test gain's is its left input) goes to both.
#[test]
fn a_stereo_in_mono_out_effect_hears_both_sides_and_its_output_goes_to_both() {
    let (left, right) = through(test_plugin::GAIN_MONO_OUT_ID, 0.5, 0.1);
    assert!(close(left, 0.5) && close(right, 0.5), "0.5 and 0.1 came out {left}, {right}");
}

/// **A mono effect placed in a chain is a centred channel unchanged**: a
/// drum loop, whose channel is centred (L = R), through the mono test gain
/// at 0 dB, is the loop without it.
#[test]
fn a_mono_effect_in_a_chain_leaves_a_centred_channel_as_it_was() {
    let (mut project, _) = drum_loop(1, &[]);
    let slot = project.add_plugin_slot(PluginSlotState::new(gain_ref(test_plugin::GAIN_MONO_ID)));
    let mut device = EffectSlotState::of_kind(EffectKind::Plugin);
    device.params = EffectParams::Plugin(slot);
    project.channels[0].setup.push_effect(device).expect("room in the chain");

    let frames = SAMPLE_RATE as usize;
    let dry = render_hosted(&project, BTreeMap::new(), frames, 256);
    let peak = dry.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak > 1e-3, "the loop is silent");
    assert!(
        dry.chunks(2).all(|frame| frame[0] == frame[1]),
        "the loop is not centred, so the mono sum would change it"
    );

    let mut instance = open(test_plugin::GAIN_MONO_ID);
    let life = Lifeline::new();
    let node = instance.build_processor(life.tie()).expect("a processor");
    let wet = render_hosted(&project, BTreeMap::from([(slot, node)]), frames, 256);
    assert!(life.is_alone());
    assert!(!instance.failed());
    let worst = worst_difference(&wet, &dry);
    assert!(worst < 1e-6, "the mono effect moved the loop by {worst}");
}
