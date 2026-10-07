//! The engine running the song's one modulation set
//! (`docs/plans/archive/song-modulation/02`):
//!
//! - routes onto a track (MOO-497): a track's inserts and its fader take the
//!   song's routes as a channel's do, in playback and in an export, and a
//!   track fed again after a silence reads each route where it is;
//! - a set that grows: a new set, however much larger, is installed by the
//!   callback without allocating or freeing.

use std::sync::Arc;

use mooloop_core::{
    EffectSlotState, EffectTarget, FilterMode, FilterParams, ModLfoParams,
    ModLfoWaveform, ModPolarity, ModRoute, ModSourceId, ModTimeDivision, ModulatorParams,
    NoteEvent, ParamAddr, PatternPlacement, Project, ProjectChannel, SongModule,
    FILTER_PARAM_CUTOFF_HZ, STRIP_PARAM_VOLUME, TICKS_PER_STEP,
};
use mooloop_dsp::SampleData;

use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use crate::{
    ExportFormat, ExportSpec, OfflineRenderer, RenderScope, SongModulator, StructuralCommand,
    StructuralReclaim, WavEncoding,
};

const BLOCK: usize = 256;
/// A bar at 120 BPM, in frames.
const BAR: usize = SAMPLE_RATE as usize * 2;

fn tone() -> Arc<SampleData> {
    Arc::new(SampleData {
        frames: (0..SAMPLE_RATE as usize / 2)
            .map(|frame| {
                let s = (frame as f32 * 2_000.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin();
                [s * 0.5, s * 0.5]
            })
            .collect(),
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    })
}

/// What on track 1 a route drives.
#[derive(Debug, Clone, Copy)]
enum Drives {
    /// The cutoff of the filter in its chain.
    Insert,
    /// Its fader.
    Fader,
}

/// One sampler channel playing a bright tone into track 1, whose chain is a
/// low-pass filter, with notes on the steps given in a pattern `bars` long.
/// `lfo` drives each of `destinations` at full depth.
fn song(steps: &[u32], bars: u16, lfo: ModLfoParams, destinations: &[Drives]) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.ensure_tracks(2);
    let mut channel = ProjectChannel::sampler(0, 1);
    channel.setup.channel.bus = 1;
    for (id, step) in steps.iter().enumerate() {
        channel.notes[0].push(NoteEvent::new(
            id as u32 + 1,
            step * TICKS_PER_STEP,
            TICKS_PER_STEP * 2,
            60,
            127,
        ));
    }
    project.channels.push(channel);
    project.assign_channel_ids();
    project.pattern_lengths[0] = 16 * bars;
    project.playlist = vec![PatternPlacement::new(0, 0)];
    let filter = project.buses[1]
        .push_effect(EffectSlotState::filter(FilterParams {
            cutoff_hz: 1_200.0,
            resonance: 0.0,
            mode: FilterMode::LowPass,
            ..FilterParams::default()
        }))
        .expect("room in track 1's chain");
    let cutoff = ParamAddr::effect(EffectTarget::Bus(1), filter, FILTER_PARAM_CUTOFF_HZ);
    let id = ModSourceId(1);
    project.modulation.modules.push(SongModule {
        id,
        name: String::new(),
        seed: 0,
        at: Default::default(),
        open: false,
        rack: None,
        params: ModulatorParams::Lfo(lfo),
    });
    project.modulation.next_source_id = 2;
    for destination in destinations {
        let destination = match destination {
            Drives::Insert => cutoff,
            Drives::Fader => ParamAddr::strip(EffectTarget::Bus(1), STRIP_PARAM_VOLUME),
        };
        project
            .modulation
            .routes
            .push(ModRoute::from_module(id, destination, 1.0, ModPolarity::Bipolar));
    }
    project
}

