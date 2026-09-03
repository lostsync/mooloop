//! DS-01 through the engine.
//!
//! `mooloop_dsp::ds01` tests the device against its own `process`; these test
//! the three claims in `docs/plans/drum-synth-v2/02-the-voice-and-the-descriptor-table.md`
//! that are only true of the *assembled* program — that it plays from a
//! pattern, that a channel modulator reaches it, and that what it renders does
//! not depend on how the audio is cut into blocks.
//!
//! The last of those is what "renders identically offline and live" means
//! mechanically. An offline render and a realtime callback differ in exactly
//! one thing that DS-01 can see: the block size. If the two agree sample for
//! sample at two very different block sizes, they agree.

use crate::render::RenderState;
use mooloop_core::{
    ds01, AutomationLane, AutomationPoint, Ds01Params, EffectTarget, ModLfoParams,
    ModLfoWaveform, ModPolarity, ModRoute, ModulatorParams, NoteEvent, ParamAddr, ParamOwner,
    Project, ProjectChannel,
};

const SAMPLE_RATE: u32 = 48_000;

/// A one-channel project whose DS-01 plays four hits in the first bar.
fn ds01_project(params: Ds01Params) -> Project {
    let mut channel = ProjectChannel::ds01(0, 1);
    channel.setup.channel.volume = 1.0;
    if let Some(state) = channel.setup.source.ds01_state_mut() {
        state.params = params;
    }
    for (index, tick) in [0, 96, 192, 288].into_iter().enumerate() {
        channel.notes[0].push(NoteEvent::new(index as u32 + 1, tick, 48, 60, 110));
    }
    Project {
        channels: vec![channel],
        ..Project::default()
    }
}

/// Render `seconds` of a project in fixed `block` frames and return the master
/// left channel.
fn render_blocks(project: &Project, seconds: f32, block: usize) -> Vec<f32> {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut out = Vec::new();
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    while remaining > 0 {
        let frames = remaining.min(block);
        render.process_once_block(frames);
        out.extend_from_slice(&render.master().l[..frames]);
        remaining -= frames;
    }
    out
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()))
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// It plays from a pattern, through the whole program rather than through its
/// own `process`.
#[test]
fn ds01_plays_from_a_pattern() {
    let out = render_blocks(&ds01_project(Ds01Params::default()), 1.0, 256);
    assert!(out.iter().all(|s| s.is_finite()));
    assert!(peak(&out) > 0.05, "the pattern is inaudible at {}", peak(&out));
    assert!(peak(&out) <= 1.0, "the pattern peaked at {}", peak(&out));
}

/// What DS-01 renders does not depend on how the audio is cut into blocks —
/// which is what makes an offline render and a live take of the same event
/// stream the same samples.
///
/// It is not free: the device resolves its own matrix and its envelopes on a
/// control tick it walks itself, and that grid restarts at each block. It
/// lands on the same absolute frames anyway because every block boundary is a
/// multiple of the control rate — which is true of every driver buffer size
/// (they are powers of two) and of the offline renderer's chunking. A block
/// that was not would shift the grid within it, so that is the condition this
/// property rests on rather than an unconditional guarantee.
#[test]
fn ds01_renders_the_same_at_any_block_size() {
    let params = Ds01Params {
        noise_level: 0.6,
        body_level: 0.5,
        burst_repeats: 3,
        ..Ds01Params::default()
    };
    let project = ds01_project(params);
    let small = render_blocks(&project, 1.0, 128);
    let large = render_blocks(&project, 1.0, 1024);
    assert_eq!(small.len(), large.len());
    let worst = small
        .iter()
        .zip(large.iter())
        .fold(0.0_f32, |worst, (a, b)| worst.max((a - b).abs()));
    assert!(worst == 0.0, "block size changed the render by {worst}");
}

