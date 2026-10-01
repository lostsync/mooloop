//! Pattern edits as incremental commands: a clone, a clear or a removal
//! reaches the sequencer as one `StructuralCommand` and leaves it as an
//! install of the edited project would (MOO-466).
//!
//! The acceptance case is parity, in-process: the same playing song, edited
//! once by the command and once by the install path the executor runs
//! (prepare, carry, swap), renders the same samples afterwards; and where an
//! edit should be inaudible, the same as a song that never had it made.
//! Beside them: the callback allocates nothing, and a refused edit changes
//! nothing.

use std::sync::Arc;

use mooloop_core::{
    AutomationLane, AutomationPoint, DeviceKind, EffectKind, EffectSlotState, EffectTarget,
    EngineCommand, ModLfoParams, ModPolarity, ModRoute, ModulatorParams, NoteEvent, ParamAddr,
    PatternEdit, PatternPlacement, PlaybackMode, Project, ProjectChannel, TICKS_PER_BAR,
};
use mooloop_dsp::SampleData;

use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use crate::sequencer::PatternChange;
use crate::{carry_plan, StructuralCommand, StructuralReclaim};

const BLOCK: usize = 256;
const PATTERNS: usize = 4;

fn tone() -> Arc<SampleData> {
    Arc::new(SampleData {
        frames: (0..SAMPLE_RATE as usize / 2)
            .map(|frame| {
                let s = (frame as f32 * 220.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin();
                [s * 0.5, s * 0.4]
            })
            .collect(),
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    })
}

/// Four channels of different kinds and four patterns of different notes, a
/// note in every pattern held for its whole bar, a lane on channel 3's filter
/// in pattern 0, and an LFO routed to channel 1's delay and strip.
///
/// Song mode places patterns 0 and 1 together on bar 0, then 2, then 3, then
/// 1 again; pattern mode plays `current`.
fn song(mode: PlaybackMode, current: usize) -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths = vec![16; PATTERNS];
    let kinds = [DeviceKind::Ds01, DeviceKind::PolySynth, DeviceKind::Sampler, DeviceKind::MlM1];
    for (index, kind) in kinds.into_iter().enumerate() {
        let mut channel = match kind {
            DeviceKind::Ds01 => ProjectChannel::ds01(index, PATTERNS),
            DeviceKind::PolySynth => ProjectChannel::poly_synth(index, PATTERNS),
            DeviceKind::Sampler => ProjectChannel::sampler(index, PATTERNS),
            _ => ProjectChannel::mlm1(index, PATTERNS),
        };
        channel.setup.channel.volume = 0.4;
        for pattern in 0..PATTERNS {
            let notes = &mut channel.notes[pattern];
            let base = 40 + (pattern * 5 + index) as u8;
            let id = pattern as u32 * 100;
            for (step, offset) in [(0u32, 0u8), (4, 7), (8, 12), (12, 3)] {
                notes.push(NoteEvent::new(id + step + 1, step * 24, 96 + 24, base + 12 + offset, 110));
            }
            // Held from the top of the bar to its end, across the edit.
            notes.push(NoteEvent::new(id + 50, 0, 16 * 24, base, 90));
        }
        project.channels.push(channel);
    }
    project.assign_channel_ids();
    for (index, kind) in [(1, EffectKind::Delay), (3, EffectKind::Filter)] {
        project.channels[index]
            .setup
            .push_effect(EffectSlotState::of_kind(kind))
            .expect("room in the chain");
    }
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
    let lane = filter_lane(&project);
    project.channels[3].automation[0].push(lane);
    project.playback_mode = mode;
    project.current_pattern = current as u16;
    project.playlist = [(0, 0), (1, 0), (2, 1), (3, 2), (1, 3)]
        .into_iter()
        .map(|(pattern, bar)| PatternPlacement::new(pattern, bar * TICKS_PER_BAR))
        .collect();
    project
}

