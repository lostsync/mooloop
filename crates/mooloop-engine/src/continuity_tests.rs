//! Control changes are continuous (MOO-104).
//!
//! Every case here plays something sustained, changes one control in the
//! middle of it, and holds the largest sample-to-sample step across the
//! change to [`Transition::is_continuous`]'s bound: no more than one percent
//! of the signal's peak above the largest step the material takes on its own.
//! A switch made in one sample steps by up to the whole peak, so the bound
//! tells a ramp from a switch with two orders of magnitude to spare.
//!
//! The material is a sine, because a sine is the signal that shows a click
//! most plainly: its own steps are small and smooth, so anything the change
//! adds stands out against them. A saw would hide a hard mute inside its own
//! reset.
//!
//! One module rather than a test beside each fix, because the family is the
//! point: the report that asked for it (`reports/teams-2026-09-22.md` §4.2
//! item 3) found that no test anywhere measured a control *change*, and the
//! largest-step checks that did exist were private helpers inside single
//! device tests. The cases are grouped by the team whose code the transition
//! runs through.

use std::sync::Arc;

use mooloop_core::{
    DelayParams, EffectKind, EffectParams, EffectSlotState, EffectTarget, EngineCommand,
    FilterMode, FilterParams, MonoSynthParams, NoteEvent, OscParams, OscWave, Project,
    ProjectChannel, DEFAULT_STEPS,
};
use mooloop_dsp::SampleData;

use crate::render::RenderState;
use crate::render_test_support::{render_frames, step_across, Transition, SAMPLE_RATE};
use crate::StructuralCommand;

/// Frames rendered before each change: long enough for the note's attack to
/// be over, and deliberately not a multiple of the block size, so the change
/// lands inside what would otherwise be a block.
///
/// **And on a crest of the sine, which is not a detail.** The first draft
/// used 12 000, which is exactly fifty-five cycles of 220 Hz: every change
/// landed on a zero crossing, where a hard switch has nothing to step, and
/// ten of the eleven cases passed against the unfixed tree. Fifty-five and a
/// quarter cycles puts the switch where the sine is at its peak, which is
/// the worst case and the one a bound has to hold at.
const LEAD: usize = 12_055;

/// Frames measured after each change: past any ramp this family expects to
/// see, and short of the note's end.
const TAIL: usize = 12_000;

/// The track the sine is routed through in the track cases.
const TRACK: u8 = 1;

/// A 220 Hz sine held for a whole bar on a mono synth with everything
/// between the oscillator and the channel opened up: filter out, drive out,
/// no LFO, full sustain.
fn sine_channel() -> ProjectChannel {
    let mut params = MonoSynthParams {
        attack: 0.005,
        decay: 0.0,
        sustain: 1.0,
        release: 0.15,
        filter_cutoff: 1.0,
        drive: 0.0,
        ..MonoSynthParams::default()
    };
    params.osc = [
        OscParams {
            wave: OscWave::Sine,
            level: 1.0,
            ..OscParams::default()
        },
        OscParams::default(),
        OscParams::default(),
    ];
    let mut channel = ProjectChannel::mono_synth_with_params(0, 1, params);
    channel.setup.channel.volume = 1.0;
    // A bar: two seconds at the default tempo, far past `LEAD + TAIL` twice.
    channel.notes[0].push(NoteEvent::new(1, 0, 384, 57, 127));
    channel
}

/// The sine alone, straight to the master.
fn sine_project() -> Project {
    Project {
        channels: vec![sine_channel()],
        pattern_lengths: vec![DEFAULT_STEPS],
        ..Project::default()
    }
}

/// The sine through a track, so the track's own fader, balance, mute, solo
/// and polarity are what it passes on its way to the master.
fn tracked_sine_project() -> Project {
    let mut project = sine_project();
    project.ensure_tracks(usize::from(TRACK) + 1);
    project.channels[0].setup.channel.bus = TRACK;
    project
}

fn playing(project: &Project) -> RenderState {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    render
}

/// One command, landing mid-note.
fn across_command(project: &Project, command: EngineCommand) -> Transition {
    let mut render = playing(project);
    step_across(&mut render, LEAD, |render| render.apply_command(command), TAIL)
}

/// Two commands, the second after the first has had `TAIL` to act, measured
/// across the second against the material as it was before the first. What
/// an unmute needs: the lead before it is silence, which says nothing about
/// how large a step the sine takes.
fn across_return(project: &Project, away: EngineCommand, back: EngineCommand) -> Transition {
    let mut render = playing(project);
    let first = step_across(&mut render, LEAD, |render| render.apply_command(away), TAIL);
    // One frame of lead, not none: the step onto the first frame after the
    // change is measured from the last frame before it, and with no lead
    // there is no such frame -- which is how the first draft of this passed
    // an unmute that jumped straight to the crest.
    let second = step_across(&mut render, 1, |render| render.apply_command(back), TAIL);
    Transition {
        before: first.before,
        after: second.after,
        peak: first.peak,
        across: second.across,
    }
}

fn assert_continuous(what: &str, transition: Transition) {
    assert!(
        transition.peak > 0.05,
        "{what}: the material has to be sounding, peak {}",
        transition.peak
    );
    assert!(
        transition.is_continuous(),
        "{what} stepped by {:.5} where the sine alone steps by at most {:.5} \
         before and {:.5} after (peak {:.4}; the bound is {:.5})",
        transition.across,
        transition.before,
        transition.after,
        transition.peak,
        transition.bound()
    );
}

// --- Mixer & Routing: MOO-107 ------------------------------------------------

#[test]
fn a_channel_fader_move_is_continuous() {
    assert_continuous(
        "a channel fader from 1.0 to 0.1",
        across_command(
            &sine_project(),
            EngineCommand::SetChannelVolume {
                channel: 0,
                volume: 0.1,
            },
        ),
    );
}

#[test]
fn a_channel_pan_move_is_continuous() {
    assert_continuous(
        "a channel pan from centre to hard left",
        across_command(
            &sine_project(),
            EngineCommand::SetChannelPan {
                channel: 0,
                pan: -1.0,
            },
        ),
    );
}

