//! The engine soak (MOO-113): every device kind at once, through the
//! executor, block after block, allocating and freeing nothing on the
//! callback and producing nothing that is not a number.
//!
//! Every allocation test before this one guards a single path --
//! `carrying_strips_allocates_nothing`, `a_routing_change_frees_nothing_on_the_callback`,
//! `opening_a_lane_allocates_nothing_on_the_callback` -- and every allocation
//! finding so far was on a path no test covered (MOO-52 was four `Vec`s freed
//! by every structural edit, MOO-61 12 KB malloc'd by opening a lane). A test
//! per path finds the paths somebody already suspected. This one drives the
//! program instead: one project with every source kind and every effect kind,
//! a container, sends between tracks, modulators and automation lanes, a take,
//! a preview and live MIDI, and partway through an install and a run of
//! structural edits -- at several block sizes, through
//! `Executor::process_with_input`, which is what a driver calls.
//!
//! It asserts on **every** block, not after a warm-up. A first block that
//! allocates is still an allocation on the audio thread, and a warm-up would
//! hide precisely the one-time paths (a lazily grown buffer, a first-use
//! table) that no other test reaches.
//!
//! What it does not cover, deliberately: the driver adapters (they are
//! Platform's and need a device), and offline export (`offline.rs` runs the
//! same `process_block_inner` on an ordinary thread, where allocating is
//! allowed).

use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use mooloop_core::{
    AudioTap, AutomationLane, AutomationPoint, AuxSend, DeviceKind, EffectKind, EffectSlotState,
    EffectTarget, EngineCommand, MidiChannelFilter, MidiInputRoute, MidiPortId, MidiRouteSource,
    ModLfoParams, ModPolarity, ModRoute, ModulatorParams, NoteEvent, ParamAddr, Project,
    ProjectChannel,
};
use mooloop_dsp::{
    build_effect_at_tempo, IntegerDelay, SampleData, SpectrumAnalyzer, MAX_BLOCK_SIZE,
};

use crate::executor::{Executor, ExecutorIo};
use crate::load::LoadMeters;
use crate::meters::BusMeters;
use crate::render::{AudioInputRouting, EffectSlot, MidiRouting, RenderState};
use crate::render_test_support::SAMPLE_RATE;
use crate::take::{Take, TakeStatus};
use crate::{PreparedProject, PreviewCommand, RealtimeCommand, StructuralCommand};

/// Every source kind a channel can hold.
///
/// Built by an exhaustive `match` rather than written as a list, so a new
/// kind fails to compile here instead of quietly staying out of the soak --
/// the failure the per-path tests had.
fn every_source_kind() -> Vec<DeviceKind> {
    let mut kinds = Vec::new();
    for candidate in 0.. {
        let kind = match candidate {
            0 => DeviceKind::Sampler,
            1 => DeviceKind::DrumSynth,
            2 => DeviceKind::MonoSynth,
            3 => DeviceKind::PolySynth,
            4 => DeviceKind::MlM1,
            5 => DeviceKind::MlP8,
            6 => DeviceKind::Ds01,
            7 => DeviceKind::AuxIn,
            _ => break,
        };
        // The exhaustiveness check: a ninth variant is a compile error here,
        // and the fix is a ninth arm above.
        match kind {
            DeviceKind::Sampler
            | DeviceKind::DrumSynth
            | DeviceKind::MonoSynth
            | DeviceKind::PolySynth
            | DeviceKind::MlM1
            | DeviceKind::MlP8
            | DeviceKind::Ds01
            | DeviceKind::AuxIn => kinds.push(kind),
            // Not a device the soak can build: a hosted source needs a
            // plugin's processor, and without one it is silence. A hosted
            // source through the executor is `plugin_source_tests.rs`, a
            // hosted CLAP `plugin_host_tests.rs` (MOO-84).
            DeviceKind::Plugin => {}
        }
    }
    kinds
}

fn channel_of(kind: DeviceKind, index: usize) -> ProjectChannel {
    match kind {
        DeviceKind::Sampler => ProjectChannel::sampler(index, 1),
        DeviceKind::DrumSynth => ProjectChannel::drum_synth(index, 1),
        DeviceKind::MonoSynth => ProjectChannel::mono_synth(index, 1),
        DeviceKind::PolySynth => ProjectChannel::poly_synth(index, 1),
        DeviceKind::MlM1 => ProjectChannel::mlm1(index, 1),
        DeviceKind::MlP8 => ProjectChannel::mlp8(index, 1),
        DeviceKind::Ds01 => ProjectChannel::ds01(index, 1),
        DeviceKind::AuxIn => ProjectChannel::aux_in(index, 1),
        DeviceKind::Plugin => unreachable!("the soak builds native sources only"),
    }
}

