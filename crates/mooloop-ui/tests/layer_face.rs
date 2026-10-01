//! The layer's face, driven by real pointer events
//! (`docs/plans/archive/containers/09`).
//!
//! What matters about the list is that a press on a row reaches the callback
//! carrying the **branch head's rack index** -- `branch.slot` -- and not the
//! row's place in the list, which is the `lvl`/`level` trap the plan warns
//! about. A face test that invoked the callbacks directly would pass while
//! every press named the wrong row, so these click, and drag, and open the
//! menus they test.

mod common;

use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, LogicalSize};
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { LayerBranchListRow, LayerBranchRow, LayerDeviceFace, LayerEndPanel, LayerMetrics } from "../ui/layer-device.slint";

    export component LayerHarness inherits Window {
        width: 380px;
        height: 280px;
        background: #101010;
        // Branch heads at rack indices 3 and 7: nothing like 0 and 1, so a
        // callback that sent the list position would be seen to.
        in property <[LayerBranchRow]> branches: [
            { slot: 3, name: "Clean", controls: true },
            { slot: 7, name: "Crushed", controls: true },
        ];
        in property <bool> second-bypassed;
        out property <length> face-width: LayerMetrics.face-width;
        callback selected(int);
        callback muted(int);
        callback soloed(int);
        callback added();
        callback removed(int);
        callback moved(int, int);
        callback branch-leveled(float);
        callback branch-mixed(float);
        callback branch-bypassed();
        callback branch-input-trimmed(float);
        callback branch-output-trimmed(float);
        callback branch-preset(int);
        callback branch-saved();
        callback layer-gained(float);
        callback layer-mixed(float);

        LayerDeviceFace {
            x: 0px; y: 0px;
            width: LayerMetrics.face-width; height: 268px;
            name: "Layer";
            branch-count: root.branches.length;
            branch-available: true;
            branch-name: "Clean";
            branch-preset-options: ["Warm", "Cold"];
            add-branch-requested => { root.added(); }
            branch-level-changed(v) => { root.branch-leveled(v); }
            branch-mix-changed(v) => { root.branch-mixed(v); }
            branch-bypass-toggled => { root.branch-bypassed(); }
            branch-input-trim-changed(v) => { root.branch-input-trimmed(v); }
            branch-output-trim-changed(v) => { root.branch-output-trimmed(v); }
            branch-preset-selected(i) => { root.branch-preset(i); }
            branch-save-preset-requested => { root.branch-saved(); }
            for branch[i] in root.branches : LayerBranchListRow {
                name: branch.name;
                controls: branch.controls;
                bypassed: i == 1 && root.second-bypassed;
                layer: 0;
                index: i;
                count: root.branches.length;
                select-requested => { root.selected(branch.slot); }
                mute-toggled => { root.muted(branch.slot); }
                solo-toggled => { root.soloed(branch.slot); }
                remove-requested => { root.removed(branch.slot); }
                move-requested(place) => { root.moved(branch.slot, place); }
            }
        }
        LayerEndPanel {
            x: 300px; y: 0px;
            height: 268px;
            // Off the ends of their travel, so a wheel moves them either way.
            gain: 0.75;
            mix: 0.5;
            gain-changed(v) => { root.layer-gained(v); }
            mix-changed(v) => { root.layer-mixed(v); }
        }
    }
}

/// The face's list starts below the 28px header, inset 6px by the face and
/// 3px by the list; rows are 22px on a 23px pitch, 144px wide. These are the
/// middles of the two rows' names, and of the second row's S and M buttons,
/// which sit at the row's right end before the 5px meter.
const FIRST_ROW: (f32, f32) = (40.0, 48.0);
const SECOND_ROW: (f32, f32) = (40.0, 71.0);
const SECOND_SOLO: (f32, f32) = (115.0, 71.0);
const SECOND_MUTE: (f32, f32) = (135.0, 71.0);
/// The second row's menu opens under it: 4px inset, one 22px row.
const SECOND_ROW_REMOVE: (f32, f32) = (60.0, 97.0);
/// Under the two rows: 3px of padding above an 18px button.
const PLUS: (f32, f32) = (81.0, 95.0);
/// Where the second row's badge sits when its branch is bypassed, between
/// the name and S.
const SECOND_BADGE: (f32, f32) = (92.0, 71.0);
/// The selected branch's controls, beside the list (x 162 to 286).
const BRANCH_BYPASS: (f32, f32) = (269.0, 49.0);
const BRANCH_LEVEL: (f32, f32) = (196.0, 100.0);
const BRANCH_MIX: (f32, f32) = (252.0, 100.0);
const BRANCH_IN_TRIM: (f32, f32) = (210.0, 173.0);
const BRANCH_OUT_TRIM: (f32, f32) = (238.0, 173.0);
const BRANCH_SAVE: (f32, f32) = (196.0, 200.0);
const BRANCH_LOAD: (f32, f32) = (253.0, 200.0);
/// The preset list opens over the bottom of the controls, two rows of 22px
/// inside 4px of padding: the second, "Cold".
const BRANCH_PRESET_COLD: (f32, f32) = (200.0, 247.0);
/// The layer's own Gain and Mix, at the far end of its box.
const LAYER_GAIN: (f32, f32) = (330.0, 88.0);
const LAYER_MIX: (f32, f32) = (330.0, 176.0);

