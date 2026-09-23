//! Step 01's six checks (`docs/plans/plugin-hosting/01-spike.md`): can
//! `clack-host` 0.2 load and run a CLAP plugin, with no `unsafe` beyond
//! loading the library?
//!
//! Every check runs against `mooloop-test-plugin`, loaded by path the way a
//! third-party `.clap` would be. Processing happens on a thread of its own,
//! because the test plugin asks the host (`thread-check`) whether each call
//! is on the thread CLAP says it must be, and logs `HostMisbehaving` if not;
//! every check ends by asserting the plugin logged nothing of the kind.

use std::ffi::CStr;
use std::path::PathBuf;

use clack_extensions::latency::PluginLatency;
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_extensions::state::PluginState;
use clack_host::events::event_types::{NoteEndEvent, NoteOffEvent, NoteOnEvent, ParamValueEvent};
use clack_host::prelude::*;
use mooloop_plugin_host::{instantiate, load_entry, SpikeHost};
use mooloop_test_plugin as test_plugin;

const SAMPLE_RATE: f64 = 48_000.0;
const MAX_FRAMES: u32 = 4096;

/// The test plugin's library, which cargo built because this crate names it
/// as a dev-dependency.
///
/// Neither of the two ways `01-spike.md` suggested works on the pinned stable
/// toolchain: `CARGO_CDYLIB_FILE_*` is an artifact-dependency variable
/// (nightly `-Z bindeps`), and `CARGO_TARGET_DIR` is not set when a caller
/// uses the default target directory, nor when the build box redirects it. A
/// test binary lives in `<target>/<profile>/deps/`, and cargo writes a
/// dependency's cdylib into that same `deps/` directory, so the library is
/// found next to the running test.
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
    panic!(
        "{name} is not next to {}. Cargo builds it for these tests because this \
         crate names `mooloop-test-plugin` as a dev-dependency; if it is \
         missing, build it into the same target with \
         `cargo build -p mooloop-test-plugin` and run the tests again",
        exe.display()
    );
}

fn entry() -> PluginEntry {
    // SAFETY: the library is this workspace's own test plugin.
    unsafe { load_entry(&test_plugin_path()) }.expect("the test plugin loads")
}

fn id(id: &str) -> std::ffi::CString {
    std::ffi::CString::new(id).expect("no NUL in a plugin id")
}

fn config(max: u32) -> PluginAudioConfiguration {
    PluginAudioConfiguration {
        sample_rate: SAMPLE_RATE,
        min_frames_count: 1,
        max_frames_count: max,
    }
}

/// No call the plugin checked was made on the wrong thread.
fn assert_well_behaved(instance: &mut PluginInstance<SpikeHost>) {
    let (misbehaviour, log) =
        instance.access_shared_handler(|shared| (shared.misbehaviour(), shared.log()));
    assert_eq!(misbehaviour, 0, "the plugin reported the host misbehaving: {log:?}");
}

/// Run `blocks` through an active stereo effect, each block its own length,
/// with `events` for each block, on a thread that is not the main one.
/// Returns the output, concatenated.
fn run_effect(
    processor: StoppedPluginAudioProcessor<SpikeHost>,
    blocks: Vec<(Vec<f32>, EventBuffer)>,
) -> (StoppedPluginAudioProcessor<SpikeHost>, Vec<f32>, Vec<Result<ProcessStatus, String>>) {
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut processor = processor.start_processing().expect("processing starts");
                let mut out = Vec::new();
                let mut statuses = Vec::new();
                let mut inputs = AudioPorts::with_capacity(2, 1);
                let mut outputs = AudioPorts::with_capacity(2, 1);
                for (input, events) in blocks {
                    let mut in_l = input.clone();
                    let mut in_r = input.clone();
                    let mut out_l = vec![0.0f32; input.len()];
                    let mut out_r = vec![0.0f32; input.len()];
                    let input_buffers = inputs.with_input_buffers([AudioPortBuffer {
                        latency: 0,
                        channels: AudioPortBufferType::f32_input_only(
                            [&mut in_l, &mut in_r]
                                .into_iter()
                                .map(|b| InputChannel::variable(b.as_mut_slice())),
                        ),
                    }]);
                    let mut output_buffers = outputs.with_output_buffers([AudioPortBuffer {
                        latency: 0,
                        channels: AudioPortBufferType::f32_output_only(
                            [&mut out_l, &mut out_r].into_iter().map(|b| b.as_mut_slice()),
                        ),
                    }]);
                    let mut output_events = EventBuffer::new();
                    let status = processor
                        .process(
                            &input_buffers,
                            &mut output_buffers,
                            &events.as_input(),
                            &mut output_events.as_output(),
                            None,
                            None,
                        )
                        .map_err(|error| error.to_string());
                    statuses.push(status);
                    assert_eq!(out_l, out_r, "the gain is the same on both sides");
                    out.extend_from_slice(&out_l);
                }
                (processor.stop_processing(), out, statuses)
            })
            .join()
            .expect("the audio thread did not panic")
    })
}

