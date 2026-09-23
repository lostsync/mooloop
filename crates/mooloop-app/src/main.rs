//! mooloop — entry point. Boots the audio engine, builds the UI, runs the
//! Slint event loop.

use mooloop_core::{log_error, log_info, log_warn};
use mooloop_engine::CommandSink;

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
    // Opened on the saved output and buffer size, not the defaults: startup
    // is where a saved output that has gone falls back to one that works.
    let (engine, handle) = mooloop_engine::Engine::new(mooloop_ui::saved_audio_config())?;
    log_info!("audio", "engine started at {} Hz", handle.sample_rate());
    let app = mooloop_ui::AppUi::new(handle)?;
    // `engine` stays alive on the stack for the duration of the event loop and
    // is dropped (stopping the audio driver) when `run` returns.
    let _ = &engine;
    let ran = app.run();
    // After the loop and before `engine` drops: a take still recording is not
    // an unsaved *edit*, so nothing on the quit path has dealt with it, and
    // its WAV header counts only up to its last one-second checkpoint until
    // its drain finishes.
    // Whether or not the loop ended in an error -- a `?` on `run` skipped
    // this, and lost the take along with the event loop.
    app.finish_takes();
    ran?;
    Ok(())
}