/// A sweep of channel 3's filter across the bar.
fn filter_lane(project: &Project) -> AutomationLane {
    let device = project.channels[3].setup.effects[0].id;
    let mut lane = AutomationLane::new(ParamAddr::effect(EffectTarget::Channel(3), device, 0));
    lane.reserve_points();
    lane.reset_points([
        AutomationPoint::new(1, 0, 0.1),
        AutomationPoint::new(2, 192, 0.9),
        AutomationPoint::new(3, 383, 0.2),
    ]);
    lane
}

/// `project` with `edit` made the way the document makes it.
fn edited(project: &Project, edit: PatternEdit) -> Project {
    let mut project = project.clone();
    let at = usize::from(edit.at());
    let changed = match edit {
        PatternEdit::Cloned(_) => project.clone_pattern(at),
        PatternEdit::Removed(_) => project.remove_pattern(at),
        PatternEdit::Cleared(_) => {
            for channel in &mut project.channels {
                channel.notes[at].clear();
                channel.automation[at].clear();
            }
            true
        }
    };
    assert!(changed, "{edit:?} is an edit the document can make");
    project
}

fn samples(project: &Project) -> Vec<Option<Arc<SampleData>>> {
    project
        .channels
        .iter()
        .map(|channel| (channel.setup.source.kind() == DeviceKind::Sampler).then(tone))
        .collect()
}

/// A state playing `project`, run far enough into bar 0 that every channel
/// is sounding.
fn playing(project: &Project) -> RenderState {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &samples(project));
    render.play();
    for _ in 0..40 {
        render.process_once_block(BLOCK);
    }
    render
}

/// Apply `edit` to `live` the way the handle does, `incoming` being the
/// project it produced.
fn command(live: &mut RenderState, edit: PatternEdit, incoming: &Project) -> Box<PatternChange> {
    let change = PatternChange::new(edit, incoming);
    match live.apply_structural(StructuralCommand::EditPattern { change }) {
        Some(StructuralReclaim::PatternEdited(change)) => change,
        _ => panic!("a pattern edit comes back as PatternEdited"),
    }
}

/// Install `incoming` over `live` the way the executor does for an edit.
fn install(mut live: RenderState, live_project: &Project, incoming: &Project) -> RenderState {
    let mut fresh = RenderState::from_project(SAMPLE_RATE, incoming, &samples(incoming));
    fresh.adopt_performance_state(&live);
    fresh.carry_strips_from(&mut live, &carry_plan(live_project, incoming));
    fresh
}

/// Bar 0's remainder and the three bars after it, so every renumbered
/// placement plays.
fn render(render: &mut RenderState) -> Vec<f32> {
    let blocks = (4 * 2 * SAMPLE_RATE as usize) / BLOCK;
    let mut out = Vec::with_capacity(blocks * BLOCK * 2);
    for _ in 0..blocks {
        render.process_once_block(BLOCK);
        let master = render.master();
        out.extend_from_slice(&master.l[..BLOCK]);
        out.extend_from_slice(&master.r[..BLOCK]);
    }
    out
}

fn assert_same(edited: &[f32], reference: &[f32], what: &str) {
    assert!(
        reference.iter().any(|sample| sample.abs() > 1e-3),
        "{what}: the song is silent, so nothing is compared"
    );
    let first = edited
        .iter()
        .zip(reference)
        .position(|(a, b)| a.to_bits() != b.to_bits());
    assert_eq!(first, None, "{what}: parted at sample {first:?}");
}