#[test]
fn muting_a_channel_is_continuous() {
    assert_continuous(
        "muting a channel",
        across_command(
            &sine_project(),
            EngineCommand::SetChannelMuted {
                channel: 0,
                muted: true,
            },
        ),
    );
}

#[test]
fn unmuting_a_channel_is_continuous() {
    assert_continuous(
        "unmuting a channel",
        across_return(
            &sine_project(),
            EngineCommand::SetChannelMuted {
                channel: 0,
                muted: true,
            },
            EngineCommand::SetChannelMuted {
                channel: 0,
                muted: false,
            },
        ),
    );
}

#[test]
fn a_channel_silenced_by_a_solo_elsewhere_is_continuous() {
    assert_continuous(
        "a channel silenced by someone else's solo",
        across_command(
            &sine_project(),
            EngineCommand::SetChannelSoloSilenced {
                channel: 0,
                silenced: true,
            },
        ),
    );
    assert_continuous(
        "a channel given back when the solo is dropped",
        across_return(
            &sine_project(),
            EngineCommand::SetChannelSoloSilenced {
                channel: 0,
                silenced: true,
            },
            EngineCommand::SetChannelSoloSilenced {
                channel: 0,
                silenced: false,
            },
        ),
    );
}

#[test]
fn a_track_fader_move_is_continuous() {
    assert_continuous(
        "a track fader from 1.0 to 0.1",
        across_command(
            &tracked_sine_project(),
            EngineCommand::SetBusVolume {
                bus: TRACK,
                volume: 0.1,
            },
        ),
    );
}

#[test]
fn a_track_balance_move_is_continuous() {
    assert_continuous(
        "a track balance from centre to hard right",
        across_command(
            &tracked_sine_project(),
            EngineCommand::SetBusPan {
                bus: TRACK,
                pan: 1.0,
            },
        ),
    );
}

#[test]
fn muting_and_unmuting_a_track_is_continuous() {
    let mute = EngineCommand::SetBusMuted {
        bus: TRACK,
        muted: true,
    };
    assert_continuous(
        "muting a track",
        across_command(&tracked_sine_project(), mute),
    );
    assert_continuous(
        "unmuting a track",
        across_return(
            &tracked_sine_project(),
            mute,
            EngineCommand::SetBusMuted {
                bus: TRACK,
                muted: false,
            },
        ),
    );
}

#[test]
fn a_track_silenced_by_a_solo_elsewhere_is_continuous() {
    let silence = EngineCommand::SetTrackSoloSilenced {
        bus: TRACK,
        silenced: true,
    };
    assert_continuous(
        "a track silenced by someone else's solo",
        across_command(&tracked_sine_project(), silence),
    );
    assert_continuous(
        "a track given back when the solo is dropped",
        across_return(
            &tracked_sine_project(),
            silence,
            EngineCommand::SetTrackSoloSilenced {
                bus: TRACK,
                silenced: false,
            },
        ),
    );
}

#[test]
fn flipping_a_tracks_polarity_is_continuous() {
    let flip = EngineCommand::SetTrackPolarity { bus: TRACK, on: true };
    assert_continuous(
        "flipping a track's polarity",
        across_command(&tracked_sine_project(), flip),
    );
    assert_continuous(
        "flipping it back",
        across_return(
            &tracked_sine_project(),
            flip,
            EngineCommand::SetTrackPolarity {
                bus: TRACK,
                on: false,
            },
        ),
    );
}

#[test]
fn muting_the_master_is_continuous() {
    let mute = EngineCommand::SetBusMuted {
        bus: mooloop_core::MASTER_BUS,
        muted: true,
    };
    assert_continuous("muting the master", across_command(&sine_project(), mute));
    assert_continuous(
        "unmuting the master",
        across_return(
            &sine_project(),
            mute,
            EngineCommand::SetBusMuted {
                bus: mooloop_core::MASTER_BUS,
                muted: false,
            },
        ),
    );
}

// --- Instruments: MOO-110 ----------------------------------------------------

/// A sine sample at 221 Hz rather than the 220 the synth plays, so that
/// the frames these cases change things at -- 12 000, a quarter of a second
/// -- fall on a crest: fifty-five and a quarter cycles. A switch at a zero
/// crossing has nothing to step, and passes whether or not it is fixed.
const SAMPLE_HZ: f32 = 221.0;

/// `frames` of a sine starting at phase zero, at the engine's rate, so
/// that the root note plays it at unity.
fn sine_sample(frames: usize) -> Arc<SampleData> {
    let step = std::f32::consts::TAU * SAMPLE_HZ / SAMPLE_RATE as f32;
    Arc::new(SampleData {
        frames: (0..frames)
            .map(|frame| {
                let value = 0.5 * (step * frame as f32).sin();
                [value, value]
            })
            .collect(),
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    })
}

/// The default sampler patch -- one voice, `Restart`, one-shot -- playing
/// `sample` at its root for each `(start, length)` in ticks.
fn sampler_render(sample: Arc<SampleData>, notes: &[(u32, u32)]) -> RenderState {
    let mut channel = ProjectChannel::sampler(0, 1);
    channel.setup.channel.volume = 1.0;
    for (index, &(start, length)) in notes.iter().enumerate() {
        channel.notes[0].push(NoteEvent::new(index as u32 + 1, start, length, 60, 127));
    }
    let project = Project {
        channels: vec![channel],
        pattern_lengths: vec![DEFAULT_STEPS],
        ..Project::default()
    };
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[Some(sample)]);
    render.play();
    render
}

/// A second note on the default patch, a quarter of a second into a sample
/// a second long. The patch has one voice and restarts it, so the first
/// note is stolen mid-waveform -- and used to stop in the sample the second
/// began, which was the loudest click in the default song.
#[test]
fn a_retrigger_on_the_default_sampler_is_continuous() {
    // Tick 48 is half a beat: 12 000 frames at 120 bpm.
    let mut render = sampler_render(sine_sample(48_000), &[(0, 96), (48, 96)]);
    assert_continuous(
        "a retrigger on the default sampler",
        step_across(&mut render, 12_000, |_| {}, TAIL),
    );
}