/// The soak's song: every source kind on its own channel, every effect kind
/// somewhere, a container, three tracks with a send between two of them, an
/// LFO routed to an effect and to a strip, and an automation lane.
fn soak_project() -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths[0] = 16;
    for (index, kind) in every_source_kind().into_iter().enumerate() {
        let mut channel = channel_of(kind, index);
        channel.setup.channel.volume = 0.5;
        // Four notes a bar, overlapping, so voices start, steal and release
        // inside the run rather than holding one state throughout.
        for (step, pitch) in [(0u32, 48u8), (4, 55), (8, 60), (12, 67)] {
            channel.notes[0].push(NoteEvent::new(
                step + 1,
                step * 24,
                96 + 24,
                pitch + index as u8,
                110,
            ));
        }
        if channel.automation.is_empty() {
            channel.automation.push(Vec::new());
        }
        project.channels.push(channel);
    }
    project.assign_channel_ids();

    // Every effect kind but the container, dealt round the channels.
    let channels = project.channels.len();
    for (index, kind) in EffectKind::ALL
        .into_iter()
        .filter(|kind| *kind != EffectKind::Chain)
        .enumerate()
    {
        project.channels[index % channels]
            .setup
            .push_effect(EffectSlotState::of_kind(kind))
            .expect("room in the chain");
    }
    // The container, around channel 0's whole chain.
    {
        let setup = &mut project.channels[0].setup;
        let run = 0..setup.effects.len();
        mooloop_core::wrap_in_container(
            &mut setup.effects,
            &mut setup.next_device_id,
            run,
            EffectSlotState::of_kind(EffectKind::Chain),
        )
        .expect("wrapped");
    }

    // Tracks: half the channels to track 1, half to 2, a send from 1 to 2,
    // and a reverb on track 2 so the send lands in a chain.
    project.ensure_tracks(3);
    for (index, channel) in project.channels.iter_mut().enumerate() {
        channel.setup.channel.bus = 1 + (index % 2) as u8;
    }
    project.buses[1].sends.push(AuxSend::new(2));
    project.buses[2]
        .push_effect(EffectSlotState::of_kind(EffectKind::Reverb))
        .expect("room in the track's chain");

    // An LFO on channel 1, routed to its first effect and to its strip.
    {
        let channel = &mut project.channels[1];
        let target = EffectTarget::Channel(1);
        let device = channel.setup.effects[0].id;
        channel.setup.modulation.install(
            0,
            ModulatorParams::Lfo(ModLfoParams {
                rate_hz: 3.0,
                ..ModLfoParams::default()
            }),
        );
        for destination in [ParamAddr::effect(target, device, 0), ParamAddr::strip(target, 0)] {
            channel
                .setup
                .modulation
                .add_route(ModRoute::to_slot(0, destination, 0.4, ModPolarity::Bipolar))
                .expect("room in the matrix");
        }
    }

    // An automation lane on channel 2's first effect, sweeping across the bar.
    {
        let channel = &mut project.channels[2];
        let device = channel.setup.effects[0].id;
        let mut lane = AutomationLane::new(ParamAddr::effect(EffectTarget::Channel(2), device, 0));
        lane.reserve_points();
        lane.reset_points([
            AutomationPoint::new(1, 0, 0.1),
            AutomationPoint::new(2, 192, 0.9),
            AutomationPoint::new(3, 383, 0.2),
        ]);
        channel.automation[0].push(lane);
    }
    project
}

