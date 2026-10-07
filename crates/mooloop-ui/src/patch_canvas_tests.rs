//! The patch canvas driven through its one `TouchArea`, as a hand drives it
//! (song patch step 03).
//!
//! `patch_canvas.rs` tests the geometry and the gestures as data. These
//! press, drag and type on the real window with the canvas's handlers wired
//! as `AppUi::new` wires them, and ask the song and the undo history what
//! came of it: a wire drawn, replaced and deleted, a box moved, an outlet
//! armed, each one undo step.

use super::*;
use crate::window_probe::install_backend;
use i_slint_core::accessibility::AccessibleStringProperty;
use i_slint_core::item_tree::ItemRc;
use i_slint_core::items::AccessibleRole;
use i_slint_core::window::WindowInner;
use mooloop_core::{CanvasPoint, Jack, ModSourceId};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{LogicalPosition, LogicalSize};
use std::ops::ControlFlow;

const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 800.0;

struct Harness {
    window: MainWindow,
    state: Rc<RefCell<UiState>>,
    commands: Rc<RefCell<CommandState>>,
}

/// The real window with the Modulation pane in the bottom dock, and a song
/// with two LFOs and a Math box placed where the test knows them.
fn harness() -> (Harness, [ModSourceId; 3]) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    window.window().set_size(LogicalSize::new(WIDTH, HEIGHT));
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    let ids = {
        let mut st = state.borrow_mut();
        let lfo = st.session.add_patch_box(ModulatorKind::Lfo, CanvasPoint::new(200, 40)).unwrap();
        let other = st.session.add_patch_box(ModulatorKind::Lfo, CanvasPoint::new(420, 40)).unwrap();
        let math = st.session.add_patch_box(ModulatorKind::Math, CanvasPoint::new(300, 180)).unwrap();
        // A new box gates from the selected channel; these start unwired.
        st.session.modulation.wires.clear();
        st.session.modulation.tags.clear();
        st.refresh_modulation(&window);
        [lfo, other, math]
    };
    // In the bottom dock, where a first run keeps it: the main slot's pane
    // leaves the canvas too short for the boxes below.
    window.invoke_show_view(view::MODULATION);
    let commands = Rc::new(RefCell::new(CommandState::default()));
    patch_canvas::wire(&window, &state, &commands);
    WindowInner::from_pub(window.window()).ensure_tree_instantiated();
    (Harness { window, state, commands }, ids)
}

impl Harness {
    /// Where canvas point `(x, y)` is in the window: the canvas's touch
    /// area, found by its name, scrolled however the pane has it.
    fn at(&self, (x, y): (f32, f32)) -> (f32, f32) {
        let inner = WindowInner::from_pub(self.window.window());
        let mut origin = None;
        ItemRc::new_root(inner.component()).visit_descendants(|item| {
            let named = item.accessible_role() == AccessibleRole::Groupbox
                && item
                    .accessible_string_property(AccessibleStringProperty::Label)
                    .is_some_and(|label| label == "Patch canvas");
            if named && item.is_visible() {
                origin = Some(item.map_to_window(item.geometry().origin));
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        });
        let origin = origin.expect("the Modulation pane draws the canvas");
        (origin.x + x, origin.y + y)
    }

    /// Where a box's jack is on the canvas, as the canvas lays it out.
    fn jack(&self, id: ModSourceId, outlet: bool, port: usize) -> (f32, f32) {
        let layout = self.state.borrow().patch_layout();
        let anchor = layout
            .node(patch_canvas::NodeKey::Node(id))
            .and_then(|node| node.anchor(outlet, port))
            .expect("the box has that jack");
        (anchor.x, anchor.y)
    }

    /// A drag from canvas point `from` to `to`, in several moves with a
    /// frame's worth of mock time after the press and after each move, the
    /// way a hand makes one: the canvas scrolls, and its `ScrollView` holds a
    /// press back until a frame has passed (see `window_probe::drag`).
    fn drag(&self, from: (f32, f32), to: (f32, f32)) {
        let (from, to) = (self.at(from), self.at(to));
        let pos = |(x, y): (f32, f32)| LogicalPosition::new(x, y);
        let frame = |ms| i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(ms));
        let w = self.window.window();
        w.dispatch_event(WindowEvent::PointerMoved { position: pos(from) });
        w.dispatch_event(WindowEvent::PointerPressed { position: pos(from), button: PointerEventButton::Left });
        frame(200);
        const STEPS: usize = 8;
        for i in 1..=STEPS {
            let t = i as f32 / STEPS as f32;
            w.dispatch_event(WindowEvent::PointerMoved {
                position: pos((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)),
            });
            frame(16);
        }
        w.dispatch_event(WindowEvent::PointerReleased { position: pos(to), button: PointerEventButton::Left });
        frame(16);
    }