/// A one-shot running off the end of its sample, which here ends on a
/// crest. The voice used to stop wherever the waveform was.
#[test]
fn a_sample_ending_mid_waveform_is_continuous() {
    let mut render = sampler_render(sine_sample(12_000), &[(0, 384)]);
    assert_continuous(
        "a sample running out on a crest",
        step_across(&mut render, 11_000, |_| {}, 3_000),
    );
}

// --- Effects: MOO-108 --------------------------------------------------------

/// The sine through a low-pass at 150 Hz, which both attenuates and delays
/// a 220 Hz tone: the device's output and the dry path differ everywhere,
/// so switching between them steps wherever the switch lands.
fn filtered_sine(configure: impl FnOnce(&mut EffectSlotState)) -> RenderState {
    let mut channel = sine_channel();
    let mut effect = EffectSlotState::new(EffectParams::Filter(FilterParams {
        cutoff_hz: 150.0,
        resonance: 0.0,
        mode: FilterMode::LowPass,
        ..FilterParams::default()
    }));
    configure(&mut effect);
    channel.setup.push_effect(effect).expect("room");
    render_of(vec![channel])
}

/// The sine's channel, with `effects` built on it, playing.
fn render_of(channels: Vec<ProjectChannel>) -> RenderState {
    playing(&Project {
        channels,
        pattern_lengths: vec![DEFAULT_STEPS],
        ..Project::default()
    })
}

const FX: EffectTarget = EffectTarget::Channel(0);

#[test]
fn bypassing_an_effect_is_continuous() {
    let mut render = filtered_sine(|_| {});
    assert_continuous(
        "bypassing an effect",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(
                    EngineCommand::SetEffectBypassed {
                        target: FX,
                        slot: 0,
                        bypassed: true,
                    },
                )
            },
            TAIL,
        ),
    );
}

#[test]
fn un_bypassing_an_effect_is_continuous() {
    let mut render = filtered_sine(|effect| effect.bypassed = true);
    assert_continuous(
        "un-bypassing an effect",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(
                    EngineCommand::SetEffectBypassed {
                        target: FX,
                        slot: 0,
                        bypassed: false,
                    },
                )
            },
            TAIL,
        ),
    );
}

#[test]
fn moving_wet_dry_is_continuous() {
    let mut render = filtered_sine(|_| {});
    assert_continuous(
        "wet/dry from wet to dry",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(
                    EngineCommand::SetEffectWetDry {
                        target: FX,
                        slot: 0,
                        wet_dry: 0.0,
                    },
                )
            },
            TAIL,
        ),
    );
}

#[test]
fn moving_the_effect_trims_is_continuous() {
    let mut render = filtered_sine(|effect| effect.wet_dry = 0.5);
    assert_continuous(
        "the input trim from unity to a quarter",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(
                    EngineCommand::SetEffectInputTrim {
                        target: FX,
                        slot: 0,
                        input_trim: 0.25,
                    },
                )
            },
            TAIL,
        ),
    );
    let mut render = filtered_sine(|_| {});
    assert_continuous(
        "the output trim from unity to a quarter",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(
                    EngineCommand::SetEffectOutputTrim {
                        target: FX,
                        slot: 0,
                        output_trim: 0.25,
                    },
                )
            },
            TAIL,
        ),
    );
}

#[test]
fn moving_a_containers_mix_is_continuous() {
    let mut channel = sine_channel();
    channel
        .setup
        .push_effect(EffectSlotState::new(EffectParams::Filter(FilterParams {
            cutoff_hz: 150.0,
            resonance: 0.0,
            mode: FilterMode::LowPass,
            ..FilterParams::default()
        })))
        .expect("room");
    let setup = &mut channel.setup;
    mooloop_core::wrap_in_container(
        &mut setup.effects,
        &mut setup.next_device_id,
        0..1,
        EffectSlotState::of_kind(EffectKind::Chain),
    )
    .expect("wrapped");
    let mut render = render_of(vec![channel]);
    assert_continuous(
        "a container's Mix from wet to dry",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(
                    EngineCommand::SetEffectParam {
                        target: FX,
                        slot: 0,
                        id: mooloop_core::CONTAINER_PARAM_MIX,
                        value: 0.0,
                    },
                )
            },
            TAIL,
        ),
    );
}

/// The sine through a low-pass inside a chain container, rows
/// `[Chain, Filter]`, with the box's host controls set by `configure`.
fn contained_filtered_sine(configure: impl FnOnce(&mut EffectSlotState)) -> RenderState {
    let mut channel = sine_channel();
    channel
        .setup
        .push_effect(EffectSlotState::new(EffectParams::Filter(FilterParams {
            cutoff_hz: 150.0,
            resonance: 0.0,
            mode: FilterMode::LowPass,
            ..FilterParams::default()
        })))
        .expect("room");
    let setup = &mut channel.setup;
    let mut container = EffectSlotState::of_kind(EffectKind::Chain);
    configure(&mut container);
    mooloop_core::wrap_in_container(&mut setup.effects, &mut setup.next_device_id, 0..1, container)
        .expect("wrapped");
    render_of(vec![channel])
}

/// A container's trims are heard since MOO-210, so moving one is a
/// transition like a leaf's, and ramps like one. The input trim is taken at
/// half Mix, where it is on both sides of the blend.
#[test]
fn moving_a_containers_trims_is_continuous() {
    let mut render = contained_filtered_sine(|container| {
        container
            .params
            .set(mooloop_core::CONTAINER_PARAM_MIX, 0.5)
            .expect("a mix");
    });
    assert_continuous(
        "a container's input trim from unity to a quarter",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(EngineCommand::SetEffectInputTrim {
                    target: FX,
                    slot: 0,
                    input_trim: 0.25,
                })
            },
            TAIL,
        ),
    );
    let mut render = contained_filtered_sine(|_| {});
    assert_continuous(
        "a container's output trim from unity to a quarter",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(EngineCommand::SetEffectOutputTrim {
                    target: FX,
                    slot: 0,
                    output_trim: 0.25,
                })
            },
            TAIL,
        ),
    );
}

