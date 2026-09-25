//! Folding a device to its header (MOO-219).
//!
//! Adam, 2026-09-24: *"let's turn that old `+` button into a `<` button that
//! collapses the device"*, a collapsed device being *"essentially its header,
//! rotated 90° clockwise"*; nested devices fold too, and a folded container
//! hides everything in it. The `<` and `>` are pressed in the real window.
//! The one rule the markup cannot hold is that a fold survives an undo, so
//! that one is held on the function the undo handler calls.

use super::*;
use crate::window_probe::{click, controls, install_backend, Control};
use i_slint_core::items::AccessibleRole;
use mooloop_engine::{ExportSpec, OfflineRenderer};
use slint::LogicalSize;

const WIDTH: f32 = 4000.0;
const HEIGHT: f32 = 1400.0;

/// A window showing the rack with `kinds` in it, containers holding the next
/// `children` rows, and the fold toggle wired as `AppUi::new` wires it.
fn rack_with(kinds: &[(EffectKind, u8)]) -> (MainWindow, Rc<RefCell<UiState>>) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    window.window().set_size(LogicalSize::new(WIDTH, HEIGHT));
    install_strip_spec(&window);
    install_eq_spec(&window);
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    window.invoke_move_view(view::DEVICES, 0);
    window.invoke_show_view(view::DEVICES);
    window.set_bottom_pane_visible(false);
    {
        let mut st = state.borrow_mut();
        let project = st.session.project_snapshot(window.get_bpm(), window.get_swing_percent());
        st.replace_project(&project, &[None], &window);
        for (at, (kind, _)) in kinds.iter().enumerate() {
            st.session.insert_effect_at(*kind, at).expect("the rack has room");
        }
        let chain = st.session.effect_chain_mut().expect("a channel's chain");
        for (at, (_, children)) in kinds.iter().enumerate() {
            if *children > 0 {
                chain[at].params.set_container_children(*children);
            }
        }
        st.sync_effects();
    }
    let st = state.clone();
    window.on_effect_collapse_toggled(move |slot| {
        let mut st = st.borrow_mut();
        if let Some(effect) = st
            .session
            .effect_chain_mut()
            .and_then(|chain| chain.get_mut(slot as usize))
        {
            effect.collapsed = !effect.collapsed;
        }
        st.sync_effects();
    });
    (window, state)
}

fn buttons(window: &MainWindow, label: &str) -> Vec<Control> {
    let mut found: Vec<Control> = controls(window, AccessibleRole::Button)
        .into_iter()
        .filter(|button| button.label.ends_with(label))
        .collect();
    found.sort_by(|a, b| a.centre.0.total_cmp(&b.centre.0));
    found
}

fn strips(window: &MainWindow) -> Vec<String> {
    controls(window, AccessibleRole::Groupbox)
        .into_iter()
        .filter(|group| group.label.ends_with(", collapsed"))
        .map(|group| group.label)
        .collect()
}

/// `<` folds a device to a strip, the strip's `>` opens it again, and while
/// it is folded its face's controls are gone.
#[test]
fn a_device_folds_from_its_rail_and_opens_from_its_strip() {
    let (window, state) = rack_with(&[(EffectKind::Drive, 0), (EffectKind::Filter, 0)]);
    let knobs = |window: &MainWindow| controls(window, AccessibleRole::Slider).len();
    let open = knobs(&window);

    // The effects' `<`, left to right: Drive's first.
    let folds = buttons(&window, "Collapse");
    assert!(folds.len() >= 2, "every device's rail carries a `<`: {folds:?}");
    let drive_fold = folds
        .iter()
        .find(|fold| fold.label.starts_with("Fold"))
        .expect("an enabled `<`")
        .clone();
    click(&window, drive_fold.centre);
    assert!(state.borrow().session.effect_chain().unwrap()[0].collapsed);
    assert_eq!(strips(&window), ["Drive, collapsed"]);
    assert!(knobs(&window) < open, "the folded face's knobs are still drawn");

    click(&window, buttons(&window, "Expand")[0].centre);
    assert!(!state.borrow().session.effect_chain().unwrap()[0].collapsed);
    assert!(strips(&window).is_empty());
    assert_eq!(knobs(&window), open);
}

/// A folded container shows its strip alone: everything inside it goes, and
/// comes back as it was -- a device folded inside it stays folded.
#[test]
fn a_folded_container_hides_what_it_holds_and_restores_it() {
    let (window, state) = rack_with(&[
        (EffectKind::Chain, 2),
        (EffectKind::Drive, 0),
        (EffectKind::Filter, 0),
        (EffectKind::Delay, 0),
    ]);
    {
        let mut st = state.borrow_mut();
        st.session.effect_chain_mut().unwrap()[1].collapsed = true;
        st.session.effect_chain_mut().unwrap()[0].collapsed = true;
        st.sync_effects();
    }
    assert_eq!(strips(&window), ["Chain, collapsed"], "the Drive inside is hidden");
    {
        let mut st = state.borrow_mut();
        st.session.effect_chain_mut().unwrap()[0].collapsed = false;
        st.sync_effects();
    }
    let mut shown = strips(&window);
    shown.sort();
    assert_eq!(shown, ["Drive, collapsed"], "the Drive comes back folded");
}

/// **A fold survives an undo.** Folding is not an undo step, and an undo
/// installs a whole-project snapshot, so the snapshot has to be given the
/// live folds first -- or undoing an unrelated edit would unfold the rack.
#[test]
fn a_fold_survives_undoing_an_unrelated_edit() {
    let (window, state) = rack_with(&[(EffectKind::Drive, 0)]);
    let commands = Rc::new(RefCell::new(CommandState::default()));

    // An unrelated edit, recorded.
    let original = state.borrow().session.channels[0].volume;
    let before = project_snapshot(&state.borrow(), &window);
    state.borrow_mut().session.channels[0].volume = 0.5;
    record_project_history(&commands, before, &state, &window, "Channel Volume");

    // Fold after it.
    state.borrow_mut().session.effect_chain_mut().unwrap()[0].collapsed = true;

    // Undo it, the way the undo handler does.
    let entry = commands.borrow().history.undo_target().cloned().expect("an undo step");
    let entry = with_live_view_state(entry, &state.borrow().session);
    {
        let mut st = state.borrow_mut();
        st.replace_project(&entry.before.project, &[None], &window);
    }
    let st = state.borrow();
    assert_eq!(st.session.channels[0].volume, original, "the undo undid the volume");
    assert!(
        st.session.effect_chain().unwrap()[0].collapsed,
        "undoing the volume change unfolded the device"
    );
}

/// **A song renders identically whether its devices are folded or not.**
/// Folding is view state; nothing that makes sound may read it.
#[test]
fn a_fold_changes_nothing_that_is_heard() {
    let render = |collapsed: bool| {
        let mut project = Project::starter_kit(7);
        let mut drive = EffectSlotState::drive(mooloop_core::DriveParams::default());
        drive.collapsed = collapsed;
        project.channels[0].setup.effects.push(drive);
        // A hand-built chain takes its identities the way a loaded one does.
        project.assign_device_ids();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("render.wav");
        OfflineRenderer::render(
            &project,
            &[],
            48_000,
            &ExportSpec {
                path: path.clone(),
                scope: mooloop_engine::RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format: mooloop_engine::ExportFormat::Wav(mooloop_engine::WavEncoding::Float32),
            },
        )
        .expect("it renders offline");
        std::fs::read(path).unwrap()
    };
    let open = render(false);
    assert!(open.len() > 1024, "the render has audio in it");
    assert_eq!(open, render(true), "a folded device changed the render");
}
