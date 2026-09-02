//! The UI mockup tool, standalone. Everything it does lives in
//! `mooloop_ui::mockup`, which the in-app Developer entry calls too, so this
//! is a window, the wiring, and the snapshot mode the tests use.

use mooloop_ui::{load_mockup_layout, wire_mockup, MockupCanvas};
use slint::ComponentHandle;
use std::path::Path;

fn main() -> Result<(), slint::PlatformError> {
    let canvas = MockupCanvas::new()?;
    wire_mockup(&canvas);

    if let Some(path) = std::env::var_os("MOOLOOP_MOCKUP_LAYOUT") {
        if let Err(error) = load_mockup_layout(&canvas, Path::new(&path)) {
            eprintln!("could not load {}: {error}", Path::new(&path).display());
        }
    }

    // The properties panel and the layers list only exist once something is
    // selected, so a snapshot of either needs to say what.
    if let Ok(index) = std::env::var("MOOLOOP_MOCKUP_SELECT") {
        if let Ok(index) = index.trim().parse() {
            canvas.set_selected_index(index);
        }
    }
    if let Ok(tab) = std::env::var("MOOLOOP_MOCKUP_TAB") {
        canvas.set_sidebar_tab(i32::from(tab.trim() == "layers"));
    }

    let Some(snapshot_path) = std::env::var_os("MOOLOOP_MOCKUP_SNAPSHOT") else {
        return canvas.run();
    };

    // The canvas is wider than a default window, so let the capture size be
    // overridden (MOOLOOP_MOCKUP_SIZE=1400x900).
    let (width, height) = std::env::var("MOOLOOP_MOCKUP_SIZE")
        .ok()
        .and_then(|spec| {
            let (w, h) = spec.split_once('x')?;
            Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
        })
        .unwrap_or((1280.0, 800.0));
    canvas
        .window()
        .set_size(slint::LogicalSize::new(width, height));
    canvas.show()?;
    let snapshot = canvas.window().take_snapshot()?;
    let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
    for pixel in snapshot.as_bytes().as_chunks::<4>().0 {
        ppm.extend_from_slice(&pixel[..3]);
    }
    std::fs::write(snapshot_path, ppm).expect("failed to write mockup snapshot");
    Ok(())
}
