slint::include_modules!();

use slint::ComponentHandle;

fn write_snapshot(
    concepts: &DeviceRackConcepts,
    path: &std::path::Path,
) -> Result<(), slint::PlatformError> {
    let snapshot = concepts.window().take_snapshot()?;
    let mut ppm = format!("P6\n{} {}\n255\n", snapshot.width(), snapshot.height()).into_bytes();
    for pixel in snapshot.as_bytes().chunks_exact(4) {
        ppm.extend_from_slice(&pixel[..3]);
    }
    std::fs::write(path, ppm).expect("failed to write device rack snapshot");
    Ok(())
}

fn main() -> Result<(), slint::PlatformError> {
    let concepts = DeviceRackConcepts::new()?;
    if let Some(prefix) = std::env::var_os("MOOLOOP_DEVICE_CONCEPT_SNAPSHOT") {
        concepts
            .window()
            .set_size(slint::LogicalSize::new(1200.0, 620.0));
        concepts.show()?;
        concepts.set_concept_index(0);
        write_snapshot(
            &concepts,
            &std::path::PathBuf::from(format!("{}-horizontal.ppm", prefix.to_string_lossy())),
        )?;
        concepts.set_concept_index(1);
        write_snapshot(
            &concepts,
            &std::path::PathBuf::from(format!("{}-vertical.ppm", prefix.to_string_lossy())),
        )?;
        Ok(())
    } else {
        concepts.run()
    }
}
