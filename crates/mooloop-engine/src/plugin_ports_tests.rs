//! A CLAP effect with ports beyond its main ones, in a channel's chain
//! (MOO-306, MOO-308): the test plugin's `mooloop.test.sidechain`, a unity
//! effect whose main input and output are ports 1 and 2, beside a mono
//! sidechain and two extra outputs it fills with `AUX_LEVEL`.
//!
//! The adapter's own test (`mooloop-plugin-host/tests/extra_ports.rs`)
//! drives the processor alone; this one plays it where it lives, on a
//! channel, as the render thread would.

use std::collections::BTreeMap;

use mooloop_core::{EffectKind, EffectParams, EffectSlotState, PluginFormat, PluginRef, PluginSlotState};
use mooloop_plugin_host::clap::{ClapInstance, MAX_FRAMES};
use mooloop_plugin_host::{AudioConfig, HostedInstance, Lifeline};
use mooloop_test_plugin as test_plugin;

use crate::plugin_host_tests::{drum_loop, render_hosted, test_plugin_path, worst_difference};
use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;

fn sidechain_ref() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: test_plugin::SIDECHAIN_ID.to_owned(),
        name: "Test Sidechain".to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    }
}

fn open() -> ClapInstance {
    ClapInstance::open(
        &test_plugin_path(),
        &sidechain_ref(),
        &mooloop_core::PluginState::default(),
        AudioConfig {
            sample_rate: SAMPLE_RATE,
            max_frames: MAX_FRAMES,
        },
    )
    .expect("the sidechain effect opens")
}

/// **A sidechain effect in a chain is the channel unchanged, and the render
/// thread allocates nothing for its extra ports.** A drum loop through the
/// unity sidechain effect is the loop without it: the chain's signal went
/// into its main input at port 1 and came back from its main output at port
/// 2, not from an extra output. Its sidechain heard only silence.
#[test]
fn a_sidechain_effect_in_a_chain_leaves_the_channel_as_it_was_and_allocates_nothing() {
    let (mut project, _) = drum_loop(1, &[]);
    let slot = project.add_plugin_slot(PluginSlotState::new(sidechain_ref()));
    let mut device = EffectSlotState::of_kind(EffectKind::Plugin);
    device.params = EffectParams::Plugin(slot);
    project.channels[0].setup.push_effect(device).expect("room in the chain");

    let frames = SAMPLE_RATE as usize / 2;
    let dry = render_hosted(&project, BTreeMap::new(), frames, 256);
    assert!(dry.iter().any(|&s| s.abs() > 1e-3), "the loop is silent");

    let mut instance = open();
    assert!(instance.fits_effect());
    let life = Lifeline::new();
    let node = instance.build_processor(life.tie()).expect("a processor");
    let wet = render_hosted(&project, BTreeMap::from([(slot, node)]), frames, 256);
    assert!(life.is_alone());
    assert!(!instance.failed(), "the plugin failed: it was not handed every port");
    let worst = worst_difference(&wet, &dry);
    assert!(worst < 1e-6, "the sidechain effect moved the loop by {worst}");
    assert_eq!(instance.param_value(test_plugin::PROBE_SIDECHAIN_LOUD), Some(0.0));
    assert!(instance.param_value(test_plugin::PROBE_SIDECHAIN_BLOCKS).is_some_and(|n| n > 0.0));

    // The same chain block by block, counting the render thread's
    // allocator calls once the first blocks have started everything.
    let node = instance.build_processor(life.tie()).expect("a second processor");
    let mut state = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    state.host_plugins(&project, BTreeMap::from([(slot, node)]));
    let counted = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                crate::executor::prepare_audio_thread();
                state.play();
                for _ in 0..4 {
                    state.process_once_block(256);
                }
                let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
                for block in [256, 64, 1, 511, 256] {
                    state.process_once_block(block);
                }
                let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
                drop(state);
                (after.0 - before.0, after.1 - before.1)
            })
            .join()
            .expect("the render thread did not panic")
    });
    assert!(life.is_alone());
    assert!(!instance.failed());
    assert_eq!(counted, (0, 0), "the render thread allocated or freed with the sidechain effect in the chain");
}
