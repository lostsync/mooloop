//! A pattern clone, clear or removal sent as one engine command keeps what
//! the reconcilers had sent, so the tick after it resends nothing (MOO-466):
//! a pattern edit moves no channel, track or edge.

use std::sync::Arc;

use mooloop_core::{DeviceKind, EffectKind, EffectTarget, EngineCommand, MusicalEdge, PatternEdit};
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

/// Four channels, a Drive (which declares a latency) on the last, so the
/// compensation plan has an entry for every channel before it, and a second
/// pattern.
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

/// **The tick after a command pattern edit sends nothing**, where the
/// install it replaces would have the reconcilers resend the whole plan.
#[test]
fn a_command_pattern_edit_leaves_the_reconcilers_nothing_to_resend() {
    for edit in [PatternEdit::Cloned(0), PatternEdit::Cleared(0), PatternEdit::Removed(0)] {
        let mut session = session_owing_compensation();
        // Two patterns, so a removal leaves one behind.
        let mut two = session.project_snapshot(120, 0);
        assert!(two.clone_pattern(0));
        session.replace_project(&two, &[]);
        let mut handle = EngineHandle::without_device("pattern edit test");
        assert!(handle.install_project(Arc::new(two), Vec::new(), InputState::default(), false));
        sync_all(&mut session, &mut handle);
        let mut edited = session.project_snapshot(120, 0);
        match edit {
            PatternEdit::Cloned(at) => assert!(edited.clone_pattern(usize::from(at))),
            PatternEdit::Removed(at) => assert!(edited.remove_pattern(usize::from(at))),
            PatternEdit::Cleared(at) => {
                for channel in &mut edited.channels {
                    channel.notes[usize::from(at)].clear();
                    channel.automation[usize::from(at)].clear();
                }
            }
        }

        let sent = session.engine_mirrors();
        session.replace_project(&edited, &[]);
        assert!(session.send_pattern_edit(&mut handle, edit, Arc::new(edited), sent), "{edit:?}");
        let mut after = Counting::default();
        sync_all(&mut session, &mut after);
        assert_eq!(after.sent, 0, "{edit:?}: the tick after the edit resent what the engine kept");
    }
}