    /// A press and a release in one place, a frame apart.
    fn click(&self, point: (f32, f32)) {
        let at = self.at(point);
        let pos = LogicalPosition::new(at.0, at.1);
        let w = self.window.window();
        w.dispatch_event(WindowEvent::PointerMoved { position: pos });
        w.dispatch_event(WindowEvent::PointerPressed { position: pos, button: PointerEventButton::Left });
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
        w.dispatch_event(WindowEvent::PointerReleased { position: pos, button: PointerEventButton::Left });
        i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    }

    /// Two clicks in one place, close enough together to be a double-click.
    fn double_click(&self, point: (f32, f32)) {
        let at = self.at(point);
        let pos = LogicalPosition::new(at.0, at.1);
        let w = self.window.window();
        w.dispatch_event(WindowEvent::PointerMoved { position: pos });
        for _ in 0..2 {
            w.dispatch_event(WindowEvent::PointerPressed { position: pos, button: PointerEventButton::Left });
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
            w.dispatch_event(WindowEvent::PointerReleased { position: pos, button: PointerEventButton::Left });
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
        }
    }

    fn type_text(&self, text: &str) {
        crate::window_probe::type_text(&self.window, text);
    }

    /// What each box on the canvas says, in list order.
    fn boxes(&self) -> Vec<String> {
        let st = self.state.borrow();
        st.session
            .modulation
            .modules
            .iter()
            .map(|module| mooloop_core::box_text::spell(&module.params, &module.text))
            .collect()
    }

    fn key(&self, text: &str) {
        let text = slint::SharedString::from(text);
        self.window.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        self.window.window().dispatch_event(WindowEvent::KeyReleased { text });
    }

    fn steps(&self) -> Vec<&'static str> {
        self.commands.borrow().history.entries().iter().map(|entry| entry.label).collect()
    }

    fn feeds(&self, inlet: Jack) -> Option<Jack> {
        self.state.borrow().session.modulation.wire_into(inlet).map(|wire| wire.from)
    }
}

#[test]
fn a_wire_is_drawn_replaced_and_deleted_through_the_canvas() {
    let (h, [lfo, other, math]) = harness();
    let math_in = Jack::new(math, 0);

    h.drag(h.jack(lfo, true, 0), h.jack(math, false, 0));
    assert_eq!(h.feeds(math_in), Some(Jack::new(lfo, 0)), "the drop wired it");
    assert_eq!(h.steps(), ["Patch wire"], "one undo step");

    h.drag(h.jack(other, true, 0), h.jack(math, false, 0));
    assert_eq!(h.feeds(math_in), Some(Jack::new(other, 0)), "dropping on a wired inlet replaces its wire");
    assert_eq!(h.steps(), ["Patch wire", "Patch wire"]);

    // Click the wire on its horizontal run, then Delete.
    let line = h
        .state
        .borrow()
        .patch_layout()
        .wires
        .iter()
        .find(|wire| wire.key == patch_canvas::WireKey::Patch(math_in))
        .map(|wire| wire.points.clone())
        .expect("the wire is drawn");
    let (a, b) = (line[1], line[2]);
    h.click(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0));
    assert_eq!(h.state.borrow().patch_canvas.wire, Some(patch_canvas::WireKey::Patch(math_in)));
    h.key("\u{7f}");
    assert_eq!(h.feeds(math_in), None, "Delete removed the selected wire");
    assert_eq!(h.steps().last(), Some(&"Delete from patch"));
    assert_eq!(h.steps().len(), 3);
}

