slint::include_modules!();

use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    let gallery = ControlGallery::new()?;
    if let Some(path) = std::env::var_os("MOOLOOP_GALLERY_SNAPSHOT") {
        if std::env::var_os("MOOLOOP_GALLERY_DIALOG").is_some() {
            gallery.set_gallery_dialog_open(true);
        }
        gallery
            .window()
            .set_size(slint::LogicalSize::new(980.0, 720.0));
        gallery.show()?;
        let snapshot = gallery.window().take_snapshot()?;
        let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
        for pixel in snapshot.as_bytes().chunks_exact(4) {
            ppm.extend_from_slice(&pixel[..3]);
        }
        std::fs::write(path, ppm).expect("failed to write gallery snapshot");
        Ok(())
    } else {
        gallery.run()
    }
}
