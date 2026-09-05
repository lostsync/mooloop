//! Typed audio edges through the assembled program.
//!
//! `mooloop_dsp` tests that a producer fills its taps and that Aux In copies
//! what it is handed. These are the claims that are only true of the whole
//! engine — that the producer runs *first*, that the samples arrive in the
//! same block whatever the block size, that a muted producer still publishes,
//! that a ring refuses itself and gives the edge back when it is broken, and
//! that an unused feature costs nothing.
//!
//! `docs/plans/archive/typed-audio-edges/05-acceptance.md` is the list these come
//! from.

use crate::render::RenderState;
use mooloop_core::{
    aux_in, ds01, mlp8, AudioSubscription, AuxInParams, Ds01Params, MlP8Params, ModLfoParams,
    ModPolarity, ModRoute, ModulatorParams, NoteEvent, ParamAddr, Project, ProjectChannel,
    EffectTarget, STRIP_PARAM_PAN,
};

const SAMPLE_RATE: u32 = 48_000;

/// An ML-P8 channel playing one held note, with `Osc 3` a fifth above the
/// fundamental and **silent in the device's own mix**.
///
/// A distinct interval on purpose: the acceptance case is that the consumer
/// hears a pitch the producer's own output does not contain, and a unison
/// oscillator could not tell the two apart.
fn muted_osc3_producer() -> ProjectChannel {
    let mut params = MlP8Params::default();
    params.osc[1].level = 0.0;
    params.osc[2].level = 0.0;
    params.osc[2].semitones = 7.0;
    // Nothing but the raw oscillators: a filter sweep would put energy at the
    // measured frequency for reasons that have nothing to do with the edge.
    params.filter_cutoff = 1.0;
    params.filter_env_amount = 0.0;
    let mut channel = ProjectChannel::mlp8_with_params(0, 1, params);
    channel.setup.channel.volume = 1.0;
    channel.notes[0].push(NoteEvent::new(1, 0, 384, 60, 127));
    channel
}

/// An Aux In channel subscribed to `(channel, outlet)`.
fn aux_in_channel(index: usize, source: Option<AudioSubscription>) -> ProjectChannel {
    let mut params = AuxInParams {
        level: 1.0,
        ..AuxInParams::default()
    };
    params.set_subscription(source);
    let mut channel = ProjectChannel::aux_in_with_params(index, 1, params);
    channel.setup.channel.volume = 1.0;
    channel
}

/// Render `seconds` of a project in fixed `block` frames; master left.
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

/// Energy in a narrow band around `hz`, from a naive DFT. A band rather than
/// one bin because these renders are neither an integer number of periods nor
/// of constant amplitude.
fn magnitude_at(signal: &[f32], hz: f32) -> f32 {
    let n = signal.len();
    let centre = (hz * n as f32 / SAMPLE_RATE as f32).round() as usize;
    (centre.saturating_sub(2)..=centre + 2)
        .map(|bin| {
            let step = -std::f64::consts::TAU * bin as f64 / n as f64;
            let (mut re, mut im) = (0.0_f64, 0.0_f64);
            for (index, sample) in signal.iter().enumerate() {
                let angle = step * index as f64;
                re += *sample as f64 * angle.cos();
                im += *sample as f64 * angle.sin();
            }
            ((re * re + im * im).sqrt() / n as f64) as f32
        })
        .sum()
}

/// Middle C, and the fifth `Osc 3` is tuned to.
const FUNDAMENTAL_HZ: f32 = 261.626;
const OSC3_HZ: f32 = 392.0;

