//! Adding a device from the arrow between two devices (MOO-218).
//!
//! Adam, 2026-09-24: *"the add device buttons...let's just put them between
//! devices where those little white arrows are."* Each join is pressed in the
//! real window, a kind is picked from the menu it opens, and what reaches
//! Rust is held to where the join is drawn: the row it inserts before, or
//! the container it inserts into.

use super::*;
use crate::window_probe::{click, controls, install_backend, Control};
use i_slint_core::items::AccessibleRole;
use slint::platform::{PointerEventButton, WindowEvent};
use slint::LogicalSize;

// Wide enough for a four-unit source and five devices side by side: a join
// past the window edge is one the probe cannot see.
const WIDTH: f32 = 4000.0;
const HEIGHT: f32 = 1400.0;

/// A window showing the rack, holding `kinds` in order, with every
/// container given the next `children` rows.
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
    (window, state)
}

/// The joins in the rack, left to right.
fn joins(window: &MainWindow) -> Vec<Control> {
    let mut joins: Vec<Control> = controls(window, AccessibleRole::Button)
        .into_iter()
        .filter(|button| button.label == "Add device")
        .collect();
    joins.sort_by(|a, b| a.centre.0.total_cmp(&b.centre.0));
    joins
}

/// Press `join`, then the menu's `kind` row.
fn add_from(window: &MainWindow, join: &Control, kind: &str) {
    click(window, join.centre);
    let row = controls(window, AccessibleRole::Button)
        .into_iter()
        .find(|row| row.label == kind)
        .unwrap_or_else(|| panic!("the join's menu offers no {kind:?}"));
    click(window, row.centre);
}

/// What the rack asked Rust for, in order.
fn listen(window: &MainWindow) -> Rc<RefCell<Vec<String>>> {
    let heard = Rc::new(RefCell::new(Vec::new()));
    {
        let heard = heard.clone();
        window.on_add_effect_clicked(move |kind, before| {
            heard.borrow_mut().push(format!("insert {kind} before {before}"));
        });
    }
    {
        let heard = heard.clone();
        window.on_add_effect_into_container(move |kind, container| {
            heard.borrow_mut().push(format!("insert {kind} into {container}"));
        });
    }
    {
        let heard = heard.clone();
        window.on_append_effect_into_container(move |kind, container| {
            heard.borrow_mut().push(format!("append {kind} into {container}"));
        });
    }
    {
        let heard = heard.clone();
        window.on_add_plugin_requested(move |before| {
            heard.borrow_mut().push(format!("plugin before {before}"));
        });
    }
    {
        let heard = heard.clone();
        window.on_add_plugin_into_requested(move |container| {
            heard.borrow_mut().push(format!("plugin into {container}"));
        });
    }
    heard
}

fn delay() -> i32 {
    effect_kind_index(EffectKind::Delay)
}

/// Every join inserts at its own gap: after the head, between the two
/// devices, and after the last one.
#[test]
fn each_join_inserts_at_its_own_gap() {
    let (window, _state) = rack_with(&[(EffectKind::Drive, 0), (EffectKind::Filter, 0)]);
    let heard = listen(&window);
    let found = joins(&window);
    assert_eq!(found.len(), 3, "a head, two devices: three joins");
    for join in &found {
        add_from(&window, join, "Delay");
    }
    let d = delay();
    assert_eq!(
        *heard.borrow(),
        [format!("insert {d} before 0"), format!("insert {d} before 1"), format!("insert {d} before 2")]
    );
}

/// No device carries a `+` of its own any more, and nothing sits after the
/// last join.
#[test]
fn the_rail_has_no_insert_button() {
    let (window, _state) = rack_with(&[(EffectKind::Drive, 0)]);
    let inserts: Vec<Control> = controls(&window, AccessibleRole::Button)
        .into_iter()
        .filter(|button| button.label.contains("Insert effect") || button.label == "+")
        .collect();
    assert!(inserts.is_empty(), "a rail still offers insert: {inserts:?}");
}

