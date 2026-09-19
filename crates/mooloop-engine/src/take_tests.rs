//! Takes at the level they are used: a project rendered block by block, with
//! a channel recording its audio input (`audio-recording/03`).
//!
//! `take.rs` holds the state machine to its arithmetic. What it cannot see is
//! whether the *engine* reads the right buffer at the right moment -- after
//! the source has rendered, before the preview, silence for a source that did
//! not sound -- and that is where a take would be quietly wrong.

use std::sync::Arc;

use arc_swap::ArcSwap;
use mooloop_core::{AudioTap, EngineCommand, NoteEvent, Project, ProjectChannel};

use crate::render::{AudioInputRouting, RenderState};
use crate::render_test_support::SAMPLE_RATE;
use crate::take::{Take, TakeFrame, TakePhase, TakeStatus};
use crate::StructuralCommand;

const BLOCK: usize = 256;
/// One bar at the default 120 bpm in 4/4, at 48 kHz: two seconds.
const BAR_FRAMES: usize = 96_000;

/// `channels` channels, each holding a note for the whole of its one-bar
/// pattern, so every bar sounds.
fn sounding(channels: usize) -> Project {
    let mut project = Project {
        channels: (0..channels)
            .map(|index| {
                let mut channel = ProjectChannel::poly_synth(index, 1);
                channel.setup.channel.volume = 1.0;
                channel.notes[0].push(NoteEvent::new(1, 0, 96 * 4, 45 + index as u8 * 7, 127));
                channel
            })
            .collect(),
        ..Project::default()
    };
    project.assign_channel_ids();
    project
}

fn route(render: &mut RenderState, taps: Vec<Option<AudioTap>>) {
    render.attach_audio_input_routing(Arc::new(ArcSwap::from_pointee(AudioInputRouting { taps })));
}

fn arm(
    render: &mut RenderState,
    channel: u8,
    clip_ticks: Option<u32>,
) -> (Arc<TakeStatus>, rtrb::Consumer<TakeFrame>) {
    let (producer, consumer) = rtrb::RingBuffer::new(1 << 19);
    let status = TakeStatus::new();
    let returned = render.apply_structural(StructuralCommand::StartTake {
        channel,
        take: Take::new(producer, status.clone(), clip_ticks),
    });
    assert!(returned.is_none(), "a take displaced nothing");
    (status, consumer)
}

fn drain(ring: &mut rtrb::Consumer<TakeFrame>) -> Vec<TakeFrame> {
    let mut frames = Vec::new();
    while let Ok(frame) = ring.pop() {
        frames.push(frame);
    }
    frames
}

fn is_silent(frames: &[TakeFrame]) -> bool {
    frames.iter().all(|frame| frame[0] == 0.0 && frame[1] == 0.0)
}

/// **Resampling the master records exactly what the master played**, from
/// the bar line on -- the cheapest proof the read site is right -- and the
/// blocks that record allocate and free nothing.
#[test]
fn resampling_the_master_records_what_the_master_played() {
    assert_eq!(SAMPLE_RATE, 48_000, "BAR_FRAMES assumes it");
    let project = sounding(1);
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    route(&mut render, vec![Some(AudioTap::Master)]);
    let (status, mut ring) = arm(&mut render, 0, None);
    render.play();

    let blocks = BAR_FRAMES / BLOCK + 40;
    let mut master = Vec::with_capacity(blocks * BLOCK);
    let mut counted = None;
    for block in 0..blocks {
        if block == BAR_FRAMES / BLOCK + 1 {
            counted = Some((crate::COUNTING.allocations(), crate::COUNTING.frees()));
        }
        render.process_block(BLOCK);
        let out = render.master();
        for frame in 0..BLOCK {
            master.push([out.l[frame], out.r[frame]]);
        }
    }
    assert_eq!(
        counted,
        Some((crate::COUNTING.allocations(), crate::COUNTING.frees())),
        "recording allocated or freed on the thread that would be the callback"
    );

    assert_eq!(status.phase(), TakePhase::Recording);
    assert_eq!(status.start_tick(), f64::from(mooloop_core::TICKS_PER_BAR));
    let take = drain(&mut ring);
    assert_eq!(take.len(), 40 * BLOCK);
    assert!(!is_silent(&take), "nothing was playing, so this proves nothing");
    assert_eq!(take, master[BAR_FRAMES..BAR_FRAMES + take.len()]);
    assert_eq!(status.dropped(), 0);
}

