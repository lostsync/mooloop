//! The master's output guard, end to end (MOO-93).
//!
//! `mooloop_dsp::output_guard` holds the stage's own arithmetic -- the
//! transparency below the ceiling, the ceiling above it. These hold what the
//! engine does with it: that the driver's ports are downstream of it, that a
//! fault reaches the interface as a latched count, and that the master's meter
//! is not fooled by what the guard cleans up after it.
//!
//! The broken device is a sampler playing a file of NaN, which is also the
//! most likely way one arrives: nothing checks a decoded float WAV.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use mooloop_core::{NoteEvent, Project, ProjectChannel, MASTER_BUS};
use mooloop_dsp::sampler::SampleData;

use crate::executor::{Executor, ExecutorIo};
use crate::load::LoadMeters;
use crate::meters::BusMeters;
use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;

const BLOCK: usize = 512;

/// A sampler at unity trim playing `sample` from the downbeat, with its
/// channel fader at `volume`.
fn sampler_project(volume: f32) -> Project {
    let mut channel = ProjectChannel::sampler(0, 1);
    channel.setup.channel.volume = volume;
    channel
        .setup
        .sampler_state_mut()
        .expect("a sampler channel")
        .params
        .output_gain = 1.0;
    channel.notes[0].push(NoteEvent::new(1, 0, 96, 60, 127));
    Project {
        channels: vec![channel],
        ..Project::default()
    }
}

fn sample_of(value: impl Fn(usize) -> f32) -> Arc<SampleData> {
    Arc::new(SampleData {
        frames: (0..SAMPLE_RATE as usize / 2)
            .map(|index| {
                let v = value(index);
                [v, v]
            })
            .collect(),
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    })
}

/// An executor driving `render`, with the rings it needs and nothing in
/// them.
fn executor(render: RenderState) -> Executor {
    let (_cmd_tx, cmd_rx) = rtrb::RingBuffer::new(8);
    let (evt_tx, _evt_rx) = rtrb::RingBuffer::new(64);
    let (reclaim_tx, _reclaim_rx) = rtrb::RingBuffer::new(8);
    Executor::new(
        ExecutorIo {
            cmd_rx,
            evt_tx,
            reclaim_tx,
        },
        Box::new(render),
        Arc::new(AtomicU64::new(0)),
        SAMPLE_RATE,
        LoadMeters::new(),
    )
}

/// **A device that emits NaN reaches the speakers as silence, and says so.**
///
/// Shaped against the unfixed tree, where the executor copied the master
/// straight to the ports -- so every sample here was NaN -- and
/// `StereoBus::peak`'s `f32::max` dropped every one of them, so the master's
/// meter read exactly `0.0` while it did.
#[test]
fn a_device_that_emits_nan_reaches_the_ports_as_silence_and_latches_the_fault() {
    let meters = BusMeters::new();
    let mut render = RenderState::from_project(
        SAMPLE_RATE,
        &sampler_project(1.0),
        &[Some(sample_of(|_| f32::NAN))],
    );
    render.attach_meters(meters.clone());
    render.play();
    let mut executor = executor(render);

    let mut out_l = [0.0f32; BLOCK];
    let mut out_r = [0.0f32; BLOCK];
    let mut master_meter = 0.0f32;
    for block in 0..16 {
        out_l.fill(0.5);
        out_r.fill(0.5);
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        for (frame, sample) in out_l.iter().chain(&out_r).enumerate() {
            assert_eq!(
                *sample, 0.0,
                "block {block}, sample {frame}: {sample} reached the ports"
            );
        }
        let (left, right) = meters.take(MASTER_BUS as usize);
        master_meter = master_meter.max(left).max(right);
    }
    assert!(
        meters.output_faults() > 0,
        "the fault did not latch: nothing tells the interface a device blew up"
    );
    assert!(
        master_meter > 1.0,
        "the master meter read {master_meter} for a bus full of NaN"
    );
}

/// The master's meter reads the *mix*, before the limiter: a mix over 0 dBFS
/// lights the clip latch even though nothing over 0 dBFS leaves. That is what
/// tells a user the limiter is working, rather than the limiter hiding it.
#[test]
fn the_master_meter_reads_the_mix_and_the_ports_read_the_ceiling() {
    let meters = BusMeters::new();
    let full_scale = |index: usize| (index as f32 * std::f32::consts::TAU * 220.0 / SAMPLE_RATE as f32).sin();
    // +12 dB on a full-scale file at unity trim: about +9 dBFS after the pan
    // law, far over the ceiling.
    let mut render = RenderState::from_project(
        SAMPLE_RATE,
        &sampler_project(mooloop_core::MAX_LINEAR_GAIN),
        &[Some(sample_of(full_scale))],
    );
    render.attach_meters(meters.clone());
    render.play();
    let mut executor = executor(render);

    let mut out_l = [0.0f32; BLOCK];
    let mut out_r = [0.0f32; BLOCK];
    let mut port_peak = 0.0f32;
    let mut master_meter = 0.0f32;
    for _ in 0..32 {
        executor.process(std::iter::empty(), &mut out_l, &mut out_r);
        port_peak = out_l
            .iter()
            .chain(&out_r)
            .fold(port_peak, |peak, sample| peak.max(sample.abs()));
        let (left, right) = meters.take(MASTER_BUS as usize);
        master_meter = master_meter.max(left).max(right);
    }
    assert!(port_peak <= 1.0, "{port_peak} reached the ports");
    assert!(port_peak > 0.99, "the limiter held the output at {port_peak}");
    assert!(
        master_meter > 2.0,
        "the master meter read {master_meter}; the mix was about +9 dBFS"
    );
    assert_eq!(meters.output_faults(), 0, "an over is not a fault");
}
