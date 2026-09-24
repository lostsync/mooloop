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
/// the join after its head inserts first inside it, and the join past its
/// box inserts after it.
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
    assert_eq!(found.len(), 6, "a head and five rows: six joins");
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
            // Past the box: before Reverb, which `insert_effect` puts outside.
            format!("insert {d} before 4"),
            format!("insert {d} before 5"),
        ]
    );
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

/// A preset dragged out of the browser and dropped on a join lands there:
/// before the row that join leads into (MOO-218, with MOO-9's drag).
#[test]
fn a_preset_dropped_on_a_join_lands_at_that_gap() {
    let (window, _state) = rack_with(&[(EffectKind::Drive, 0), (EffectKind::Filter, 0)]);
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
    }]))));
    let dropped = Rc::new(RefCell::new(Vec::new()));
    {
        let dropped = dropped.clone();
        window.on_browser_preset_dropped(move |path, before| {
            dropped.borrow_mut().push(format!("{path} before {before}"));
        });
    }
    let row = controls(&window, AccessibleRole::ListItem)
        .into_iter()
        .find(|row| row.label == "Slapback")
        .expect("the browser draws the preset");
    // The join between Drive and Filter.
    let between = joins(&window)[1].centre;
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
                row.centre.0 + (between.0 - row.centre.0) * t,
                row.centre.1 + (between.1 - row.centre.1) * t,
            )),
        });
    }
    w.dispatch_event(WindowEvent::PointerReleased {
        position: at(between),
        button: PointerEventButton::Left,
    });
    assert_eq!(*dropped.borrow(), ["/presets/Slapback before 1"]);
}
