//! A track addition, removal or move sent as one engine command leaves the
//! reconcilers' mirrors at what the command installed, so the tick after it
//! resends nothing (MOO-466).

use std::sync::Arc;

use mooloop_core::{
    AuxSend, EffectKind, EffectSlotState, EngineCommand, MusicalEdge, Project, TrackEdit,
};
use mooloop_engine::{CommandSink, EngineHandle, InputState, StructuralCommand};
use mooloop_session::session::Session;

/// A command sink that takes everything and counts it.
#[derive(Default)]
struct Counting {
    sent: usize,
}

impl CommandSink for Counting {
    fn send(&mut self, _cmd: EngineCommand) -> bool {
        self.sent += 1;
        true
    }

    fn send_structural(&mut self, _cmd: StructuralCommand) -> bool {
        self.sent += 1;
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

fn sync_all(session: &mut Session, sink: &mut impl CommandSink) {
    session.sync_compensation(sink);
    session.sync_audio_graph(sink);
    session.sync_console_sums(sink);
    session.sync_solo(sink);
    session.sync_channel_solo(sink);
    session.sync_track_graph(sink);
}

/// The starter kit over four tracks: a latent limiter on track 1 with a
/// send to track 3, track 2 console-encoded into track 4 and soloed, the kit
/// spread across them -- so compensation, a send, a console sum and a solo
/// all have something to say about every track.
fn song() -> Project {
    let mut project = Project::starter_kit();
    project.ensure_tracks(5);
    for (index, channel) in project.channels.iter_mut().enumerate() {
        channel.setup.channel.bus = 1 + (index % 4) as u8;
    }
    project.buses[1]
        .push_effect(EffectSlotState::of_kind(EffectKind::Limiter))
        .expect("room in track 1's chain");
    project.buses[1].sends.push(AuxSend::new(3));
    project.buses[2].bus.output = 4;
    project.buses[2].bus.console = true;
    project.buses[2].bus.solo = true;
    project
}

/// A session and an engine that have taken `project` in and been told
/// everything the reconcilers say about it.
fn synced(project: &Project) -> (Session, EngineHandle) {
    let mut session = Session::default();
    session.replace_project(project, &[]);
    let mut handle = EngineHandle::without_device("track edit test");
    assert!(handle.install_project(Arc::new(project.clone()), Vec::new(), InputState::default(), false));
    sync_all(&mut session, &mut handle);
    (session, handle)
}

/// **The tick after a command track edit sends nothing**, where the install
/// it replaces would have the reconcilers resend the plan.
#[test]
fn a_command_track_edit_leaves_the_reconcilers_nothing_to_resend() {
    let project = song();
    let mut edits: Vec<(TrackEdit, Project)> = Vec::new();
    for track in 1..=4 {
        let mut incoming = project.clone();
        incoming.remove_track(track).expect("a track to remove");
        edits.push((TrackEdit::Removed(track as u8), incoming));
    }
    for (from, to) in [(1, 4), (4, 1), (2, 3)] {
        let mut incoming = project.clone();
        let edit = incoming.move_track(from, to).expect("a track to move");
        edits.push((edit, incoming));
    }
    let mut incoming = project.clone();
    let at = incoming.add_track().expect("room for a track");
    edits.push((TrackEdit::Inserted(at as u8), incoming));

    for (edit, incoming) in edits {
        let (mut installed, _) = synced(&project);
        installed.replace_project(&incoming, &[]);
        let mut resent = Counting::default();
        sync_all(&mut installed, &mut resent);
        assert!(resent.sent > 0, "{edit:?}: the install would resend nothing, so nothing is measured");

        let (mut session, mut handle) = synced(&project);
        let installs = handle.installs_queued();
        let sent = session.engine_mirrors();
        session.replace_project(&incoming, &[]);
        assert!(
            session.send_track_edit(&mut handle, edit, Arc::new(incoming), InputState::default(), sent),
            "{edit:?} was refused"
        );
        assert_eq!(handle.installs_queued(), installs, "{edit:?} installed a project");
        let mut after = Counting::default();
        sync_all(&mut session, &mut after);
        assert_eq!(after.sent, 0, "{edit:?}: the tick after the edit resent what the command installed");
    }
}

/// **An edit that is not what the engine holds is refused**, and the
/// caller installs: a removal of a track the engine never had, or an edit
/// before the engine took any project in.
#[test]
fn a_track_edit_the_engine_cannot_take_is_refused() {
    let project = song();
    let (mut session, mut handle) = synced(&project);
    let mut incoming = project.clone();
    incoming.remove_track(2).unwrap();
    let sent = session.engine_mirrors();
    session.replace_project(&incoming, &[]);
    assert!(!session.send_track_edit(
        &mut handle,
        TrackEdit::Removed(6),
        Arc::new(incoming.clone()),
        InputState::default(),
        sent.clone()
    ));
    let mut fresh = EngineHandle::without_device("track edit test, nothing installed");
    assert!(!session.send_track_edit(
        &mut fresh,
        TrackEdit::Removed(2),
        Arc::new(incoming),
        InputState::default(),
        sent
    ));
}
