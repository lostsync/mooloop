//! One source slot, and what it must not change (MOO-56).
//!
//! `ChannelStrip` held eight concrete generators and picked one with a
//! `DeviceKind` tag; it now holds one boxed `SourceNode`, and changing a
//! channel's instrument moves ownership instead of flipping a tag. None of
//! that is supposed to be audible, and a source change must still neither
//! allocate nor free on the audio thread. These tests are what say so.
//!
//! ## Before and after
//!
//! Every kind was rendered two ways with eight resident generators and again
//! with the slot, on the build box, and the whole output hashed (FNV-1a over
//! the `f32` bits of both channels): *loaded* from a song, and *switched*
//! away mid-note and back with its patch. **Fifteen of the sixteen hashes
//! were bit-identical.** The sixteenth is DS-01 switched back, whose RMS
//! moved in its ninth significant digit (0.045377830 to 0.045377829, on
//! `origin/main` at `af374fef`, 2026-09-23): the
//! resident DS-01 was reused across the switch, and its `reset()` does not
//! clear the per-voice `voice_continuous` it resolved for the patch it was
//! playing before, so the old code started the returning device with the
//! previous patch's state in it. The slot builds a fresh one, which is what
//! a channel that had never been a DS-01 always got -- and what the load
//! path, bit-identical before and after, gets.
//!
//! The hashes themselves are not pinned here, because they depend on libm
//! agreeing to the last bit between machines. [`every_kind_sounds_as_it_did_with_eight_fields`]
//! pins the RMS of the same renders to a relative 1e-5 instead, which no
//! difference in rounding reaches and any change to what a device is sent
//! does.

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use crate::executor::{Executor, ExecutorIo};
use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use crate::{RealtimeCommand, StructuralCommand, StructuralReclaim};
use mooloop_core::{
    mlp8, AudioSubscription, AuxInParams, DeviceKind, EngineCommand, GeneratorParams,
    MlP8Params, NoteEvent, ParamCurve, Project, ProjectChannel,
};

const KINDS: [DeviceKind; 8] = [
    DeviceKind::Sampler,
    DeviceKind::DrumSynth,
    DeviceKind::MonoSynth,
    DeviceKind::PolySynth,
    DeviceKind::MlM1,
    DeviceKind::MlP8,
    DeviceKind::Ds01,
    DeviceKind::AuxIn,
];

const BLOCK: usize = 256;

/// The kind a switch goes away to and comes back from.
fn other(kind: DeviceKind) -> DeviceKind {
    match kind {
        DeviceKind::Sampler => DeviceKind::MonoSynth,
        _ => DeviceKind::Sampler,
    }
}

/// `kind`'s defaults with every continuous parameter moved a tenth of the
/// way towards its maximum, so a parameter block that failed to arrive would
/// be heard rather than coincide with the default the node started at.
fn patch(kind: DeviceKind) -> GeneratorParams {
    let mut params = match kind {
        // An Aux In with nothing subscribed is silent whatever it is sent,
        // so it listens to the ML-P8 on channel 0.
        DeviceKind::AuxIn => {
            let mut params = AuxInParams {
                level: 1.0,
                ..AuxInParams::default()
            };
            params.set_subscription(Some(AudioSubscription::new(0, mlp8::OUTLET_OSC1)));
            GeneratorParams::AuxIn(params)
        }
        kind => kind.default_generator_params(),
    };
    for descriptor in kind.descriptors() {
        if matches!(descriptor.curve, ParamCurve::Stepped(_)) {
            continue;
        }
        let value = descriptor.default + 0.1 * (descriptor.max - descriptor.default);
        let _ = params.set(descriptor.id, value);
    }
    params
}

fn channel_with(index: usize, params: GeneratorParams) -> ProjectChannel {
    let mut channel = match params {
        GeneratorParams::Sampler(params) => {
            let mut channel = ProjectChannel::sampler(index, 1);
            let state = channel.setup.sampler_state_mut().expect("a sampler");
            state.params = params;
            state.sample = mooloop_core::SampleReference::Builtin {
                id: "default_kick".into(),
            };
            channel
        }
        GeneratorParams::DrumSynth(params) => {
            ProjectChannel::drum_synth_with_params(index, 1, params)
        }
        GeneratorParams::MonoSynth(params) => {
            ProjectChannel::mono_synth_with_params(index, 1, params)
        }
        GeneratorParams::PolySynth(params) => {
            ProjectChannel::poly_synth_with_params(index, 1, params)
        }
        GeneratorParams::MlM1(params) => ProjectChannel::mlm1_with_params(index, 1, params),
        GeneratorParams::MlP8(params) => ProjectChannel::mlp8_with_params(index, 1, params),
        GeneratorParams::Ds01(params) => ProjectChannel::ds01_with_params(index, 1, params),
        GeneratorParams::AuxIn(params) => ProjectChannel::aux_in_with_params(index, 1, params),
    };
    channel.setup.channel.volume = 1.0;
    // A note every eighth of a second for a second, with some overlap, so
    // a polyphonic kind is holding several when a switch lands.
    for (id, start) in (0..8u32).map(|n| (n + 1, n * 48)) {
        channel.notes[0].push(NoteEvent::new(id, start, 72, 48 + (id as u8 % 5) * 3, 110));
    }
    channel
}