/// Bypassing a trimmed box, and bringing it back. Its bypassed path is
/// untrimmed, so the trims have to fade with the bypass rather than drop
/// off when the run stops being called (MOO-210).
#[test]
fn bypassing_a_trimmed_container_is_continuous() {
    // Mild enough that the sine stays well above the check's floor while
    // trimmed, and still a 28% step if the trims dropped off unfaded.
    let trimmed = |container: &mut EffectSlotState| {
        container.input_trim = 0.8;
        container.output_trim = 0.9;
    };
    let mut render = contained_filtered_sine(trimmed);
    assert_continuous(
        "bypassing a trimmed container",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(EngineCommand::SetEffectBypassed {
                    target: FX,
                    slot: 0,
                    bypassed: true,
                })
            },
            TAIL,
        ),
    );
    let mut render = contained_filtered_sine(|container| {
        trimmed(container);
        container.bypassed = true;
    });
    assert_continuous(
        "un-bypassing a trimmed container",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(EngineCommand::SetEffectBypassed {
                    target: FX,
                    slot: 0,
                    bypassed: false,
                })
            },
            TAIL,
        ),
    );
}

/// The sine through a layer of two chain branches, each holding the same
/// low-pass: rows `[Layer, Chain, Filter, Chain, Filter]`, so the branch
/// heads are rows 1 and 3 (containers/09).
fn layered_sine() -> RenderState {
    let filter = || {
        EffectSlotState::new(EffectParams::Filter(FilterParams {
            cutoff_hz: 150.0,
            resonance: 0.0,
            mode: FilterMode::LowPass,
            ..FilterParams::default()
        }))
    };
    let mut channel = sine_channel();
    let setup = &mut channel.setup;
    setup.push_effect(filter()).expect("room");
    setup.push_effect(filter()).expect("room");
    for row in [1, 0] {
        mooloop_core::wrap_in_container(
            &mut setup.effects,
            &mut setup.next_device_id,
            row..row + 1,
            EffectSlotState::of_kind(EffectKind::Chain),
        )
        .expect("each filter in its own chain");
    }
    mooloop_core::wrap_in_container(
        &mut setup.effects,
        &mut setup.next_device_id,
        0..4,
        EffectSlotState::of_kind(EffectKind::Layer),
    )
    .expect("both chains in a layer");
    render_of(vec![channel])
}

/// The sine through a layer of an empty chain -- a clean branch -- and a
/// chain holding the low-pass: rows `[Layer, Chain, Chain, Filter]`.
fn layered_sine_beside_a_clean_branch() -> RenderState {
    let mut channel = sine_channel();
    let setup = &mut channel.setup;
    setup
        .push_effect(EffectSlotState::new(EffectParams::Filter(FilterParams {
            cutoff_hz: 150.0,
            resonance: 0.0,
            mode: FilterMode::LowPass,
            ..FilterParams::default()
        })))
        .expect("room");
    mooloop_core::wrap_in_container(
        &mut setup.effects,
        &mut setup.next_device_id,
        0..1,
        EffectSlotState::of_kind(EffectKind::Chain),
    )
    .expect("the filter in a chain");
    let mut layer = EffectSlotState::of_kind(EffectKind::Layer);
    layer.params.set_container_children(3);
    let mut clean = EffectSlotState::of_kind(EffectKind::Chain);
    clean.params.set_container_children(0);
    setup.effects.insert(0, clean);
    setup.effects.insert(0, layer);
    mooloop_core::assign_device_ids(&mut setup.effects, &mut setup.next_device_id);
    render_of(vec![channel])
}

fn set_container_param(slot: u8, id: u32, value: f32) -> EngineCommand {
    EngineCommand::SetEffectParam {
        target: FX,
        slot,
        id,
        value,
    }
}

/// A branch's Mute, Solo and Level, and the layer's own Level, all ramp
/// (containers/09). Muting one of two equal branches halves the sum and
/// unmuting it brings it back; soloing one silences the other, which is
/// the switch a solo makes in a branch that was not itself touched.
#[test]
fn a_layer_branchs_mute_solo_and_level_are_continuous() {
    use mooloop_core::{CONTAINER_PARAM_LEVEL, CONTAINER_PARAM_MUTE, CONTAINER_PARAM_SOLO};
    let mut render = layered_sine();
    let muting = step_across(
        &mut render,
        LEAD,
        |render| render.apply_command(set_container_param(3, CONTAINER_PARAM_MUTE, 1.0)),
        TAIL,
    );
    assert_continuous("muting a branch", muting);
    // Measured against the material before the mute, as `across_return`
    // does: one frame of lead says nothing about how far the sine steps.
    let unmuting = step_across(
        &mut render,
        1,
        |render| render.apply_command(set_container_param(3, CONTAINER_PARAM_MUTE, 0.0)),
        TAIL,
    );
    assert_continuous(
        "unmuting it",
        Transition {
            before: muting.before,
            after: unmuting.after,
            peak: muting.peak,
            across: unmuting.across,
        },
    );
    let mut render = layered_sine();
    assert_continuous(
        "soloing the other branch",
        step_across(
            &mut render,
            LEAD,
            |render| render.apply_command(set_container_param(1, CONTAINER_PARAM_SOLO, 1.0)),
            TAIL,
        ),
    );
    // An empty chain is a clean branch, and its switches ramp too: an empty
    // box used to settle every ramp it had on every block.
    let mut render = layered_sine_beside_a_clean_branch();
    assert_continuous(
        "muting a clean branch",
        step_across(
            &mut render,
            LEAD,
            |render| render.apply_command(set_container_param(1, CONTAINER_PARAM_MUTE, 1.0)),
            TAIL,
        ),
    );
    let mut render = layered_sine_beside_a_clean_branch();
    assert_continuous(
        "a clean branch's Level",
        step_across(
            &mut render,
            LEAD,
            |render| render.apply_command(set_container_param(1, CONTAINER_PARAM_LEVEL, 0.1)),
            TAIL,
        ),
    );
    for (what, slot) in [("a branch's Level", 3), ("the layer's Level", 0)] {
        let mut render = layered_sine();
        assert_continuous(
            what,
            step_across(
                &mut render,
                LEAD,
                |render| render.apply_command(set_container_param(slot, CONTAINER_PARAM_LEVEL, 0.1)),
                TAIL,
            ),
        );
    }
}

