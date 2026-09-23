//! Document fixtures (MOO-121): a project that uses as much of the document
//! as can be built here, compared **whole** after each trip it takes --
//! through the session (`replace_project` then `project_snapshot`, which is
//! what every undo and every save does) and through a file (`save_song` then
//! `load_bundle`).
//!
//! The session's other round trip (`structure.rs`) checks chosen fields. A
//! field it does not name can be dropped by either direction and nothing
//! fails; `Project` derives `PartialEq`, so here nothing is chosen.

use mooloop_core::{
    AutomationLane, AutomationPoint, AuxSend, ChannelSetup, ControlBinding, ControlMode,
    ControlSource, ControlTarget, DeviceKind, EffectKind, EffectSlotState, EffectTarget,
    GeneratorParams, ModPolarity, ModRoute, ModulatorKind, NoteEvent, ParamAddr, ParamCurve,
    ParamDescriptor, ParamOwner, PatternPlacement, Project, ProjectChannel, ProjectColor,
    TransportControl,
};
use mooloop_session::session::Session;

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

/// A value for `descriptor` that is not its default, and that it reads back
/// as itself: the second step of a stepped parameter, else a point 37% of
/// the way along its range.
fn off_default(descriptor: &ParamDescriptor) -> f32 {
    match descriptor.curve {
        ParamCurve::Stepped(steps) if steps > 1 => {
            let step = (descriptor.max - descriptor.min) / f32::from(steps - 1);
            let first = descriptor.min + step;
            if (first - descriptor.default).abs() < step * 0.5 {
                descriptor.min
            } else {
                first
            }
        }
        _ => {
            let value = descriptor.min + (descriptor.max - descriptor.min) * 0.37;
            if (value - descriptor.default).abs() < 1.0e-6 {
                descriptor.min + (descriptor.max - descriptor.min) * 0.61
            } else {
                value
            }
        }
    }
}

fn source_setup(name: &str, params: GeneratorParams) -> ChannelSetup {
    match params {
        GeneratorParams::Sampler(params) => {
            let mut setup = ChannelSetup::sampler(name);
            setup.sampler_state_mut().expect("a sampler").params = params;
            setup
        }
        GeneratorParams::DrumSynth(params) => ChannelSetup::drum_synth_with_params(name, params),
        GeneratorParams::MonoSynth(params) => ChannelSetup::mono_synth_with_params(name, params),
        GeneratorParams::PolySynth(params) => ChannelSetup::poly_synth_with_params(name, params),
        GeneratorParams::MlM1(params) => ChannelSetup::mlm1_with_params(name, params),
        GeneratorParams::MlP8(params) => ChannelSetup::mlp8_with_params(name, params),
        GeneratorParams::Ds01(params) => ChannelSetup::ds01_with_params(name, params),
        GeneratorParams::AuxIn(params) => ChannelSetup::aux_in_with_params(name, params),
    }
}