/// The case both instrument plans are waiting on.
///
/// **Both halves are the test.** An edge that worked by leaking the
/// oscillator into ML-P8's own output would pass the first assertion and fail
/// the second, and that is the failure a peak measurement on the consumer
/// alone would never see.
#[test]
fn an_aux_in_hears_an_oscillator_its_producer_does_not_carry() {
    let alone = Project {
        channels: vec![muted_osc3_producer()],
        ..Project::default()
    };
    let edged = Project {
        channels: vec![
            muted_osc3_producer(),
            aux_in_channel(1, Some(AudioSubscription::new(0, mlp8::OUTLET_OSC3))),
        ],
        ..Project::default()
    };

    let without = render_blocks(&alone, 0.5, 256);
    let with = render_blocks(&edged, 0.5, 256);

    // The producer is playing, so the comparison is against audio.
    assert!(peak(&without) > 0.05, "the producer is silent");
    // Channel 0 does not contain the oscillator: it is muted in ML-P8's own
    // mix, and subscribing to it did not turn it back on.
    let carried = magnitude_at(&without, OSC3_HZ);
    let fundamental = magnitude_at(&without, FUNDAMENTAL_HZ);
    assert!(
        carried < fundamental * 0.02,
        "the producer carries Osc 3 at {carried} against a fundamental of {fundamental}"
    );
    // And channel 1 is audible with it.
    let heard = magnitude_at(&with, OSC3_HZ);
    assert!(
        heard > carried * 20.0,
        "the Aux In heard {heard}, the producer already carried {carried}"
    );
}

/// The mechanism is not ML-P8's, and one device passing is not evidence that
/// the edge type is general.
#[test]
fn the_same_edge_works_through_ds01s_tone() {
    let params = Ds01Params {
        // Silent in DS-01's own mix, so this is the pre-level claim again.
        tone_level: 0.0,
        noise_level: 0.5,
        ..Ds01Params::default()
    };
    let mut producer = ProjectChannel::ds01_with_params(0, 1, params);
    producer.setup.channel.volume = 1.0;
    producer.notes[0].push(NoteEvent::new(1, 0, 96, 48, 120));

    let alone = Project {
        channels: vec![producer.clone()],
        ..Project::default()
    };
    let edged = Project {
        channels: vec![
            producer,
            aux_in_channel(1, Some(AudioSubscription::new(0, ds01::DS01_OUTLET_TONE))),
        ],
        ..Project::default()
    };

    let without = render_blocks(&alone, 0.5, 256);
    let with = render_blocks(&edged, 0.5, 256);
    assert!(peak(&without) > 0.01, "DS-01 is silent without the edge");
    assert!(
        peak(&with) > peak(&without) * 1.2,
        "the Aux In added nothing: {} against {}",
        peak(&with),
        peak(&without)
    );
}

/// The producer's samples reach the consumer in the block they were made in,
/// not the one after.
///
/// The block-size null below would catch a deferral too -- a delay whose
/// length is the buffer size renders differently at 128 and 512 -- but it
/// catches it as an inequality between two runs rather than as a statement
/// about *when* the audio arrives. This is the alignment assertion, and it
/// fails by exactly one block if delivery ever becomes deferred.
///
/// The frame is not computed from the tick; it is measured from the same note
/// heard directly, so the test calibrates itself against the engine's own
/// scheduling rather than against a second derivation of it.
#[test]
fn the_consumers_copy_lands_in_the_frame_the_producer_made_it() {
    // A note part-way into the first bar, so "frame zero" cannot pass by
    // accident and a one-block slip is visible on either side of it.
    let mut heard = muted_osc3_producer();
    heard.notes[0].clear();
    heard.notes[0].push(NoteEvent::new(1, 96, 384, 60, 127));

    let direct = Project {
        channels: vec![heard.clone()],
        ..Project::default()
    };
    let mut producer = heard;
    producer.setup.channel.muted = true;
    let through_edge = Project {
        channels: vec![
            producer,
            // `Osc 1` rather than `Osc 3`: this is about *when* the samples
            // arrive, so the outlet that carries the note's own fundamental
            // is the one to compare against hearing it directly.
            aux_in_channel(1, Some(AudioSubscription::new(0, mlp8::OUTLET_OSC1))),
        ],
        ..Project::default()
    };

    let onset = |samples: &[f32]| {
        samples
            .iter()
            .position(|sample| sample.abs() > 1e-6)
            .expect("the note is in the window")
    };
    let direct = render_blocks(&direct, 1.0, 256);
    let through_edge = render_blocks(&through_edge, 1.0, 256);
    assert_eq!(
        onset(&through_edge),
        onset(&direct),
        "the consumer's copy did not land in the frame the producer made it"
    );
}

