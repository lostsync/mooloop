//! **A control drawn by a `for` repeater follows a drag for the whole press.**
//!
//! Slint rebuilds every row of a repeater whose model is reset or replaced,
//! and a control in a rebuilt row has lost the press it was being dragged
//! with. Each control here sends an edit whose republish lands on the model
//! its own row is drawn from, so publishing that edit by replacing the model
//! ends the drag after its first step: the value moves once and the pointer
//! carries on alone.
//!
//! The drags are real pointer events through the real window, with a frame
//! of mock time between them, and the handlers stand in for `AppUi::new`'s
//! by calling the same publishers. What each test counts is how many of the
//! drag's moves reached the handler. The slice handles have no accessible
//! role to find them by, so their test watches the model they are drawn
//! from instead, for the reset that would rebuild them.

use super::*;
use crate::window_probe::{click, controls, drag, install_backend, sliders, Control};
use i_slint_core::items::AccessibleRole;
use i_slint_core::model::{ModelChangeListener, ModelChangeListenerContainer};
use mooloop_core::{KeyRange, PlayMode, ProjectChannel, SampleReference, SampleZone};
use slint::LogicalSize;
use std::cell::Cell;
use std::path::PathBuf;
use std::pin::Pin;

const WIDTH: f32 = 2400.0;

/// How many moves each drag makes. Every one of them changes the value, so a
/// drag that survives the first edit reaches the handler this many times.
const MOVES: usize = 6;

fn window_with_state() -> (MainWindow, Rc<RefCell<UiState>>) {
    install_backend();
    let window = MainWindow::new().expect("the testing backend builds a window");
    install_strip_spec(&window);
    install_eq_spec(&window);
    let state = Rc::new(RefCell::new(UiState::new(None, 48_000, &window)));
    (window, state)
}

/// Two tracks after the master, the first sending to the second, every
/// strip's EQ in, on a mixer tall enough to draw each strip whole -- the
/// sections, the sends and the fader at once -- so nothing has to be turned
/// over to reach them. Returns the sending track.
fn mixer_window() -> (MainWindow, Rc<RefCell<UiState>>, usize) {
    let (window, state) = window_with_state();
    let from = {
        let mut st = state.borrow_mut();
        let from = st.session.add_track().expect("room for a track");
        let to = st.session.add_track().expect("and for another");
        assert!(
            matches!(st.session.add_send(from as i32, to as i32), Some(Ok(_))),
            "a send to a later track is legal and closes no loop"
        );
        // A section that is out disables its knobs, and a disabled knob
        // ignores a drag for a reason that has nothing to do with this.
        for bus in 0..st.session.buses.len() {
            let _ = st.session.set_strip_param(bus as i32, STRIP_EQ_IN as i32, 1.0);
        }
        st.sync_mixer(&window);
        from
    };
    window.invoke_move_view(view::MIXER, 0);
    window.invoke_show_view(view::MIXER);
    window.set_bottom_pane_visible(false);
    let full = window.global::<MixerMetrics>().get_full_height();
    window.window().set_size(LogicalSize::new(WIDTH, full + 600.0));
    (window, state, from)
}

/// Every value a handler heard, in order.
type Heard = Rc<RefCell<Vec<f32>>>;

/// The strip and send handlers as `AppUi::new` wires them: the session takes
/// the write, then the mixer strip and the bus editor are restated. That
/// restatement is the publish that used to replace the model under the drag.
///
/// The receiver is `ui` rather than `window` so `scripts/dupe-audit
/// unrecorded-edit`, which reads `src/` as the application, does not report
/// a test's stand-in as an edit path the application has.
fn wire_mixer(ui: &MainWindow, state: &Rc<RefCell<UiState>>) -> (Heard, Heard) {
    let strip: Heard = Rc::default();
    let sends: Heard = Rc::default();
    {
        let st = state.clone();
        let weak = ui.as_weak();
        let heard = strip.clone();
        ui.on_bus_strip_param(move |bus, param, value| {
            heard.borrow_mut().push(value);
            let Some(ui) = weak.upgrade() else { return };
            let mut st = st.borrow_mut();
            if st.session.set_strip_param(bus, param, value).is_some() {
                st.sync_mixer_strip(bus.max(0) as usize);
                st.sync_bus_editor(&ui);
            }
        });
    }
    {
        let st = state.clone();
        let weak = ui.as_weak();
        let heard = sends.clone();
        ui.on_send_level_changed(move |bus, send, level| {
            heard.borrow_mut().push(level);
            let Some(ui) = weak.upgrade() else { return };
            let mut st = st.borrow_mut();
            if st.session.set_send_level(bus, send, level).is_some() {
                st.sync_mixer_strip(bus.max(0) as usize);
                st.sync_bus_editor(&ui);
            }
        });
    }
    (strip, sends)
}