/// Removal is two moments: the executor asking, which starts the fade, and
/// the removal itself once the fade has run. Both are held to the bound.
#[test]
fn removing_an_effect_is_continuous() {
    let mut render = filtered_sine(|_| {});
    assert_continuous(
        "the fade a removal starts",
        step_across(
            &mut render,
            LEAD,
            |render| assert!(!render.effect_slot_vacated(FX, 0, 0)),
            TAIL,
        ),
    );
    assert!(
        render.effect_slot_vacated(FX, 0, TAIL),
        "the fade should have run within {TAIL} frames"
    );
    assert_continuous(
        "the removal itself",
        step_across(
            &mut render,
            1_000,
            |render| {
                let displaced =
                    render.apply_structural(StructuralCommand::RemoveEffect { target: FX, slot: 0 });
                assert!(displaced.is_some(), "the removal should have displaced the filter");
                // Dropped here, in a test; the executor sends it back.
                drop(displaced);
            },
            1_000,
        ),
    );
}

/// A delay taken out of the path and brought back plays none of the repeats
/// it was holding when it went (MOO-108).
#[test]
fn un_bypassing_a_delay_plays_no_repeats_from_before() {
    let mut channel = sine_channel();
    // A short note, so the input is silent long before the delay returns.
    channel.notes[0].clear();
    channel.notes[0].push(NoteEvent::new(1, 0, 24, 57, 127));
    channel
        .setup
        .push_effect(EffectSlotState::new(EffectParams::Delay(DelayParams {
            time_ms: 250.0,
            tempo_sync: false,
            feedback: 0.8,
            mix: 1.0,
            ..DelayParams::default()
        })))
        .expect("room");
    let mut render = render_of(vec![channel]);
    // The delay is all wet, so the first thing out of it is the first
    // repeat, a quarter of a second in; by 18 000 frames the note and its
    // release are over and the repeats are ringing.
    let (ringing, _) = render_frames(&mut render, 18_000);
    assert!(
        ringing[12_000..].iter().any(|sample| sample.abs() > 0.01),
        "the delay has to be ringing when it is bypassed"
    );
    render.apply_command(EngineCommand::SetEffectBypassed {
        target: FX,
        slot: 0,
        bypassed: true,
    });
    // Half a second out of the path: the fade runs, then the device stops.
    render_frames(&mut render, 24_000);
    render.apply_command(EngineCommand::SetEffectBypassed {
        target: FX,
        slot: 0,
        bypassed: false,
    });
    let (after, _) = render_frames(&mut render, 48_000);
    let loudest = after.iter().fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    assert!(
        loudest < 1.0e-4,
        "the delay came back playing repeats from before its bypass, peak {loudest}"
    );
}

// --- Effects: MOO-172 --------------------------------------------------------

/// The 150 Hz filter the MOO-108 cases run through, as a structural install
/// carries it: built off the audio thread, with its own dry ring.
fn install_filter(slot: u8, device: u32, mode: FilterMode) -> StructuralCommand {
    let effect = EffectSlotState::new(EffectParams::Filter(FilterParams {
        cutoff_hz: 150.0,
        resonance: 0.0,
        mode,
        ..FilterParams::default()
    }))
    .with_id(mooloop_core::DeviceId(device));
    let node = mooloop_dsp::build_effect_at_tempo(effect.params, SAMPLE_RATE, 120.0);
    let align = mooloop_dsp::IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
    StructuralCommand::InstallEffect {
        target: FX,
        slot,
        kind: EffectKind::Filter,
        resource_key: None,
        node,
        align,
        analyzer: Box::new(mooloop_dsp::SpectrumAnalyzer::new()),
        state: Box::new(crate::EffectSlot::for_effect(&effect)),
    }
}

/// Adding a device to a chain that is playing brings it in along the bypass
/// crossfade rather than switching the sound over in one sample.
#[test]
fn installing_an_effect_is_continuous() {
    let mut render = playing(&sine_project());
    assert_continuous(
        "installing a filter into a sounding chain",
        step_across(
            &mut render,
            LEAD,
            |render| {
                let displaced =
                    render.apply_structural(install_filter(0, 900, FilterMode::LowPass));
                assert!(displaced.is_none(), "an install into an empty slot displaces nothing");
            },
            TAIL,
        ),
    );
}

/// Replacing what is in a slot -- a preset loaded into a row -- is the
/// removal's first moment and then the install: the executor holds the
/// install until the occupant has faded out (`effect_slot_vacated`), and the
/// new device then fades in. Both are held to the bound.
#[test]
fn installing_over_an_effect_is_continuous() {
    let mut render = filtered_sine(|_| {});
    assert_continuous(
        "the fade an install over an occupied slot starts",
        step_across(
            &mut render,
            LEAD,
            |render| assert!(!render.effect_slot_vacated(FX, 0, 0)),
            TAIL,
        ),
    );
    assert!(
        render.effect_slot_vacated(FX, 0, TAIL),
        "the occupant should have faded out within {TAIL} frames"
    );
    assert_continuous(
        "the install itself, and the new device fading in",
        step_across(
            &mut render,
            1_000,
            |render| {
                let displaced =
                    render.apply_structural(install_filter(0, 901, FilterMode::LowPass));
                assert!(displaced.is_some(), "the install should have displaced the first low-pass");
                drop(displaced);
            },
            TAIL,
        ),
    );
}