/// A song whose channel `channel` plays `params`. Every kind is channel 0
/// except the Aux In, which is channel 1 listening to an ML-P8 on channel 0.
fn project_with(params: GeneratorParams) -> (Project, u8) {
    let mut project = Project::default();
    if params.kind() == DeviceKind::AuxIn {
        let producer = channel_with(0, GeneratorParams::MlP8(MlP8Params::default()));
        project.channels = vec![producer, channel_with(1, params)];
        (project, 1)
    } else {
        project.channels = vec![channel_with(0, params)];
        (project, 0)
    }
}

/// Change `channel`'s instrument and send it `params`, the way the session
/// does: a source change, then one parameter at a time.
///
/// The source change is what `EngineHandle` sends for
/// `EngineCommand::SetChannelSource` -- the device built off the audio thread
/// at its defaults, installed structurally, the displaced one handed back.
/// With eight resident generators it was that command applied inline, and
/// the fingerprints in the module comment were taken through it.
fn switch_source(render: &mut RenderState, channel: u8, params: GeneratorParams) {
    let defaults = params.kind().default_generator_params();
    let node = render.build_source_for(usize::from(channel), &defaults);
    let displaced = render.apply_structural(StructuralCommand::InstallSource { channel, node });
    assert!(
        matches!(displaced, Some(StructuralReclaim::Source(_))),
        "a source change must hand the old device back"
    );
    for descriptor in params.kind().descriptors() {
        if let Some(value) = params.get(descriptor.id) {
            render.apply_command(EngineCommand::SetChannelGeneratorParam {
                channel,
                id: descriptor.id,
                value,
            });
        }
    }
}

fn render(render: &mut RenderState, seconds: f32, out: &mut (Vec<f32>, Vec<f32>)) {
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    while remaining > 0 {
        let frames = remaining.min(BLOCK);
        render.process_once_block(frames);
        let master = render.master();
        out.0.extend_from_slice(&master.l[..frames]);
        out.1.extend_from_slice(&master.r[..frames]);
        remaining -= frames;
    }
}

/// A channel loaded with `kind`'s patch and played for a second.
fn loaded(kind: DeviceKind) -> (Vec<f32>, Vec<f32>) {
    let (project, _) = project_with(patch(kind));
    let mut state = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    state.play();
    let mut out = (Vec::new(), Vec::new());
    render(&mut state, 1.0, &mut out);
    out
}

/// A channel loaded with `kind`'s patch, switched away mid-note and back
/// again with its patch, and played on.
fn switched(kind: DeviceKind) -> (Vec<f32>, Vec<f32>) {
    let (project, channel) = project_with(patch(kind));
    let mut state = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    state.play();
    let mut out = (Vec::new(), Vec::new());
    render(&mut state, 0.3, &mut out);
    switch_source(&mut state, channel, other(kind).default_generator_params());
    render(&mut state, 0.25, &mut out);
    switch_source(&mut state, channel, patch(kind));
    render(&mut state, 0.45, &mut out);
    out
}

fn rms(audio: &(Vec<f32>, Vec<f32>)) -> f64 {
    let sum: f64 = audio
        .0
        .iter()
        .chain(audio.1.iter())
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum();
    (sum / (audio.0.len() + audio.1.len()) as f64).sqrt()
}

/// What each kind measured with eight resident generators, `(loaded,
/// switched)`, which the slot reproduces; see the module comment for the
/// one ninth-digit exception and why it is the correction rather than the
/// regression.
const BEFORE: [(DeviceKind, f64, f64); 8] = [
    (DeviceKind::Sampler, 0.002_547_445, 0.026_572_627),
    (DeviceKind::DrumSynth, 0.029_344_743, 0.024_621_880),
    (DeviceKind::MonoSynth, 0.049_565_657, 0.010_736_454),
    (DeviceKind::PolySynth, 0.035_469_519, 0.010_738_889),
    (DeviceKind::MlM1, 0.065_406_552, 0.014_437_033),
    (DeviceKind::MlP8, 0.069_664_091, 0.021_642_598),
    (DeviceKind::Ds01, 0.065_798_288, 0.045_377_830),
    (DeviceKind::AuxIn, 0.369_662_357, 0.324_161_655),
];

