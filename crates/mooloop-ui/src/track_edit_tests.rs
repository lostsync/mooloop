//! A track added, removed or moved from the window reaches the engine as one
//! command, not an install (MOO-466), and its undo entry still restores the
//! document.

use super::*;

/// A window and an engine holding the starter kit, as a song just opened.
fn opened() -> (MainWindow, Rc<RefCell<UiState>>, EngineHandle) {
    i_slint_backend_testing::init_no_event_loop();
    let window = MainWindow::new().expect("the testing backend builds a window");
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    let mut handle = EngineHandle::without_device("track edit tests");
    let project = Project::starter_kit();
    assert!(install_project_in_ui(&mut handle, None, &state, &window, &project, &[], false));
    (window, state, handle)
}

/// What the pump receives from one of the track verbs.
fn queued(queue: impl FnOnce(&ProjectEditSender) -> bool) -> ProjectEdit {
    let (tx, rx) = std::sync::mpsc::channel();
    assert!(queue(&ProjectEditSender(tx)), "the verb queued nothing");
    match rx.try_recv() {
        Ok(PendingEngineMessage::ProjectEdit(edit)) => edit,
        _ => panic!("a track verb queues a project edit"),
    }
}

fn track_ids(state: &Rc<RefCell<UiState>>) -> Vec<mooloop_core::TrackId> {
    state.borrow().session.buses.iter().map(|track| track.id).collect()
}

/// Route `edit` the way the pump does, asserting it is a command.
fn apply(
    handle: &mut EngineHandle,
    state: &Rc<RefCell<UiState>>,
    window: &MainWindow,
    edit: &ProjectEdit,
) -> TrackEdit {
    let track_edit = lone_track_edit(edit, false).expect("a lone track edit is a command");
    assert!(edit_tracks_in_ui(
        handle,
        None,
        state,
        window,
        &edit.project,
        &edit.samples,
        track_edit
    ));
    track_edit
}

/// Undo `edit` the way the pump does today: a whole-document install.
fn undo(handle: &mut EngineHandle, state: &Rc<RefCell<UiState>>, window: &MainWindow, edit: &ProjectEdit) {
    let Some((HistoryMove::Record, entry)) = edit.history.clone() else {
        panic!("a track edit records an undo entry")
    };
    let restored = entry.before.clone();
    assert!(install_project_in_ui(
        handle,
        None,
        state,
        window,
        &restored.project,
        &restored.seated(),
        true
    ));
}

/// **Adding, removing and moving a track each install nothing**, and the
/// window and session hold the edited bank; undoing each (which still
/// installs) puts the bank back.
#[test]
fn a_track_add_remove_and_move_reach_the_engine_without_an_install() {
    let (window, state, mut handle) = opened();
    let installs = handle.installs_queued();

    // Two added, so there is something to move and remove past.
    for _ in 0..2 {
        let before = track_ids(&state);
        let edit = queued(|tx| queue_track_add(tx, &state, &window));
        let track_edit = apply(&mut handle, &state, &window, &edit);
        assert_eq!(track_edit, TrackEdit::Inserted(before.len() as u8));
        assert_eq!(track_ids(&state).len(), before.len() + 1);
        assert_eq!(&track_ids(&state)[..before.len()], &before[..]);
    }
    assert_eq!(handle.installs_queued(), installs, "adding a track installed a project");

    let before = track_ids(&state);
    let last = before.len() - 1;
    let edit = queued(|tx| queue_track_move(tx, &state, &window, last, 1));
    assert_eq!(
        apply(&mut handle, &state, &window, &edit),
        TrackEdit::Moved {
            from: last as u8,
            to: 1
        }
    );
    let mut expected = before.clone();
    let lifted = expected.remove(last);
    expected.insert(1, lifted);
    assert_eq!(track_ids(&state), expected);
    assert_eq!(handle.installs_queued(), installs, "moving a track installed a project");
    undo(&mut handle, &state, &window, &edit);
    assert_eq!(track_ids(&state), before, "the undo did not put the order back");

    let installs = handle.installs_queued();
    let before = track_ids(&state);
    let edit = queued(|tx| queue_track_remove(tx, &state, &window, 1));
    assert_eq!(apply(&mut handle, &state, &window, &edit), TrackEdit::Removed(1));
    let mut expected = before.clone();
    expected.remove(1);
    assert_eq!(track_ids(&state), expected);
    assert_eq!(handle.installs_queued(), installs, "removing a track installed a project");
    undo(&mut handle, &state, &window, &edit);
    assert_eq!(track_ids(&state), before, "the undo did not bring the track back");
}

/// **A command that the engine cannot take installs instead**: with the
/// handle's last project out of step with the edit, the pump's route falls
/// back to the install and the document still lands.
#[test]
fn a_refused_track_command_installs_instead() {
    let (window, state, mut handle) = opened();
    let edit = queued(|tx| queue_track_add(tx, &state, &window));
    // Add a second track behind the engine's back: the edit now claims a
    // bank one track short of what the engine holds.
    let mut ahead = edit.project.clone();
    ahead.add_track().unwrap();
    assert!(install_project_in_ui(&mut handle, None, &state, &window, &ahead, &[], true));
    let installs = handle.installs_queued();

    assert!(edit_tracks_in_ui(
        &mut handle,
        None,
        &state,
        &window,
        &edit.project,
        &edit.samples,
        lone_track_edit(&edit, false).unwrap()
    ));
    assert_eq!(handle.installs_queued(), installs + 1, "the refused command did not install");
    assert_eq!(state.borrow().session.buses.len(), edit.project.buses.len());
}

/// **Only a lone, recorded track edit is a command.** A channel edit, an
/// untagged edit, an undo and an edit merged behind a full ring do not go
/// this way.
#[test]
fn only_a_lone_recorded_track_edit_skips_the_install() {
    let (window, state, _handle) = opened();
    let edit = queued(|tx| queue_track_add(tx, &state, &window));
    let added = lone_track_edit(&edit, false).expect("a lone add is a command");
    assert!(matches!(added, TrackEdit::Inserted(_)));
    assert_eq!(lone_track_edit(&edit, true), None, "a merged install");

    let retagged = |list: Option<ListEdit>| ProjectEdit {
        project: edit.project.clone(),
        samples: edit.samples.clone(),
        status: edit.status.clone(),
        history: edit.history.clone(),
        edit: list,
    };
    for other in [Some(ListEdit::Channel(ChannelEdit::Removed(1))), None] {
        assert_eq!(lone_track_edit(&retagged(other), false), None, "{other:?}");
    }
    let mut undone = retagged(edit.edit);
    if let Some((movement, _)) = undone.history.as_mut() {
        *movement = HistoryMove::Undo;
    }
    assert_eq!(lone_track_edit(&undone, false), None, "an undo");
}