/// A square LFO a few times a second: a route on it moves whatever it
/// drives by its whole throw, twice a cycle.
fn square() -> ModLfoParams {
    ModLfoParams {
        rate_hz: 6.0,
        waveform: ModLfoWaveform::Square,
        ..ModLfoParams::default()
    }
}

fn live(project: &Project, blocks: usize, skip_idle: bool) -> (Vec<f32>, bool) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[Some(tone())]);
    render.set_idle_skipping(skip_idle);
    render.play();
    let mut out = Vec::with_capacity(blocks * BLOCK);
    let mut slept = false;
    for _ in 0..blocks {
        render.process_once_block(BLOCK);
        out.extend_from_slice(&render.master().l[..BLOCK]);
        slept |= render.track_sleeping(1);
    }
    (out, slept)
}

fn offline(project: &Project, name: &str) -> Vec<f32> {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(name);
    OfflineRenderer::render(
        project,
        &[Some(tone())],
        SAMPLE_RATE,
        &ExportSpec {
            path: path.clone(),
            scope: RenderScope::Song,
            tail_seconds: 0.0,
            format: ExportFormat::Wav(WavEncoding::Float32),
        },
    )
    .expect("it renders");
    hound::WavReader::open(&path)
        .expect("readable")
        .samples::<f32>()
        .map(Result::unwrap)
        .step_by(2)
        .collect()
}

fn rms_db(block: &[f32]) -> f64 {
    let power = block.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / block.len().max(1) as f64;
    10.0 * power.max(1e-20).log10()
}

/// The largest level difference between two renders, window by window,
/// over the windows where the plain one sounds.
fn widest_gap(ours: &[f32], plain: &[f32]) -> f64 {
    const WINDOW: usize = 1_200;
    ours.chunks(WINDOW)
        .zip(plain.chunks(WINDOW))
        .filter(|(_, plain)| rms_db(plain) > -60.0)
        .map(|(ours, plain)| (rms_db(ours) - rms_db(plain)).abs())
        .fold(0.0, f64::max)
}

/// **A route moves a track's insert and a track's fader, in playback and in
/// an export** (MOO-497). Shaped against the tree before step 02, where a
/// track's chain was handed no modulation and its fader ran plain: both
/// renders matched the unrouted song.
#[test]
fn a_route_moves_a_track_insert_and_a_track_fader_live_and_offline() {
    let notes = [0, 4, 8, 12];
    let plain = song(&notes, 1, square(), &[]);
    let (plain_live, _) = live(&plain, 300, true);
    let plain_offline = offline(&plain, "plain.wav");
    assert!(rms_db(&plain_live) > -40.0, "the song sounds");
    for drives in [Drives::Insert, Drives::Fader] {
        let routed = song(&notes, 1, square(), &[drives]);
        let (routed_live, _) = live(&routed, 300, true);
        let routed_offline = offline(&routed, "routed.wav");
        let gap = widest_gap(&routed_live, &plain_live);
        assert!(gap > 3.0, "{drives:?} did not move in playback: {gap:.2} dB");
        let gap = widest_gap(&routed_offline, &plain_offline);
        assert!(gap > 3.0, "{drives:?} did not move in an export: {gap:.2} dB");
    }
}