/// **A pattern edit sounds exactly like the install it replaces**, in both
/// modes: the edited pattern's held notes end where the install ends them,
/// the others ring on, and every renumbered placement plays its pattern.
///
/// Pattern mode edits the pattern it is playing, as the window does. The
/// lane is on pattern 0, which no case here clears or removes while it is
/// under the playhead; the next test but one holds that case to a live lane
/// clear instead.
#[test]
fn a_pattern_edit_renders_what_installing_the_result_renders() {
    let song_cases = [
        PatternEdit::Cloned(0),
        PatternEdit::Cloned(1),
        PatternEdit::Cloned(3),
        PatternEdit::Cleared(1),
        PatternEdit::Cleared(2),
        PatternEdit::Removed(1),
        PatternEdit::Removed(2),
        PatternEdit::Removed(3),
    ];
    let pattern_cases = [
        PatternEdit::Cloned(0),
        PatternEdit::Cloned(2),
        PatternEdit::Cleared(1),
        PatternEdit::Removed(1),
        PatternEdit::Removed(3),
    ];
    let cases = song_cases
        .into_iter()
        .map(|edit| (PlaybackMode::Song, edit))
        .chain(pattern_cases.into_iter().map(|edit| (PlaybackMode::Pattern, edit)));
    for (mode, edit) in cases {
        let project = song(mode, usize::from(edit.at()));
        let incoming = edited(&project, edit);

        let mut by_command = playing(&project);
        let change = command(&mut by_command, edit, &incoming);
        assert!(change.applied(), "{mode:?} {edit:?} was refused");
        let mut by_install = install(playing(&project), &project, &incoming);

        assert_same(
            &render(&mut by_command),
            &render(&mut by_install),
            &format!("{mode:?} {edit:?}: the command against the install"),
        );
    }
}

/// **An edit to a pattern that is not sounding is inaudible** until that
/// pattern comes round: the song plays exactly as one that had the edit made
/// before it started -- held notes, the lane on pattern 0 and the routed LFO
/// all running across it. A clone of what pattern mode is playing is
/// inaudible outright: the copy plays on from where the original was.
#[test]
fn an_edit_away_from_the_playhead_changes_nothing_audible() {
    let cases = [
        (PlaybackMode::Song, PatternEdit::Cloned(1)),
        (PlaybackMode::Song, PatternEdit::Cloned(2)),
        (PlaybackMode::Song, PatternEdit::Cleared(2)),
        (PlaybackMode::Song, PatternEdit::Cleared(3)),
        (PlaybackMode::Song, PatternEdit::Removed(2)),
        (PlaybackMode::Song, PatternEdit::Removed(3)),
        (PlaybackMode::Pattern, PatternEdit::Cloned(0)),
        (PlaybackMode::Pattern, PatternEdit::Cloned(2)),
        (PlaybackMode::Pattern, PatternEdit::Cleared(1)),
    ];
    for (mode, edit) in cases {
        // Pattern mode plays what it clones, and something else than what
        // it clears.
        let current = match edit {
            PatternEdit::Cleared(_) => 0,
            _ => usize::from(edit.at()),
        };
        let project = song(mode, current);
        let incoming = edited(&project, edit);

        let mut by_command = playing(&project);
        assert!(command(&mut by_command, edit, &incoming).applied());
        let mut untouched = playing(&incoming);

        assert_same(
            &render(&mut by_command),
            &render(&mut untouched),
            &format!("{mode:?} {edit:?}: the command against a song made that way"),
        );
    }
}

/// **A cleared lane under the playhead hands its knob back**, exactly as
/// clearing that lane live does. Pattern 0 holds only the lane here, so the
/// clear takes nothing else away; pattern 1, under the same bar, sounds
/// through the filter.
#[test]
fn clearing_a_pattern_under_its_lane_hands_the_knob_back() {
    let mut project = song(PlaybackMode::Song, 0);
    for channel in &mut project.channels {
        channel.notes[0].clear();
    }
    let edit = PatternEdit::Cleared(0);
    let incoming = edited(&project, edit);
    let target = project.channels[3].automation[0][0].target;

    let mut by_command = playing(&project);
    assert!(command(&mut by_command, edit, &incoming).applied());
    let mut by_lane_clear = playing(&project);
    by_lane_clear.apply_command(EngineCommand::ClearAutomationLane {
        pattern: 0,
        channel: 3,
        target,
    });

    assert_same(
        &render(&mut by_command),
        &render(&mut by_lane_clear),
        "a pattern clear against a live lane clear",
    );
}