/// Every kind, loaded and switched, renders what it did when the strip held
/// all eight. One per kind, because a kind the slot built wrongly would be
/// the only one to move.
#[test]
fn every_kind_sounds_as_it_did_with_eight_fields() {
    assert_eq!(BEFORE.map(|(kind, _, _)| kind), KINDS);
    for (kind, was_loaded, was_switched) in BEFORE {
        let close = |now: f64, was: f64| (now - was).abs() <= was * 1e-5;
        let (load, switch) = (loaded(kind), switched(kind));
        let now_loaded = rms(&load);
        let now_switched = rms(&switch);
        assert!(
            close(now_loaded, was_loaded),
            "{kind:?} loaded from a song renders at {now_loaded:.9} RMS, and did at {was_loaded:.9}"
        );
        assert!(
            close(now_switched, was_switched),
            "{kind:?} switched away and back renders at {now_switched:.9} RMS, and did at {was_switched:.9}"
        );
    }
}

/// The two paths that build a source -- a song loading and a source change
/// -- build the same device: a kind switched in and sent its patch before
/// the song starts renders sample for sample what the song with that patch
/// renders.
///
/// Except Aux In's first stretch, by design and as before: a loaded Aux In
/// starts at its saved level with nothing to click, and one switched in
/// glides to the level it is sent. From the end of that glide they agree
/// exactly, which is the claim that matters -- the device is the same.
#[test]
fn a_switched_in_source_renders_as_one_loaded_from_the_song() {
    // Past the end of Aux In's level glide. Its 5 ms is a one-pole *time
    // constant*, not a length: the lag only snaps onto the target once the
    // gap is under 1e-9 (`smooth.rs`, `SNAP_EPSILON`), about twenty time
    // constants from a gap of a few tenths, then creeps the last few units
    // in the last place. A quarter of a second is past all of it.
    let settled = (SAMPLE_RATE as usize) / 4;
    for kind in KINDS {
        let expected = loaded(kind);

        let (project, channel) = project_with(patch(kind));
        let mut state = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        switch_source(&mut state, channel, other(kind).default_generator_params());
        switch_source(&mut state, channel, patch(kind));
        state.play();
        let mut actual = (Vec::new(), Vec::new());
        render(&mut state, 1.0, &mut actual);

        let from = if kind == DeviceKind::AuxIn { settled } else { 0 };
        assert!(rms(&expected) > 1e-3, "{kind:?} is silent, so this compares nothing");
        assert_eq!(expected.0.len(), actual.0.len());
        let same = expected.0[from..] == actual.0[from..] && expected.1[from..] == actual.1[from..];
        assert!(same, "{kind:?} switched in does not render as {kind:?} loaded");
    }
}

/// An executor running `project`, with a reclaim ring of `reclaim` slots.
fn executor(
    project: &Project,
    reclaim: usize,
) -> (
    Executor,
    rtrb::Producer<RealtimeCommand>,
    rtrb::Consumer<StructuralReclaim>,
) {
    let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(16);
    let (evt_tx, _evt_rx) = rtrb::RingBuffer::new(64);
    let (reclaim_tx, reclaim_rx) = rtrb::RingBuffer::new(reclaim);
    let executor = Executor::new(
        ExecutorIo {
            cmd_rx,
            evt_tx,
            reclaim_tx,
        },
        Box::new(RenderState::from_project(SAMPLE_RATE, project, &[])),
        Arc::new(AtomicU64::new(0)),
        SAMPLE_RATE,
        crate::load::LoadMeters::new(),
    );
    (executor, cmd_tx, reclaim_rx)
}

fn block(executor: &mut Executor) {
    let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
    let no_midi = std::iter::empty::<(mooloop_core::MidiPortId, u32, &[u8])>();
    executor.process(no_midi, &mut left, &mut right);
}

/// What `EngineHandle` sends for a source change, built as it builds it.
fn source_change(channel: u8, kind: DeviceKind) -> RealtimeCommand {
    let bank = crate::render::empty_channel_audio_bank();
    crate::realtime_command(EngineCommand::SetChannelSource { channel, source: kind }, &bank, SAMPLE_RATE)
        .expect("an addressable channel")
}

