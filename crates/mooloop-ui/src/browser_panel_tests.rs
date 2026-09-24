//! The browser sidebar's filter, and what a click, a double-click and a drag
//! do to a preset (MOO-9).
//!
//! Adam, 2026-09-23: *"click 1 time is just going to select it. double click
//! will actually load it."* and *"drag to rack should work"*. The row's
//! gestures are pressed in the real window and held to which callback they
//! reach; the filter and the placement rule are held as the functions they
//! are.

use super::*;
use crate::window_probe::{click, install_backend};
use mooloop_project::PresetKind;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{LogicalPosition, LogicalSize};

fn summary(name: &str, category: &str, tags: &[&str], kind: EffectKind) -> PresetSummary {
    PresetSummary {
        path: PathBuf::from(format!("/presets/{name}.mooloop-effect")),
        name: name.to_string(),
        category: category.to_string(),
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        kind: PresetKind::Effect(kind),
    }
}

fn group(label: &str, kind: EffectKind, presets: Vec<PresetSummary>) -> PresetGroup {
    PresetGroup {
        dir: PathBuf::from(format!("/presets/{label}")),
        label: label.to_string(),
        slot: PresetSlot::Effect(kind),
        presets,
    }
}

fn names(rows: &[BrowserRow]) -> Vec<String> {
    rows.iter().map(|row| row.name.to_string()).collect()
}

/// A filter opens every group that has a match, shows only the matches, and
/// leaves out a group with none -- whether or not it was expanded.
#[test]
fn a_filter_shows_matching_presets_in_open_groups() {
    let groups = vec![
        group(
            "Delay",
            EffectKind::Delay,
            vec![
                summary("Slapback", "Factory", &[], EffectKind::Delay),
                summary("Warm Tape", "Tape", &["warm"], EffectKind::Delay),
            ],
        ),
        group(
            "Reverb",
            EffectKind::Reverb,
            vec![summary("Hall", "Factory", &[], EffectKind::Reverb)],
        ),
    ];
    let rows = build_preset_rows(&groups, &HashSet::new(), None, "warm");
    assert_eq!(names(&rows), ["Delay", "Warm Tape"]);
    assert!(rows[0].expanded, "a group with a match is shown open");
    assert!(rows[1].effect, "an effect preset says so, for its menu");

    // By the group's own label, and by several words at once.
    assert_eq!(
        names(&build_preset_rows(&groups, &HashSet::new(), None, "reverb hall")),
        ["Reverb", "Hall"]
    );
    assert!(build_preset_rows(&groups, &HashSet::new(), None, "nothing like it").is_empty());
}

/// Under a filter the sample tab searches every folder, not only the open
/// ones, and says where each match sits.
#[test]
fn a_filter_finds_samples_in_closed_folders() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("Drums/Snares")).unwrap();
    std::fs::write(root.join("Drums/kick.wav"), b"x").unwrap();
    std::fs::write(root.join("Drums/Snares/snare tight.wav"), b"x").unwrap();
    std::fs::write(root.join("Drums/Snares/snare notes.txt"), b"x").unwrap();

    // Nothing is expanded, which is the point.
    let rows = build_browser_rows(&[root.to_path_buf()], &HashSet::new(), "snare");
    assert_eq!(rows.len(), 2, "{:?}", names(&rows));
    assert_eq!(rows[1].name.as_str(), "snare tight.wav");
    assert!(rows[1].detail.as_str().ends_with("Snares"), "{}", rows[1].detail);
    assert_eq!(rows[1].kind, 1);
    assert!(build_browser_rows(&[root.to_path_buf()], &HashSet::new(), "hat").is_empty());
}

