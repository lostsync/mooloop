//! Skipping idle devices and idle channels, through the assembled program.
//!
//! `mooloop_dsp` holds each device to its own rest and tail contract. These
//! test the two claims that are only true of the whole engine: that a project
//! rendered with idle channels skipped sounds like the same project rendered
//! with every channel running, and that the skipping does not make what comes
//! out depend on how the audio is cut into blocks.
//!
//! The second one is easy to lose and expensive to notice. An offline render
//! and a realtime callback differ in exactly one thing a device can see — the
//! block size — and a skip decision taken at a block boundary is a decision
//! whose timing the block size sets. Everything a sleeping device still runs
//! on the clock has to arrive in the same place either way, or a bounce stops
//! matching a take.

use mooloop_core::{
    mlp8, AudioSubscription, AuxInParams, EffectKind, EffectSlotState, NoteEvent, Project,
    ProjectChannel, ReverbParams,
};
use mooloop_dsp::SILENCE_PEAK;

use crate::render::RenderState;

const SAMPLE_RATE: u32 = 48_000;

/// Three channels with gaps in them: a drum machine with a long reverb and a
/// delay behind it, a polyphonic synth playing two chords a bar apart, and a
/// third channel that is never played at all.
///
/// The gaps are the point. A project with something sounding on every channel
/// at every moment would never exercise any of this, and a project that is
/// silent throughout would not prove that waking works.
fn sparse_project() -> Project {
    let mut drums = ProjectChannel::ds01(0, 1);
    drums.setup.channel.volume = 0.9;
    for (index, tick) in [0, 96, 480, 576].into_iter().enumerate() {
        drums.notes[0].push(NoteEvent::new(index as u32 + 1, tick, 24, 60, 110));
    }
    drums
        .setup
        .effects
        .push(EffectSlotState::new(mooloop_core::EffectParams::Reverb(
            ReverbParams {
                decay_s: 3.0,
                ..ReverbParams::default()
            },
        )));
    drums
        .setup
        .effects
        .push(EffectSlotState::of_kind(EffectKind::Delay));
    drums
        .setup
        .effects
        .push(EffectSlotState::of_kind(EffectKind::Eq));

    let mut keys = ProjectChannel::poly_synth(1, 1);
    keys.setup.channel.volume = 0.7;
    for (index, note) in [60u8, 64, 67].into_iter().enumerate() {
        keys.notes[0].push(NoteEvent::new(index as u32 + 10, 0, 48, note, 100));
        keys.notes[0].push(NoteEvent::new(index as u32 + 20, 384, 48, note + 5, 100));
    }
    keys.setup
        .effects
        .push(EffectSlotState::of_kind(EffectKind::Compressor));

    let mut silent = ProjectChannel::sampler(2, 1);
    silent.setup.channel.volume = 0.8;
    silent
        .setup
        .effects
        .push(EffectSlotState::of_kind(EffectKind::Plate));

    Project {
        channels: vec![drums, keys, silent],
        ..Project::default()
    }
}

/// Render `seconds` of a project in fixed `block` frames and return the master
/// left channel, with idle skipping on or off.
fn render_blocks(project: &Project, seconds: f32, block: usize, skip: bool) -> Vec<f32> {
    render_counting(project, seconds, block, skip).0
}

/// The render, and how many channel-blocks it skipped.
fn render_counting(
    project: &Project,
    seconds: f32,
    block: usize,
    skip: bool,
) -> (Vec<f32>, u64, u64) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.set_idle_skipping(skip);
    render.play();
    let mut out = Vec::new();
    let mut blocks = 0u64;
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    while remaining > 0 {
        let frames = remaining.min(block);
        render.process_once_block(frames);
        out.extend_from_slice(&render.master().l[..frames]);
        remaining -= frames;
        blocks += 1;
    }
    let channel_blocks = blocks * project.channels.len() as u64;
    (out, render.slept_strip_blocks(), channel_blocks)
}

fn worst_difference(a: &[f32], b: &[f32]) -> (f32, usize) {
    assert_eq!(a.len(), b.len());
    let mut worst = 0.0f32;
    let mut at = 0;
    for (index, (x, y)) in a.iter().zip(b).enumerate() {
        if (x - y).abs() > worst {
            worst = (x - y).abs();
            at = index;
        }
    }
    (worst, at)
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
}

