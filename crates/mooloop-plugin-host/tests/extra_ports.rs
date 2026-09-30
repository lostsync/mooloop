//! A CLAP plugin with ports beyond its main ones, run through the adapter:
//! the test plugin's `mooloop.test.sidechain`, a unity
//! effect with a mono sidechain at input 0, its main stereo input at 1, and
//! outputs `[aux, aux mono, main]`, its main output at 2.
//!
//! What the adapter owes such a plugin: a buffer for every port it declared,
//! the chain's signal on its main input and nowhere else, silence on the
//! sidechain, and its main output -- not an extra one, which it fills with
//! `AUX_LEVEL` -- back on the bus. And none of that may allocate in
//! `process`: this binary counts every allocator call the audio thread
//! makes, frees and reallocations included.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::PathBuf;

use mooloop_core::{PluginFormat, PluginRef, PluginState};
use mooloop_dsp::{Event, EventList, ProcessContext, StereoBus, TimedEvent};
use mooloop_plugin_host::clap::{ClapInstance, MAX_FRAMES};
use mooloop_plugin_host::{AudioConfig, HostedInstance, Lifeline};
use mooloop_test_plugin as test_plugin;

/// Allocator calls on this thread: allocations, frees and reallocations
/// alike, because the callback's contract is that it makes none of any.
struct Counting;

thread_local! {
    static CALLS: Cell<usize> = const { Cell::new(0) };
}

fn count() {
    let _ = CALLS.try_with(|calls| calls.set(calls.get() + 1));
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count();
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static COUNTING: Counting = Counting;

fn allocator_calls() -> usize {
    CALLS.try_with(Cell::get).unwrap_or(0)
}

const SAMPLE_RATE: u32 = 48_000;

/// The test plugin's library, next to the running test (see `tests/spike.rs`).
fn test_plugin_path() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary has a path");
    let deps = exe.parent().expect("the test binary is in a directory");
    let name = format!(
        "{}mooloop_test_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    for dir in [deps, deps.parent().unwrap_or(deps)] {
        let candidate = dir.join(&name);
        if candidate.is_file() {
            return candidate;
        }
    }
    panic!("{name} is not next to {}", exe.display());
}

fn open(id: &str) -> ClapInstance {
    let plugin = PluginRef {
        format: PluginFormat::Clap,
        id: id.to_owned(),
        name: id.to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    };
    ClapInstance::open(
        &test_plugin_path(),
        &plugin,
        &PluginState::default(),
        AudioConfig {
            sample_rate: SAMPLE_RATE,
            max_frames: MAX_FRAMES,
        },
    )
    .unwrap_or_else(|error| panic!("{id} opens: {error:?}"))
}

/// A signal that is different on each side and at every frame, so a
/// swapped side, a lost frame or a stale buffer shows.
fn signal(frame: usize) -> (f32, f32) {
    let t = frame as f32;
    ((t * 0.013).sin() * 0.5, (t * 0.029).cos() * 0.25)
}

