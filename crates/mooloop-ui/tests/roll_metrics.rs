//! The roll's tick count, in the markup, is the engine's.
//!
//! `TICKS_PER_STEP` was a literal `24` twenty-five times in the markup --
//! fifteen in `piano-grid.slint` and ten in `main.slint` -- because Slint
//! cannot read a Rust constant. `docs/LOOSE_ENDS.md` recorded the cost of that
//! precisely: drift was already *detected*, because `piano_tools.rs` and
//! `piano_drag.rs` compute their fixtures from the engine's value, but acting
//! on the failure meant finding twenty-five literals by hand, and the failure
//! itself arrived as nineteen unrelated-looking assertion errors.
//!
//! There is now one copy, in `RollMetrics`, and this test is what makes it a
//! copy that cannot drift silently: it fails on its own, before the roll's
//! suites do, and it says which of the two numbers moved.

slint::slint! {
    import { RollMetrics } from "../ui/piano-grid.slint";

    // Nothing is drawn. The harness exists to lift one global property into
    // the generated Rust API, which is the only way to read a Slint global
    // from a test -- the same shape `rack_reorder.rs` uses for the rack's
    // geometry.
    export component RollMetricsHarness inherits Window {
        out property <int> ticks-per-step: RollMetrics.ticks-per-step;
        out property <int> steps-per-bar: RollMetrics.steps-per-bar;
        out property <int> ticks-per-bar: RollMetrics.ticks-per-bar;
    }
}

#[test]
fn the_markups_tick_count_is_the_engines() {
    i_slint_backend_testing::init_no_event_loop();
    let harness = RollMetricsHarness::new().expect("harness builds");
    let markup = harness.get_ticks_per_step();
    let engine = mooloop_core::TICKS_PER_STEP as i32;
    assert_eq!(
        markup, engine,
        "RollMetrics.ticks-per-step in piano-grid.slint is {markup}, and \
         mooloop_core::TICKS_PER_STEP is {engine}. Whichever one moved, the \
         other has to follow: the roll draws every note from the markup's \
         value and the engine schedules it from its own."
    );
}

/// The bar, which is derived in both places rather than spelled. The playlist
/// had `384` as a literal four times, including once *beside* the
/// `playlist-bar-ticks` property that two of the others then ignored.
#[test]
fn the_markups_bar_is_the_engines() {
    i_slint_backend_testing::init_no_event_loop();
    let harness = RollMetricsHarness::new().expect("harness builds");
    assert_eq!(
        harness.get_steps_per_bar(),
        mooloop_core::STEPS_PER_BAR as i32,
        "steps per bar"
    );
    assert_eq!(
        harness.get_ticks_per_bar(),
        mooloop_core::TICKS_PER_BAR as i32,
        "ticks per bar: the markup derives it from its own two numbers and the \
         engine derives it from its own, so this fails if either pair parts"
    );
}