/// The test that matters most: the same project, rendered twice, once with
/// every idle channel skipped and once with every channel running.
///
/// It has reverbs, a delay and long release tails in it on purpose — those are
/// what a skip taken too early would cut off — and it runs past the end of the
/// pattern so that everything falls asleep before the loop brings it back.
#[test]
fn a_project_renders_the_same_whether_or_not_idle_channels_are_skipped() {
    let project = sparse_project();
    let (slept, skipped, channel_blocks) = render_counting(&project, 12.0, 256, true);
    let ran = render_blocks(&project, 12.0, 256, false);
    assert!(peak(&ran) > 0.05, "the comparison is against silence");
    // Otherwise this is the same render twice and proves nothing. A quarter
    // is a floor rather than a figure -- the measured share is about two
    // fifths -- and most of what is *not* skipped is the reverb and plate
    // tails these channels are carrying, which is exactly the work this
    // mechanism must not cut short.
    assert!(
        skipped * 4 > channel_blocks,
        "only {skipped} of {channel_blocks} channel-blocks were skipped, so \
         this comparison is not measuring the thing it is named for"
    );

    let (worst, at) = worst_difference(&slept, &ran);
    assert!(
        worst <= SILENCE_PEAK * 16.0,
        "skipping idle channels changed the master by {worst} at frame {at} \
         ({:.3} s in)",
        at as f32 / SAMPLE_RATE as f32
    );
}

/// And the skipping does not make the render depend on the block size, which
/// is what "an export is the same as a take" comes down to.
///
/// Asserted exactly, like every other block-size test here: a difference of a
/// single bit would mean something a sleeping device runs on the clock is
/// being advanced in strides rather than in samples.
#[test]
fn skipping_renders_the_same_at_any_block_size() {
    let project = sparse_project();
    let small = render_blocks(&project, 6.0, 128, true);
    let large = render_blocks(&project, 6.0, 1024, true);
    assert!(peak(&small) > 0.05, "the comparison is against silence");
    assert_eq!(small.len(), large.len());
    let (worst, at) = worst_difference(&small, &large);
    assert!(
        worst == 0.0,
        "block size changed the render by {worst} at frame {at}"
    );
}

/// A channel that is never played contributes nothing, and skipping it does
/// not change that. Rendered against the same project with the channel taken
/// out, the two masters have to be identical sample for sample — which is
/// also the strongest available statement that a sleeping strip really does
/// stop reaching its bus.
#[test]
fn a_channel_that_never_sounds_reaches_the_master_either_way() {
    let with_silent = sparse_project();
    let mut without = sparse_project();
    without.channels.pop();

    let a = render_blocks(&with_silent, 4.0, 256, true);
    let b = render_blocks(&without, 4.0, 256, true);
    let (worst, at) = worst_difference(&a, &b);
    assert!(
        worst == 0.0,
        "a silent channel changed the master by {worst} at frame {at}"
    );
}

/// Aux In is the one generator that declines the whole mechanism, because its
/// sound is another channel's and it can start without an event of its own.
///
/// So the consumer keeps rendering however quiet its own recent history was,
/// and the way to see that is to mute the producer: with nothing else reaching
/// the master, every sample of this render arrives through the edge. A note,
/// a second and a half of silence long enough to put anything to sleep, and
/// then another note that has to be heard.
#[test]
fn an_aux_in_channel_is_never_slept_out_of_its_producer() {
    let mut producer = ProjectChannel::mlp8(0, 1);
    producer.setup.channel.volume = 1.0;
    // Muted, so the master carries the edge and nothing else. A muted
    // producer still fills its tap; `audio_edge_tests` is where that is
    // established.
    producer.setup.channel.muted = true;
    producer.notes[0].push(NoteEvent::new(1, 0, 24, 60, 110));
    producer.notes[0].push(NoteEvent::new(2, 288, 48, 60, 110));

    let mut params = AuxInParams {
        level: 1.0,
        ..AuxInParams::default()
    };
    params.set_subscription(Some(AudioSubscription::new(0, mlp8::OUTLET_OSC1)));
    let mut aux = ProjectChannel::aux_in_with_params(1, 1, params);
    aux.setup.channel.volume = 1.0;
    let project = Project {
        channels: vec![producer, aux],
        ..Project::default()
    };

    let out = render_blocks(&project, 2.5, 256, true);
    // Tick 288 is three beats, which at 120 bpm is a second and a half in.
    let late = &out[(1.6 * SAMPLE_RATE as f32) as usize..];
    assert!(
        peak(late) > 1.0e-3,
        "the Aux In went to sleep and missed its producer's second note: {}",
        peak(late)
    );
}