fn param_event(time: u32, param: u32, value: f64) -> ParamValueEvent {
    ParamValueEvent::new(time, ClapId::new(param), Pckn::match_all(), value)
}

fn db_to_gain(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// 1. Load the gain, read its descriptor, create it, and activate it at
///    48 kHz for blocks of 1 to 4096 frames -- and run blocks of 1, 64 and
///    4096 through it.
#[test]
fn one_the_gain_loads_describes_itself_and_runs_any_block_size() {
    let entry = entry();
    let factory = entry.get_plugin_factory().expect("a plugin factory");
    let ids: Vec<String> = factory
        .plugin_descriptors()
        .filter_map(|d| d.id().map(|id| id.to_string_lossy().into_owned()))
        .collect();
    assert_eq!(
        ids,
        [test_plugin::GAIN_ID, test_plugin::GAIN_GUI_ID, test_plugin::SINE_ID, test_plugin::SINE_GUI_ID]
    );
    let gain = factory
        .plugin_descriptors()
        .find(|d| d.id().is_some_and(|id| id.to_bytes() == test_plugin::GAIN_ID.as_bytes()))
        .expect("the gain's descriptor");
    assert_eq!(gain.name().map(CStr::to_bytes), Some(&b"Test Gain"[..]));
    assert_eq!(gain.vendor().map(CStr::to_bytes), Some(test_plugin::VENDOR.as_bytes()));

    let mut instance = instantiate(&entry, &id(test_plugin::GAIN_ID)).expect("the gain is created");
    let processor = instance.activate(|_, _| (), config(MAX_FRAMES)).expect("the gain activates");
    let blocks = [1usize, 64, 4096]
        .into_iter()
        .map(|n| (vec![0.5f32; n], EventBuffer::new()))
        .collect();
    let (processor, out, statuses) = run_effect(processor, blocks);
    assert!(statuses.iter().all(Result::is_ok), "{statuses:?}");
    assert_eq!(out.len(), 1 + 64 + 4096);
    assert!(out.iter().all(|&s| s == 0.5), "unity gain passes the input through");
    instance.deactivate(processor);
    assert_well_behaved(&mut instance);
}

/// 2. A block of known audio comes out with the gain applied, and a
///    parameter event at offset 100 lands at frame 100.
#[test]
fn two_a_parameter_event_lands_on_its_frame() {
    let entry = entry();
    let mut instance = instantiate(&entry, &id(test_plugin::GAIN_ID)).expect("created");
    let processor = instance.activate(|_, _| (), config(MAX_FRAMES)).expect("activated");

    let mut events = EventBuffer::new();
    events.push(&param_event(0, test_plugin::PARAM_GAIN, -12.0));
    events.push(&param_event(100, test_plugin::PARAM_GAIN, -6.0));
    let (processor, out, statuses) = run_effect(processor, vec![(vec![1.0; 256], events)]);
    assert!(statuses[0].is_ok());
    let (before, after) = (db_to_gain(-12.0), db_to_gain(-6.0));
    assert!(out[..100].iter().all(|&s| s == before), "frames 0..100 are at -12 dB");
    assert!(out[100..].iter().all(|&s| s == after), "frame 100 on is at -6 dB");

    instance.deactivate(processor);
    assert_well_behaved(&mut instance);
}

/// 3. Save state, create a new instance, load that state into it, and get
///    the same output.
#[test]
fn three_state_saved_from_one_instance_makes_another_sound_the_same() {
    let entry = entry();
    let mut first = instantiate(&entry, &id(test_plugin::GAIN_ID)).expect("created");
    let processor = first.activate(|_, _| (), config(MAX_FRAMES)).expect("activated");
    let mut events = EventBuffer::new();
    events.push(&param_event(0, test_plugin::PARAM_GAIN, -9.5));
    let (processor, first_out, _) = run_effect(processor, vec![(vec![0.75; 64], events)]);
    first.deactivate(processor);

    let state: PluginState = first.plugin_shared_handle().get_extension().expect("the state extension");
    let mut saved = Vec::new();
    state.save(&first.plugin_handle(), &mut saved).expect("state saves");
    assert!(saved.starts_with(&test_plugin::STATE_MAGIC));

    let mut second = instantiate(&entry, &id(test_plugin::GAIN_ID)).expect("created");
    let state: PluginState = second.plugin_shared_handle().get_extension().expect("the state extension");
    state.load(&second.plugin_handle(), &mut saved.as_slice()).expect("state loads");
    let processor = second.activate(|_, _| (), config(MAX_FRAMES)).expect("activated");
    let (processor, second_out, _) = run_effect(processor, vec![(vec![0.75; 64], EventBuffer::new())]);
    second.deactivate(processor);

    assert_eq!(first_out, second_out);
    assert!(first_out.iter().all(|&s| s == 0.75 * db_to_gain(-9.5)));
    assert_well_behaved(&mut first);
    assert_well_behaved(&mut second);
}

/// 4. Parameter info -- ids, names, ranges, the stepped flag -- and a value
///    turned into display text and back.
#[test]
fn four_parameters_describe_themselves_and_print_their_values() {
    let entry = entry();
    let mut instance = instantiate(&entry, &id(test_plugin::GAIN_ID)).expect("created");
    let params: PluginParams = instance.plugin_shared_handle().get_extension().expect("params");
    let handle = instance.plugin_handle();

    let count = params.count(&handle);
    assert_eq!(count, 3);
    let mut infos = Vec::new();
    for index in 0..count {
        let mut buffer = ParamInfoBuffer::new();
        let info = params.get_info(&handle, index, &mut buffer).expect("an info per index");
        infos.push((
            info.id.get(),
            String::from_utf8_lossy(info.name).into_owned(),
            info.min_value,
            info.max_value,
            info.default_value,
            info.flags.contains(ParamInfoFlags::IS_STEPPED),
        ));
    }
    assert_eq!(
        infos,
        [
            (test_plugin::PARAM_GAIN, "Gain".into(), test_plugin::GAIN_DB_MIN, test_plugin::GAIN_DB_MAX, 0.0, false),
            (test_plugin::PARAM_LATENCY, "Latency".into(), 0.0, 2.0, 0.0, true),
            (test_plugin::PARAM_FAIL, "Fail".into(), 0.0, 1.0, 0.0, true),
        ]
    );

    let mut text = [0u8; 64];
    let shown = params
        .value_to_text(&handle, ClapId::new(test_plugin::PARAM_GAIN), -6.0, &mut text)
        .expect("the gain prints");
    assert_eq!(shown, b"-6.0 dB");
    assert_eq!(
        params.text_to_value(&handle, ClapId::new(test_plugin::PARAM_LATENCY), c"512 frames"),
        Some(2.0)
    );
    assert_eq!(params.get_value(&handle, ClapId::new(test_plugin::PARAM_GAIN)), Some(0.0));
    assert_well_behaved(&mut instance);
}

/// 5. Changing `latency` while active asks for a restart; deactivate,
///    activate, and the new latency is reported (and the plugin says it
///    changed, from inside `activate`, as CLAP requires).
#[test]
fn five_a_latency_change_restarts_the_plugin_and_reports_the_new_latency() {
    let entry = entry();
    let mut instance = instantiate(&entry, &id(test_plugin::GAIN_ID)).expect("created");
    let latency: PluginLatency = instance.plugin_shared_handle().get_extension().expect("latency");
    let processor = instance.activate(|_, _| (), config(MAX_FRAMES)).expect("activated");
    assert_eq!(latency.get(&instance.plugin_handle()), 0);

    let mut events = EventBuffer::new();
    events.push(&param_event(0, test_plugin::PARAM_LATENCY, 2.0));
    let (processor, _, _) = run_effect(processor, vec![(vec![1.0; 64], events)]);
    assert!(
        instance.access_shared_handler(|shared| shared.take_restart_request()),
        "a latency change while active asks for a restart"
    );

    instance.deactivate(processor);
    let processor = instance.activate(|_, _| (), config(MAX_FRAMES)).expect("reactivated");
    assert!(
        instance.access_handler(|main| main.take_latency_changed()),
        "the plugin said its latency changed"
    );
    assert_eq!(latency.get(&instance.plugin_handle()), 512);

    // And the delay is real: an impulse comes out 512 frames late.
    let mut impulse = vec![0.0; 1024];
    impulse[0] = 1.0;
    let (processor, out, _) = run_effect(processor, vec![(impulse, EventBuffer::new())]);
    assert_eq!(out.iter().position(|&s| s != 0.0), Some(512));
    instance.deactivate(processor);
    assert_well_behaved(&mut instance);
}

/// 6. The sine: a note on starts a voice, a note off starts its release, the
///    voice finishes its tail, says so with a `note_end`, and the plugin
///    asks to sleep.
#[test]
fn six_the_sine_plays_a_note_and_finishes_its_tail() {
    const BLOCK: usize = 512;
    let entry = entry();
    let mut instance = instantiate(&entry, &id(test_plugin::SINE_ID)).expect("created");
    let processor = instance.activate(|_, _| (), config(BLOCK as u32)).expect("activated");
    let release = (test_plugin::RELEASE_SECONDS * SAMPLE_RATE).ceil() as usize;

    let (processor, blocks) = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut processor = processor.start_processing().expect("processing starts");
                let mut outputs = AudioPorts::with_capacity(2, 1);
                let pckn = Pckn::new(0u16, 0u16, 69u16, 1u32);
                let mut blocks = Vec::new();
                for block in 0..12 {
                    let mut events = EventBuffer::new();
                    if block == 0 {
                        events.push(&NoteOnEvent::new(0, pckn, 1.0));
                    }
                    if block == 2 {
                        events.push(&NoteOffEvent::new(0, pckn, 0.0));
                    }
                    let (mut l, mut r) = (vec![1.0f32; BLOCK], vec![1.0f32; BLOCK]);
                    let mut output_buffers = outputs.with_output_buffers([AudioPortBuffer {
                        latency: 0,
                        channels: AudioPortBufferType::f32_output_only(
                            [&mut l, &mut r].into_iter().map(|b| b.as_mut_slice()),
                        ),
                    }]);
                    let mut output_events = EventBuffer::new();
                    let status = processor
                        .process(
                            &InputAudioBuffers::empty(),
                            &mut output_buffers,
                            &events.as_input(),
                            &mut output_events.as_output(),
                            None,
                            None,
                        )
                        .expect("the sine processes");
                    let ends: Vec<u32> = output_events
                        .iter()
                        .filter_map(|event| event.as_event::<NoteEndEvent>())
                        .map(|end| end.header().time())
                        .collect();
                    blocks.push((l, status, ends));
                }
                (processor.stop_processing(), blocks)
            })
            .join()
            .expect("the audio thread did not panic")
    });

    let peak = |block: &[f32]| block.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    // Held for two blocks at full velocity.
    assert!((peak(&blocks[0].0) - test_plugin::SINE_AMPLITUDE).abs() < 1e-3);
    assert_eq!(blocks[1].1, ProcessStatus::Continue);
    // The release runs from block 2 for `release` frames, and ends there.
    let ends: Vec<usize> = blocks
        .iter()
        .enumerate()
        .flat_map(|(n, (_, _, ends))| ends.iter().map(move |&t| n * BLOCK + t as usize))
        .collect();
    assert_eq!(ends, [2 * BLOCK + release], "one note_end, where the release finishes");
    let end_block = ends[0] / BLOCK;
    assert_eq!(blocks[end_block].1, ProcessStatus::Sleep, "the plugin sleeps once silent");
    let after = &blocks[end_block].0[ends[0] % BLOCK..];
    assert!(after.iter().all(|&s| s == 0.0), "nothing sounds after the tail");
    assert!(blocks[end_block + 1..].iter().all(|(l, _, _)| l.iter().all(|&s| s == 0.0)));

    instance.deactivate(processor);
    assert_well_behaved(&mut instance);
}
