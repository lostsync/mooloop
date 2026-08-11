//! mooloop — entry point. Boots the audio engine, builds the UI, runs the
//! Slint event loop.

fn main() {
    if let Err(e) = run() {
        eprintln!("mooloop: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (engine, handle) = mooloop_engine::Engine::new()?;
    let app = mooloop_ui::AppUi::new(handle)?;
    // `engine` stays alive on the stack for the duration of the event loop and
    // is dropped (deactivating JACK) when `run` returns.
    let _ = &engine;
    app.run()?;
    Ok(())
}
