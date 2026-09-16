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
    import { BbtText } from "../ui/controls.slint";

    // Nothing is drawn. The harness exists to lift one global property into
    // the generated Rust API, which is the only way to read a Slint global
    // from a test -- the same shape `rack_reorder.rs` uses for the rack's
    // geometry.
    export component RollMetricsHarness inherits Window {
        out property <int> ticks-per-step: RollMetrics.ticks-per-step;
        out property <int> steps-per-bar: RollMetrics.steps-per-bar;
        out property <int> ticks-per-bar: RollMetrics.ticks-per-bar;

        // Bar, beat and tick go in; the formatted string comes back out.
        // `BbtText.formatted` is an `out property` for exactly this: a
        // harness can lift a property into the generated Rust API and cannot
        // read a rendered glyph. **If this stops compiling because
        // `formatted` is gone, that is the check working** -- the format has
        // become unreadable from Rust and nothing can hold it to
        // `BbtPosition` any more.
        in-out property <int> bar <=> bbt.bar;
        in-out property <int> beat <=> bbt.beat;
        in-out property <int> tick <=> bbt.tick;
        out property <string> formatted: bbt.formatted;
        out property <string> padded: padded-bbt.formatted;

        bbt := BbtText {
            // One digit is no padding, so this reads what `BbtPosition`
            // writes. The toolbar's padded form is checked separately below.
            bar-digits: 1;
            tick-digits: 1;
        }
        padded-bbt := BbtText {
            bar: root.bar;
            beat: root.beat;
            tick: root.tick;
        }
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

/// The markup's format is the engine's.
///
/// One layer up from the two tests above, and the same subject: the padding
/// and the join lived in `PositionReadout` and the Buffer face was about to
/// write a second copy. `BbtText` is the one copy; this is what stops it
/// drifting from `BbtPosition`'s `Display`, which is what every Rust-side
/// message prints.
#[test]
fn the_markups_bbt_format_is_the_engines() {
    i_slint_backend_testing::init_no_event_loop();
    let harness = RollMetricsHarness::new().expect("harness builds");
    let ppq = mooloop_core::Ppq::DEFAULT;
    let ticks_per_beat = u64::from(ppq.ticks_per_beat());

    let mut checked = 0;
    for raw in [0_u64, 1, 95, 96, 383, 384, 385, 1_000, 40_000] {
        let ticks = mooloop_core::Ticks(raw);
        let position = mooloop_core::BbtPosition::from_ticks(ticks, ppq);
        harness.set_bar(position.bar as i32);
        harness.set_beat(position.beat as i32);
        harness.set_tick(position.tick as i32);
        assert_eq!(
            harness.get_formatted(),
            position.to_string(),
            "BbtText in controls.slint formatted tick {raw} differently from \
             BbtPosition's Display. Whichever side changed, the other has to \
             follow: the transport readout draws the markup's string and every \
             Rust-side message prints the type's."
        );
        checked += 1;
    }
    assert_eq!(checked, 9, "the sweep stopped covering anything");

    // A duration is the same format and different numbers, which is the point
    // of there being two types rather than one with a flag.
    let one_bar = mooloop_core::Ticks(ticks_per_beat * u64::from(mooloop_core::BEATS_PER_BAR));
    let length = mooloop_core::BbtDuration::from_ticks(one_bar, ppq);
    harness.set_bar(length.bars as i32);
    harness.set_beat(length.beats as i32);
    harness.set_tick(length.ticks as i32);
    assert_eq!(harness.get_formatted(), length.to_string());
    assert_eq!(harness.get_formatted(), "1:0:0", "a one-bar length is 1:0:0");
}

/// The transport's own width. `001:1:000` is what the toolbar has always
/// shown, and moving the format into `controls.slint` must not have changed
/// it -- nothing on screen changes when `musical-time/` lands.
#[test]
fn the_transports_readout_is_still_padded_to_three_digits() {
    i_slint_backend_testing::init_no_event_loop();
    let harness = RollMetricsHarness::new().expect("harness builds");
    harness.set_bar(1);
    harness.set_beat(1);
    harness.set_tick(0);
    assert_eq!(harness.get_padded(), "001:1:000");
    harness.set_bar(42);
    harness.set_tick(7);
    assert_eq!(harness.get_padded(), "042:1:007");
    harness.set_bar(137);
    harness.set_tick(95);
    assert_eq!(harness.get_padded(), "137:1:095");
}
