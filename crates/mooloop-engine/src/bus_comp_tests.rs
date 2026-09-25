//! The Bus Comp insert where the engine runs it (MOO-216).
//!
//! `mooloop_dsp::effects::bus_comp` shows the node is the master section's
//! `BusComp` to the bit. These show the chain adds nothing around it: an
//! insert at the end of the master's chain *is* the master's built-in
//! section at the same settings, and one on a drum bus is the section run
//! over that bus. And that it saves, reopens and exports as it plays.

use crate::render::RenderState;
use crate::render_test_support::{peak_of, render_mix, worst_difference, SAMPLE_RATE};
use mooloop_core::strip::{BusCompVoicing, MasterSectionParams};
use mooloop_core::{
    BusCompParams, DrumMode, DrumSynthParams, EffectParams, EffectSlotState, NoteEvent, Project,
    ProjectChannel, MASTER_BUS,
};

/// The drum bus the insert is for.
const DRUMS: usize = 1;

/// Two bars of kick, snare and hats on the drum bus, loud enough at unity to
/// be compressed.
fn drum_bus() -> Project {
    let mut channels = Vec::new();
    for (mode, steps) in [
        (DrumMode::Kick, vec![0u32, 6, 10, 16, 22, 26]),
        (DrumMode::Snare, vec![4, 12, 20, 28]),
        (DrumMode::Hat, (0..32).step_by(2).collect::<Vec<u32>>()),
    ] {
        let mut channel = ProjectChannel::drum_synth_with_params(
            channels.len(),
            1,
            DrumSynthParams {
                mode,
                ..DrumSynthParams::default()
            },
        );
        channel.setup.channel.bus = DRUMS as u8;
        channel.setup.channel.volume = 1.0;
        for (n, step) in steps.into_iter().enumerate() {
            let tick = step * mooloop_core::TICKS_PER_STEP;
            channel.notes[0].push(NoteEvent::new((n + 1) as _, tick, 12, 60, 120));
        }
        channels.push(channel);
    }
    let mut project = Project {
        channels,
        pattern_lengths: vec![32],
        ..Project::default()
    };
    project.ensure_tracks(DRUMS + 1);
    project
}

fn settings(voicing: BusCompVoicing) -> BusCompParams {
    BusCompParams {
        voicing,
        threshold_db: -24.0,
        makeup_db: 2.0,
        mix: 0.9,
        ..BusCompParams::default()
    }
}

/// `project` with a Bus Comp at the end of bus `bus`'s chain.
fn with_insert(mut project: Project, bus: usize, params: BusCompParams) -> Project {
    let bus = &mut project.buses[bus];
    bus.effects.push(EffectSlotState::bus_comp(params));
    mooloop_core::assign_device_ids(&mut bus.effects, &mut bus.next_device_id);
    project
}

fn with_section(mut project: Project, section: MasterSectionParams) -> Project {
    project.buses[MASTER_BUS as usize].bus.strip.master = section;
    project
}

/// **An insert at the end of the master's chain is the master's section.**
/// The section runs after the master's inserts and before its fader, so a
/// Bus Comp in the last slot runs at the same point in the signal, and with
/// the host at full wet and unity trims the two renders are the same under
/// every voicing, sample for sample: the host at full wet is its device
/// exactly since MOO-226. Rendered with the safety limiter off, so it is the
/// mix being compared and not the limiter.
#[test]
fn a_bus_comp_last_on_the_masters_chain_is_the_masters_section() {
    let dry = render_mix(&drum_bus(), 2.0);
    for voicing in BusCompVoicing::ALL {
        let params = settings(voicing);
        let insert = render_mix(&with_insert(drum_bus(), MASTER_BUS as usize, params), 2.0);
        let section = render_mix(&with_section(drum_bus(), params.section()), 2.0);
        assert!(
            worst_difference(&insert.0, &dry.0) > 1e-3,
            "{voicing:?} did not compress, so this proves nothing"
        );
        assert!(
            insert == section,
            "{voicing:?}: the insert and the section differ, by up to {:e}",
            worst_difference(&insert.0, &section.0).max(worst_difference(&insert.1, &section.1))
        );
    }
}

/// On the drum bus, which is all the master hears here, the insert does
/// what the master's section does over the same drums: the bus's own strip
/// is at unity, so the reduction measured at the master is the same, sample
/// for sample.
#[test]
fn a_bus_comp_on_the_drum_bus_reduces_as_the_masters_section_does() {
    for voicing in BusCompVoicing::ALL {
        let params = settings(voicing);
        let insert = render_mix(&with_insert(drum_bus(), DRUMS, params), 2.0);
        let section = render_mix(&with_section(drum_bus(), params.section()), 2.0);
        assert!(peak_of(&insert.0) > 0.05, "the drums have to be sounding");
        assert!(
            insert == section,
            "{voicing:?}: the insert and the section differ, by up to {:e}",
            worst_difference(&insert.0, &section.0).max(worst_difference(&insert.1, &section.1))
        );
    }
}

/// Saved, reopened with nothing repaired and the same settings, and
/// exported: the export is the live render of the reopened song, frame for
/// frame.
#[test]
fn a_bus_comp_saves_reopens_and_exports_as_it_plays() {
    let params = BusCompParams {
        voicing: BusCompVoicing::Tube,
        tube_time: 4,
        ..settings(BusCompVoicing::Tube)
    };
    let song = with_insert(drum_bus(), DRUMS, params);
    let temp = tempfile::tempdir().expect("a temp dir");
    let path = temp.path().join("bus-comp.mooloop");
    mooloop_project::save_song(&path, &song, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let report = mooloop_project::load_bundle(&path).expect("it reopens");
    assert!(report.repairs.is_empty(), "repairs: {:?}", report.repairs);
    let mooloop_project::LoadedDocument::Song(mut reopened) = report.document else {
        panic!("a song came back as something else");
    };
    let saved = reopened.buses[DRUMS].effects.last().expect("the insert").params;
    assert_eq!(saved, EffectParams::BusComp(params), "the settings moved on the way");

    let wav = temp.path().join("bus-comp.wav");
    crate::offline::OfflineRenderer::render(
        &reopened,
        &[],
        SAMPLE_RATE,
        &crate::offline::ExportSpec {
            path: wav.clone(),
            scope: crate::offline::RenderScope::Pattern { index: 0 },
            tail_seconds: 0.0,
            format: crate::offline::ExportFormat::Wav(crate::offline::WavEncoding::Float32),
        },
    )
    .expect("it renders offline");
    let exported: Vec<f32> = hound::WavReader::open(&wav)
        .expect("the export reads back")
        .samples::<f32>()
        .map(|sample| sample.expect("a sample"))
        .collect();

    reopened.playback_mode = mooloop_core::PlaybackMode::Pattern;
    reopened.current_pattern = 0;
    let mut live_state = RenderState::from_project(SAMPLE_RATE, &reopened, &[]);
    live_state.play();
    let frames = exported.len() / 2;
    let mut live = Vec::with_capacity(frames * 2 + 1024);
    while live.len() < frames * 2 {
        live_state.process_block(256);
        let master = live_state.master();
        for frame in 0..256 {
            live.push(master.l[frame]);
            live.push(master.r[frame]);
        }
    }
    live.truncate(frames * 2);
    assert!(peak_of(&exported) > 0.05, "the export is silent");
    // The last frame is the pattern's end in the export and its wrap live;
    // the layer's parity test found the two a denormal apart there.
    let body = frames * 2 - 2;
    assert_eq!(exported[..body], live[..body], "export and live disagree");
}