/// A short stereo tone for the sampler and the preview, so neither plays
/// the built-in fallback and both have something to read.
fn tone() -> Arc<SampleData> {
    Arc::new(SampleData {
        frames: (0..SAMPLE_RATE as usize / 4)
            .map(|frame| {
                let s = (frame as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin();
                [s * 0.5, s * 0.5]
            })
            .collect(),
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    })
}

/// The effect a structural edit installs partway through, built the way the
/// session builds one: node, dry-align ring and host state all allocated here,
/// off the callback.
fn install_filter(target: EffectTarget, slot: u8, device: mooloop_core::DeviceId) -> StructuralCommand {
    let node = build_effect_at_tempo(
        EffectSlotState::of_kind(EffectKind::Filter).params,
        SAMPLE_RATE,
        120.0,
    );
    let align = IntegerDelay::new(node.dry_path_latency_frames()).map(Box::new);
    StructuralCommand::InstallEffect {
        target,
        slot,
        kind: EffectKind::Filter,
        resource_key: None,
        node,
        align,
        analyzer: Box::new(SpectrumAnalyzer::new()),
        state: Box::new(EffectSlot::for_device(device)),
    }
}

/// What the soak does at block `block`, queued on this thread -- the control
/// thread -- before the counted block runs.
///
/// Spread over the run rather than bunched, so each lands in a different
/// block and a failure names the edit that caused it.
fn edits_at(
    block: usize,
    blocks: usize,
    project: &Project,
    meters: &Arc<BusMeters>,
) -> Vec<RealtimeCommand> {
    let at = |fraction: usize| block == blocks * fraction / 16;
    let mut commands = Vec::new();
    if block == 0 {
        commands.push(RealtimeCommand::Engine(EngineCommand::Play));
        commands.push(RealtimeCommand::Structural(StructuralCommand::SetMidiRouting(
            Box::new(MidiRouting {
                routes: vec![
                    MidiInputRoute {
                        source: MidiRouteSource::AllPorts,
                        channel: MidiChannelFilter::Omni,
                    };
                    project.channels.len()
                ],
            }),
        )));
        // The Aux In channel records the driver's input, and so does a take.
        let aux = project
            .channels
            .iter()
            .position(|channel| channel.setup.kind() == DeviceKind::AuxIn)
            .expect("an Aux In channel");
        let mut taps = vec![None; project.channels.len()];
        taps[aux] = Some(AudioTap::Input);
        commands.push(RealtimeCommand::Structural(
            StructuralCommand::SetAudioInputRouting(Box::new(AudioInputRouting { taps })),
        ));
    }
    if at(1) {
        let (producer, _consumer) = rtrb::RingBuffer::new(1 << 16);
        commands.push(RealtimeCommand::Structural(StructuralCommand::StartTake {
            channel: 0,
            take: Take::new(producer, TakeStatus::new(), None, 0),
        }));
        commands.push(RealtimeCommand::Preview(PreviewCommand::Play { sample: tone() }));
    }
    if at(2) {
        // Value edits of every shape the faces send.
        commands.push(RealtimeCommand::Engine(EngineCommand::SetEffectParam {
            target: EffectTarget::Channel(3),
            slot: 0,
            id: 0,
            value: 0.7,
        }));
        commands.push(RealtimeCommand::Engine(EngineCommand::SetChannelVolume {
            channel: 4,
            volume: 0.3,
        }));
        commands.push(RealtimeCommand::Engine(EngineCommand::SetSendLevel {
            producer: EffectTarget::Bus(1),
            index: 0,
            level: 0.8,
        }));
        commands.push(RealtimeCommand::Engine(EngineCommand::OpenAutomationLane {
            pattern: 0,
            channel: 5,
            target: ParamAddr::strip(EffectTarget::Channel(5), 0),
        }));
        commands.push(RealtimeCommand::Engine(EngineCommand::UpsertAutomationPoint {
            pattern: 0,
            channel: 5,
            target: ParamAddr::strip(EffectTarget::Channel(5), 0),
            point: AutomationPoint::new(1, 96, 0.25),
        }));
    }
    if at(4) {
        commands.push(RealtimeCommand::Preview(PreviewCommand::Stop));
    }
    if at(6) {
        // A structural run: an effect installed on a channel's tail, one
        // removed from another, one moved.
        let target = EffectTarget::Channel(3);
        let tail = project.channels[3].setup.effects.len() as u8;
        commands.push(RealtimeCommand::Structural(install_filter(
            target,
            tail,
            mooloop_core::DeviceId(9_000),
        )));
        commands.push(RealtimeCommand::Engine(EngineCommand::MoveEffect {
            target,
            from: tail,
            to: 0,
        }));
        commands.push(RealtimeCommand::Structural(StructuralCommand::RemoveEffect {
            target: EffectTarget::Channel(4),
            slot: 0,
        }));
        commands.push(RealtimeCommand::Engine(EngineCommand::RemoveAutomationLane {
            pattern: 0,
            channel: 5,
            target: ParamAddr::strip(EffectTarget::Channel(5), 0),
        }));
    }
    if at(8) {
        commands.push(RealtimeCommand::Engine(EngineCommand::StopTake { channel: 0 }));
        // The install: the song with a channel moved and a track moved, so
        // the carry has strips to take across and seats to renumber.
        let mut next = project.clone();
        next.move_channel(0, 1).expect("a real move");
        next.move_track(1, 2).expect("a real move");
        let mut render = RenderState::from_project(SAMPLE_RATE, &next, &samples_for(&next));
        render.attach_meters(meters.clone());
        commands.push(RealtimeCommand::InstallProject(PreparedProject {
            generation: 1,
            render: Box::new(render),
            keep_transport: true,
            carry: crate::carry_plan(project, &next),
        }));
    }
    commands
}

/// The sampler's audio, seated by channel.
fn samples_for(project: &Project) -> Vec<Option<Arc<SampleData>>> {
    project
        .channels
        .iter()
        .map(|channel| (channel.setup.kind() == DeviceKind::Sampler).then(tone))
        .collect()
}

/// Run the soak at one block size and report the loudest sample, so the
/// caller can tell a run that proved something from one that was silent.
fn soak_at(frames: usize, blocks: usize) -> f32 {
    let project = soak_project();
    // The output guard (MOO-93) scrubs a NaN to 0 at the master, so the
    // ports' samples alone can no longer show a device blowing up. The
    // guard's running fault count can: it is read after every block.
    let meters = BusMeters::new();
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &samples_for(&project));
    render.attach_meters(meters.clone());
    let (mut cmd_tx, cmd_rx) = rtrb::RingBuffer::new(256);
    let (evt_tx, mut evt_rx) = rtrb::RingBuffer::new(1024);
    let (reclaim_tx, mut reclaim_rx) = rtrb::RingBuffer::new(256);
    let mut executor = Executor::new(
        ExecutorIo {
            cmd_rx,
            evt_tx,
            reclaim_tx,
        },
        Box::new(render),
        Arc::new(AtomicU64::new(0)),
        SAMPLE_RATE,
        LoadMeters::new(),
    );

    let mut out_l = vec![0.0f32; frames];
    let mut out_r = vec![0.0f32; frames];
    let mut in_l = vec![0.0f32; frames];
    let mut in_r = vec![0.0f32; frames];
    let mut phase = 0.0f32;
    let mut loudest = 0.0f32;
    // What a driver's thread-start hook does before the first callback. The
    // executor also does it on its first block, but that block is counted
    // here, and the point is that no block allocates.
    crate::executor::prepare_audio_thread();
    let note_on = [0x90u8, 64, 100];
    let note_off = [0x80u8, 64, 0];

    for block in 0..blocks {
        // Control-thread work, all outside the counted window: queue this
        // block's edits, fill the input, and throw away whatever came back.
        for command in edits_at(block, blocks, &project, &meters) {
            if cmd_tx.push(command).is_err() {
                panic!("the soak's own command ring filled at block {block}");
            }
        }
        for frame in 0..frames {
            let s = phase.sin() * 0.25;
            in_l[frame] = s;
            in_r[frame] = -s;
            phase = (phase + 220.0 * std::f32::consts::TAU / SAMPLE_RATE as f32)
                % std::f32::consts::TAU;
        }
        let midi: Vec<(MidiPortId, u32, &[u8])> = match block % 8 {
            1 => vec![(MidiPortId::FIRST, 0, &note_on[..])],
            5 => vec![(MidiPortId::FIRST, (frames / 2) as u32, &note_off[..])],
            _ => Vec::new(),
        };

        let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
        executor.process_with_input(midi.iter().copied(), &in_l, &in_r, &mut out_l, &mut out_r);
        let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());

        assert_eq!(
            after,
            before,
            "block {block} of {blocks} at {frames} frames allocated {} times and freed {} \
             times on the callback",
            after.0 - before.0,
            after.1 - before.1
        );
        if let Some((index, sample)) = out_l
            .iter()
            .chain(out_r.iter())
            .enumerate()
            .find(|(_, sample)| !sample.is_finite())
        {
            panic!("block {block} at {frames} frames produced {sample} at sample {index}");
        }
        assert_eq!(
            meters.output_faults(),
            0,
            "block {block} at {frames} frames sent a non-finite sample to the master, \
             which the output guard scrubbed"
        );
        loudest = out_l
            .iter()
            .chain(out_r.iter())
            .fold(loudest, |peak, sample| peak.max(sample.abs()));

        while evt_rx.pop().is_ok() {}
        while let Ok(reclaim) = reclaim_rx.pop() {
            drop(reclaim);
        }
    }
    loudest
}

/// **Every device kind, through the executor, allocates nothing, frees
/// nothing and stays finite** -- at a small block, an odd one, the ordinary
/// one and the largest the engine accepts.
#[test]
fn every_device_kind_soaks_through_the_executor_without_allocating() {
    // About two bars at each size: long enough for every note to start and
    // end, the take to run, the install to land and its retired generation
    // to leave through the reclaim ring.
    for frames in [64, 333, 1_024, MAX_BLOCK_SIZE] {
        let blocks = (4 * SAMPLE_RATE as usize).div_ceil(frames).max(32);
        let loudest = soak_at(frames, blocks);
        assert!(
            loudest > 1.0e-3,
            "the soak at {frames} frames was silent, so it proves nothing: {loudest}"
        );
    }
}