/// The acceptance case the whole plan exists for: a channel LFO assigned to
/// Filter Cutoff sweeps a hat pattern.
///
/// v1's drum synth cannot be reached by a channel modulator at all — its
/// parameters have no ids — so this is the difference DS-01 was built to make,
/// and it is asserted through the real modulation rack rather than by calling
/// the device directly.
#[test]
fn a_channel_lfo_sweeps_ds01s_filter() {
    let hat = Ds01Params {
        tone_level: 0.0,
        noise_level: 1.0,
        filter_morph: 0.0,
        filter_cutoff: 6_000.0,
        ..Ds01Params::default()
    };
    let plain = render_blocks(&ds01_project(hat), 1.0, 256);

    let mut project = ds01_project(hat);
    let channel = &mut project.channels[0];
    channel
        .setup
        .modulation
        .install(
            0,
            ModulatorParams::Lfo(ModLfoParams {
                waveform: ModLfoWaveform::Triangle,
                rate_hz: 2.0,
                depth: 1.0,
                ..ModLfoParams::default()
            }),
        )
        .expect("a fresh rack has a free slot");
    channel
        .setup
        .modulation
        .add_route(ModRoute {
            // `add_route` stamps the durable id from the slot, so the one
            // written here is a placeholder rather than an authored value.
            source: mooloop_core::ModSourceId(0),
            source_slot: 0,
            destination: ParamAddr {
                scope: EffectTarget::Channel(0),
                owner: ParamOwner::Source,
                param: ds01::PARAM_FILTER_CUTOFF,
            },
            depth: -0.7,
            polarity: ModPolarity::Bipolar,
        })
        .expect("the destination is a legal one");
    let swept = render_blocks(&project, 1.0, 256);

    assert_ne!(plain, swept, "the LFO did not reach the filter");
    // A sweep, not an offset: closing the low-pass over the bar takes the
    // brightness with it, so the later hits are duller than the earlier ones
    // by more than they are in the unmodulated take.
    let brightness = |window: &[f32]| {
        let level = rms(window);
        if level <= 0.0 {
            return 0.0;
        }
        let difference: Vec<f32> = window.windows(2).map(|w| w[1] - w[0]).collect();
        rms(&difference) / level
    };
    let quarter = SAMPLE_RATE as usize / 4;
    let plain_fall = brightness(&plain[..quarter]) / brightness(&plain[quarter..quarter * 2]).max(1e-9);
    let swept_fall = brightness(&swept[..quarter]) / brightness(&swept[quarter..quarter * 2]).max(1e-9);
    assert!(
        swept_fall > plain_fall * 1.1,
        "the filter did not sweep: {plain_fall} against {swept_fall}"
    );
}

/// An automation lane on Tone Pitch reaches the device and survives being cut
/// into blocks differently, which is `02`'s "renders identically offline and
/// live" for a drawn curve rather than for a knob.
#[test]
fn an_automation_lane_on_ds01_renders_the_same_at_any_block_size() {
    let mut project = ds01_project(Ds01Params::default());
    let channel = &mut project.channels[0];
    let mut lane = AutomationLane::new(ParamAddr {
        scope: EffectTarget::Channel(0),
        owner: ParamOwner::Source,
        param: ds01::PARAM_TONE_PITCH,
    });
    lane.upsert(AutomationPoint::new(1, 0, 0.2));
    lane.upsert(AutomationPoint::new(2, 384, 0.9));
    channel.automation[0].push(lane);

    let plain = render_blocks(&ds01_project(Ds01Params::default()), 1.0, 256);
    let small = render_blocks(&project, 1.0, 128);
    let large = render_blocks(&project, 1.0, 1024);

    assert_ne!(plain, small, "the lane did not reach the device");
    assert_eq!(small.len(), large.len());
    let worst = small
        .iter()
        .zip(large.iter())
        .fold(0.0_f32, |worst, (a, b)| worst.max((a - b).abs()));
    assert!(worst == 0.0, "block size changed the automated render by {worst}");
}
