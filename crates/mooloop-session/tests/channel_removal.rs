//! A channel removal, move or paste sent as one engine command keeps what
//! the reconcilers had sent, renumbered, so the tick after it resends nothing
//! the edit did not change (MOO-466).

use std::sync::Arc;

use mooloop_core::{ChannelEdit, DeviceKind, EffectKind, EffectTarget, EngineCommand, MusicalEdge};
use mooloop_engine::{CommandSink, EngineHandle, InputState, StructuralCommand};
use mooloop_session::session::Session;

/// A command sink that takes everything and counts it, and which channels
/// were sent a compensation ring.
#[derive(Default)]
struct Counting {
    sent: usize,
    compensated: Vec<EffectTarget>,
}

impl CommandSink for Counting {
    fn send(&mut self, _cmd: EngineCommand) -> bool {
        self.sent += 1;
        true
    }

    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        self.sent += 1;
        if let StructuralCommand::SetCompensation { target, .. } = cmd {
            self.compensated.push(target);
        }
        true
    }

    fn send_deferred(&mut self, _cmd: EngineCommand, _when: MusicalEdge) -> bool {
        self.sent += 1;
        true
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }
}

/// Four channels, a Drive (which declares a latency) on the last, so the
/// compensation plan has an entry for every channel before it.
fn session_owing_compensation() -> Session {
    let mut session = Session::default();
    for _ in 0..3 {
        session.add_channel(DeviceKind::Sampler);
    }
    session.selected = 3;
    session.effect_target = EffectTarget::Channel(3);
    session
        .insert_effect_at(EffectKind::Drive, 0)
        .expect("an empty chain has room");
    session.selected = 0;
    session
}

fn sync_all(session: &mut Session, sink: &mut impl CommandSink) {
    session.sync_compensation(sink);
    session.sync_audio_graph(sink);
    session.sync_console_sums(sink);
    session.sync_solo(sink);
    session.sync_channel_solo(sink);
    session.sync_track_graph(sink);
}

fn no_input() -> InputState {
    InputState {
        record_armed: false,
        midi_routing: Vec::new(),
        audio_input: Vec::new(),
        monitor: Vec::new(),
    }
}

/// **The tick after a command removal sends nothing**, where the install it
/// replaces would have the reconcilers resend the whole plan.
#[test]
fn a_command_removal_leaves_the_reconcilers_nothing_to_resend() {
    let mut session = session_owing_compensation();
    let mut handle = EngineHandle::without_device("channel removal test");
    sync_all(&mut session, &mut handle);
    let mut edited = session.project_snapshot(120, 0);
    edited.remove_channel(1).expect("a channel to remove");

    // The install: the session forgets what it sent.
    let mut installed = session_owing_compensation();
    sync_all(&mut installed, &mut Counting::default());
    installed.replace_project(&edited, &[]);
    let mut resent = Counting::default();
    sync_all(&mut installed, &mut resent);
    assert!(resent.sent > 0, "the install would resend nothing, so nothing is measured");

    // The command: it keeps it, renumbered.
    let sent = session.engine_mirrors();
    session.replace_project(&edited, &[]);
    assert!(session.send_channel_edit(
        &mut handle,
        ChannelEdit::Removed(1),
        Arc::new(edited),
        no_input(),
        sent
    ));
    let mut after = Counting::default();
    sync_all(&mut session, &mut after);
    assert_eq!(after.sent, 0, "the tick after the removal resent what the engine kept");
}

/// **The tick after a command move sends nothing either**: the compensation
/// the latent channel was owed moves with it, so nothing is resent, where an
/// install would resend the plan.
#[test]
fn a_command_move_leaves_the_reconcilers_nothing_to_resend() {
    let mut session = session_owing_compensation();
    let mut handle = EngineHandle::without_device("channel move test");
    sync_all(&mut session, &mut handle);
    let mut edited = session.project_snapshot(120, 0);
    let edit = edited.move_channel(3, 0).expect("a channel to move");

    let mut installed = session_owing_compensation();
    sync_all(&mut installed, &mut Counting::default());
    installed.replace_project(&edited, &[]);
    let mut resent = Counting::default();
    sync_all(&mut installed, &mut resent);
    assert!(resent.sent > 0, "the install would resend nothing, so nothing is measured");

    let sent = session.engine_mirrors();
    session.replace_project(&edited, &[]);
    assert!(session.send_channel_edit(&mut handle, edit, Arc::new(edited), no_input(), sent));
    let mut after = Counting::default();
    sync_all(&mut session, &mut after);
    assert_eq!(after.sent, 0, "the tick after the move resent what the engine kept");
}

/// **The tick after a command paste sends only what the arrival is owed.**
/// A copy of the latent channel owes nothing and nothing is sent; a copy of
/// a channel that waits for it owes that wait, and its compensation is the
/// one thing sent -- where an install would resend the plan.
#[test]
fn a_command_paste_leaves_the_reconcilers_only_the_arrivals_compensation() {
    for (copied, at, owed) in [(3, 0, false), (3, 2, false), (1, 0, true), (0, 4, true)] {
        let mut session = session_owing_compensation();
        let mut handle = EngineHandle::without_device("channel paste test");
        sync_all(&mut session, &mut handle);
        let mut edited = session.project_snapshot(120, 0);
        let copy = edited.channels[copied].clone();
        assert_eq!(edited.insert_channel(at, copy), Some(at));

        let mut installed = session_owing_compensation();
        sync_all(&mut installed, &mut Counting::default());
        installed.replace_project(&edited, &[]);
        let mut resent = Counting::default();
        sync_all(&mut installed, &mut resent);
        assert!(resent.sent > 1, "the install would resend nothing, so nothing is measured");

        let installs = handle.installs_queued();
        let sent = session.engine_mirrors();
        session.replace_project(&edited, &[]);
        assert!(session.send_channel_edit(
            &mut handle,
            ChannelEdit::Inserted(at as u8),
            Arc::new(edited),
            no_input(),
            sent
        ));
        assert_eq!(handle.installs_queued(), installs, "the paste installed a project");
        let mut after = Counting::default();
        sync_all(&mut session, &mut after);
        let expected = if owed { vec![EffectTarget::Channel(at as u8)] } else { Vec::new() };
        assert_eq!(after.compensated, expected, "pasting {copied} at {at}");
        assert_eq!(after.sent, expected.len(), "pasting {copied} at {at}: resent what the engine kept");
    }
}