/// One channel per source kind with every parameter moved off its default,
/// every effect kind on the first channel's chain with its parameters,
/// bypass, wet/dry and trims moved too, notes and automation in three
/// patterns, a playlist, a loop, names, colours and mixer settings.
///
/// It also has one modulator of each kind with a route each, both container
/// kinds (empty; `EffectKind::ALL` includes them), two MIDI bindings and a
/// send. Not yet in it: a container holding devices, and a binding on a
/// parameter (it needs a `ParamKey`).
fn maximal_project() -> Project {
    let mut project = Project::starter_kit(0x5eed);
    project.bpm = 97;
    project.swing_percent = 61;
    // Not `beats_per_bar`: the integrity pass holds it to 4 until the engine
    // can honour a signature (`Project::beats_per_bar`), so a 3 is repaired,
    // by design, on the way to disk.
    project.pattern_lengths = vec![16, 8, 32];
    project.playlist = vec![
        PatternPlacement::new(0, 0),
        PatternPlacement::new(2, 384),
        PatternPlacement::new(1, 1152),
    ];
    project.current_pattern = 2;
    let patterns = project.pattern_lengths.len();

    for channel in &mut project.channels {
        channel.notes.resize_with(patterns, Vec::new);
        channel.automation.resize_with(patterns, Vec::new);
    }
    for (index, kind) in KINDS.into_iter().enumerate() {
        let mut params = kind.default_generator_params();
        for descriptor in kind.descriptors() {
            params.set(descriptor.id, off_default(descriptor));
        }
        // A sample region the integrity pass keeps: start before end and the
        // loop inside it. Moved one by one, all four land on one fraction,
        // which is a region of nothing and is repaired on save.
        if let GeneratorParams::Sampler(sampler) = &mut params {
            sampler.start = 0.1;
            sampler.end = 0.9;
            sampler.loop_start = 0.3;
            sampler.loop_end = 0.7;
        }
        let mut setup = source_setup(&format!("{kind:?} maximal"), params);
        setup.channel.color = Some(ProjectColor::new(10 * index as u8, 200, 90));
        setup.channel.volume = 0.3 + 0.05 * index as f32;
        setup.channel.pan = -0.4 + 0.1 * index as f32;
        setup.channel.muted = index % 3 == 0;
        setup.channel.solo = index % 4 == 1;
        let id = mooloop_core::mint_channel_id(&mut project.next_channel_id);
        let mut channel = ProjectChannel {
            id,
            setup,
            notes: vec![Vec::new(); patterns],
            automation: vec![Vec::new(); patterns],
            next_note_id: 1,
        };
        for pattern in 0..patterns {
            channel.notes[pattern].push(NoteEvent::new(
                channel.next_note_id,
                (pattern as u32 + 1) * 24,
                48,
                36 + index as u8,
                70 + pattern as u8,
            ));
            channel.next_note_id += 1;
        }
        project.channels.push(channel);
    }

    // Every effect kind, on the first maximal channel.
    let first = project.channels.len() - KINDS.len();
    let target = EffectTarget::Channel(first as u8);
    for (index, kind) in EffectKind::ALL.into_iter().enumerate() {
        let mut slot = EffectSlotState::of_kind(kind);
        for descriptor in kind.descriptors() {
            slot.params.set(descriptor.id, off_default(descriptor));
        }
        slot.bypassed = index % 2 == 0;
        slot.wet_dry = 0.8;
        slot.input_trim = 0.9;
        slot.output_trim = 1.1;
        project.channels[first]
            .setup
            .push_effect(slot)
            .expect("the chain has room for one of each kind");
    }
    // Automation on a source parameter, in two patterns.
    let source_param = DeviceKind::Sampler.descriptors()[0].id;
    let mut lane = AutomationLane::new(ParamAddr {
        scope: target,
        owner: ParamOwner::Source,
        param: source_param,
    });
    let point = lane.allocate_id();
    let _ = lane.upsert(AutomationPoint::new(point, 0, 0.25));
    let point = lane.allocate_id();
    let _ = lane.upsert(AutomationPoint::new(point, 96, 0.75));
    // One modulator of each kind, each routed to a source parameter, half
    // of them unipolar.
    let rack = &mut project.channels[first].setup.modulation;
    for (slot, kind) in ModulatorKind::ALL.into_iter().enumerate() {
        rack.install(slot, kind.default_params())
            .expect("the rack has a slot for one of each kind");
        let destination = ParamAddr {
            scope: target,
            owner: ParamOwner::Source,
            param: DeviceKind::Sampler.descriptors()[slot + 1].id,
        };
        let polarity = if slot % 2 == 0 {
            ModPolarity::Bipolar
        } else {
            ModPolarity::Unipolar
        };
        rack.add_route(ModRoute::to_slot(slot as u8, destination, 0.2 + 0.1 * slot as f32, polarity))
            .expect("the rack has room for the route");
    }
    project.channels[first].automation[0].push(lane.clone());
    project.channels[first].automation[2].push(lane);
    // A MIDI binding in each mode that has a setting of its own, on the
    // transport, whose targets need no addressing to be valid.
    project.control_map.bindings = vec![
        ControlBinding {
            source: ControlSource::Cc {
                port: Default::default(),
                channel: Default::default(),
                controller: 21,
            },
            target: ControlTarget::Transport(TransportControl::Play),
            mode: ControlMode::Toggle,
            min: 0.0,
            max: 1.0,
        },
        ControlBinding {
            source: ControlSource::Cc {
                port: Default::default(),
                channel: Default::default(),
                controller: 22,
            },
            target: ControlTarget::Transport(TransportControl::Stop),
            mode: ControlMode::Relative {
                encoding: Default::default(),
                step: 0.25,
            },
            min: 0.2,
            max: 0.8,
        },
    ];
    // The starter kit's sends into its reverb, one of them switched off and
    // at a level: the state a send can be in that a default one is not.
    let send: &mut AuxSend = project
        .buses
        .iter_mut()
        .flat_map(|track| track.sends.iter_mut())
        .next()
        .expect("the starter kit sends to its reverb");
    send.level = 0.4;
    send.enabled = false;
    // A hosted plugin nobody here has installed: its slot, a sparse
    // parameter list, a state, the device that names it, and a lane on one
    // of its parameters (`docs/plans/plugin-hosting/`, steps 02 and 03).
    let mut slot = mooloop_core::PluginSlotState::new(mooloop_core::PluginRef {
        format: mooloop_core::PluginFormat::Clap,
        id: "com.example.fixture".to_owned(),
        name: "Fixture".to_owned(),
        vendor: "Example".to_owned(),
        version: "2.0".to_owned(),
    });
    slot.params = vec![mooloop_core::PluginParamInfo {
        id: 4_000_000_000,
        name: "Drive".to_owned(),
        module: "Main".to_owned(),
        min: 0.0,
        max: 10.0,
        default: 1.0,
        stepped: Some(11),
        automatable: true,
        modulatable: false,
        hidden: false,
    }];
    slot.state = mooloop_core::PluginStateText(mooloop_core::PluginState {
        chunks: vec![mooloop_core::PluginStateChunk {
            tag: "clap".to_owned(),
            data: (0..=255u8).collect(),
        }],
    });
    let plugin_slot = project.add_plugin_slot(slot);
    let mut device = EffectSlotState::of_kind(EffectKind::Plugin);
    device.params = mooloop_core::EffectParams::Plugin(plugin_slot);
    project.channels[first].setup.push_effect(device);
    let plugin_device = project.channels[first].setup.effects.last().unwrap().id;
    let mut plugin_lane = AutomationLane::new(ParamAddr::plugin_param(
        EffectTarget::Channel(first as u8),
        plugin_device,
        4_000_000_000,
    ));
    let point = plugin_lane.allocate_id();
    let _ = plugin_lane.upsert(AutomationPoint::new(point, 0, 0.25));
    project.channels[first].automation[1].push(plugin_lane);
    project.selected_channel = project.channels[first].id;
    // What the app does to every document it holds: references that name a
    // channel by seat take that channel's identity. Without it the save does
    // it, and the comparison below would be of two different documents.
    project.identify_channel_references();
    project
}

