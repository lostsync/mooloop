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
    EngineCommand, MonoSynthParams, NoteEvent, OscParams, OscWave, Project, ProjectChannel,
    DEFAULT_STEPS,
};
use mooloop_dsp::SampleData;

use crate::render::RenderState;
use crate::render_test_support::{step_across, Transition, SAMPLE_RATE};

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
