//! What every `mooloop-ui` integration test needs before it can do anything.
//!
//! Two things lived in copies before this module existed, and they were the
//! same shape: a value or a stanza spelled out in file after file with
//! nothing holding the copies together.
//!
//! `cargo` does not build `tests/common/mod.rs` as a test binary of its own --
//! only the top-level `.rs` files in `tests/` are targets -- so this is the
//! one arrangement an integration test can share code through. Each test file
//! that wants it declares `mod common;` and takes what it needs; the
//! `allow(dead_code)` below is because every file then compiles the whole
//! module and uses part of it.

#![allow(dead_code)]

/// Install the software renderer, the only backend with `take_snapshot`, and
/// the only one that draws at all without a display.
///
/// **`.ok()` rather than `.expect`, and not for the reason the copies gave.**
/// Eight of the files this replaced said `.expect("initialize headless
/// renderer")` and two said `.ok()` with a comment explaining that "the
/// platform is per *process*, so whichever runs second finds it already
/// installed". That is not what Slint 1.17 does: `set_platform` writes to
/// `GLOBAL_CONTEXT`, a `thread_local!` (`i-slint-core/context.rs:51`), and
/// with `threading: false` the testing backend returns no event-loop proxy,
/// so the one genuinely process-global cell in that function is never
/// touched. Each `#[test]` runs on its own thread and installs its own
/// platform, which is why both spellings have always worked -- including
/// `source_snapshot.rs`, where ten of fifteen tests each said `.expect` and
/// every one of them got the `Ok`.
///
/// The case that does exist is a single test building two harnesses: that is
/// one thread installing twice, and the second call is an `Err` meaning the
/// backend it wanted is already there. `.ok()` is right for that, so it is
/// what this does.
pub fn install_testing_backend() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(slint::SharedString::from("software")),
        },
    )))
    .ok();
}

/// The piano roll's grid geometry in logical pixels, and the coordinate
/// helpers built on it.
///
/// Derived empirically from a software render of the 960x760 window on the
/// Notes page. These move if the editor's left gutter or the toolbar above it
/// is resized, and the point of holding them once is that such a move is one
/// edit rather than two.
///
/// `GRID_TOP_Y` moved up 34px on 2026-09-08, when the dock's two stacked
/// toolbars -- a 30px slot header and a 34px per-page row -- became the one
/// 30px row every view now carries. The grid's *top* is what moved: the
/// horizontal scrollbar and the velocity lane are anchored to the dock's
/// bottom edge, which did not move, so their constants are unchanged. Both
/// were re-measured off software renders of the old and new layouts rather
/// than adjusted by arithmetic -- and `piano_drag.rs` and `piano_tools.rs`
/// each held their own copy, so fixing the first reported the second as
/// nineteen fresh failures. That is how the copy was found.
///
/// `rack_tools.rs` has a third `GRID_ORIGIN_X`; that one is the step grid and
/// is genuinely a different grid, so it does not belong here.
pub mod piano_grid {
    pub const GRID_ORIGIN_X: f32 = 54.0;
    pub const GRID_TOP_Y: f32 = 349.0;
    pub const ROW_HEIGHT: f32 = 8.0;
    pub const STEP_WIDTH: f32 = 32.0;
    pub const HIGH_NOTE: i32 = 84;
    pub const H_SCROLLBAR_Y: f32 = 649.0;
    pub const V_SCROLLBAR_X: f32 = 946.0;

    /// The engine's own value rather than a copy of it.
    ///
    /// This is what makes the coordinate helpers below able to see a drift
    /// instead of sharing it. `piano-grid.slint` spells the same number as a
    /// bare `24` in fifteen places; if the table ever moves and the markup
    /// does not, the positions computed here stop matching where the grid
    /// draws and these tests fail -- which is the whole job. Held as its own
    /// `const 24`, they would have gone on passing while the roll drew every
    /// note in the wrong place.
    pub const TICKS_PER_STEP: i32 = mooloop_core::TICKS_PER_STEP as i32;

    /// The roll's default snap, 1/16, which at 96 PPQ is one step -- so it is
    /// the engine's number too, not a second `24`.
    pub const SNAP_TICKS: i32 = TICKS_PER_STEP;

    pub fn note_centre_y(midi_note: i32) -> f32 {
        GRID_TOP_Y + (HIGH_NOTE - midi_note) as f32 * ROW_HEIGHT + ROW_HEIGHT / 2.0
    }

    pub fn tick_x(tick: i32) -> f32 {
        GRID_ORIGIN_X + tick as f32 * STEP_WIDTH / TICKS_PER_STEP as f32
    }
}