fn harness() -> LayerHarness {
    // The software renderer, so a test can look at what was drawn.
    common::install_testing_backend();
    let ui = LayerHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(380.0, 280.0));
    ui
}

fn at(at: (f32, f32)) -> LogicalPosition {
    LogicalPosition::new(at.0, at.1)
}

fn click(window: &slint::Window, position: (f32, f32)) {
    let position = at(position);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

/// Press at `from`, move to `to` in steps, release there.
fn drag(window: &slint::Window, from: (f32, f32), to: (f32, f32)) {
    window.dispatch_event(WindowEvent::PointerMoved { position: at(from) });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: at(from),
        button: PointerEventButton::Left,
    });
    for step in 1..=6 {
        let t = step as f32 / 6.0;
        let position = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        window.dispatch_event(WindowEvent::PointerMoved { position: at(position) });
    }
    window.dispatch_event(WindowEvent::PointerReleased {
        position: at(to),
        button: PointerEventButton::Left,
    });
}

fn scroll(window: &slint::Window, position: (f32, f32)) {
    window.dispatch_event(WindowEvent::PointerMoved { position: at(position) });
    window.dispatch_event(WindowEvent::PointerScrolled {
        position: at(position),
        delta_x: 0.0,
        delta_y: 60.0,
    });
}

fn log<T: 'static>() -> Rc<RefCell<Vec<T>>> {
    Rc::default()
}

#[test]
fn the_harness_draws_the_face_at_its_own_width() {
    // Every coordinate above is measured on a face this wide; a change to
    // `LayerMetrics` moves them all.
    assert_eq!(harness().get_face_width(), 292.0);
}

#[test]
fn a_press_on_a_row_names_the_branch_head() {
    let ui = harness();
    let seen = log::<i32>();
    let s = seen.clone();
    ui.on_selected(move |slot| s.borrow_mut().push(slot));
    click(ui.window(), SECOND_ROW);
    click(ui.window(), FIRST_ROW);
    assert_eq!(*seen.borrow(), [7, 3], "a row reported its list position, not its branch");
}

#[test]
fn s_and_m_name_the_branch_head_and_do_not_select_it() {
    let ui = harness();
    let (selected, muted, soloed) = (log::<i32>(), log::<i32>(), log::<i32>());
    let (s, m, o) = (selected.clone(), muted.clone(), soloed.clone());
    ui.on_selected(move |slot| s.borrow_mut().push(slot));
    ui.on_muted(move |slot| m.borrow_mut().push(slot));
    ui.on_soloed(move |slot| o.borrow_mut().push(slot));
    click(ui.window(), SECOND_SOLO);
    click(ui.window(), SECOND_MUTE);
    assert_eq!(*soloed.borrow(), [7]);
    assert_eq!(*muted.borrow(), [7]);
    assert!(
        selected.borrow().is_empty(),
        "pressing S or M also selected the row: {:?}",
        selected.borrow()
    );
}

/// The badge sits between the name and S, and does not move them: S and M
/// still answer where they did.
#[test]
fn a_bypassed_branch_wears_a_badge_on_its_row() {
    let ui = harness();
    let plain = ui.window().take_snapshot().unwrap();
    ui.set_second_bypassed(true);
    let badged = ui.window().take_snapshot().unwrap();
    // The pixels around the badge's place on a row: a 30 by 14 box centred
    // where it sits.
    let around = |shot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>, (x, y): (f32, f32)| {
        let width = shot.width() as usize;
        let (x, y) = (x as usize, y as usize);
        (y - 7..y + 7)
            .flat_map(|row| (x - 15..x + 15).map(move |column| (row, column)))
            .map(|(row, column)| shot.as_slice()[row * width + column])
            .collect::<Vec<_>>()
    };
    assert_ne!(
        around(&plain, SECOND_BADGE),
        around(&badged, SECOND_BADGE),
        "nothing was drawn on the bypassed branch's row"
    );
    // The first row is unchanged: only the bypassed branch wears one.
    let first = (SECOND_BADGE.0, FIRST_ROW.1);
    assert_eq!(around(&plain, first), around(&badged, first));

    let soloed = log::<i32>();
    let o = soloed.clone();
    ui.on_soloed(move |slot| o.borrow_mut().push(slot));
    click(ui.window(), SECOND_SOLO);
    assert_eq!(*soloed.borrow(), [7], "the badge pushed S out from under the pointer");
}