/// A clip length ends the take by itself, to the frame: one bar is one bar.
#[test]
fn a_one_bar_clip_is_one_bar_long() {
    let project = sounding(1);
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    route(&mut render, vec![Some(AudioTap::Master)]);
    let (status, mut ring) = arm(&mut render, 0, Some(mooloop_core::TICKS_PER_BAR));
    render.play();
    for _ in 0..(3 * BAR_FRAMES / BLOCK) {
        render.process_block(BLOCK);
    }
    assert_eq!(status.phase(), TakePhase::Ended);
    assert_eq!(status.frames(), BAR_FRAMES as u64);
    assert_eq!(drain(&mut ring).len(), BAR_FRAMES);
}

/// A muted source records silence, not the buffer it last held. The source
/// plays first and is muted just before the take starts, so its buffer is
/// full of real audio when it goes quiet -- a source that never sounded would
/// hold zeros anyway and prove nothing.
#[test]
fn a_muted_source_records_silence() {
    let project = sounding(2);
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    route(&mut render, vec![Some(AudioTap::Channel(1)), None]);
    render.play();
    for _ in 0..(BAR_FRAMES / BLOCK - 4) {
        render.process_block(BLOCK);
    }
    render.apply_command(EngineCommand::SetChannelMuted {
        channel: 1,
        muted: true,
    });
    let (status, mut ring) = arm(&mut render, 0, None);
    for _ in 0..24 {
        render.process_block(BLOCK);
    }
    assert_eq!(status.phase(), TakePhase::Recording);
    let take = drain(&mut ring);
    assert!(!take.is_empty());
    assert!(is_silent(&take));
}

/// Two channels take at once, each from its own input, and one of them can
/// be recording the other.
#[test]
fn two_channels_take_at_once() {
    let project = sounding(2);
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    route(
        &mut render,
        vec![Some(AudioTap::Master), Some(AudioTap::Channel(0))],
    );
    let (first, mut first_ring) = arm(&mut render, 0, None);
    let (second, mut second_ring) = arm(&mut render, 1, None);
    render.play();
    for _ in 0..(BAR_FRAMES / BLOCK + 20) {
        render.process_block(BLOCK);
    }
    assert_eq!(first.frames(), second.frames());
    let (a, b) = (drain(&mut first_ring), drain(&mut second_ring));
    assert!(!is_silent(&a) && !is_silent(&b));
    assert_ne!(a, b, "the master and one channel of two are not the same signal");
}

/// The record button pressed again ends the take where it is.
#[test]
fn a_stop_ends_the_take() {
    let project = sounding(1);
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    route(&mut render, vec![Some(AudioTap::Master)]);
    let (status, _ring) = arm(&mut render, 0, None);
    render.play();
    for _ in 0..(BAR_FRAMES / BLOCK + 10) {
        render.process_block(BLOCK);
    }
    render.apply_command(EngineCommand::StopTake { channel: 0 });
    let frames = status.frames();
    render.process_block(BLOCK);
    assert_eq!(status.phase(), TakePhase::Ended);
    assert_eq!(status.frames(), frames, "a stopped take went on recording");
}

/// **A take survives an install that carries its strip**: it is the strip's,
/// so the same take -- the same ring, the same status -- is live in the new
/// generation.
#[test]
fn a_take_rides_its_strip_across_an_install() {
    let project = sounding(2);
    let mut live = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    let (status, _ring) = arm(&mut live, 0, None);

    let mut moved = project.clone();
    moved.move_channel(0, 1).expect("a real move");
    let mut incoming = RenderState::from_project(SAMPLE_RATE, &moved, &[]);
    incoming.carry_strips_from(&mut live, &crate::carry_plan(&project, &moved));

    let carried = incoming.take_status(1).expect("the take moved with its channel");
    assert!(Arc::ptr_eq(&carried, &status));
    assert!(incoming.take_status(0).is_none());
}