/// Inside a Chain, a join between two of its devices inserts between them,
/// the join after its head inserts first inside it, the join after its last
/// device -- inside the box, before its rail -- adds at the end of it
/// (MOO-340), and the join past its box inserts after it.
#[test]
fn joins_inside_a_chain_land_inside_it() {
    // Drive, then a Chain holding Filter and Bitcrush, then Reverb.
    let (window, _state) = rack_with(&[
        (EffectKind::Drive, 0),
        (EffectKind::Chain, 2),
        (EffectKind::Filter, 0),
        (EffectKind::Bitcrush, 0),
        (EffectKind::Reverb, 0),
    ]);
    let heard = listen(&window);
    let found = joins(&window);
    assert_eq!(found.len(), 7, "a head, five rows, and the Chain's own end: seven joins");
    for join in &found {
        add_from(&window, join, "Delay");
    }
    let d = delay();
    assert_eq!(
        *heard.borrow(),
        [
            format!("insert {d} before 0"),
            format!("insert {d} before 1"),
            // After the chain's head: before Filter, which is inside.
            format!("insert {d} before 2"),
            format!("insert {d} before 3"),
            // After Bitcrush, inside the box: the end of the Chain.
            format!("append {d} into 1"),
            // Past the box: before Reverb, which `insert_effect` puts outside.
            format!("insert {d} before 4"),
            format!("insert {d} before 5"),
        ]
    );
}

/// **MOO-299's case, pressed.** A Chain holding a Drive and a Filter: the
/// join after the Filter, inside the box, adds a Delay after the Filter and
/// still inside the Chain, as one undo step; undoing it takes the Delay
/// back out and nothing else. Wired as `AppUi::new` wires it, through
/// `append_effect_into_container`.
#[test]
fn the_join_at_a_chains_end_adds_inside_it_as_one_undo_step() {
    let (window, state) = rack_with(&[
        (EffectKind::Chain, 2),
        (EffectKind::Drive, 0),
        (EffectKind::Filter, 0),
        (EffectKind::Reverb, 0),
    ]);
    {
        let mut st = state.borrow_mut();
        let project = st.session.project_snapshot(window.get_bpm(), window.get_swing_percent());
        st.replace_project(&project, &[None], &window);
        st.sync_effects();
    }
    let commands = Rc::new(RefCell::new(CommandState::default()));
    let (sender, _engine) = std::sync::mpsc::channel();
    let tx = EngineCommandSender(sender.clone());
    let stx = StructuralCommandSender(sender);
    {
        let (st, commands, weak) = (state.clone(), commands.clone(), window.as_weak());
        window.on_append_effect_into_container(move |kind, container| {
            let Some(window) = weak.upgrade() else { return };
            let kind = effect_kind_from_index(kind).expect("a kind the menu offers");
            let container = usize::try_from(container).expect("a row");
            append_effect_into_container(&st, &window, &commands, (&tx, &stx), kind, container);
        });
    }
    let kinds = |state: &Rc<RefCell<UiState>>| -> Vec<EffectKind> {
        state.borrow().session.effect_chain().expect("a chain").iter().map(|effect| effect.kind()).collect()
    };
    let children = |state: &Rc<RefCell<UiState>>| {
        state.borrow().session.effect_chain().expect("a chain")[0].params.container_children()
    };
    let before = kinds(&state);

    // Head, Chain's head, Drive, Filter's append join, past the box, Reverb.
    let found = joins(&window);
    assert_eq!(found.len(), 6, "{found:?}");
    add_from(&window, &found[3], "Delay");
    assert_eq!(
        kinds(&state),
        [EffectKind::Chain, EffectKind::Drive, EffectKind::Filter, EffectKind::Delay, EffectKind::Reverb],
        "the Delay lands after the Filter"
    );
    assert_eq!(children(&state), Some(3), "and inside the Chain");
    assert_eq!(
        commands.borrow().history.undo_target().map(|entry| entry.label),
        Some("Effect added"),
        "one undo step"
    );

    // Undo it, the way the undo handler does.
    let entry = commands.borrow().history.undo_target().cloned().expect("an undo step");
    let entry = with_live_view_state(entry, &state.borrow().session);
    state.borrow_mut().replace_project(&entry.before.project, &[None], &window);
    assert_eq!(kinds(&state), before, "the undo takes the Delay back out");
    assert_eq!(children(&state), Some(2));
}

/// The Chain's end join offers **Plugin…** aimed at the end of the box, and
/// so does an empty Chain's own join, whose first and last are one place
/// (MOO-340). The join past a box still aims before the next row.
#[test]
fn plugin_from_a_chains_end_is_aimed_into_the_box() {
    let (window, _state) = rack_with(&[
        (EffectKind::Chain, 1),
        (EffectKind::Drive, 0),
        (EffectKind::Chain, 0),
    ]);
    let heard = listen(&window);
    // Head, Chain's head, Drive's append join, past the box, the empty
    // Chain's own join, past it.
    let found = joins(&window);
    assert_eq!(found.len(), 6, "{found:?}");
    add_from(&window, &found[2], "Plugin…");
    add_from(&window, &found[3], "Plugin…");
    add_from(&window, &found[4], "Plugin…");
    assert_eq!(*heard.borrow(), ["plugin into 0", "plugin before 2", "plugin into 2"]);
}