/// A container preset replacing a run, in the order the interface mirrors
/// it (`install_loaded_run`): the old box is bypassed, its rows go last
/// first once it is out of the path, and the new box and its row arrive in
/// the holes they left, the row fading in.
#[test]
fn replacing_a_containers_run_is_continuous() {
    let mut channel = sine_channel();
    channel
        .setup
        .push_effect(EffectSlotState::new(EffectParams::Filter(FilterParams {
            cutoff_hz: 150.0,
            resonance: 0.0,
            mode: FilterMode::LowPass,
            ..FilterParams::default()
        })))
        .expect("room");
    let setup = &mut channel.setup;
    mooloop_core::wrap_in_container(
        &mut setup.effects,
        &mut setup.next_device_id,
        0..1,
        EffectSlotState::of_kind(EffectKind::Chain),
    )
    .expect("wrapped");
    let mut render = render_of(vec![channel]);
    assert_continuous(
        "the old box fading out",
        step_across(
            &mut render,
            LEAD,
            |render| {
                render.apply_command(EngineCommand::SetEffectBypassed {
                    target: FX,
                    slot: 0,
                    bypassed: true,
                });
                assert!(!render.effect_slot_vacated(FX, 1, 0), "the box has not faded yet");
            },
            TAIL,
        ),
    );
    assert!(
        render.effect_slot_vacated(FX, 1, TAIL),
        "a row inside a box that is out of the path is not heard"
    );
    assert_continuous(
        "the new run arriving",
        step_across(
            &mut render,
            1_000,
            |render| {
                drop(render.apply_structural(StructuralCommand::RemoveEffect { target: FX, slot: 1 }));
                assert!(render.effect_slot_vacated(FX, 0, 0), "the box is out of the path");
                drop(render.apply_structural(StructuralCommand::RemoveEffect { target: FX, slot: 0 }));
                let head = EffectSlotState::of_kind(EffectKind::Chain)
                    .with_id(mooloop_core::DeviceId(910));
                let node = mooloop_dsp::build_effect_at_tempo(head.params, SAMPLE_RATE, 120.0);
                assert!(render
                    .apply_structural(StructuralCommand::InstallEffect {
                        target: FX,
                        slot: 0,
                        kind: EffectKind::Chain,
                        resource_key: None,
                        node,
                        align: None,
                        analyzer: Box::new(mooloop_dsp::SpectrumAnalyzer::new()),
                        state: Box::new(crate::EffectSlot::for_effect(&head)),
                    })
                    .is_none());
                assert!(render
                    .apply_structural(install_filter(1, 911, FilterMode::HighPass))
                    .is_none());
                drop(render.apply_structural(StructuralCommand::SetContainerSpan {
                    target: FX,
                    slot: 0,
                    children: 1,
                    align: None,
                    scratch: None,
                }));
            },
            TAIL,
        ),
    );
}

/// A device swapped in under a bypassed slot stays bypassed: the fade that
/// made room for it is not the user's bypass. Bypassed, the chain is the
/// dry path, so it renders exactly what the bypassed low-pass did.
#[test]
fn installing_over_a_bypassed_effect_keeps_it_bypassed() {
    const LEAD_IN: usize = 4_800;
    const AFTER: usize = 9_600;
    let mut render = filtered_sine(|effect| effect.bypassed = true);
    render_frames(&mut render, LEAD_IN);
    assert!(
        render.effect_slot_vacated(FX, 0, 0),
        "a bypassed slot is already out of the path"
    );
    let mut install = install_filter(0, 902, FilterMode::HighPass);
    if let StructuralCommand::InstallEffect { state, .. } = &mut install {
        // The occupant's host controls, as an ordinary swap inherits them.
        **state = crate::EffectSlot::for_device(mooloop_core::DeviceId(902));
    }
    drop(render.apply_structural(install));
    let (swapped, _) = render_frames(&mut render, AFTER);

    let mut reference = filtered_sine(|effect| effect.bypassed = true);
    render_frames(&mut reference, LEAD_IN);
    let (bypassed, _) = render_frames(&mut reference, AFTER);
    let worst = swapped
        .iter()
        .zip(&bypassed)
        .fold(0.0f32, |worst, (a, b)| worst.max((a - b).abs()));
    assert!(
        worst < 1.0e-6,
        "the swapped-in device is in the path: it differs from the bypassed chain by {worst}"
    );
}

// --- Effects: MOO-137 --------------------------------------------------------

/// **An undo that changed one device keeps the rest of the channel
/// sounding.** Every undo is a whole-project install. A channel whose chain
/// differed in any device used to be rebuilt, which cut its voice and
/// emptied every device on it. Now it is carried: the voice keeps playing,
/// the delay keeps its repeats, and only the device that changed is new --
/// here an EQ band moved while it sits at 0 dB, which changes nothing
/// audible, so any step is the install's.
#[test]
fn an_install_that_changed_one_device_keeps_the_channel_sounding() {
    let mut channel = sine_channel();
    channel
        .setup
        .push_effect(EffectSlotState::new(EffectParams::Delay(DelayParams {
            time_ms: 120.0,
            tempo_sync: false,
            feedback: 0.5,
            mix: 0.5,
            ..DelayParams::default()
        })))
        .expect("room");
    channel
        .setup
        .push_effect(EffectSlotState::of_kind(EffectKind::Eq))
        .expect("room");
    let mut project = Project {
        channels: vec![channel],
        pattern_lengths: vec![DEFAULT_STEPS],
        ..Project::default()
    };
    // The carry matches channels by identity, as a loaded song has them.
    project.assign_channel_ids();
    let mut edited = project.clone();
    let EffectParams::Eq(eq) = &mut edited.channels[0].setup.effects[1].params else {
        panic!("the second row is the EQ");
    };
    eq.bands[0].frequency_hz *= 2.0;
    let plan = crate::carry_plan(&project, &edited);
    assert_eq!(plan.rechained_channels, vec![(0, 0)], "the premise: only the chain changed");

    let mut render = playing(&project);
    assert_continuous(
        "an install that changed one device on a sounding channel",
        step_across(
            &mut render,
            LEAD,
            |render| {
                let mut incoming = RenderState::from_project(SAMPLE_RATE, &edited, &[]);
                incoming.adopt_performance_state(render);
                incoming.carry_strips_from(render, &plan);
                std::mem::swap(render, &mut incoming);
            },
            TAIL,
        ),
    );
}