/// The project as the save path normalises it, so the comparisons below are
/// between two documents that were each through the same repairs once.
fn saved_and_loaded(project: &Project) -> Project {
    let temp = tempfile::tempdir().expect("a temp dir");
    let path = temp.path().join("maximal.mooloop");
    mooloop_project::save_song(&path, project, mooloop_project::AssetMode::Referenced)
        .expect("the maximal project saves");
    let report = mooloop_project::load_bundle(&path).expect("the maximal project loads");
    assert!(
        report.repairs.is_empty(),
        "the maximal project needed repairs: {:?}",
        report.repairs
    );
    match report.document {
        mooloop_project::LoadedDocument::Song(project) => project,
        _ => panic!("a song came back as something else"),
    }
}

/// **Through a file, nothing is lost.** Save then load gives back the
/// project that was saved.
#[test]
fn the_maximal_project_survives_a_file_whole() {
    let project = maximal_project();
    let once = saved_and_loaded(&project);
    assert_same(&once, &project);
}

/// **Through the session, nothing is lost** (what every undo and every save
/// does): `replace_project` then `project_snapshot` gives back the project.
#[test]
fn the_maximal_project_survives_the_session_whole() {
    let project = saved_and_loaded(&maximal_project());
    let mut session = Session::default();
    let samples = vec![None; project.channels.len()];
    session.replace_project(&project, &samples);
    let snapshot = session.project_snapshot(i32::from(project.bpm), i32::from(project.swing_percent));
    assert_same(&snapshot, &project);
}

