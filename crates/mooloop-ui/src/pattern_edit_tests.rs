//! A pattern cloned, cleared or removed from the window reaches the engine
//! as one command, not an install (MOO-466), and its undo entry still
//! restores the document.

use super::*;
use mooloop_core::PatternEdit;

/// A window and an engine holding the starter kit with a beat in its one
/// pattern, as a song just opened.
fn opened() -> (MainWindow, Rc<RefCell<UiState>>, EngineHandle) {
    i_slint_backend_testing::init_no_event_loop();
    let window = MainWindow::new().expect("the testing backend builds a window");
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    let mut handle = EngineHandle::without_device("pattern edit tests");
    let mut project = Project::starter_kit();
    for (index, channel) in project.channels.iter_mut().enumerate() {
        channel.notes[0].push(mooloop_core::NoteEvent::new(1, index as u32 * 96, 24, 36, 110));
        channel.next_note_id = 2;
    }
    assert!(install_project_in_ui(&mut handle, None, &state, &window, &project, &[], false));
    (window, state, handle)
}

type Queue = fn(&ProjectEditSender, &Rc<RefCell<UiState>>, &MainWindow, usize, &'static str) -> bool;

/// Queue a pattern edit the way the pattern menu does, and hand back what
/// the pump receives.
fn queued(state: &Rc<RefCell<UiState>>, window: &MainWindow, queue: Queue) -> ProjectEdit {
    let (tx, rx) = std::sync::mpsc::channel();
    let index = state.borrow().session.current_pattern;
    assert!(queue(&ProjectEditSender(tx), state, window, index, "Pattern edited"));
    match rx.try_recv() {
        Ok(PendingEngineMessage::ProjectEdit(edit)) => edit,
        _ => panic!("a pattern edit queues a project edit"),
    }
}

/// Run `edit` through the pump's routing: the command when it is one, the
/// install otherwise. Answers whether it went as the command.
fn pump(
    handle: &mut EngineHandle,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    edit: &ProjectEdit,
) -> bool {
    let pattern_edit = lone_pattern_edit(edit, false).expect("a lone pattern edit is a command");
    assert!(edit_pattern_in_ui(
        handle,
        None,
        state,
        window,
        &edit.project,
        &edit.samples,
        pattern_edit
    ));
    true
}

fn patterns(state: &Rc<RefCell<UiState>>) -> usize {
    state.borrow().session.pattern_lengths.len()
}

/// **A clone, a clear and a removal install nothing.** Each is routed to
/// the command, the engine counts no install, and the session holds the
/// edited document: two patterns after the clone, the copy current, its
/// notes gone after the clear, one pattern again after the removal.
#[test]
fn a_pattern_edit_reaches_the_engine_without_an_install() {
    let (window, state, mut handle) = opened();
    let installs = handle.installs_queued();
    let notes = |state: &Rc<RefCell<UiState>>, pattern: usize| -> usize {
        state
            .borrow()
            .session
            .project_snapshot(120, 0)
            .channels
            .iter()
            .map(|channel| channel.notes[pattern].len())
            .sum()
    };
    let starter = notes(&state, 0);
    assert!(starter > 0, "the starter kit has notes to clone");

    let clone = queued(&state, &window, queue_pattern_clone);
    assert_eq!(lone_pattern_edit(&clone, false), Some(PatternEdit::Cloned(0)));
    assert!(pump(&mut handle, &state, &window, &clone));
    assert_eq!(patterns(&state), 2);
    assert_eq!(state.borrow().session.current_pattern, 1, "the copy is current");
    assert_eq!(notes(&state, 1), starter);

    let clear = queued(&state, &window, queue_pattern_clear);
    assert_eq!(lone_pattern_edit(&clear, false), Some(PatternEdit::Cleared(1)));
    assert!(pump(&mut handle, &state, &window, &clear));
    assert_eq!(notes(&state, 1), 0);
    assert_eq!(notes(&state, 0), starter);

    let remove = queued(&state, &window, queue_pattern_remove);
    assert_eq!(lone_pattern_edit(&remove, false), Some(PatternEdit::Removed(1)));
    assert!(pump(&mut handle, &state, &window, &remove));
    assert_eq!(patterns(&state), 1);

    assert_eq!(handle.installs_queued(), installs, "a pattern edit installed a project");
}

/// **Each edit's undo entry still restores the document**: undoing it
/// (which still installs) brings back what it took.
#[test]
fn undoing_a_command_pattern_edit_restores_the_pattern() {
    let (window, state, mut handle) = opened();
    let clone = queued(&state, &window, queue_pattern_clone);
    assert!(pump(&mut handle, &state, &window, &clone));
    let before = state.borrow().session.project_snapshot(120, 0);

    for queue in [queue_pattern_clear as Queue, queue_pattern_remove] {
        let edit = queued(&state, &window, queue);
        let Some((HistoryMove::Record, entry)) = edit.history.clone() else {
            panic!("a pattern edit records an undo entry")
        };
        assert!(pump(&mut handle, &state, &window, &edit));

        let restored = entry.before.clone();
        assert!(install_project_in_ui(
            &mut handle,
            None,
            &state,
            &window,
            &restored.project,
            &restored.seated(),
            true
        ));
        let after = state.borrow().session.project_snapshot(120, 0);
        assert_eq!(after.pattern_lengths, before.pattern_lengths, "the undo did not restore the bank");
        for (restored, original) in after.channels.iter().zip(&before.channels) {
            assert_eq!(restored.notes, original.notes, "the undo did not restore the notes");
        }
    }
}

/// **Only a lone, recorded pattern edit is a command.** One merged behind a
/// full ring, an undo, and a channel or track edit all still install.
#[test]
fn only_a_lone_recorded_pattern_edit_skips_the_install() {
    let (window, state, _handle) = opened();
    let edit = queued(&state, &window, queue_pattern_clone);
    assert_eq!(lone_pattern_edit(&edit, false), Some(PatternEdit::Cloned(0)));
    assert_eq!(lone_pattern_edit(&edit, true), None, "a merged install");

    let retagged = |list: Option<ListEdit>| ProjectEdit {
        project: edit.project.clone(),
        samples: edit.samples.clone(),
        status: edit.status.clone(),
        history: edit.history.clone(),
        edit: list,
    };
    for other in [
        Some(ListEdit::Channel(ChannelEdit::Removed(1))),
        Some(ListEdit::Track(mooloop_core::TrackEdit::Removed(1))),
        None,
    ] {
        assert_eq!(lone_pattern_edit(&retagged(other), false), None, "{other:?}");
    }
    assert_eq!(
        lone_channel_edit(&edit, false),
        None,
        "a pattern edit went down the channel path"
    );
    let mut undo = retagged(edit.edit);
    if let Some((movement, _)) = undo.history.as_mut() {
        *movement = HistoryMove::Undo;
    }
    assert_eq!(lone_pattern_edit(&undo, false), None, "an undo");
}