/// Same block, and the same at any block size.
///
/// This is the assertion a block-latency edge could never pass, and it is why
/// `01-what-this-is.md` rules that design out rather than treating it as a
/// simpler alternative: a delay whose length is the host's buffer size would
/// make the same project render differently at 128 and 512 frames.
#[test]
fn an_edge_renders_the_same_at_any_block_size() {
    let project = Project {
        channels: vec![
            muted_osc3_producer(),
            aux_in_channel(1, Some(AudioSubscription::new(0, mlp8::OUTLET_OSC3))),
        ],
        ..Project::default()
    };
    let small = render_blocks(&project, 0.4, 128);
    let large = render_blocks(&project, 0.4, 512);
    assert!(peak(&small) > 0.01, "the comparison is against silence");
    assert_eq!(small.len(), large.len());
    for (frame, (a, b)) in small.iter().zip(large.iter()).enumerate() {
        assert_eq!(a, b, "frame {frame}: 128 gave {a}, 512 gave {b}");
    }
}

/// The producer must render before the consumer whatever their index order
/// says, which is the whole content of the compiled schedule. Here the
/// producer has the *higher* index, so index order cannot satisfy the edge by
/// accident.
#[test]
fn a_producer_with_a_higher_index_still_renders_first() {
    let mut producer = muted_osc3_producer();
    producer.setup.channel.name = "Producer".into();
    let project = Project {
        channels: vec![
            aux_in_channel(0, Some(AudioSubscription::new(1, mlp8::OUTLET_OSC3))),
            producer,
        ],
        ..Project::default()
    };
    // The note is on channel 1's pattern, so the Aux In on channel 0 hears
    // it only if channel 1 rendered first.
    let out = render_blocks(&project, 0.3, 256);
    assert!(
        magnitude_at(&out, OSC3_HZ) > 0.001,
        "the consumer rendered before its producer and heard the block before"
    );
}

/// Mute is an output-stage decision about what reaches the bus. A producer
/// muted in the mixer is exactly the case a pre-level tap exists for, and the
/// current loop skipped a muted channel before its strip rendered at all --
/// so this is a real change and it gets its own test.
#[test]
fn a_muted_producer_still_fills_its_tap() {
    let mut producer = muted_osc3_producer();
    producer.setup.channel.muted = true;
    let project = Project {
        channels: vec![
            producer,
            aux_in_channel(1, Some(AudioSubscription::new(0, mlp8::OUTLET_OSC3))),
        ],
        ..Project::default()
    };
    let out = render_blocks(&project, 0.3, 256);
    assert!(
        magnitude_at(&out, OSC3_HZ) > 0.001,
        "a muted producer published nothing"
    );

    // And the producer itself reached nothing. Measured by taking the
    // subscription away rather than by looking for its fundamental in a
    // spectrum: with nothing subscribed the master is *exactly* silent, so
    // every sample above is the tap and nothing else.
    let mut unsubscribed = project.clone();
    if let Some(state) = unsubscribed.channels[1].setup.source.aux_in_state_mut() {
        state.params.set_subscription(None);
    }
    assert_eq!(
        peak(&render_blocks(&unsubscribed, 0.3, 256)),
        0.0,
        "a muted producer's own output reached the master"
    );
}