#[test]
fn a_wire_of_the_wrong_sort_is_refused_and_says_why() {
    let (h, [lfo, _, _]) = harness();
    let notes = h
        .state
        .borrow_mut()
        .session
        .add_patch_tag(mooloop_core::TagKind::NotesIn { channel: None, take: false }, CanvasPoint::new(16, 120))
        .unwrap();
    h.state.borrow().refresh_modulation(&h.window);
    let tag = {
        let layout = h.state.borrow().patch_layout();
        let anchor = layout.node(patch_canvas::NodeKey::Node(notes)).unwrap().anchor(true, 0).unwrap();
        (anchor.x, anchor.y)
    };
    h.drag(tag, h.jack(lfo, false, 0));
    assert_eq!(h.feeds(Jack::new(lfo, 0)), None);
    assert!(h.steps().is_empty(), "nothing to undo");
    assert!(h.window.get_status_message().contains("note inlet"), "{}", h.window.get_status_message());
}

#[test]
fn dragging_a_box_moves_it_in_one_step_and_a_click_on_its_outlet_arms_it() {
    let (h, [lfo, _, _]) = harness();
    h.drag((240.0, 54.0), (300.0, 114.0));
    let at = h.state.borrow().session.modulation.module(lfo).map(|module| module.at);
    assert_eq!(at, Some(CanvasPoint::new(260, 100)), "moved by the drag");
    assert_eq!(h.steps(), ["Move in patch"]);

    h.click(h.jack(lfo, true, 0));
    assert_eq!(h.state.borrow().session.modulation_armed.get(), Some(ModSourceRef::Id(lfo)));
    h.click(h.jack(lfo, true, 0));
    assert_eq!(h.state.borrow().session.modulation_armed.get(), None, "the second click disarms");
    assert_eq!(h.steps(), ["Move in patch"], "arming is not an edit");
}

#[test]
fn a_click_on_an_inlet_opens_its_picker_and_a_pick_feeds_it() {
    let (h, [lfo, _, math]) = harness();
    h.click(h.jack(math, false, 0));
    let patch = h.window.global::<PatchView>();
    assert!(patch.get_menu_open(), "the picker opened");
    let options: Vec<String> = patch.get_menu_options().iter().map(|option| option.to_string()).collect();
    let lfo_row = options.iter().position(|option| option.ends_with("LFO 1")).expect("the first LFO is offered");
    patch.invoke_picked(lfo_row as i32);
    assert_eq!(h.feeds(Jack::new(math, 0)), Some(Jack::new(lfo, 0)));
    assert!(!patch.get_menu_open());
    assert_eq!(h.steps(), ["Patch wire"]);
}