fn find(found: Vec<Control>, label: &str) -> Control {
    found
        .into_iter()
        .find(|control| control.label.contains(label))
        .unwrap_or_else(|| panic!("nothing labelled {label:?} is on screen"))
}

fn assert_followed(heard: &Heard, what: &str) {
    let heard = heard.borrow();
    assert!(
        heard.len() >= MOVES,
        "{what} reached its handler {} times in a drag of {MOVES} moves: {heard:?}",
        heard.len()
    );
}

/// The four band rows are the strip's only repeated knobs; COMP and DRIVE
/// are written out and always dragged.
#[test]
fn a_strip_eq_knob_follows_a_drag() {
    let (window, state, _) = mixer_window();
    let (strip, _) = wire_mixer(&window, &state);
    let gain = find(sliders(&window), ": boost or cut");
    drag(&window, gain.centre, (0.0, -6.0), MOVES);
    assert_followed(&strip, "an EQ band's gain knob");
}

#[test]
fn a_mixer_send_bar_follows_a_drag() {
    let (window, state, _) = mixer_window();
    let (_, sends) = wire_mixer(&window, &state);
    let bar = find(sliders(&window), "Send to ");
    // Left, down from unity, where there is room to go.
    drag(&window, bar.centre, (-4.0, 0.0), MOVES);
    assert_followed(&sends, "the strip's send bar");
}

/// Pressed at the fader's centre, which is on its track only while the track
/// fills the row: kept to its 40px minimum, the track is a stub at the row's
/// left and the press lands on nothing.
#[test]
fn a_sidebar_send_fader_follows_a_drag() {
    let (window, state, from) = mixer_window();
    let (_, sends) = wire_mixer(&window, &state);
    {
        let mut st = state.borrow_mut();
        st.session.select_bus(from as i32).expect("the track exists");
        st.sync_bus_editor(&window);
    }
    window.set_channel_sidebar_visible(true);
    // The panel opens by animating its width from zero, and its clip cuts
    // pointer events along with pixels until it has.
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_secs(1));
    let fader = find(sliders(&window), "Level sent to ");
    drag(&window, fader.centre, (3.0, 0.0), MOVES);
    assert_followed(&sends, "the sidebar's send fader");
}

fn tone(len: usize) -> Arc<SampleData> {
    Arc::new(SampleData {
        frames: (0..len).map(|n| [(n as f32 * 0.01).sin(); 2]).collect(),
        sample_rate: 48_000,
        root_note: 60,
    })
}

/// One sampler, selected, on the rack, with its own sample and -- when
/// `zoned` -- two key zones above it.
fn sampler_window(zoned: bool) -> (MainWindow, Rc<RefCell<UiState>>) {
    let (window, state) = window_with_state();
    window.window().set_size(LogicalSize::new(WIDTH, 1400.0));
    window.invoke_move_view(view::DEVICES, 0);
    window.invoke_show_view(view::DEVICES);
    window.set_bottom_pane_visible(false);
    let path = |name: &str| PathBuf::from(format!("/nonexistent/repeated-drag/{name}.wav"));
    let mut channel = ProjectChannel::sampler(0, 1);
    if zoned {
        let sampler = channel.setup.sampler_state_mut().expect("a sampler has its state");
        sampler.keys = KeyRange::new(0, 59);
        sampler.zones = vec![
            SampleZone {
                keys: KeyRange::new(60, 71),
                root_note: 60,
                sample: SampleReference::File { path: path("b"), embedded: false },
                ..SampleZone::default()
            },
            SampleZone {
                keys: KeyRange::new(72, 127),
                root_note: 72,
                sample: SampleReference::File { path: path("c"), embedded: false },
                ..SampleZone::default()
            },
        ];
    }
    let project = Project {
        channels: vec![channel],
        pattern_lengths: vec![16],
        ..Project::default()
    };
    {
        let mut st = state.borrow_mut();
        st.session
            .admit_zone_audio(vec![(path("b"), tone(2_000)), (path("c"), tone(3_000))], true);
        st.session.replace_project(&project, &[Some(tone(4_000))]);
        st.refresh_editor(&window);
    }
    (window, state)
}

