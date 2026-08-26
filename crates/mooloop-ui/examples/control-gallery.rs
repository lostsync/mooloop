slint::include_modules!();

use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    let gallery = ControlGallery::new()?;
    if let Some(path) = std::env::var_os("MOOLOOP_GALLERY_SNAPSHOT") {
        if std::env::var_os("MOOLOOP_GALLERY_DIALOG").is_some() {
            gallery.set_gallery_dialog_open(true);
        }
        // The gallery is taller than a window, so allow the snapshot size to be
        // overridden (MOOLOOP_GALLERY_SIZE=980x1620) to capture it in one pass.
        let (width, height) = std::env::var("MOOLOOP_GALLERY_SIZE")
            .ok()
            .and_then(|spec| {
                let (w, h) = spec.split_once('x')?;
                Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
            })
            .unwrap_or((980.0, 720.0));
        gallery
            .window()
            .set_size(slint::LogicalSize::new(width, height));
        gallery.show()?;
        let snapshot = gallery.window().take_snapshot()?;
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for pixel in snapshot.as_bytes().as_chunks::<4>().0 {
            ppm.extend_from_slice(&pixel[..3]);
        }
        std::fs::write(path, ppm).expect("failed to write gallery snapshot");
        Ok(())
    } else {
        gallery.run()
    }
}