/// **A right-click on empty canvas makes a tag, and an inlet asks what it
/// reads** (song patch step 06): pick Inlet, then Bar from the list that
/// opens at once; a click on the tag later opens the list again and
/// rebinds it, and the tag can feed a box's inlet from that box's picker.
#[test]
fn a_right_click_makes_an_inlet_tag_and_its_list_binds_it() {
    use mooloop_core::{InletSource, TagKind};
    let (h, [lfo, _, _]) = harness();
    let at = h.at((660.0, 110.0));
    let pos = LogicalPosition::new(at.0, at.1);
    let w = h.window.window();
    w.dispatch_event(WindowEvent::PointerMoved { position: pos });
    w.dispatch_event(WindowEvent::PointerPressed { position: pos, button: PointerEventButton::Right });
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(200));
    w.dispatch_event(WindowEvent::PointerReleased { position: pos, button: PointerEventButton::Right });
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    let patch = h.window.global::<PatchView>();
    assert!(patch.get_menu_open(), "the right-click opened the menu");
    let options = || patch.get_menu_options().iter().map(|option| option.to_string()).collect::<Vec<_>>();
    assert_eq!(options(), ["Inlet", "Notes in", "Notes out"]);
    patch.invoke_picked(0);
    let tag = {
        let st = h.state.borrow();
        let tag = st.session.modulation.tags.last().expect("a tag was made").clone();
        assert_eq!(tag.kind, TagKind::Inlet { bind: None });
        assert_eq!(tag.at, CanvasPoint::new(660, 110));
        tag.id
    };
    assert!(patch.get_menu_open(), "a new inlet asks what it reads");
    assert_eq!(&options()[..5], ["None", "Beat", "Bar", "Pattern position", "Pattern (topmost row)"]);
    assert!(options().iter().any(|option| option.ends_with("· gate")));
    patch.invoke_picked(2);
    let bind = |h: &Harness| match h.state.borrow().session.modulation.tag(tag).unwrap().kind {
        TagKind::Inlet { bind } => bind,
        _ => unreachable!(),
    };
    assert_eq!(bind(&h), Some(InletSource::Bar));
    assert_eq!(h.steps(), ["Patch tag", "Patch tag source"]);

    let node = h.state.borrow().patch_layout().node(patch_canvas::NodeKey::Node(tag)).map(|node| (node.x + 8.0, node.y + 8.0)).unwrap();
    h.click(node);
    assert!(patch.get_menu_open(), "a click on the tag opens its list");
    assert_eq!(patch.get_menu_current(), 2, "lit on what it reads");
    patch.invoke_picked(1);
    assert_eq!(bind(&h), Some(InletSource::Beat));

    // The LFO's retrigger inlet can now be fed from the Beat tag.
    h.click(h.jack(lfo, false, 1));
    let row = options().iter().position(|option| option == "Beat").expect("the beat tag is offered");
    patch.invoke_picked(row as i32);
    assert_eq!(h.feeds(Jack::new(lfo, 1)), Some(Jack::new(tag, 0)));
}

#[test]
fn a_double_click_on_empty_canvas_types_a_box_there() {
    let (h, _) = harness();
    h.double_click((600.0, 120.0));
    let patch = h.window.global::<PatchView>();
    assert!(patch.get_typing(), "the field opened");
    assert_eq!(patch.get_words().row_count(), mooloop_core::box_text::VOCABULARY.len());
    h.type_text("counter 4\n");
    assert!(!patch.get_typing(), "Enter closed it");
    assert_eq!(h.boxes(), ["lfo", "lfo", "* 1", "counter 4"]);
    let st = h.state.borrow();
    let counter = st.session.modulation.modules.last().unwrap();
    assert_eq!(counter.at, CanvasPoint::new(600, 120));
    drop(st);
    assert_eq!(h.steps(), ["Type a box"]);
}

#[test]
fn the_completion_list_picks_a_name_and_enter_makes_it() {
    use slint::Model;
    let (h, _) = harness();
    h.double_click((600.0, 120.0));
    let patch = h.window.global::<PatchView>();
    h.type_text("s");
    let names: Vec<String> = patch.get_words().iter().map(|word| word.name.to_string()).collect();
    assert_eq!(names, ["step", "select", "slew"]);
    h.type_text("\u{f701}");
    assert_eq!(patch.get_word_active(), 1);
    h.type_text("\n");
    assert_eq!(h.boxes().last().map(String::as_str), Some("select 4"));
}

