//! Slint's fader taper against Rust's, by running both.
//!
//! `gain_slint_agreement.rs` checks that `GainMath`'s breakpoint lists match
//! `mooloop_core::gain::FADER_BREAKPOINTS`, and until 2026-09-12 that was the
//! only thing holding the two tapers together -- while the functions the faders
//! actually call spelled all seven breakpoints inline and read those lists not
//! at all. So the test could pass on a list nothing consulted: change the Rust
//! table, watch it fail, edit the lists, watch it pass, and the taper a fader
//! runs would be exactly as it was.
//!
//! The lists are load-bearing now. This is the other half, and the half that
//! would have caught the mistake either version could make: the functions are
//! unrolled per breakpoint, so a transposed index changes the curve between two
//! of them and nothing above would notice. Run both implementations over the
//! whole throw and compare.

slint::slint! {
    import { GainMath } from "../ui/gain.slint";

    // Nothing is drawn. The harness lifts the taper into properties, which is
    // how `rack_reorder.rs` and `roll_metrics.rs` reach a global: importing one
    // does not give it a Rust API, because Slint generates that only for
    // globals the *main* document exports. Bindings are reactive, so setting an
    // input and reading an output runs the function under test.
    export component TaperHarness inherits Window {
        in property <float> travel;
        in property <float> db;
        in property <float> gain;
        out property <float> db-for-travel: GainMath.fader-position-to-db(root.travel);
        out property <float> travel-for-db: GainMath.fader-db-to-position(root.db);
        out property <float> travel-for-gain:
            GainMath.fader-gain-to-position(root.gain);
        out property <float> round-tripped:
            GainMath.fader-db-to-position(GainMath.fader-position-to-db(root.travel));
    }
}

/// Slint spells negative infinity as this, because it has no infinity literal.
const SLINT_SILENCE: f32 = -99_999.0;

#[test]
fn the_slint_taper_is_the_rust_taper_across_the_whole_throw() {
    i_slint_backend_testing::init_no_event_loop();
    let harness = TaperHarness::new().expect("harness builds");

    for step in 0..=200 {
        let travel = step as f32 / 200.0;
        harness.set_travel(travel);
        let slint_db = harness.get_db_for_travel();
        let rust_db = mooloop_core::gain::fader_position_to_db(travel);
        if rust_db.is_infinite() {
            assert!(
                slint_db <= SLINT_SILENCE + 1.0,
                "travel {travel}: rust says silence, slint says {slint_db}"
            );
            continue;
        }
        assert!(
            (slint_db - rust_db).abs() < 1e-3,
            "travel {travel}: slint {slint_db} dB, rust {rust_db} dB"
        );
    }
}

/// And back, over the dB range a fader can reach. Only the finite part: Rust
/// floors everything at or below the bottom breakpoint, and the two spell that
/// floor differently on purpose.
#[test]
fn the_slint_inverse_is_the_rust_inverse() {
    i_slint_backend_testing::init_no_event_loop();
    let harness = TaperHarness::new().expect("harness builds");

    let mut db = mooloop_core::gain::MIN_DB;
    while db <= 6.0 {
        harness.set_db(db);
        let slint_travel = harness.get_travel_for_db();
        let rust_travel = mooloop_core::gain::fader_db_to_position(db);
        assert!(
            (slint_travel - rust_travel).abs() < 1e-3,
            "{db} dB: slint travel {slint_travel}, rust {rust_travel}"
        );
        db += 0.25;
    }
}

/// The round trip, which is what a fader does: drag to a position, read the dB
/// out, and the caption has to name the place the handle is.
#[test]
fn a_position_survives_the_trip_through_db_and_back() {
    i_slint_backend_testing::init_no_event_loop();
    let harness = TaperHarness::new().expect("harness builds");

    // From the bottom breakpoint up: below it every travel means the same
    // silence, so the trip back cannot distinguish them.
    let mut travel = 0.05;
    while travel <= 1.0 {
        harness.set_travel(travel);
        let back = harness.get_round_tripped();
        assert!(
            (back - travel).abs() < 1e-3,
            "travel {travel} round-tripped to {back}"
        );
        travel += 0.005;
    }
}

/// What a fader and a send bar draw from a stored gain. The markup composes
/// `linear-to-db` with the taper, and `linear-to-db` floors silence at -60 dB,
/// so a gain of 0 used to land on the 5% breakpoint instead of the bottom of
/// the throw (MOO-399). The tests above cover the two taper halves, never
/// this composition.
#[test]
fn a_gain_draws_at_the_travel_rust_gives_it_including_silence() {
    i_slint_backend_testing::init_no_event_loop();
    let harness = TaperHarness::new().expect("harness builds");

    let gains = [
        0.0,
        1e-4,
        0.001,
        0.01,
        0.1,
        0.5,
        1.0,
        mooloop_core::gain::FADER_MAX_GAIN,
    ];
    for gain in gains {
        harness.set_gain(gain);
        let slint_travel = harness.get_travel_for_gain();
        let rust_travel = mooloop_core::gain::fader_gain_to_position(gain);
        assert!(
            (slint_travel - rust_travel).abs() < 1e-3,
            "gain {gain}: slint travel {slint_travel}, rust {rust_travel}"
        );
    }
}