// --- Engine: MOO-213 ---------------------------------------------------------

/// Render `frames` of the master through `live`'s executor in [`BLOCK`]-sized
/// callbacks and a shorter last one, as a driver calls it, keeping every
/// node a swap displaced.
///
/// [`BLOCK`]: crate::render_test_support::BLOCK
fn executor_frames(
    live: &mut crate::plugin_host_tests::Live,
    frames: usize,
    displaced: &mut Vec<Box<dyn mooloop_dsp::AudioNode + Send>>,
) -> (Vec<f32>, Vec<f32>) {
    let block = crate::render_test_support::BLOCK;
    let silence = vec![0.0f32; block];
    let (mut l, mut r) = (vec![0.0f32; block], vec![0.0f32; block]);
    let (mut left, mut right) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        live.executor.process_with_input(
            std::iter::empty(),
            &silence[..n],
            &silence[..n],
            &mut l[..n],
            &mut r[..n],
        );
        left.extend_from_slice(&l[..n]);
        right.extend_from_slice(&r[..n]);
        while live.events.pop().is_ok() {}
        while let Ok(reclaimed) = live.reclaim.pop() {
            match reclaimed {
                crate::StructuralReclaim::Effect(effect) => displaced.extend(effect.node),
                crate::StructuralReclaim::HostedProcessor(node) => displaced.push(node),
                _ => {}
            }
        }
        done += n;
    }
    (left, right)
}

/// **A hosted plugin's processor leaves and rejoins its chain without a
/// step** (MOO-213). The plugin rack swaps a slot between the real
/// processor and the pass-through placeholder with `ReplaceEffect` on every
/// restart, rate change, reinstall and late find, because CLAP gives an
/// instance one processor. Both moves are held to the family's bound.
#[test]
fn swapping_a_hosted_plugin_for_its_placeholder_and_back_is_continuous() {
    let (away, back) = plugin_swapped_away_and_back(0);
    assert_continuous("swapping a hosted plugin for its placeholder", away);
    assert_continuous("swapping the placeholder back for the plugin", back);
}

/// The same with the plugin reporting 64 frames of latency. The fade runs
/// against the slot's dry ring, which is the plugin's latency long, so the
/// placeholder stands in as late as the plugin (the rack's pull-back sends
/// `PluginPlaceholder::with_latency`), the ring outlives the swap, and each
/// incoming node plays its latency at the dry path before it is faded in.
/// A jump in time either side would be a step on the sine.
#[test]
fn swapping_a_latent_hosted_plugin_for_its_placeholder_and_back_is_continuous() {
    assert_eq!(mooloop_test_plugin::LATENCY_STEPS[1], 64, "the premise: step 1 is 64 frames");
    let (away, back) = plugin_swapped_away_and_back(1);
    assert_continuous("swapping a latent plugin for its placeholder", away);
    assert_continuous("swapping the placeholder back for the latent plugin", back);
}

/// The test gain at -12 dB with `latency_step`'s latency, playing the sine,
/// swapped for its placeholder mid-note through the executor as the rack's
/// pull-back sends it (as late as the plugin), and then the same processor
/// swapped back. Returns the two transitions.
///
/// -12 dB is a quarter of the dry level, so wet and dry differ by three
/// quarters of the sine everywhere. Through the executor rather than
/// `step_across` on a renderer, because the executor is what holds a swap
/// back while the slot fades.
fn plugin_swapped_away_and_back(latency_step: u32) -> (Transition, Transition) {
    use crate::plugin_host_tests::{gain_ref, live, open_gain, replace};
    use mooloop_core::PluginSlotState;
    use mooloop_plugin_host::{HostedInstance, Lifeline};

    let mut project = sine_project();
    project.assign_channel_ids();
    let slot = project.add_plugin_slot(PluginSlotState::new(gain_ref()));
    let mut device = EffectSlotState::of_kind(EffectKind::Plugin);
    device.params = EffectParams::Plugin(slot);
    project.channels[0].setup.push_effect(device).expect("room");

    // Opened here, on the test's thread, which is its CLAP main thread; the
    // processor only ever runs on the thread spawned below.
    let mut instance = open_gain(-12.0, latency_step);
    let lifeline = Lifeline::new();
    let processor = instance.build_processor(lifeline.tie()).expect("a processor");
    // Known at activation, which is what building the processor is -- and
    // what the rack's pull-back reads for its placeholder.
    let latency = instance.latency_frames();
    assert_eq!(
        latency,
        mooloop_test_plugin::LATENCY_STEPS[latency_step as usize],
        "the plugin reports the latency it was opened with"
    );
    let mut live = live(RenderState::from_project(SAMPLE_RATE, &project, &[]));
    // Before the first block, as a song opening gets it: nothing has been
    // heard yet, so it goes straight in.
    assert!(live.commands.push(replace(FX, 0, slot, processor)).is_ok());
    assert!(live
        .commands
        .push(crate::RealtimeCommand::Engine(EngineCommand::Play))
        .is_ok());

    let (away, back) = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                crate::executor::prepare_audio_thread();
                let mut displaced = Vec::new();
                let lead = executor_frames(&mut live, LEAD, &mut displaced);
                assert_eq!(displaced.len(), 1, "the placeholder the song opened with came back");
                displaced.clear();
                let placeholder =
                    Box::new(mooloop_dsp::effects::PluginPlaceholder::with_latency(slot, latency));
                assert!(live.commands.push(replace(FX, 0, slot, placeholder)).is_ok());
                let tail = executor_frames(&mut live, TAIL, &mut displaced);
                let away = Transition::measure((&lead.0, &lead.1), (&tail.0, &tail.1));

                let processor = displaced.pop().expect("the processor came back");
                assert!(live.commands.push(replace(FX, 0, slot, processor)).is_ok());
                let again = executor_frames(&mut live, TAIL, &mut displaced);
                // The lead into the second swap is the settled half of the
                // first one's tail: the dry sine, clear of the first move.
                let settled = TAIL / 2;
                let back = Transition::measure(
                    (&tail.0[settled..], &tail.1[settled..]),
                    (&again.0, &again.1),
                );
                drop(displaced);
                drop(live);
                (away, back)
            })
            .join()
            .expect("the audio thread did not panic")
    });
    assert!(lifeline.is_alone(), "the processor came home");
    assert_eq!(instance.misbehaviour(), 0, "no call on the wrong thread");
    (away, back)
}