#[test]
fn tab_completes_and_an_unknown_name_stays_as_typed() {
    let (h, _) = harness();
    h.double_click((600.0, 120.0));
    h.type_text("cou\t");
    let patch = h.window.global::<PatchView>();
    assert_eq!(patch.get_typing_text(), "counter ");
    h.type_text("8\n");
    assert_eq!(h.boxes().last().map(String::as_str), Some("counter 8"));

    h.double_click((600.0, 200.0));
    h.type_text("chord min7\n");
    assert_eq!(h.boxes().last().map(String::as_str), Some("chord min7"));
    assert!(h.window.get_status_message().contains("not a box"), "{}", h.window.get_status_message());
    let st = h.state.borrow();
    assert_eq!(st.session.modulation.modules.last().unwrap().params, ModulatorParams::Unknown);
}

#[test]
fn a_double_click_on_a_box_retypes_it_and_escape_leaves_it() {
    let (h, [lfo, _, math]) = harness();
    h.double_click((240.0, 54.0));
    let patch = h.window.global::<PatchView>();
    assert_eq!(patch.get_typing_text(), "lfo", "the field holds what the box says");
    h.type_text("\u{1b}");
    assert!(!patch.get_typing());
    assert_eq!(h.boxes()[0], "lfo");

    h.state.borrow_mut().session.rewire_patch(Jack::new(lfo, 0), Jack::new(math, 0)).unwrap();
    h.double_click((240.0, 54.0));
    // The field opens with its text selected, so typing replaces it.
    h.type_text("slew 0.5\n");
    assert_eq!(h.boxes()[0], "slew 0.5");
    let st = h.state.borrow();
    let slew = st.session.modulation.modules[0].id;
    assert_ne!(slew, lfo, "a new kind is a new box");
    assert_eq!(
        st.session.modulation.wire_into(Jack::new(math, 0)).map(|wire| wire.from),
        Some(Jack::new(slew, 0)),
        "its outlet's wire followed it"
    );
    drop(st);
    assert_eq!(h.steps(), ["Retype a box"]);
}

/// The prototype's first example drawn by the real window, written as a PPM
/// to `MOOLOOP_PATCH_SNAPSHOT` for a look: `cargo test -p mooloop-ui --lib
/// patch_canvas_snapshot -- --ignored`. `MOOLOOP_PATCH_SNAPSHOT_DOCK` set
/// leaves the pane in the bottom dock instead of the main area.
#[test]
#[ignore = "writes a picture for a person to look at"]
fn patch_canvas_snapshot() {
    use mooloop_core::{EffectTarget, ModMathParams, ModPolarity, ModRoute, ParamAddr, TagKind, InletSource, STRIP_PARAM_PAN, STRIP_PARAM_VOLUME};
    // The software renderer: the only backend that can take a snapshot.
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(slint::SharedString::from("software")),
        },
    )))
    .ok();
    let window = MainWindow::new().expect("the testing backend builds a window");
    window.window().set_size(LogicalSize::new(1280.0, 760.0));
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    {
        let mut st = state.borrow_mut();
        let kick = st.session.channel_id(0).unwrap();
        let lfo = st.session.add_patch_box(ModulatorKind::Lfo, CanvasPoint::new(204, 72)).unwrap();
        let math = st.session.add_patch_box(ModulatorKind::Math, CanvasPoint::new(600, 140)).unwrap();
        st.session.modulation.wires.clear();
        st.session.modulation.tags.clear();
        if let Some(module) = st.session.modulation.module_mut(math) {
            module.params = ModulatorParams::Math(ModMathParams { operand: -0.5, ..ModMathParams::default() });
        }
        let gate = st.session.add_patch_tag(TagKind::Inlet { bind: Some(InletSource::Gate(kick)) }, CanvasPoint::new(24, 24)).unwrap();
        st.session.connect_patch(Jack::new(gate, 0), Jack::new(lfo, 1)).unwrap();
        st.session.connect_patch(Jack::new(lfo, 0), Jack::new(math, 0)).unwrap();
        st.session.modulation.routes.push(ModRoute::from_module(lfo, ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME), 0.4, ModPolarity::Bipolar));
        st.session.modulation.routes.push(ModRoute::from_module(math, ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_PAN), 0.5, ModPolarity::Bipolar));
        st.session.select_modulation_source(0);
        st.patch_canvas.selected = vec![patch_canvas::NodeKey::Node(lfo)];
        // MOOLOOP_PATCH_SNAPSHOT_TYPING set: a field open with its list.
        if let Ok(text) = std::env::var("MOOLOOP_PATCH_SNAPSHOT_TYPING") {
            st.patch_canvas.typing = Some(patch_canvas::Typing {
                at: (700.0, 30.0),
                retype: None,
                text: text.clone(),
                active: 0,
            });
            window.global::<PatchView>().set_typing_text(text.into());
        }
        st.refresh_modulation(&window);
    }
    if std::env::var("MOOLOOP_PATCH_SNAPSHOT_DOCK").is_err() {
        window.invoke_move_view(view::MODULATION, 0);
    }
    window.invoke_show_view(view::MODULATION);
    WindowInner::from_pub(window.window()).ensure_tree_instantiated();
    let snapshot = window.window().take_snapshot().unwrap();
    if let Ok(path) = std::env::var("MOOLOOP_PATCH_SNAPSHOT") {
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for rgba in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&rgba[..3]);
        }
        std::fs::write(&path, ppm).unwrap();
    }
}

