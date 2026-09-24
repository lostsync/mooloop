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
    start_plugin_scan();
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

/// Bring the plugin cache up to date on a thread of its own (MOO-80): every
/// new or changed `.clap` on the search paths is scanned in a child process,
/// one at a time, and the cache is written when the scan ends. Unchanged
/// files, failed ones included, launch nothing, so after the first run this
/// is a directory walk.
///
/// Nothing waits for it. If mooloop quits mid-scan the thread goes with the
/// process, the cache keeps what the previous scan wrote, and a child that is
/// still running ends at its own deadline.
fn start_plugin_scan() {
    let settings = mooloop_ui::saved_plugin_settings();
    if !settings.scan_on_startup {
        log_info!("plugins", "not scanning for plugins at startup (turned off in settings)");
        return;
    }
    let timeout = std::time::Duration::from_secs(u64::from(settings.scan_timeout_s.max(1)));
    let config = match mooloop_plugin_host::scan::ScanConfig::new(settings.extra_paths, timeout) {
        Ok(config) => config,
        Err(e) => {
            log_warn!("plugins", "cannot scan for plugins: no path to this binary ({e})");
            return;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("plugin-scan".into())
        .spawn(move || {
            let path = mooloop_ui::plugin_cache_path();
            let mut cache = mooloop_plugin_host::scan::PluginCache::load(&path);
            let summary = mooloop_plugin_host::scan::scan(&config, &mut cache, |_, _, _| {});
            if summary.launched > 0 || summary.removed > 0 {
                if let Err(e) = cache.save(&path) {
                    log_warn!("plugins", "could not write {}: {e}", path.display());
                }
            }
            log_info!(
                "plugins",
                "{} plugin files: {} scanned, {} unchanged, {} gone; {} plugins, {} files failed",
                summary.candidates,
                summary.launched,
                summary.reused,
                summary.removed,
                summary.plugins,
                summary.failed
            );
        });
    if let Err(e) = spawned {
        log_warn!("plugins", "could not start the plugin scan: {e}");
    }
}
