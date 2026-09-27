//! What an icon costs a frame, drawn three ways: a registry `Icon` (a
//! `Path`), a glyph `Text`, and an SVG `Image` with `colorize`.
//!
//! Step 01 of `docs/plans/icon-pass/` (MOO-279) gates the registry's storage
//! on this. FemtoVG pays per element per frame (MOO-256, MOO-258), so before
//! every icon became a `Path` the plan measured what one costs against what
//! it replaces. The gate and the numbers are in `00-status.md`.
//!
//! ```sh
//! scripts/antibox --release-bin ...   # or build this example on the box
//! ICON_COST_SECS=4 ICON_COST_ROUNDS=3 icon_cost
//! ```
//!
//! It draws 200 icons (`ICON_COST_COUNT`) on a 16px grid and repaints the
//! window every frame by changing the colour of a rectangle behind them, so
//! the icons themselves never change -- the steady state of a real window,
//! where FemtoVG redraws everything whenever anything moves. It runs each
//! mode (`none`, `path`, `outline`, `text`, `svg`) for `ICON_COST_SECS`,
//! interleaved for `ICON_COST_ROUNDS`, so load on the machine falls on every
//! mode alike, and prints each mode's frame cost on the UI thread
//! (`BeforeRendering` to `AfterRendering`, as `MOOLOOP_PROFILE_UI` measures
//! it). Run it in a real GPU window: the software renderer is not what the
//! application ships.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::ComponentHandle;

slint::slint! {
    import { Theme } from "ui/theme.slint";
    import { Icon, Icons } from "ui/icons.slint";

    export component IconCost inherits Window {
        in property <int> mode;
        in property <int> count: 200;
        in property <image> svg;
        in property <bool> phase;
        title: "icon cost";
        width: 440px;
        height: 240px;
        background: Theme.background;

        Rectangle {
            background: root.phase ? Theme.surface : Theme.surface-raised;
        }
        for i in root.count : Rectangle {
            x: 10px + mod(i, 20) * 21px;
            y: 10px + floor(i / 20) * 21px;
            width: 16px;
            height: 16px;
            if root.mode == 1 : Icon {
                icon: Icons.tool-select;
                tint: Theme.text;
            }
            if root.mode == 2 : Icon {
                icon: Icons.tool-select;
                tint: Theme.text;
                outline: true;
            }
            if root.mode == 3 : Text {
                text: "×";
                color: Theme.text;
                font-size: Theme.text-xl;
                horizontal-alignment: center;
                vertical-alignment: center;
            }
            if root.mode == 4 : Image {
                width: 16px;
                height: 16px;
                source: root.svg;
                colorize: Theme.text;
            }
        }
    }
}

const MODES: [&str; 5] = ["none", "path", "outline", "text", "svg"];

/// The registry's `tool-select` as an SVG document, so the three ways draw
/// the same shape.
const SVG: &str = concat!(
    r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">"#,
    r#"<path d="M 3.5 2.5 L 3.5 13 L 6.4 10.2 L 8.2 13.6 L 9.9 12.7 L 8.1 9.4 L 12 9.4 Z" fill="black"/>"#,
    "</svg>"
);

fn env_number(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[derive(Default)]
struct Frames {
    started: Option<Instant>,
    /// Frame costs, per mode, for frames past the settling time.
    costs: Vec<Vec<Duration>>,
    mode: usize,
    counting: bool,
}

fn main() -> Result<(), slint::PlatformError> {
    let secs = env_number("ICON_COST_SECS", 4);
    let rounds = env_number("ICON_COST_ROUNDS", 3) as usize;
    let count = env_number("ICON_COST_COUNT", 200) as i32;

    let window = IconCost::new()?;
    window.set_count(count);
    window.set_svg(
        slint::Image::load_from_svg_data(SVG.as_bytes()).expect("the icon SVG parses"),
    );

    let frames = Rc::new(RefCell::new(Frames {
        costs: vec![Vec::new(); MODES.len()],
        ..Frames::default()
    }));
    let notified = frames.clone();
    window
        .window()
        .set_rendering_notifier(move |state, _| {
            let mut frames = notified.borrow_mut();
            match state {
                slint::RenderingState::BeforeRendering => frames.started = Some(Instant::now()),
                slint::RenderingState::AfterRendering => {
                    if let Some(started) = frames.started.take() {
                        if frames.counting {
                            let mode = frames.mode;
                            frames.costs[mode].push(started.elapsed());
                        }
                    }
                }
                _ => {}
            }
        })
        .expect("the renderer takes a rendering notifier");

    // Repaint every frame: the rectangle behind the icons changes colour.
    let repaint = slint::Timer::default();
    let weak = window.as_weak();
    repaint.start(slint::TimerMode::Repeated, Duration::from_millis(2), move || {
        if let Some(window) = weak.upgrade() {
            window.set_phase(!window.get_phase());
        }
    });

    // The schedule: every mode, `rounds` times over, each for `secs`, the
    // first half-second of each uncounted while its elements are built.
    let schedule: Vec<usize> = (0..rounds).flat_map(|_| 0..MODES.len()).collect();
    let step = Rc::new(RefCell::new(0usize));
    let driver = slint::Timer::default();
    let weak = window.as_weak();
    let driven = frames.clone();
    let settle = slint::Timer::default();
    let settle_frames = frames.clone();
    let begin = Rc::new(move |mode: usize, window: &IconCost| {
        {
            let mut frames = driven.borrow_mut();
            frames.mode = mode;
            frames.counting = false;
        }
        window.set_mode(mode as i32);
        let frames = settle_frames.clone();
        settle.start(slint::TimerMode::SingleShot, Duration::from_millis(500), move || {
            frames.borrow_mut().counting = true;
        });
    });
    if let Some(window) = weak.upgrade() {
        begin(schedule[0], &window);
    }
    let report = frames.clone();
    let next = begin.clone();
    driver.start(slint::TimerMode::Repeated, Duration::from_secs(secs), move || {
        let mut at = step.borrow_mut();
        *at += 1;
        let Some(window) = weak.upgrade() else { return };
        if *at < schedule.len() {
            next(schedule[*at], &window);
            return;
        }
        let frames = report.borrow();
        println!("icon_cost: {count} icons, {rounds} rounds of {secs} s per mode");
        println!("{:<8} {:>7} {:>9} {:>9} {:>9}", "mode", "frames", "mean µs", "p50 µs", "p99 µs");
        for (mode, name) in MODES.iter().enumerate() {
            let mut costs: Vec<u128> = frames.costs[mode].iter().map(Duration::as_micros).collect();
            costs.sort_unstable();
            if costs.is_empty() {
                println!("{name:<8} no frames");
                continue;
            }
            let mean = costs.iter().sum::<u128>() / costs.len() as u128;
            let p50 = costs[costs.len() / 2];
            let p99 = costs[(costs.len() * 99 / 100).min(costs.len() - 1)];
            println!("{name:<8} {:>7} {mean:>9} {p50:>9} {p99:>9}", costs.len());
        }
        let _ = slint::quit_event_loop();
    });

    window.run()
}
