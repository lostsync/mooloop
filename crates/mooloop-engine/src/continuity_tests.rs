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