/// The ZONES page's LOW field, as `AppUi::new`'s `zone_edit!` publishes it.
#[test]
fn a_zone_key_field_follows_a_drag() {
    let (window, state) = sampler_window(true);
    let heard: Heard = Rc::default();
    {
        let st = state.clone();
        let weak = window.as_weak();
        let heard = heard.clone();
        window.on_sampler_zone_keys_changed(move |index, low, high| {
            heard.borrow_mut().push(low as f32);
            let Some(ui) = weak.upgrade() else { return };
            let mut st = st.borrow_mut();
            let channel = st.session.selected;
            let index = usize::try_from(index).unwrap_or(usize::MAX);
            if st.session.set_zone_keys(channel, index, midi_key(low), midi_key(high)) {
                st.sync_sampler_zones(&ui);
                st.sync_sampler_zone_view(&ui);
            }
        });
    }
    let zones = find(controls(&window, AccessibleRole::Button), "ZONES");
    click(&window, zones.centre);
    // The first LOW field is the sampler's own sample, written out above the
    // repeated rows; the second is zone 1's.
    let low = controls(&window, AccessibleRole::Spinbox)
        .into_iter()
        .filter(|field| field.label == "Lowest key")
        .nth(1)
        .expect("zone 1's LOW field is on the ZONES page");
    // Up, a key a move: the field steps once per 4px.
    drag(&window, low.centre, (0.0, -4.0), MOVES);
    assert_followed(&heard, "a zone's LOW field");
}

/// What a model told the repeaters drawing it.
#[derive(Default)]
struct Notices {
    changed: Cell<usize>,
    resets: Cell<usize>,
}

impl ModelChangeListener for Notices {
    fn row_changed(self: Pin<&Self>, _row: usize) {
        self.changed.set(self.changed.get() + 1);
    }
    fn row_added(self: Pin<&Self>, _index: usize, _count: usize) {
        self.resets.set(self.resets.get() + 1);
    }
    fn row_removed(self: Pin<&Self>, _index: usize, _count: usize) {
        self.resets.set(self.resets.get() + 1);
    }
    fn reset(self: Pin<&Self>) {
        self.resets.set(self.resets.get() + 1);
    }
}

/// A marker moved, published the way every slice handler publishes, and the
/// editor restated after it as a selection or an undo would: the handles are
/// told which rows changed, and never that the model was thrown away.
#[test]
fn moving_a_slice_marker_keeps_the_handles_being_drawn() {
    let (window, state) = sampler_window(false);
    {
        let mut st = state.borrow_mut();
        st.session.channels[0]
            .sampler_params_mut()
            .expect("a sampler has its params")
            .play_mode = PlayMode::Slice;
        let markers = st.session.divide_slices(4, false).expect("the sampler has audio");
        st.publish_slices(&markers);
    }
    let drawn = window.get_slice_markers();
    assert!(drawn.row_count() >= 3, "the divide laid markers down to drag");

    let notices = Box::pin(ModelChangeListenerContainer::<Notices>::default());
    drawn.model_tracker().attach_peer(notices.as_ref().model_peer());
    let moved = {
        let mut st = state.borrow_mut();
        let between = (drawn.row_data(1).unwrap() + drawn.row_data(2).unwrap()) / 2.0;
        let markers = st.session.move_slice(1, between).expect("marker 1 moves");
        st.publish_slices(&markers);
        st.refresh_editor(&window);
        markers
    };

    assert_eq!(notices.resets.get(), 0, "a move rebuilt the slice handles");
    assert!(notices.changed.get() > 0, "the move never reached the handles");
    assert_eq!(drawn.iter().collect::<Vec<_>>(), moved);
}
