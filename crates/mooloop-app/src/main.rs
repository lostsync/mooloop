//! mooloop — entry point. Boots the audio engine, builds the UI, runs the
//! Slint event loop.

use mooloop_core::{log_error, log_info, log_warn};

fn main() {
    // First, so that everything below is on the record. Reads the saved
    // preference for whether to also write a log file.
    mooloop_ui::start_logging();
    if let Err(e) = run() {
        log_error!("app", "{e}");
        std::process::exit(1);
    }
    log_info!("app", "exited cleanly");
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(debug_assertions)]
    log_warn!(
        "app",
        "running a development build; use `cargo run --release -p mooloop-app --bin mooloop` for reliable realtime audio"
    );
    let (engine, handle) = mooloop_engine::Engine::new(mooloop_engine::AudioConfig::default())?;
    log_info!("audio", "engine started at {} Hz", handle.sample_rate());
    let app = mooloop_ui::AppUi::new(handle)?;
    // `engine` stays alive on the stack for the duration of the event loop and
    // is dropped (deactivating JACK) when `run` returns.
    let _ = &engine;
    app.run()?;
    Ok(())
}