/// An empty container draws a join inside itself, and that one adds into
/// the box -- the one position no index can name.
#[test]
fn an_empty_container_takes_a_device_from_its_own_join() {
    let (window, _state) = rack_with(&[(EffectKind::Chain, 0)]);
    let heard = listen(&window);
    let found = joins(&window);
    assert_eq!(found.len(), 3, "head, the box's inner join, and the one past it");
    add_from(&window, &found[1], "Delay");
    add_from(&window, &found[2], "Delay");
    let d = delay();
    assert_eq!(
        *heard.borrow(),
        [format!("insert {d} into 0"), format!("insert {d} before 1")]
    );
}

/// The browser open on its presets tab, showing one effect preset,
/// "Slapback", whose drops are heard: a drop before a row as
/// "`path` before `row`", a drop at the end of a box as
/// "`path` into `container`".
fn browser_with_a_preset(window: &MainWindow) -> Rc<RefCell<Vec<String>>> {
    window.set_sidebar_visible(true);
    window.set_browser_tab(1);
    window.set_browser_rows(ModelRc::from(Rc::new(VecModel::from(vec![BrowserRow {
        depth: 1,
        kind: 3,
        name: "Slapback".into(),
        path: "/presets/Slapback".into(),
        expanded: false,
        detail: Default::default(),
        loadable: true,
        effect: true,
        favourite: false,
    }]))));
    let dropped = Rc::new(RefCell::new(Vec::new()));
    {
        let dropped = dropped.clone();
        window.on_browser_preset_dropped(move |path, before| {
            dropped.borrow_mut().push(format!("{path} before {before}"));
        });
    }
    {
        let dropped = dropped.clone();
        window.on_browser_preset_dropped_into(move |path, container| {
            dropped.borrow_mut().push(format!("{path} into {container}"));
        });
    }
    dropped
}

/// Drag the browser's "Slapback" row onto `target` and let go, in steps, as
/// a pointer does.
fn drag_preset_to(window: &MainWindow, target: (f32, f32)) {
    let row = controls(window, AccessibleRole::ListItem)
        .into_iter()
        .find(|row| row.label == "Slapback")
        .expect("the browser draws the preset");
    let w = window.window();
    let at = |p: (f32, f32)| slint::LogicalPosition::new(p.0, p.1);
    w.dispatch_event(WindowEvent::PointerMoved { position: at(row.centre) });
    w.dispatch_event(WindowEvent::PointerPressed {
        position: at(row.centre),
        button: PointerEventButton::Left,
    });
    for step in 1..=12 {
        let t = step as f32 / 12.0;
        w.dispatch_event(WindowEvent::PointerMoved {
            position: at((
                row.centre.0 + (target.0 - row.centre.0) * t,
                row.centre.1 + (target.1 - row.centre.1) * t,
            )),
        });
    }
    w.dispatch_event(WindowEvent::PointerReleased {
        position: at(target),
        button: PointerEventButton::Left,
    });
}

/// A preset dragged out of the browser and dropped on a join lands there:
/// before the row that join leads into (MOO-218, with MOO-9's drag).
#[test]
fn a_preset_dropped_on_a_join_lands_at_that_gap() {
    let (window, _state) = rack_with(&[(EffectKind::Drive, 0), (EffectKind::Filter, 0)]);
    let dropped = browser_with_a_preset(&window);
    // The join between Drive and Filter.
    drag_preset_to(&window, joins(&window)[1].centre);
    assert_eq!(*dropped.borrow(), ["/presets/Slapback before 1"]);
}

/// A preset dropped on the join at a Chain's end lands at the end of the
/// box, not before the row after it (MOO-340); the join past the box still
/// takes it before that row.
#[test]
fn a_preset_dropped_at_a_chains_end_lands_inside_it() {
    let (window, _state) = rack_with(&[
        (EffectKind::Chain, 1),
        (EffectKind::Drive, 0),
        (EffectKind::Filter, 0),
    ]);
    let dropped = browser_with_a_preset(&window);
    // Head, Chain's head, the Drive's append join, past the box, Filter.
    let found = joins(&window);
    assert_eq!(found.len(), 5, "{found:?}");
    drag_preset_to(&window, found[2].centre);
    drag_preset_to(&window, found[3].centre);
    assert_eq!(
        *dropped.borrow(),
        ["/presets/Slapback into 0", "/presets/Slapback before 2"]
    );
}