// --- Engine: MOO-230 ---------------------------------------------------------

/// The sine played by `plugin_source_tests`'s fake instrument (a quarter-
/// level cosine per held note) as the channel's hosted source, through the
/// executor, with the processor swapped in ahead of Play the way a song
/// opening gets it: into a silent source, so at full level at once.
fn hosted_sine() -> (crate::plugin_host_tests::Live, mooloop_core::PluginSlotId) {
    use crate::plugin_source_tests::{fake, fake_ref};
    use mooloop_core::{ChannelSource, DeviceKind, PluginSlotState};

    let mut project = sine_project();
    let slot = project.add_plugin_slot(PluginSlotState::new(fake_ref()));
    project.channels[0].setup.source = ChannelSource::Plugin(slot);
    project.channels[0].setup.channel.kind = DeviceKind::Plugin;
    project.assign_channel_ids();
    let mut live = crate::plugin_host_tests::live(RenderState::from_project(SAMPLE_RATE, &project, &[]));
    assert!(live.commands.push(source_swap(slot, Some(fake()))).is_ok());
    assert!(live
        .commands
        .push(crate::RealtimeCommand::Engine(EngineCommand::Play))
        .is_ok());
    (live, slot)
}

/// The lead into a hosted-source change, ending on a crest.
///
/// Not [`LEAD`]: the fake is a *cosine* from its note-on, a quarter cycle
/// ahead of the mono synth's sine, so fifty-five and a quarter cycles lands
/// it on a zero crossing, the trap `LEAD`'s own note describes. The first
/// draft of these cases passed with the hold switched off for exactly that.
/// Fifty-five frames more is a quarter of 220 Hz's 218-frame cycle, and the
/// assertion keeps it true.
fn hosted_lead(
    live: &mut crate::plugin_host_tests::Live,
    displaced: &mut Vec<Box<dyn mooloop_dsp::AudioNode + Send>>,
) -> (Vec<f32>, Vec<f32>) {
    let lead = executor_frames(live, LEAD + 55, displaced);
    let peak = lead.0.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let last = lead.0.last().copied().unwrap_or(0.0).abs();
    assert!(last > 0.95 * peak, "the change lands off the crest: {last} of {peak}");
    lead
}

fn source_swap(
    slot: mooloop_core::PluginSlotId,
    node: Option<mooloop_dsp::HostedNode>,
) -> crate::RealtimeCommand {
    crate::RealtimeCommand::Structural(StructuralCommand::HostSourceProcessor {
        channel: 0,
        slot,
        node,
    })
}

/// **A hosted instrument's restart fades it out and back in** (MOO-230).
/// The rack pulls the processor out (`node: None`) on a plugin's restart
/// and on a rate change, then sends the new one. Mid-note, the pull-out
/// fades the channel to silence before the processor leaves, and a
/// processor that arrives already sounding fades in. Both moves are held to
/// the family's bound, the second against the sine as it was before the
/// first, since the silence between says nothing about step size.
#[test]
fn a_hosted_instruments_restart_is_continuous() {
    use crate::plugin_source_tests::fake_holding;

    let (mut live, slot) = hosted_sine();
    let mut displaced = Vec::new();
    let lead = hosted_lead(&mut live, &mut displaced);
    assert!(live.commands.push(source_swap(slot, None)).is_ok());
    let tail = executor_frames(&mut live, TAIL, &mut displaced);
    let away = Transition::measure((&lead.0, &lead.1), (&tail.0, &tail.1));
    assert_continuous("pulling a sounding instrument's processor out", away);
    assert_eq!(displaced.len(), 1, "the processor came back once its fade was over");
    assert!(
        tail.0[TAIL / 2..].iter().all(|&s| s == 0.0),
        "with no processor the channel is silent"
    );

    // The same note the song holds, so the material either side is alike.
    assert!(live.commands.push(source_swap(slot, Some(fake_holding(57)))).is_ok());
    let again = executor_frames(&mut live, TAIL, &mut displaced);
    let returned = Transition::measure((&tail.0[TAIL - 1..], &tail.1[TAIL - 1..]), (&again.0, &again.1));
    assert_continuous(
        "a sounding processor arriving",
        Transition {
            before: away.before,
            after: returned.after,
            peak: away.peak,
            across: returned.across,
        },
    );
    assert!(returned.after > 0.0, "the incoming processor is heard");
}

/// **Swapping one processor for another fades between them, and the notes
/// the outgoing one held end with it** (MOO-230). The incoming processor is
/// a new instance with no voices: the song's note, still held, is not
/// struck again half-way through, and its release later reaches an instance
/// that never started it, which ignores it. So after the fade the channel is
/// silent until the song's next note-on.
#[test]
fn swapping_a_hosted_instruments_processor_for_another_is_continuous() {
    use crate::plugin_source_tests::fake;

    let (mut live, slot) = hosted_sine();
    let mut displaced = Vec::new();
    let lead = hosted_lead(&mut live, &mut displaced);
    assert!(live.commands.push(source_swap(slot, Some(fake()))).is_ok());
    let tail = executor_frames(&mut live, TAIL, &mut displaced);
    assert_continuous(
        "swapping a sounding instrument's processor for another",
        Transition::measure((&lead.0, &lead.1), (&tail.0, &tail.1)),
    );
    assert_eq!(displaced.len(), 1, "the outgoing processor came back");
    assert!(
        tail.0[TAIL / 2..].iter().all(|&s| s == 0.0),
        "the held note is not struck again on the incoming processor"
    );
}