#[test]
fn the_plus_under_the_list_asks_for_a_branch() {
    let ui = harness();
    let asked = Rc::new(RefCell::new(0));
    let count = asked.clone();
    ui.on_added(move || *count.borrow_mut() += 1);
    click(ui.window(), PLUS);
    assert_eq!(*asked.borrow(), 1);
}

/// A right press on a row opens its menu, and *Remove branch* names the
/// branch head -- so the removal takes the head and its whole run, which is
/// what `remove_effect` does with a container (`containers/10`).
#[test]
fn a_rows_menu_removes_that_branch() {
    let ui = harness();
    let (removed, selected) = (log::<i32>(), log::<i32>());
    let (r, s) = (removed.clone(), selected.clone());
    ui.on_removed(move |slot| r.borrow_mut().push(slot));
    ui.on_selected(move |slot| s.borrow_mut().push(slot));
    let position = at(SECOND_ROW);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Right,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Right,
    });
    click(ui.window(), SECOND_ROW_REMOVE);
    assert_eq!(*removed.borrow(), [7], "the menu removed the wrong branch, or none");
    assert!(selected.borrow().is_empty(), "a right press selected the row");
}

/// Dragging a row along the list moves its branch: the drop names the
/// branch head and the place it lands, and is not also a click.
#[test]
fn dragging_a_row_moves_its_branch() {
    let ui = harness();
    let (moved, selected) = (log::<(i32, i32)>(), log::<i32>());
    let (m, s) = (moved.clone(), selected.clone());
    ui.on_moved(move |slot, place| m.borrow_mut().push((slot, place)));
    ui.on_selected(move |slot| s.borrow_mut().push(slot));

    // The second row up one pitch, to the top.
    drag(ui.window(), SECOND_ROW, (SECOND_ROW.0, SECOND_ROW.1 - 23.0));
    assert_eq!(*moved.borrow(), [(7, 0)]);
    // The first down past the end: it lands last, not past it.
    drag(ui.window(), FIRST_ROW, (FIRST_ROW.0, FIRST_ROW.1 + 80.0));
    assert_eq!(*moved.borrow(), [(7, 0), (3, 1)]);
    assert!(selected.borrow().is_empty(), "a drag also selected: {:?}", selected.borrow());

    // A drag that ends where it started moves nothing, and a press that
    // barely moves is a click.
    drag(ui.window(), FIRST_ROW, (FIRST_ROW.0, FIRST_ROW.1 + 8.0));
    assert_eq!(moved.borrow().len(), 2, "a drag back to its own place moved: {:?}", moved.borrow());
    drag(ui.window(), FIRST_ROW, (FIRST_ROW.0 + 2.0, FIRST_ROW.1 + 2.0));
    assert_eq!(*selected.borrow(), [3]);
}

/// The selected branch's controls, which its Chain's face and rails held
/// before a branch stopped being drawn as a device. Each is pressed.
#[test]
fn the_selected_branchs_controls_answer_a_press() {
    let ui = harness();
    let (level, mix, input, output) = (log::<f32>(), log::<f32>(), log::<f32>(), log::<f32>());
    let bypass = Rc::new(RefCell::new(0));
    let saved = Rc::new(RefCell::new(0));
    let (l, x, i, o, b, v) = (
        level.clone(),
        mix.clone(),
        input.clone(),
        output.clone(),
        bypass.clone(),
        saved.clone(),
    );
    ui.on_branch_leveled(move |value| l.borrow_mut().push(value));
    ui.on_branch_mixed(move |value| x.borrow_mut().push(value));
    ui.on_branch_input_trimmed(move |value| i.borrow_mut().push(value));
    ui.on_branch_output_trimmed(move |value| o.borrow_mut().push(value));
    ui.on_branch_bypassed(move || *b.borrow_mut() += 1);
    ui.on_branch_saved(move || *v.borrow_mut() += 1);

    scroll(ui.window(), BRANCH_LEVEL);
    scroll(ui.window(), BRANCH_MIX);
    scroll(ui.window(), BRANCH_IN_TRIM);
    scroll(ui.window(), BRANCH_OUT_TRIM);
    click(ui.window(), BRANCH_BYPASS);
    click(ui.window(), BRANCH_SAVE);
    assert_eq!(level.borrow().len(), 1, "Level: {:?}", level.borrow());
    assert_eq!(mix.borrow().len(), 1, "Mix: {:?}", mix.borrow());
    assert_eq!(input.borrow().len(), 1, "input trim: {:?}", input.borrow());
    assert_eq!(output.borrow().len(), 1, "output trim: {:?}", output.borrow());
    assert_eq!(*bypass.borrow(), 1);
    assert_eq!(*saved.borrow(), 1);
}