/// A source change neither allocates nor frees on the audio thread, while
/// the old device is sounding, and the device it displaces comes back
/// through the reclaim ring -- to every kind from the one before it, so each
/// kind is both installed and displaced once.
#[test]
fn a_source_change_allocates_nothing_and_hands_the_old_device_back() {
    let (project, _) = project_with(patch(DeviceKind::Sampler));
    let (mut executor, mut cmd_tx, mut reclaim_rx) = executor(&project, 8);
    assert!(cmd_tx.push(RealtimeCommand::Engine(EngineCommand::Play)).is_ok());
    // Warm: the first block asks the scheduler about its thread and touches
    // whatever is lazily initialised.
    block(&mut executor);

    let mut running = DeviceKind::Sampler;
    for kind in KINDS.iter().copied().cycle().skip(1).take(KINDS.len()) {
        assert!(cmd_tx.push(source_change(0, kind)).is_ok());
        let (allocations, frees) = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        block(&mut executor);
        let allocated = crate::COUNTING.allocations() - allocations;
        let freed = crate::COUNTING.frees() - frees;
        assert_eq!(
            (allocated, freed),
            (0, 0),
            "changing {running:?} to {kind:?} allocated or freed in the callback"
        );
        assert_eq!(executor.render().channel_source(0).kind(), kind);
        match reclaim_rx.pop() {
            Ok(StructuralReclaim::Source(node)) => assert_eq!(node.kind(), running),
            _ => panic!("changing {running:?} to {kind:?} did not hand {running:?} back"),
        }
        running = kind;
    }
}

/// **A source change waits for room, and its patch waits with it.** Adam,
/// 2026-09-22, on the change landing a block or more after the click:
/// *"totally fine. there's no reason to expect that this action should be
/// instantaneous."* What must not happen instead is the change being dropped,
/// the old device being freed on the audio thread, or a parameter meant for
/// the new device reaching the old one.
#[test]
fn a_source_change_waits_for_reclaim_room_and_its_patch_waits_with_it() {
    let (project, _) = project_with(patch(DeviceKind::Sampler));
    let (mut executor, mut cmd_tx, mut reclaim_rx) = executor(&project, 1);
    block(&mut executor);

    // The first change fills the one-slot ring with the sampler.
    assert!(cmd_tx.push(source_change(0, DeviceKind::MonoSynth)).is_ok());
    block(&mut executor);
    assert_eq!(executor.render().channel_source(0).kind(), DeviceKind::MonoSynth);

    // The second has nowhere to put the mono synth, so it and the parameter
    // behind it are held.
    let GeneratorParams::Ds01(mut sent) = DeviceKind::Ds01.default_generator_params() else {
        unreachable!()
    };
    sent.tone_level = 0.25;
    assert!(cmd_tx.push(source_change(0, DeviceKind::Ds01)).is_ok());
    assert!(cmd_tx
        .push(RealtimeCommand::Engine(EngineCommand::SetChannelGeneratorParam {
            channel: 0,
            id: mooloop_core::ds01::PARAM_TONE_LEVEL,
            value: sent.tone_level,
        }))
        .is_ok());
    block(&mut executor);
    block(&mut executor);
    assert_eq!(
        executor.render().channel_source(0).kind(),
        DeviceKind::MonoSynth,
        "the change went ahead with nowhere to put the device it displaced"
    );

    // Room again: the change lands, the parameter lands on the new device.
    assert!(matches!(reclaim_rx.pop(), Ok(StructuralReclaim::Source(_))));
    block(&mut executor);
    let source = executor.render().channel_source(0);
    assert_eq!(source.kind(), DeviceKind::Ds01);
    let GeneratorParams::Ds01(running) = source.generator_params() else {
        unreachable!()
    };
    assert_eq!(running.tone_level, sent.tone_level);
    match reclaim_rx.pop() {
        Ok(StructuralReclaim::Source(node)) => assert_eq!(node.kind(), DeviceKind::MonoSynth),
        _ => panic!("the mono synth did not come back"),
    }
}

/// `EngineHandle::send` is how the source picker's command reaches the
/// engine (`Session::apply_engine_message`), and it is the one command that
/// does not cross as itself: it becomes an install carrying the device,
/// built reading the channel's own audio slot so a sampler plays what the
/// channel publishes. Every other command crosses unchanged.
#[test]
fn a_source_change_crosses_as_an_install_of_the_device_it_names() {
    let bank = crate::render::empty_channel_audio_bank();
    for kind in KINDS {
        let command = crate::realtime_command(
            EngineCommand::SetChannelSource { channel: 3, source: kind },
            &bank,
            SAMPLE_RATE,
        );
        let Some(RealtimeCommand::Structural(StructuralCommand::InstallSource { channel, node })) =
            command
        else {
            panic!("a change to {kind:?} did not cross as an install");
        };
        assert_eq!((channel, node.kind()), (3, kind));
        assert_eq!(node.generator_params(), kind.default_generator_params());
        if let Some(sampler) = node.as_sampler() {
            assert_eq!(sampler.audio_slot_ptr(), Arc::as_ptr(&bank[3]) as usize);
        }
    }
    assert!(matches!(
        crate::realtime_command(EngineCommand::Play, &bank, SAMPLE_RATE),
        Some(RealtimeCommand::Engine(EngineCommand::Play))
    ));
}
