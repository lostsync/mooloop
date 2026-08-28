//! The meter colour thresholds are a rendered property, so they are
//! verified with rendered pixels: three meter types at levels either side
//! of the warning and hot points from `mooloop_core::gain`. The theme's
//! default meter colours are literals in theme.slint; the reference RGBs
//! below track them.

use slint::ComponentHandle;
use slint::LogicalSize;

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
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some(slint::SharedString::from("software")),
        },
    )))
    .ok();
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