/// A ring has no valid schedule, so both edges are refused and both channels
/// go silent -- and the subscriptions survive, so breaking the ring gives the
/// surviving edge back rather than making the user author it again.
#[test]
fn a_ring_refuses_both_edges_and_gives_one_back_when_it_is_broken() {
    // The producer is muted, so the master carries an edge or nothing at
    // all. Measuring a refusal against a channel that is audible on its own
    // would only ever be a measurement of the spectrum's skirts.
    let mut producer = muted_osc3_producer();
    producer.setup.channel.muted = true;
    let ring = Project {
        channels: vec![
            producer,
            aux_in_channel(1, Some(AudioSubscription::new(2, aux_in::OUTLET_OUT))),
            aux_in_channel(2, Some(AudioSubscription::new(1, aux_in::OUTLET_OUT))),
        ],
        ..Project::default()
    };
    let graph = ring.audio_graph();
    assert_eq!(
        graph.edge(1).refusal(),
        Some(mooloop_core::EdgeRefusal::Cycle)
    );
    assert_eq!(
        graph.edge(2).refusal(),
        Some(mooloop_core::EdgeRefusal::Cycle)
    );
    // Refused, and therefore holding no buffer at all.
    assert_eq!(graph.tap_count(), 0);
    assert_eq!(
        peak(&render_blocks(&ring, 0.3, 256)),
        0.0,
        "a ring was audible"
    );

    // Break it: channel 1 reads the producer instead, and channel 2 keeps the
    // subscription it already had.
    let mut broken = ring.clone();
    if let Some(state) = broken.channels[1].setup.source.aux_in_state_mut() {
        state
            .params
            .set_subscription(Some(AudioSubscription::new(0, mlp8::OUTLET_OSC3)));
    }
    let graph = broken.audio_graph();
    assert!(graph.edge(1).resolved().is_some());
    assert_eq!(
        graph.edge(2).resolved(),
        Some(AudioSubscription::new(1, aux_in::OUTLET_OUT)),
        "the untouched edge did not come back"
    );
    // Channel 1 reads the producer and channel 2 reads channel 1, so hearing
    // anything at all means both edges resolved *and* the three channels
    // rendered in the order the ring's remains imply.
    assert!(
        peak(&render_blocks(&broken, 0.3, 256)) > 0.01,
        "the chain is silent after the ring was broken"
    );
}

/// A three-channel ring, because a two-cycle can be caught by an equality
/// check that a longer ring walks straight past.
#[test]
fn a_three_channel_ring_is_refused_too() {
    let ring = Project {
        channels: vec![
            aux_in_channel(0, Some(AudioSubscription::new(1, aux_in::OUTLET_OUT))),
            aux_in_channel(1, Some(AudioSubscription::new(2, aux_in::OUTLET_OUT))),
            aux_in_channel(2, Some(AudioSubscription::new(0, aux_in::OUTLET_OUT))),
        ],
        ..Project::default()
    };
    let graph = ring.audio_graph();
    for channel in 0..3 {
        assert_eq!(
            graph.edge(channel).refusal(),
            Some(mooloop_core::EdgeRefusal::Cycle),
            "channel {channel}"
        );
    }
    assert_eq!(graph.tap_count(), 0);
    // And every channel still renders, in index order, reading nothing.
    assert_eq!(peak(&render_blocks(&ring, 0.1, 256)), 0.0);
}

/// What the feature costs a project that does not use it, and what one edge
/// costs a project that does.
///
/// The first figure is the one that matters: ML-P8 declares seven stereo
/// outlets, so materialising them unconditionally would be 448 KB a channel
/// and 7 MB across a full bank for something switched off. This is the test
/// that has asked the question twice already and got a design change out of
/// it both times.
#[test]
fn an_unused_edge_costs_no_buffers_at_all() {
    let unused = Project {
        channels: vec![muted_osc3_producer(), aux_in_channel(1, None)],
        ..Project::default()
    };
    assert_eq!(unused.audio_graph().tap_count(), 0);
    // And the order it compiles to is the one the engine has always walked.
    assert_eq!(
        unused.audio_graph().order(),
        &mooloop_core::mixer::identity_audio_order()
    );

    let one = Project {
        channels: vec![
            muted_osc3_producer(),
            aux_in_channel(1, Some(AudioSubscription::new(0, mlp8::OUTLET_OSC3))),
        ],
        ..Project::default()
    };
    assert_eq!(one.audio_graph().tap_count(), 1);

    // Two consumers on the same outlet are still one buffer: storage is
    // keyed by the (producer, outlet) pair, not by the consumer.
    let shared = Project {
        channels: vec![
            muted_osc3_producer(),
            aux_in_channel(1, Some(AudioSubscription::new(0, mlp8::OUTLET_OSC3))),
            aux_in_channel(2, Some(AudioSubscription::new(0, mlp8::OUTLET_OSC3))),
        ],
        ..Project::default()
    };
    assert_eq!(shared.audio_graph().tap_count(), 1);
}