/// **A route into a track that goes quiet is not lost** (`docs/plans/
/// song-modulation/02`, "Sleeping chains"). A synced saw over two bars
/// drives the track's filter, or its fader; the track is fed in bar 1, has
/// nothing through bar 2, and is fed again in bar 3. Bar 3 sounds as it does
/// when the track is never let sleep -- not as the value bar 1 left would.
///
/// A moving route keeps what it drives from settling, so the track stays
/// awake through the silence rather than sleeping and catching up; the same
/// song unrouted does sleep, so the mechanism is live here and either answer
/// would be held to the same bar 3.
#[test]
fn a_track_fed_again_after_a_silence_reads_its_routes_where_they_are() {
    let saw = ModLfoParams {
        tempo_sync: true,
        rate_division: ModTimeDivision::DoubleWhole,
        waveform: ModLfoWaveform::Saw,
        // Half way up the saw at the top of bar 3, so the fader is open.
        phase: 0.5,
        ..ModLfoParams::default()
    };
    let blocks = 3 * BAR / BLOCK;
    let bar_3 = 2 * BAR;
    // Steps 0 and 32: the top of bar 1 and of bar 3.
    let (_, unrouted_slept) = live(&song(&[0, 32], 3, saw, &[]), blocks, true);
    assert!(unrouted_slept, "the track never sleeps in this song, so nothing is tested");
    for drives in [Drives::Insert, Drives::Fader] {
        let project = song(&[0, 32], 3, saw, &[drives]);
        let (skipping, _) = live(&project, blocks, true);
        let (awake, stayed_awake) = live(&project, blocks, false);
        assert!(!stayed_awake);
        assert!(rms_db(&awake[bar_3..bar_3 + 4_800]) > -40.0, "{drives:?}: bar 3 sounds");
        let gap = widest_gap(&skipping[bar_3..], &awake[bar_3..]);
        assert!(gap < 0.5, "{drives:?}: bar 3 is {gap:.2} dB from the track that stayed awake");
    }
}

/// **An edit that grows the set past anything it held installs without the
/// callback allocating** (the plan's "ceiling"). There is no ceiling to pass:
/// a change of shape arrives as a whole set built off the audio thread, the
/// callback carries each surviving module's state into it and swaps, and
/// the old set goes back in the reclaim to be dropped off it.
#[test]
fn a_set_grown_past_what_it_held_installs_without_allocating() {
    assert!(
        mooloop_core::lock_check::counting(),
        "a build without debug assertions counts no locks, so the lock half would pass unchecked"
    );
    let project = song(&[0, 4, 8, 12], 1, square(), &[Drives::Insert, Drives::Fader]);
    let mut live = RenderState::from_project(SAMPLE_RATE, &project, &[Some(tone())]);
    live.play();
    for _ in 0..20 {
        live.process_once_block(BLOCK);
    }

    // Forty more modules, each routed onto the channel's fader and the
    // track's: well past the eight modules and sixteen routes a 0.1.6 rack
    // was sized for, and past the one module this set was built with.
    let mut grown = project.clone();
    for id in 2..42 {
        grown.modulation.modules.push(SongModule {
            id: ModSourceId(id),
            name: String::new(),
            seed: id,
            at: Default::default(),
            open: false,
            rack: None,
            params: ModulatorParams::Lfo(ModLfoParams {
                rate_hz: 0.5 + id as f32 * 0.1,
                ..ModLfoParams::default()
            }),
        });
        for destination in [
            ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME),
            ParamAddr::strip(EffectTarget::Bus(1), STRIP_PARAM_VOLUME),
        ] {
            grown.modulation.routes.push(ModRoute::from_module(
                ModSourceId(id),
                destination,
                0.01,
                ModPolarity::Bipolar,
            ));
        }
    }
    grown.modulation.next_source_id = 42;
    let set = SongModulator::of_project(&grown);

    let locks = mooloop_core::lock_check::locks_taken();
    let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    let reclaimed = live.apply_structural(StructuralCommand::SetModulation { set });
    for _ in 0..4 {
        live.process_once_block(BLOCK);
    }
    let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
    let locked = mooloop_core::lock_check::locks_taken() - locks;
    assert_eq!(after, before, "the callback allocated or freed installing a grown set");
    assert_eq!(locked, 0, "the callback took a lock installing a grown set");

    assert!(
        matches!(&reclaimed, Some(StructuralReclaim::Modulation(old)) if old.plan().modules.len() == 1),
        "the old set comes back to be dropped off the audio thread"
    );
    let running = live.song_modulation().plan();
    assert_eq!(running.modules.len(), 41);
    assert_eq!(running.routes.len(), 82);
}