/// *Load* opens the branch's preset list and a press on a row reaches the
/// callback with that row's index. The list is a popup closed by the press
/// on its own row, the shape `dupe-audit popup-close-order` exists for, so
/// this presses it rather than invoking the callback.
#[test]
fn a_branch_preset_is_loaded_from_its_list() {
    let ui = harness();
    let picked = log::<i32>();
    let p = picked.clone();
    ui.on_branch_preset(move |index| p.borrow_mut().push(index));
    click(ui.window(), BRANCH_LOAD);
    click(ui.window(), BRANCH_PRESET_COLD);
    assert_eq!(*picked.borrow(), [1], "the preset list did not load the row pressed");
}

/// The layer's own Gain and Mix are at the far end of its box now, and are
/// still its own.
#[test]
fn the_layers_end_holds_its_gain_and_mix() {
    let ui = harness();
    let (gain, mix) = (log::<f32>(), log::<f32>());
    let (g, m) = (gain.clone(), mix.clone());
    ui.on_layer_gained(move |value| g.borrow_mut().push(value));
    ui.on_layer_mixed(move |value| m.borrow_mut().push(value));
    scroll(ui.window(), LAYER_GAIN);
    scroll(ui.window(), LAYER_MIX);
    assert_eq!(gain.borrow().len(), 1, "Gain: {:?}", gain.borrow());
    assert_eq!(mix.borrow().len(), 1, "Mix: {:?}", mix.borrow());
}

/// The face's callbacks carry no slot: `main.slint` sends each to the branch
/// head the layer is showing, and each row's to its own branch head. Read
/// from the production markup, because a harness that wired them itself
/// would pass whatever `main.slint` did (`AGENTS.md`, *Parameter identity
/// across the session boundary*).
#[test]
fn main_slint_sends_the_branch_controls_to_the_shown_branch_head() {
    let main = include_str!("../ui/main.slint");
    let start = main
        .find("if slot.is-layer : LayerDeviceFace {")
        .expect("main.slint draws the layer's face");
    let arm = &main[start..];
    let arm = &arm[..arm.find("if slot.is-container && !slot.is-layer").unwrap_or(arm.len())];
    assert!(arm.contains("property <int> shown: slot.selected-branch;"));
    for wiring in [
        "branch-level-changed(v) => { root.effect-param-changed(self.shown, 1, v); }",
        "branch-mix-changed(v) => { root.effect-param-changed(self.shown, 0, v); }",
        "branch-bypass-toggled => { root.effect-bypass-toggled(self.shown); }",
        "branch-input-trim-changed(v) => { root.effect-input-trim-changed(self.shown, v); }",
        "branch-output-trim-changed(v) => { root.effect-output-trim-changed(self.shown, v); }",
        "branch-preset-selected(i) => { root.effect-preset-selected(self.shown, i); }",
        "branch-save-preset-requested => { root.save-effect-preset-requested(self.shown); }",
        "branch-level: root.effect-slots[self.shown].p1;",
        "branch-mix: root.effect-slots[self.shown].p0;",
        "move-requested(place) => { root.layer-branch-moved(index, branch.slot, place); }",
        "bypassed: root.effect-slots[branch.slot].bypassed;",
    ] {
        assert!(arm.contains(wiring), "main.slint's layer arm lost `{wiring}`");
    }
    // The ids, against the table they name.
    assert_eq!(mooloop_core::CONTAINER_PARAM_LEVEL, 1);
    assert_eq!(mooloop_core::CONTAINER_PARAM_MIX, 0);
    // The end's knobs write the layer's own row.
    for wiring in [
        "mix-changed(v) => { root.effect-param-changed(container, 0, v); }",
        "gain-changed(v) => { root.effect-param-changed(container, 1, v); }",
    ] {
        assert!(main.contains(wiring), "main.slint's layer end lost `{wiring}`");
    }
}
