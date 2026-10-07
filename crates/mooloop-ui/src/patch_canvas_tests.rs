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