/// Double-clicking an effect preset loads it into the selected device when
/// that device is its kind, and otherwise adds a new one.
#[test]
fn a_preset_loads_into_the_selected_device_of_its_kind() {
    // Through a project install, which is what gives a channel the identity
    // a device selection is keyed by.
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    let mut st = UiState::new(None, 48_000, &window);
    let project = st.session.project_snapshot(window.get_bpm(), window.get_swing_percent());
    st.replace_project(&project, &[None], &window);
    let session = &mut st.session;
    session.insert_effect_at(EffectKind::Drive, 0).unwrap();
    session.insert_effect_at(EffectKind::Delay, 1).unwrap();
    let slapback = summary("Slapback", "Factory", &[], EffectKind::Delay);
    session.effect_presets = vec![
        summary("Other", "Factory", &[], EffectKind::Drive),
        summary("Echo", "Factory", &[], EffectKind::Delay),
        slapback.clone(),
    ];

    assert_eq!(
        preset_load_target(session, EffectKind::Delay, &slapback.path),
        None,
        "nothing selected: a new device"
    );
    session.select_device(Some(0));
    assert_eq!(
        preset_load_target(session, EffectKind::Delay, &slapback.path),
        None,
        "a Drive selected: a new Delay"
    );
    session.select_device(Some(1));
    assert_eq!(
        preset_load_target(session, EffectKind::Delay, &slapback.path),
        Some((1, 1)),
        "the selected Delay, at the preset's place in the Delay menu"
    );
}

/// The real window with the browser open on one preset row, and the rack in
/// the main pane.
fn browser_window() -> (MainWindow, Rc<RefCell<Vec<String>>>) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    window.window().set_size(LogicalSize::new(1800.0, 1000.0));
    let _state = UiState::new(None, 48_000, &window);
    window.invoke_move_view(view::DEVICES, 0);
    window.invoke_show_view(view::DEVICES);
    window.set_bottom_pane_visible(false);
    window.set_sidebar_visible(true);
    window.set_browser_tab(1);
    let row = |depth, kind, name: &str, effect| BrowserRow {
        depth,
        kind,
        name: name.into(),
        path: format!("/presets/{name}").into(),
        expanded: kind == 2,
        detail: Default::default(),
        loadable: true,
        effect,
    };
    window.set_browser_rows(ModelRc::from(Rc::new(VecModel::from(vec![
        row(0, 2, "Delay", false),
        row(1, 3, "Slapback", true),
    ]))));
    let heard = Rc::new(RefCell::new(Vec::new()));
    {
        let heard = heard.clone();
        window.on_browser_preset_loaded(move |path| heard.borrow_mut().push(format!("load {path}")));
    }
    {
        let heard = heard.clone();
        window.on_browser_preset_appended(move |path| heard.borrow_mut().push(format!("append {path}")));
    }
    (window, heard)
}

/// The preset row's centre, found by its name among the browser's rows.
fn preset_row(window: &MainWindow) -> (f32, f32) {
    crate::window_probe::controls(window, i_slint_core::items::AccessibleRole::ListItem)
        .into_iter()
        .find(|row| row.label == "Slapback")
        .expect("the browser draws the preset row")
        .centre
}

#[test]
fn a_click_selects_a_preset_and_a_double_click_loads_it() {
    let (window, heard) = browser_window();
    let at = preset_row(&window);
    click(&window, at);
    assert!(heard.borrow().is_empty(), "a single click loaded: {:?}", heard.borrow());
    assert_eq!(window.get_browser_focus_index(), 1, "and it selected the row");
    click(&window, at);
    assert_eq!(*heard.borrow(), ["load /presets/Slapback"], "the double-click loads it");
}

#[test]
fn dragging_a_preset_onto_the_rack_loads_it_and_elsewhere_does_nothing() {
    let (window, heard) = browser_window();
    let from = preset_row(&window);
    let drag_to = |to: (f32, f32)| {
        let w = window.window();
        let at = |p: (f32, f32)| LogicalPosition::new(p.0, p.1);
        w.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
        w.dispatch_event(WindowEvent::PointerPressed { position: at(from), button: PointerEventButton::Left });
        for step in 1..=10 {
            let t = step as f32 / 10.0;
            w.dispatch_event(WindowEvent::PointerMoved {
                position: at((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)),
            });
        }
        w.dispatch_event(WindowEvent::PointerReleased { position: at(to), button: PointerEventButton::Left });
    };
    // Back into the browser itself: not a drop.
    drag_to((from.0, from.1 + 200.0));
    assert!(heard.borrow().is_empty(), "a drop in the browser loaded: {:?}", heard.borrow());
    // Onto the rack, which is the main pane on the left of the window.
    drag_to((300.0, 400.0));
    assert_eq!(*heard.borrow(), ["load /presets/Slapback"]);
}
