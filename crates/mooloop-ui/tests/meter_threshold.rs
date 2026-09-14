//! The meter colour thresholds are a rendered property, so they are
//! verified with rendered pixels: three meter types at levels either side
//! of the warning and hot points from `mooloop_core::gain`. The theme's
//! default meter colours are literals in theme.slint; the reference RGBs
//! below track them.

use slint::ComponentHandle;
use slint::LogicalSize;

mod common;

slint::slint! {
    import { Theme } from "../ui/theme.slint";
    import { AudioOrientation } from "../ui/controls.slint";
    import { SegmentedMeter, ChannelMeter, MasterMeter } from "../ui/meters.slint";

    export component MeterHarness inherits Window {
        width: 150px;
        height: 200px;
        in-out property <float> level-db: -60;
        background: Theme.background;

        HorizontalLayout {
            padding: 12px;
            spacing: 14px;
            alignment: center;

            SegmentedMeter {
                width: 26px;
                height: 160px;
                segments: 50;
                level-db: root.level-db;
                held-db: root.level-db;
            }
            ChannelMeter {
                width: 26px;
                height: 160px;
                segments: 50;
                left-db: root.level-db;
                right-db: root.level-db;
                held-left-db: root.level-db;
                held-right-db: root.level-db;
            }
            MasterMeter {
                width: 40px;
                height: 160px;
                segments: 50;
                orientation: AudioOrientation.vertical;
                left-db: root.level-db;
                right-db: root.level-db;
                held-left-db: root.level-db;
                held-right-db: root.level-db;
            }
        }
    }
}

/// Initialize the software renderer, the only backend with `take_snapshot`.
fn init_software_backend() {
    common::install_testing_backend();
}

/// Pixels within a small distance of the theme's meter colours
/// (theme.slint literals: safe #22c55e, warning #eab308, clip #ef4444).
const WARNING: [u8; 3] = [0xea, 0xb3, 0x08];
const CLIP: [u8; 3] = [0xef, 0x44, 0x44];

fn count_color(
    snapshot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    target: [u8; 3],
) -> usize {
    snapshot
        .as_bytes()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|rgba| {
            let (r, g, b) = (rgba[0], rgba[1], rgba[2]);
            let dr = i32::from(r) - i32::from(target[0]);
            let dg = i32::from(g) - i32::from(target[1]);
            let db = i32::from(b) - i32::from(target[2]);
            dr * dr + dg * dg + db * db < 200
        })
        .count()
}

#[test]
fn warning_and_hot_colours_transition_at_the_standard_thresholds() {
    init_software_backend();
    let ui = MeterHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(150.0, 200.0));

    // 50 segments over 60 dB is 1.2 dB per segment, fine enough that the
    // transition is visible at the exact threshold rather than quantized
    // away from it.
    ui.set_level_db(-10.5);
    let snapshot = ui.window().take_snapshot().unwrap();
    assert_eq!(
        count_color(&snapshot, WARNING),
        0,
        "-10.5 dBFS must be entirely green (warning starts at -10)"
    );
    assert_eq!(count_color(&snapshot, CLIP), 0);

    ui.set_level_db(-9.5);
    let snapshot = ui.window().take_snapshot().unwrap();
    assert!(
        count_color(&snapshot, WARNING) > 0,
        "-9.5 dBFS must light yellow (warning starts at -10)"
    );
    assert_eq!(
        count_color(&snapshot, CLIP),
        0,
        "-9.5 dBFS must not light red (hot starts at -3)"
    );

    ui.set_level_db(-2.5);
    let snapshot = ui.window().take_snapshot().unwrap();
    assert!(
        count_color(&snapshot, CLIP) > 0,
        "-2.5 dBFS must light red (hot starts at -3)"
    );
}

/// Every `ChannelMeter` in the interface either has a clip latch behind it or
/// says it has not.
///
/// `ChannelMeter` draws a `ClipIndicator` whose lamp comes from `clipping` and
/// whose click goes to `clip-reset`. A caller that binds neither gets two
/// faint red bars that cannot light and do nothing when pressed -- the
/// "convincing but inert control" the device rack's own rule names, shipped on
/// three instantiations at once: the rack's IN and OUT rails, which meter a
/// chain and have no latch, and a track's fader row, which has one and was not
/// wired to it.
///
/// So a meter must do one of two things, and both are deliberate acts:
/// bind `clipping`, or set `show-clip: false`. The failure this catches is the
/// one that happened -- somebody adds a meter, does not think about the lamp,
/// and the default draws one anyway.
///
/// Read out of the markup rather than off a render, because a lamp that cannot
/// light looks exactly like a lamp that is not lit.
#[test]
fn no_channel_meter_draws_a_clip_lamp_it_cannot_light() {
    let mut checked = 0usize;
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/ui"))
        .expect("mooloop-ui/ui is unreadable")
    {
        let path = entry.expect("unreadable directory entry").path();
        if path.extension().is_none_or(|kind| kind != "slint") {
            continue;
        }
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        // `meters.slint` declares the component; it does not instantiate one.
        //
        // `mockup-catalog.slint` is a gallery of specimens and every control
        // in it is inert on purpose -- it exists to show what a widget looks
        // like, and it is behind the `mockup` feature, so it is not part of
        // the shipped interface at all. It is excluded by name rather than by
        // a pattern, because "inert on purpose" is a claim about one file and
        // not a category a later file should be able to join by accident.
        if file == "meters.slint" || file == "mockup-catalog.slint" {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("unreadable .slint file");

        let mut from = 0usize;
        while let Some(found) = source[from..].find("ChannelMeter {") {
            let start = from + found;
            let open = source[start..].find('{').expect("the brace just matched") + start;
            let mut depth = 0i32;
            let mut end = open;
            for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
                match byte {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            from = end;
            let block = &source[open..=end];
            let line = source[..start].matches('\n').count() + 1;
            checked += 1;
            assert!(
                block.contains("clipping:") || block.contains("show-clip: false"),
                "{file}:{line}: a ChannelMeter that neither binds `clipping` nor sets \
                 `show-clip: false` draws a clip lamp nothing can light:\n{block}"
            );
            if block.contains("clipping:") {
                assert!(
                    block.contains("clip-reset"),
                    "{file}:{line}: this ChannelMeter's lamp can light and cannot be \
                     cleared, which is worse than not drawing one:\n{block}"
                );
            }
        }
    }

    // Four today: the mixer strip, the two rack rails, a track's fader row.
    // A parser that stops matching finds none and would otherwise pass.
    assert!(
        checked >= 4,
        "only {checked} ChannelMeter instantiations were found; the walk has stopped \
         matching the markup it is meant to read"
    );
}