/// **An effect with a sidechain and extra outputs runs through its main
/// ports.** Its main output is its main input, unchanged -- so the host fed
/// the right port and took back the right one, at indices 1 and 2 -- its
/// sidechain heard only silence, it failed nothing, and after the first
/// block the audio thread called the allocator not once.
#[test]
fn an_effect_with_a_sidechain_hears_silence_there_and_plays_through_its_main_ports() {
    let mut instance = open(test_plugin::SIDECHAIN_ID);
    assert!(instance.fits_effect(), "the sidechain effect is refused from a chain");
    assert!(!instance.fits_source(), "an effect is offered as a source");
    let life = Lifeline::new();
    let mut node = instance.build_processor(life.tie()).expect("a processor");

    // Uneven block sizes, one the largest the processor holds, and a
    // parameter change mid-block, which cuts the block into two process
    // calls: every piece has to hand over every port.
    let blocks: Vec<usize> = vec![64, 1, 333, MAX_FRAMES as usize, 128, 7, 256];
    // Each block with a change mid-way is two process calls.
    let calls_made = blocks.len() + blocks.iter().filter(|&&frames| frames > 2).count();
    let (heard, sent, counted, calls) = std::thread::scope(|scope| {
        let blocks = &blocks;
        scope
            .spawn(move || {
                let mut bus = StereoBus::with_capacity(MAX_FRAMES as usize);
                let mut events = EventList::empty();
                let mut heard = Vec::new();
                let mut sent = Vec::new();
                heard.reserve(blocks.iter().sum::<usize>() * 2);
                sent.reserve(blocks.iter().sum::<usize>() * 2);
                let mut position = 0;
                let mut counted = 0;
                let mut calls = 0;
                for (index, &frames) in blocks.iter().enumerate() {
                    for frame in 0..frames {
                        let (l, r) = signal(position + frame);
                        bus.l[frame] = l;
                        bus.r[frame] = r;
                        sent.push(l);
                        sent.push(r);
                    }
                    events.clear();
                    if frames > 2 {
                        let _ = events.push(TimedEvent {
                            offset: (frames / 2) as u32,
                            event: Event::ParamValue { id: 1, value: 0.5 },
                        });
                    }
                    let ctx = ProcessContext {
                        sample_rate: SAMPLE_RATE,
                        frames,
                        playing: true,
                        bpm: 120.0,
                        position_ticks: 0.0,
                        position_frames: position as u64,
                    };
                    let before = allocator_calls();
                    node.process(&ctx, &mut bus, &events, None);
                    let made = allocator_calls() - before;
                    // The first block starts the plugin processing, which
                    // is the plugin's own affair; every block after it is
                    // the adapter's alone.
                    if index > 0 {
                        counted += 1;
                        calls += made;
                    }
                    for frame in 0..frames {
                        heard.push(bus.l[frame]);
                        heard.push(bus.r[frame]);
                    }
                    position += frames;
                }
                drop(node);
                (heard, sent, counted, calls)
            })
            .join()
            .expect("the audio thread did not panic")
    });
    assert!(life.is_alone(), "the processor outlived its thread");
    assert!(!instance.failed(), "the plugin failed: it was not handed every port");
    assert_eq!(instance.misbehaviour(), 0, "the plugin reported the host misbehaving");
    assert_eq!(calls, 0, "{counted} blocks called the allocator {calls} times");

    assert_eq!(heard.len(), sent.len());
    let worst = heard
        .iter()
        .zip(&sent)
        .fold(0.0f32, |worst, (a, b)| worst.max((a - b).abs()));
    assert!(worst == 0.0, "the main output is not the main input: off by {worst}");
    assert!(
        heard.iter().all(|&sample| sample != test_plugin::AUX_LEVEL),
        "an extra output reached the bus"
    );

    let blocks_seen = instance
        .param_value(test_plugin::PROBE_SIDECHAIN_BLOCKS)
        .expect("the probe reads");
    assert_eq!(blocks_seen, calls_made as f64, "the sidechain was not delivered every call");
    assert_eq!(
        instance.param_value(test_plugin::PROBE_SIDECHAIN_LOUD),
        Some(0.0),
        "the sidechain heard something"
    );
}

/// **An effect's extra input stays silent however loud the chain is**: the
/// same plugin, driven with a full-scale signal from the first frame, never
/// sees a sample on its sidechain.
#[test]
fn a_loud_chain_never_reaches_the_sidechain() {
    let mut instance = open(test_plugin::SIDECHAIN_ID);
    let life = Lifeline::new();
    let mut node = instance.build_processor(life.tie()).expect("a processor");
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let frames = 256;
                let mut bus = StereoBus::with_capacity(frames);
                for block in 0..16u64 {
                    bus.l[..frames].fill(1.0);
                    bus.r[..frames].fill(-1.0);
                    let ctx = ProcessContext {
                        sample_rate: SAMPLE_RATE,
                        frames,
                        playing: true,
                        bpm: 120.0,
                        position_ticks: 0.0,
                        position_frames: block * frames as u64,
                    };
                    node.process(&ctx, &mut bus, &EventList::empty(), None);
                    assert!(bus.l[..frames].iter().all(|&s| s == 1.0), "block {block}: left");
                    assert!(bus.r[..frames].iter().all(|&s| s == -1.0), "block {block}: right");
                }
            })
            .join()
            .expect("the audio thread did not panic")
    });
    assert!(life.is_alone());
    assert!(!instance.failed());
    assert_eq!(instance.param_value(test_plugin::PROBE_SIDECHAIN_BLOCKS), Some(16.0));
    assert_eq!(instance.param_value(test_plugin::PROBE_SIDECHAIN_LOUD), Some(0.0));
}