#[test]
fn a_cables_middle_drags_to_a_bend_and_a_double_click_straightens_it() {
    let (h, [lfo, _, math]) = harness();
    let math_in = Jack::new(math, 0);
    h.drag(h.jack(lfo, true, 0), h.jack(math, false, 0));
    let line = h
        .state
        .borrow()
        .patch_layout()
        .wires
        .iter()
        .find(|wire| wire.key == patch_canvas::WireKey::Patch(math_in))
        .map(|wire| wire.points.clone())
        .expect("the wire is drawn");
    // The LFO's outlet is above the Math box's inlet, so the middle run is
    // horizontal and drags up and down.
    let (a, b) = (line[1], line[2]);
    assert_eq!(a.1, b.1, "{line:?}");
    let middle = ((a.0 + b.0) / 2.0, a.1);
    h.drag(middle, (middle.0, middle.1 + 24.0));
    let bend = h.state.borrow().session.modulation.wire_into(math_in).and_then(|wire| wire.bend);
    assert_eq!(
        bend,
        Some(mooloop_core::Bend { axis: mooloop_core::BendAxis::Horizontal, at: (a.1 + 24.0).round() as i32 })
    );
    assert_eq!(h.feeds(math_in), Some(Jack::new(lfo, 0)), "bending keeps the wire");
    assert_eq!(h.steps(), ["Patch wire", "Bend a cable"], "one undo step per drag");

    h.double_click((middle.0, a.1 + 24.0));
    let bend = h.state.borrow().session.modulation.wire_into(math_in).and_then(|wire| wire.bend);
    assert_eq!(bend, None, "a double-click goes back to automatic routing");
    assert_eq!(h.steps(), ["Patch wire", "Bend a cable", "Straighten a cable"]);
}