/// **The install leaves a cleared lane's knob where the lane parked it**
/// (MOO-494), where the command hands it back: the reason the install parity
/// test clears no pattern whose lane is under the playhead. With the
/// command's hand-back taken out the two are bit-identical, so the lane is
/// the whole difference. When this starts failing, the install has learned
/// the hand-back and `Cleared(0)` can join the parity test.
#[test]
fn the_install_leaves_a_cleared_lanes_knob_parked() {
    let edit = PatternEdit::Cleared(0);
    let project = song(PlaybackMode::Song, 0);
    let incoming = edited(&project, edit);
    let mut by_command = playing(&project);
    assert!(command(&mut by_command, edit, &incoming).applied());
    let mut by_install = install(playing(&project), &project, &incoming);

    let (commanded, installed) = (render(&mut by_command), render(&mut by_install));
    assert!(
        commanded.iter().zip(&installed).any(|(a, b)| a.to_bits() != b.to_bits()),
        "the install handed the knob back"
    );
}

/// **The callback allocates nothing, frees nothing and takes no lock**
/// applying a pattern edit: the pattern it displaces, notes and lane points
/// included, goes back in the payload.
#[test]
fn a_pattern_edit_allocates_nothing_on_the_callback() {
    assert!(
        mooloop_core::lock_check::counting(),
        "a build without debug assertions counts no locks, so the lock half would pass unchecked"
    );
    for mode in [PlaybackMode::Song, PlaybackMode::Pattern] {
        for edit in [PatternEdit::Cloned(0), PatternEdit::Cleared(0), PatternEdit::Removed(0)] {
            let project = song(mode, 0);
            let incoming = edited(&project, edit);
            let mut live = playing(&project);
            let change = PatternChange::new(edit, &incoming);
            let command = StructuralCommand::EditPattern { change };

            let locks = mooloop_core::lock_check::locks_taken();
            let before = (crate::COUNTING.allocations(), crate::COUNTING.frees());
            let reclaimed = live.apply_structural(command);
            live.process_once_block(BLOCK);
            let after = (crate::COUNTING.allocations(), crate::COUNTING.frees());
            let locked = mooloop_core::lock_check::locks_taken() - locks;

            assert_eq!(after, before, "{mode:?} {edit:?} allocated or freed on the callback");
            assert_eq!(locked, 0, "{mode:?} {edit:?} took a lock on the callback");
            match reclaimed {
                Some(StructuralReclaim::PatternEdited(change)) => {
                    assert!(change.applied());
                    if !matches!(edit, PatternEdit::Cloned(_)) {
                        assert!(
                            !change.displaced().channels[3].lanes().is_empty(),
                            "{edit:?}: pattern 0's lane did not come back in the payload"
                        );
                    }
                }
                _ => panic!("{mode:?} {edit:?}: the payload did not come back"),
            }
        }
    }
}

/// **An edit the bank cannot take changes nothing**: a pattern past the
/// song, the only pattern removed, a clone into a full bank. The payload
/// comes back unapplied and the song plays on as if it was never sent.
#[test]
fn a_refused_pattern_edit_changes_nothing() {
    let project = song(PlaybackMode::Song, 0);
    let mut one = song(PlaybackMode::Pattern, 0);
    assert!(one.remove_pattern(3) && one.remove_pattern(2) && one.remove_pattern(1));
    let mut full = song(PlaybackMode::Song, 0);
    while full.clone_pattern(0) {}
    assert_eq!(full.pattern_lengths.len(), mooloop_core::MAX_PATTERNS);
    let cases = [
        (&project, PatternEdit::Cleared(PATTERNS as u8)),
        (&project, PatternEdit::Removed(PATTERNS as u8)),
        (&project, PatternEdit::Cloned(PATTERNS as u8)),
        (&one, PatternEdit::Removed(0)),
        (&full, PatternEdit::Cloned(0)),
    ];
    for (song, edit) in cases {
        let mut live = playing(song);
        let change = command(&mut live, edit, song);
        assert!(!change.applied(), "{edit:?} landed");
        let mut untouched = playing(song);
        assert_same(
            &render(&mut live),
            &render(&mut untouched),
            &format!("refused {edit:?} against no edit"),
        );
    }
}
