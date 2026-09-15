//! Headless engine self-test. Exercises the full driver path (audio callback,
//! command/event queues, transport, sequencer, sampler) without any GUI.
//!
//! Usage: `cargo run --bin engine-selftest`
//! Makes ~4 s of four-on-the-floor kick noise on system playback.

use mooloop_core::{EngineCommand, EngineEvent};
use mooloop_dsp::{ChannelAudioSnapshot, SampleData};
use mooloop_engine::{CommandSink, Engine};
use std::time::{Duration, Instant};

fn main() {
    let (engine, mut handle) =
        Engine::new(mooloop_engine::AudioConfig::default()).expect("failed to open engine");
    let _keep_alive = engine;
    handle.set_channel_audio(
        0,
        ChannelAudioSnapshot::sample(SampleData::default_kick(handle.sample_rate())),
    );

    // Four-on-the-floor so there's no ambiguity about silence-vs-no-steps.
    for step in [0, 4, 8, 12] {
        assert!(
            handle.send(EngineCommand::SetStep {
                pattern: 0,
                channel: 0,
                step,
                on: true,
                note: 60,
                velocity: 100,
            }),
            "the command queue refused a step on an idle engine"
        );
    }
    assert!(
        handle.send(EngineCommand::Play),
        "the command queue refused the transport"
    );

    let mut max_peak = 0.0f32;
    let mut saw_playing = false;
    let mut last_tick = 0u64;
    let mut position_events = 0usize;
    let mut metering_events = 0usize;
    let mut nonzero_meter_events = 0usize;

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        while let Some(ev) = handle.poll() {
            match ev {
                EngineEvent::Position { tick, playing, .. } => {
                    position_events += 1;
                    saw_playing |= playing;
                    last_tick = last_tick.max(tick);
                }
                EngineEvent::Metering { peak_l, peak_r } => {
                    metering_events += 1;
                    if peak_l > 0.0 || peak_r > 0.0 {
                        nonzero_meter_events += 1;
                    }
                    max_peak = max_peak.max(peak_l).max(peak_r);
                }
                EngineEvent::Xrun { count } => println!("{count} xrun(s) reported"),
                EngineEvent::ProjectInstalled { .. } => unreachable!("filtered engine event"),
                // This harness drives the transport itself and plugs in no
                // control surface, so neither of these can carry a fact the
                // report is about. Unlike `ProjectInstalled` they are *not*
                // filtered by `poll`, so they are ignored rather than refused.
                EngineEvent::ControlInput(_) | EngineEvent::RecordedNote { .. } => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    println!("--- engine selftest report ---");
    println!("position events : {position_events}");
    println!("metering events : {metering_events} (nonzero: {nonzero_meter_events})");
    println!("saw playing     : {saw_playing}");
    println!(
        "last tick       : {last_tick} (~{} expected after 4 s at 120 bpm)",
        4 * 120 * 96 / 60
    );
    println!("max peak        : {max_peak:.4}");

    let ok = saw_playing && last_tick > 500 && position_events > 100 && max_peak > 0.1;
    if ok {
        println!("RESULT: PASS — audio engine produces output end-to-end");
    } else {
        println!("RESULT: FAIL — see values above to locate the broken stage");
    }
    std::process::exit(if ok { 0 } else { 1 });
}
