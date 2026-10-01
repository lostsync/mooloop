//! A channel deleted from the window reaches the engine as one command, not
//! an install (MOO-466), and its undo entry still restores the document.

use super::*;

/// A window and an engine holding the starter kit, as a song just opened.
fn opened() -> (MainWindow, Rc<RefCell<UiState>>, EngineHandle, Project) {
    i_slint_backend_testing::init_no_event_loop();
    let window = MainWindow::new().expect("the testing backend builds a window");
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    let mut handle = EngineHandle::without_device("channel removal tests");
    let project = Project::starter_kit();
    assert!(install_project_in_ui(&mut handle, None, &state, &window, &project, &[], false));
    (window, state, handle, project)
}

/// Delete `index` the way the channel rack's menu does, and hand back what
/// the pump receives.
fn delete(state: &Rc<RefCell<UiState>>, window: &MainWindow, index: usize) -> ProjectEdit {
    let (tx, rx) = std::sync::mpsc::channel();
    assert!(queue_channel_delete(
        &ProjectEditSender(tx),
        state,
        window,
        index,
        "Channel deleted"
    ));
    match rx.try_recv() {
        Ok(PendingEngineMessage::ProjectEdit(edit)) => edit,
        _ => panic!("a delete queues a project edit"),
    }
}

fn channel_ids(state: &Rc<RefCell<UiState>>) -> Vec<mooloop_core::ChannelId> {
    state.borrow().session.channels.iter().map(|channel| channel.id).collect()
}

/// **A delete installs nothing.** The pump routes it to the command, the
/// engine counts no install, and the window and session hold the edited
/// document.
#[test]
fn a_channel_delete_reaches_the_engine_without_an_install() {
    let (window, state, mut handle, project) = opened();
    let installs = handle.installs_queued();
    let edit = delete(&state, &window, 1);

    let channel = lone_channel_removal(&edit, false).expect("a lone delete is a command");
    assert_eq!(channel, 1);
    assert!(remove_channel_in_ui(
        &mut handle,
        None,
        &state,
        &window,
        &edit.project,
        &edit.samples,
        channel
    ));

    assert_eq!(handle.installs_queued(), installs, "the delete installed a project");
    let mut expected: Vec<mooloop_core::ChannelId> = project.channels.iter().map(|channel| channel.id).collect();
    expected.remove(1);
    assert_eq!(channel_ids(&state), expected);
    assert_eq!(state.borrow().rows.row_count(), expected.len());
}

/// **The delete's undo entry still restores the document**: the history
/// records it as before, and undoing it (which still installs) brings the
/// channel back where it was.
#[test]
fn undoing_a_command_delete_restores_the_channel() {
    let (window, state, mut handle, project) = opened();
    let edit = delete(&state, &window, 2);
    let Some((HistoryMove::Record, entry)) = edit.history.clone() else {
        panic!("a delete records an undo entry")
    };
    assert!(remove_channel_in_ui(
        &mut handle,
        None,
        &state,
        &window,
        &edit.project,
        &edit.samples,
        2
    ));

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

    let expected: Vec<mooloop_core::ChannelId> = project.channels.iter().map(|channel| channel.id).collect();
    assert_eq!(channel_ids(&state), expected, "the undo did not bring the channel back");
}

/// **Only a lone, recorded delete is a command.** A paste, a move, an undo
/// and a delete merged behind a full ring all still install.
#[test]
fn only_a_lone_recorded_delete_skips_the_install() {
    let (window, state, _handle, _project) = opened();
    let edit = delete(&state, &window, 0);
    assert_eq!(lone_channel_removal(&edit, false), Some(0));
    assert_eq!(lone_channel_removal(&edit, true), None, "a merged install");

    let retagged = |list: Option<ListEdit>| ProjectEdit {
        project: edit.project.clone(),
        samples: edit.samples.clone(),
        status: edit.status.clone(),
        history: edit.history.clone(),
        edit: list,
    };
    for other in [
        Some(ListEdit::Channel(ChannelEdit::Inserted(1))),
        Some(ListEdit::Channel(ChannelEdit::Moved { from: 0, to: 2 })),
        None,
    ] {
        assert_eq!(lone_channel_removal(&retagged(other), false), None, "{other:?}");
    }
    let mut undo = retagged(edit.edit);
    if let Some((movement, _)) = undo.history.as_mut() {
        *movement = HistoryMove::Undo;
    }
    assert_eq!(lone_channel_removal(&undo, false), None, "an undo");
}