/// A modulator's phase must not depend on a subscription somebody made on
/// another channel, which is why the tick pass stays in index order while the
/// render pass is scheduled.
///
/// Measured through an LFO on the *consumer's* channel driving its pan: if
/// the tick pass had moved into graph order, adding the edge would shift
/// where that LFO stood at every block boundary.
#[test]
fn an_edge_does_not_move_a_modulator_phase() {
    let mut consumer = aux_in_channel(1, None);
    consumer.setup.modulation.install(
        0,
        ModulatorParams::Lfo(ModLfoParams {
            rate_hz: 3.0,
            ..ModLfoParams::default()
        }),
    );
    consumer
        .setup
        .modulation
        .add_route(ModRoute::to_slot(
            0,
            ParamAddr::strip(EffectTarget::Channel(1), STRIP_PARAM_PAN),
            0.8,
            ModPolarity::Bipolar,
        ))
        .expect("the route is legal");

    let unsubscribed = Project {
        channels: vec![muted_osc3_producer(), consumer.clone()],
        ..Project::default()
    };
    let mut subscribed = unsubscribed.clone();
    if let Some(state) = subscribed.channels[1].setup.source.aux_in_state_mut() {
        // Level zero: the edge exists and is scheduled, and contributes no
        // audio, so anything that moves is the modulator and not the signal.
        state.params.level = 0.0;
        state
            .params
            .set_subscription(Some(AudioSubscription::new(0, mlp8::OUTLET_OSC3)));
    }

    let without = render_blocks(&unsubscribed, 0.3, 256);
    let with = render_blocks(&subscribed, 0.3, 256);
    assert!(peak(&without) > 0.01, "the comparison is against silence");
    for (frame, (a, b)) in without.iter().zip(with.iter()).enumerate() {
        assert_eq!(a, b, "frame {frame}: the edge moved the render");
    }
}

/// The offline renderer builds its own `RenderState` and never runs a pump,
/// so it is a second place the audio graph is compiled -- and it is the one a
/// listener never hears until the file is finished. An export that skipped
/// the schedule would render the consumer before its producer and arrive on
/// disk silent, or a block late, with nothing in the room to say so.
///
/// Float32 so the comparison is exact, and an assertion that the window is
/// audible before an assertion that the two agree: the same two rules the
/// compensation plan's acceptance found were necessary.
#[test]
fn an_offline_render_compiles_the_same_audio_graph_as_a_live_one() {
    const FRAMES: usize = 16_384;
    let project = Project {
        channels: vec![
            muted_osc3_producer(),
            aux_in_channel(1, Some(AudioSubscription::new(0, mlp8::OUTLET_OSC3))),
        ],
        ..Project::default()
    };

    let temp = tempfile::tempdir().expect("a temporary directory");
    let path = temp.path().join("edged.wav");
    crate::offline::OfflineRenderer::render(
        &project,
        &[],
        SAMPLE_RATE,
        &crate::offline::ExportSpec {
            path: path.clone(),
            scope: crate::offline::RenderScope::Pattern { index: 0 },
            tail_seconds: 0.0,
            format: crate::offline::ExportFormat::Wav(crate::offline::WavEncoding::Float32),
        },
    )
    .expect("the project renders offline");

    let offline: Vec<f32> = hound::WavReader::open(&path)
        .expect("the export is readable")
        .samples::<f32>()
        .map(|sample| sample.expect("a decoded sample"))
        .step_by(2)
        .take(FRAMES)
        .collect();
    assert_eq!(offline.len(), FRAMES, "the export was shorter than expected");

    let mut live_state = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    live_state.play();
    let mut live = Vec::with_capacity(FRAMES);
    while live.len() < FRAMES {
        live_state.process_block(512);
        live.extend_from_slice(&live_state.master().l[..512]);
    }
    live.truncate(FRAMES);

    // The window has to contain the edge, or two silent buffers would agree
    // perfectly and this would prove nothing.
    assert!(
        magnitude_at(&offline, OSC3_HZ) > 0.001,
        "the export does not contain the edge"
    );
    for (frame, (exported, played)) in offline.iter().zip(live.iter()).enumerate() {
        assert_eq!(
            exported, played,
            "frame {frame}: the export gave {exported}, the live render {played}"
        );
    }
}
