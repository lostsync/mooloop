//! A channel deleted, moved or pasted from the window reaches the engine as
//! one command, not an install (MOO-466), and its undo entry still restores
//! the document.

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

    let channel_edit = lone_channel_edit(&edit, false).expect("a lone delete is a command");
    assert_eq!(channel_edit, ChannelEdit::Removed(1));
    assert!(edit_channels_in_ui(
        &mut handle,
        None,
        &state,
        &window,
        &edit.project,
        &edit.samples,
        channel_edit
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
    assert!(edit_channels_in_ui(
        &mut handle,
        None,
        &state,
        &window,
        &edit.project,
        &edit.samples,
        ChannelEdit::Removed(2)
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

/// **Only a lone, recorded channel edit is a command.** A track edit, an
/// untagged edit, an undo and an edit merged behind a full ring all still
/// install.
#[test]
fn only_a_lone_recorded_channel_edit_skips_the_install() {
    let (window, state, _handle, _project) = opened();
    let edit = delete(&state, &window, 0);
    assert_eq!(lone_channel_edit(&edit, false), Some(ChannelEdit::Removed(0)));
    assert_eq!(lone_channel_edit(&edit, true), None, "a merged install");

    let retagged = |list: Option<ListEdit>| ProjectEdit {
        project: edit.project.clone(),
        samples: edit.samples.clone(),
        status: edit.status.clone(),
        history: edit.history.clone(),
        edit: list,
    };
    for channel_edit in [ChannelEdit::Moved { from: 0, to: 2 }, ChannelEdit::Inserted(1)] {
        assert_eq!(
            lone_channel_edit(&retagged(Some(ListEdit::Channel(channel_edit))), false),
            Some(channel_edit)
        );
    }
    for other in [Some(ListEdit::Track(mooloop_core::TrackEdit::Removed(1))), None] {
        assert_eq!(lone_channel_edit(&retagged(other), false), None, "{other:?}");
    }
    let mut undo = retagged(edit.edit);
    if let Some((movement, _)) = undo.history.as_mut() {
        *movement = HistoryMove::Undo;
    }
    assert_eq!(lone_channel_edit(&undo, false), None, "an undo");
}

/// Move `from` to `to` the way the channel rack's drag does, and hand back
/// what the pump receives.
fn move_channel(
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    from: usize,
    to: usize,
) -> ProjectEdit {
    let (tx, rx) = std::sync::mpsc::channel();
    assert!(queue_channel_move(
        &ProjectEditSender(tx),
        state,
        window,
        from,
        to,
        "Channel moved"
    ));
    match rx.try_recv() {
        Ok(PendingEngineMessage::ProjectEdit(edit)) => edit,
        _ => panic!("a move queues a project edit"),
    }
}

/// **A move installs nothing either**, and the window and session hold the
/// channels in their new order; undoing it (which still installs) puts them
/// back.
#[test]
fn a_channel_move_reaches_the_engine_without_an_install() {
    let (window, state, mut handle, project) = opened();
    let installs = handle.installs_queued();
    let edit = move_channel(&state, &window, 0, 2);

    let channel_edit = lone_channel_edit(&edit, false).expect("a lone move is a command");
    assert_eq!(channel_edit, ChannelEdit::Moved { from: 0, to: 2 });
    assert!(edit_channels_in_ui(
        &mut handle,
        None,
        &state,
        &window,
        &edit.project,
        &edit.samples,
        channel_edit
    ));

    assert_eq!(handle.installs_queued(), installs, "the move installed a project");
    let original: Vec<mooloop_core::ChannelId> =
        project.channels.iter().map(|channel| channel.id).collect();
    let mut expected = original.clone();
    let lifted = expected.remove(0);
    expected.insert(2, lifted);
    assert_eq!(channel_ids(&state), expected);

    let Some((HistoryMove::Record, entry)) = edit.history.clone() else {
        panic!("a move records an undo entry")
    };
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
    assert_eq!(channel_ids(&state), original, "the undo did not put the order back");
}

/// Paste a copy of `copied` after `after` the way the channel rack's menu
/// does (a clone is a copy pasted straight after itself), and hand back what
/// the pump receives.
fn paste(state: &Rc<RefCell<UiState>>, window: &MainWindow, copied: usize, after: usize) -> ProjectEdit {
    let copy = state
        .borrow_mut()
        .session
        .channel_clipboard(copied, window.get_bpm(), window.get_swing_percent())
        .expect("a channel to copy");
    let (tx, rx) = std::sync::mpsc::channel();
    assert!(queue_channel_insert(
        &ProjectEditSender(tx),
        state,
        window,
        after,
        copy,
        "Channel pasted"
    ));
    match rx.try_recv() {
        Ok(PendingEngineMessage::ProjectEdit(edit)) => edit,
        _ => panic!("a paste queues a project edit"),
    }
}

/// **A paste installs nothing either.** The pasted channel lands after the
/// one it was pasted after, the window and session hold it with everyone
/// else in order, and undoing it (which still installs) takes it out again.
#[test]
fn a_channel_paste_reaches_the_engine_without_an_install() {
    let (window, state, mut handle, project) = opened();
    let installs = handle.installs_queued();
    let edit = paste(&state, &window, 0, 1);

    let channel_edit = lone_channel_edit(&edit, false).expect("a lone paste is a command");
    assert_eq!(channel_edit, ChannelEdit::Inserted(2));
    assert!(edit_channels_in_ui(
        &mut handle,
        None,
        &state,
        &window,
        &edit.project,
        &edit.samples,
        channel_edit
    ));

    assert_eq!(handle.installs_queued(), installs, "the paste installed a project");
    let original: Vec<mooloop_core::ChannelId> =
        project.channels.iter().map(|channel| channel.id).collect();
    let ids = channel_ids(&state);
    assert_eq!(ids.len(), original.len() + 1);
    let mut others = ids.clone();
    let pasted = others.remove(2);
    assert_eq!(others, original, "a channel other than the paste moved");
    assert!(!original.contains(&pasted), "the paste wears another channel's identity");
    assert_eq!(state.borrow().rows.row_count(), ids.len());

    let Some((HistoryMove::Record, entry)) = edit.history.clone() else {
        panic!("a paste records an undo entry")
    };
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
    assert_eq!(channel_ids(&state), original, "the undo did not take the paste out");
}