/// `assert_eq!` on two whole projects prints two screens of `Debug`. This
/// says where they first part, with the path of fields that leads there.
fn assert_same(got: &Project, want: &Project) {
    if got == want {
        return;
    }
    let got = format!("{got:#?}");
    let want = format!("{want:#?}");
    let (got_lines, want_lines): (Vec<_>, Vec<_>) = (got.lines().collect(), want.lines().collect());
    let first = got_lines
        .iter()
        .zip(&want_lines)
        .position(|(a, b)| a != b)
        .unwrap_or(got_lines.len().min(want_lines.len()));
    let from = first.saturating_sub(12);
    panic!(
        "the projects part at line {first}\n--- got\n{}\n--- want\n{}",
        got_lines[from..(first + 4).min(got_lines.len())].join("\n"),
        want_lines[from..(first + 4).min(want_lines.len())].join("\n"),
    );
}

/// **A damaged song never panics the loader** (MOO-121's fuzzer, without
/// `cargo-fuzz`, which needs a nightly toolchain and a CI job of its own).
/// The maximal song is saved, then loaded again after each of a few
/// thousand deterministic damages -- a byte changed, a line dropped, the
/// file cut short, a line doubled -- and every load must come back `Ok` or
/// `Err`, never unwind. A panic here is a crash on File > Open.
#[test]
fn a_damaged_song_is_refused_never_panicked_on() {
    let temp = tempfile::tempdir().expect("a temp dir");
    let path = temp.path().join("maximal.mooloop");
    mooloop_project::save_song(&path, &maximal_project(), mooloop_project::AssetMode::Referenced)
        .expect("the maximal project saves");
    let original = std::fs::read(&path).expect("the song reads back");
    let damaged = temp.path().join("damaged.mooloop");

    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let lines: Vec<&[u8]> = original.split(|byte| *byte == b'\n').collect();
    for round in 0..3000 {
        let bytes = match round % 4 {
            0 => {
                let mut bytes = original.clone();
                let at = (next() as usize) % bytes.len();
                bytes[at] = b"0-9aZ\"[]=.,\n {}#'"[(next() as usize) % 17];
                bytes
            }
            1 => {
                let drop = (next() as usize) % lines.len();
                lines
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| *index != drop)
                    .map(|(_, line)| *line)
                    .collect::<Vec<_>>()
                    .join(&b'\n')
            }
            2 => original[..(next() as usize) % original.len()].to_vec(),
            _ => {
                let twice = (next() as usize) % lines.len();
                let mut out = Vec::new();
                for (index, line) in lines.iter().enumerate() {
                    out.extend_from_slice(line);
                    out.push(b'\n');
                    if index == twice {
                        out.extend_from_slice(line);
                        out.push(b'\n');
                    }
                }
                out
            }
        };
        std::fs::write(&damaged, &bytes).expect("the damaged copy writes");
        let outcome = std::panic::catch_unwind(|| mooloop_project::load_bundle(&damaged));
        if outcome.is_err() {
            let kept = temp.path().join(format!("panicked-{round}.mooloop"));
            let _ = std::fs::copy(&damaged, &kept);
            panic!("round {round}: loading a damaged song panicked; the file is {}", kept.display());
        }
    }
}