#[test]
fn an_open_box_shows_its_knobs_and_an_armed_outlet_assigns_to_one() {
    let (h, [lfo, other, _]) = harness();
    let fold = {
        let layout = h.state.borrow().patch_layout();
        let node = layout.node(patch_canvas::NodeKey::Node(lfo)).unwrap().clone();
        (node.x + node.width - patch_canvas::FOLD_WIDTH / 2.0, node.y + patch_canvas::BOX_HEIGHT / 2.0)
    };
    h.click(fold);
    assert!(h.state.borrow().session.modulation.module(lfo).unwrap().open);
    assert_eq!(h.steps(), ["Open a box"]);
    let patch = h.window.global::<PatchView>();
    let knobs: Vec<_> = patch.get_knobs().iter().filter(|knob| knob.module == lfo.0 as i32).collect();
    assert!(!knobs.is_empty(), "the face has knobs");
    let depth = knobs
        .iter()
        .find(|knob| knob.param == mooloop_core::LFO_PARAM_DEPTH as i32)
        .expect("an LFO's face has its depth");

    // A base edit is one step.
    patch.invoke_knob_changed(lfo.0 as i32, depth.param, 0.25);
    assert_eq!(h.steps(), ["Open a box", "Box setting"]);

    h.click(h.jack(other, true, 0));
    assert_eq!(h.state.borrow().session.modulation_armed.get(), Some(ModSourceRef::Id(other)));
    let armed = patch.get_knobs().iter().find(|knob| knob.module == lfo.0 as i32 && knob.param == depth.param).unwrap();
    assert!(armed.allowed, "another box's outlet may move this knob");
    patch.invoke_knob_depth_started();
    patch.invoke_knob_depth_changed(lfo.0 as i32, depth.param, 0.5);
    patch.invoke_knob_depth_changed(lfo.0 as i32, depth.param, 0.6);
    patch.invoke_knob_depth_finished();
    let routes: Vec<_> = h
        .state
        .borrow()
        .session
        .modulation
        .routes
        .iter()
        .map(|route| (route.source, route.destination, route.depth))
        .collect();
    assert_eq!(
        routes,
        [(ModSourceRef::Id(other), ParamAddr::modulator(lfo, depth.param as u32), 0.6)],
        "one route onto the knob, at the depth the drag ended on"
    );
    assert_eq!(h.steps(), ["Open a box", "Box setting", "Modulation route changed"]);
}

#[test]
fn cables_show_their_level_and_flash_for_a_note_as_much_as_the_setting_says() {
    let (h, [lfo, _, math]) = harness();
    let notes = h
        .state
        .borrow_mut()
        .session
        .add_patch_tag(mooloop_core::TagKind::NotesIn { channel: None, take: false }, CanvasPoint::new(16, 120))
        .unwrap();
    let out = h
        .state
        .borrow_mut()
        .session
        .add_patch_tag(mooloop_core::TagKind::NotesOut { channel: None }, CanvasPoint::new(16, 300))
        .unwrap();
    h.drag(h.jack(lfo, true, 0), h.jack(math, false, 0));
    {
        let mut st = h.state.borrow_mut();
        st.session.connect_patch(Jack::new(notes, 0), Jack::new(out, 0)).unwrap();
        st.modulation_edited(&h.window);
        // No engine here to take the set, so this side says it was sent.
        st.session.modulation_sent = st.session.modulation_plan();
    }
    let read = |level: f32, count: u32| {
        let st = h.state.borrow();
        st.session.read_modulation_levels(|_| level, |_| Default::default(), |_| (count, 0.0));
        st.refresh_patch_activity(&h.window);
    };
    let wire = |inlet: Jack| {
        let patch = h.window.global::<PatchView>();
        let layout = h.state.borrow().patch_layout();
        let index = layout
            .wires
            .iter()
            .position(|wire| wire.key == patch_canvas::WireKey::Patch(inlet))
            .expect("the wire is drawn");
        patch.get_wires().row_data(index).unwrap()
    };
    let (control, note) = (Jack::new(math, 0), Jack::new(out, 0));
    assert!(wire(note).note && !wire(control).note);

    read(-0.8, 0);
    assert!((wire(control).level - 0.8).abs() < 1e-6, "a control wire carries its level, either sign");
    read(-0.8, 2);
    assert!(wire(note).flash > 0, "a note thickens its wire");
    assert!(h.state.borrow().patch_canvas.flashing.get());
    for _ in 0..40 {
        read(-0.8, 2);
    }
    assert_eq!(wire(note).flash, 0, "and only briefly");
    assert!(!h.state.borrow().patch_canvas.flashing.get());

    // Off draws plain wires and writes nothing.
    h.window.global::<crate::DisplayPrefs>().set_cable_activity(0);
    read(0.1, 3);
    assert!((wire(control).level - 0.8).abs() < 1e-6);
    assert_eq!(wire(note).flash, 0);
}
