//! mooloop — entry point. Boots the audio engine, builds the UI, runs the
//! Slint event loop.

use mooloop_core::{log_error, log_info, log_warn};
use mooloop_engine::{AudioState, CommandSink};

fn main() {
    // Before anything else, logging included: `mooloop --scan-plugin <path>`
    // is the plugin scanner's child (MOO-80), and it must load that one file
    // and nothing more -- no log file, no settings, no audio client, no
    // window. A plugin that crashes or hangs then takes down only this
    // process, never the mooloop that launched it.
    if let Some(status) = mooloop_plugin_host::scan::run_child_from_args() {
        std::process::exit(status);
    }
    // First, so that everything below is on the record.
    mooloop_ui::start_logging();
    // Before any window, and before the audio engine starts any thread:
    // winit fixes the display backend once per process, and the "Run under
    // XWayland" setting has to reach it first (MOO-301). It chooses the
    // display server only; audio and MIDI never see it. Nothing above it may
    // start a thread: under the setting it may set winit's X11 scale
    // variable, and does only while the process has one (MOO-343).
    let display = mooloop_ui::select_display_backend();
    match display.expected {
        Some(backend) => log_info!("display", "display backend: {backend}"),
        None => log_warn!("display", "no display server named in the environment"),
    }
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
    // A song to open, as the desktop file's `%F` passes one (MOO-141). Only
    // the first: mooloop has one song open at a time.
    let song = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    // Opened on the saved output and buffer size, not the defaults: startup
    // is where a saved output that has gone falls back to one that works.
    // With no JACK -- no server, or no libjack -- the engine runs on no
    // device and the window opens anyway, saying why (MOO-115); it used to
    // exit here before any window existed.
    let mut handle = mooloop_engine::EngineHandle::open(mooloop_ui::saved_audio_config());
    match handle.audio_state() {
        AudioState::NoDevice(why) => {
            log_warn!("audio", "engine started with no audio device ({why})");
        }
        _ => log_info!("audio", "engine started at {} Hz", handle.sample_rate()),
    }
    // The handle owns the audio driver and moves into the interface, which
    // drops it -- stopping the driver -- when `app` goes at the end of `run`.
    let app = mooloop_ui::AppUi::new(handle)?;
    // The scanner looks for new or changed plugin files on a thread of its
    // own (MOO-80); the window can start the same scan again from
    // Preferences > Plugins (MOO-229), which is why it lives in the UI crate.
    mooloop_ui::start_startup_plugin_scan();
    if let Some(song) = song {
        app.open_song_at_start(song);
    }
    let ran = app.run();
    // After the loop and before the engine drops: a take still recording is
    // not an unsaved *edit*, so nothing on the quit path has dealt with it,
    // and its WAV header counts only up to its last one-second checkpoint
    // until its drain finishes. Whether or not the loop ended in an error --
    // a `?` on `run` skipped this, and lost the take along with the event
    // loop.
    app.finish_takes();
    ran?;
    Ok(())
}
