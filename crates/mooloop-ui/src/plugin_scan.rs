//! Looking for plugins: the scan the binary starts at launch, and the same
//! scan started again from Preferences → Plugins (MOO-229, plugin-hosting 08).
//!
//! **Platform & Release's file** (`docs/TEAMS.md`): the startup scan policy
//! moved here, unchanged, from `mooloop-app/src/main.rs`, because the binary
//! cannot be called from the window and a rescan has to be the same scan,
//! not a copy of it. The Plugins page (Interface's, `plugin_ui.rs`) calls in.
//!
//! The scanner itself is the host's (`mooloop_plugin_host::scan`): it finds
//! the candidate files, launches one child process per new or changed file,
//! and keeps what each yielded in the cache file. What is here is the thread
//! it runs on -- never the UI thread -- the one-at-a-time rule, and what the
//! window reads of its progress.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use mooloop_core::{log_info, log_warn};
use mooloop_plugin_host::scan::{PluginCache, ScanConfig};

use crate::settings::PluginSettings;

/// Whether a scan is running in this process. Two would both write
/// `plugins.toml`, and whichever finished second would drop what the first
/// found; so a second is refused, never started beside the first.
static SCANNING: AtomicBool = AtomicBool::new(false);

/// The right to run the one scan, held by the scan's thread and given back
/// when it is dropped, however the thread ends.
pub(crate) struct ScanClaim(());

impl ScanClaim {
    /// The claim, or `None` while a scan holds it.
    pub(crate) fn take() -> Option<Self> {
        (!SCANNING.swap(true, Ordering::AcqRel)).then_some(Self(()))
    }
}

impl Drop for ScanClaim {
    fn drop(&mut self) {
        SCANNING.store(false, Ordering::Release);
    }
}

/// What a scan started from the window says about itself, written by its
/// thread and read by the pump.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ScanState {
    /// No scan has been asked for from the window this session.
    #[default]
    Idle,
    /// On the `at`th of `of` candidate files, named `file`.
    Scanning { at: usize, of: usize, file: String },
    /// Finished: the plugins and the failed files the cache now holds.
    Done { plugins: usize, failed: usize },
    /// Not started, and why.
    Refused(String),
}

/// Progress shared between a scan's thread and the window.
pub(crate) type ScanProgress = Arc<Mutex<ScanState>>;

fn publish(progress: Option<&ScanProgress>, state: ScanState) {
    if let Some(progress) = progress {
        *progress.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = state;
    }
}

/// The scan for `settings`: the folders CLAP names, then the user's, each
/// file given `scan_timeout_s` (at least a second), launching this binary.
pub(crate) fn scan_config(settings: &PluginSettings) -> std::io::Result<ScanConfig> {
    let timeout = std::time::Duration::from_secs(u64::from(settings.scan_timeout_s.max(1)));
    ScanConfig::new(settings.extra_paths.clone(), timeout)
}

/// Run `config` on a thread of its own against the cache file at
/// `cache_path`, reporting to `progress`. `forget_failures` is Rescan All:
/// the files that failed before are scanned again rather than remembered.
/// `None`, having started nothing, when another scan is running.
pub(crate) fn spawn_scan(
    config: ScanConfig,
    cache_path: PathBuf,
    forget_failures: bool,
    progress: Option<ScanProgress>,
) -> Option<JoinHandle<()>> {
    let Some(claim) = ScanClaim::take() else {
        publish(progress.as_ref(), ScanState::Refused("a scan is already running".into()));
        return None;
    };
    let thread_progress = progress.clone();
    let spawned = std::thread::Builder::new()
        .name("plugin-scan".into())
        .spawn(move || {
            let _claim = claim;
            let progress = thread_progress.as_ref();
            let mut cache = PluginCache::load(&cache_path);
            let forgot = forget_failures && cache.failures().next().is_some();
            if forget_failures {
                cache.clear_failures();
            }
            let summary = mooloop_plugin_host::scan::scan(&config, &mut cache, |at, of, file| {
                if progress.is_some() {
                    let file = file
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    publish(progress, ScanState::Scanning { at: at + 1, of, file });
                }
            });
            if summary.launched > 0 || summary.removed > 0 || forgot {
                if let Err(e) = cache.save(&cache_path) {
                    log_warn!("plugins", "could not write {}: {e}", cache_path.display());
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
            publish(
                progress,
                ScanState::Done {
                    plugins: summary.plugins,
                    failed: summary.failed,
                },
            );
        });
    match spawned {
        Ok(handle) => Some(handle),
        Err(e) => {
            log_warn!("plugins", "could not start the plugin scan: {e}");
            publish(progress.as_ref(), ScanState::Refused(format!("could not start the scan: {e}")));
            None
        }
    }
}

/// Scan with `settings` against the app's cache file, from the window.
/// `false` when nothing was started (another scan is running, or this
/// binary has no path to launch), with the reason in `progress`.
pub(crate) fn start_scan(settings: &PluginSettings, forget_failures: bool, progress: ScanProgress) -> bool {
    let config = match scan_config(settings) {
        Ok(config) => config,
        Err(e) => {
            log_warn!("plugins", "cannot scan for plugins: no path to this binary ({e})");
            publish(Some(&progress), ScanState::Refused(format!("no path to this binary ({e})")));
            return false;
        }
    };
    spawn_scan(config, crate::plugin_cache_path(), forget_failures, Some(progress)).is_some()
}

/// Bring the plugin cache up to date on a thread of its own (MOO-80), unless
/// the user turned the startup scan off: every new or changed `.clap` on the
/// search paths is scanned in a child process, one at a time, and the cache
/// is written when the scan ends. Unchanged files, failed ones included,
/// launch nothing, so after the first run this is a directory walk.
///
/// Nothing waits for it. If mooloop quits mid-scan the thread goes with the
/// process, the cache keeps what the previous scan wrote, and a child that is
/// still running ends at its own deadline. Moved here unchanged from
/// `mooloop-app/src/main.rs` (MOO-229).
pub fn start_startup_plugin_scan() {
    let settings = crate::saved_plugin_settings();
    if !settings.scan_on_startup {
        log_info!("plugins", "not scanning for plugins at startup (turned off in settings)");
        return;
    }
    let config = match scan_config(&settings) {
        Ok(config) => config,
        Err(e) => {
            log_warn!("plugins", "cannot scan for plugins: no path to this binary ({e})");
            return;
        }
    };
    spawn_scan(config, crate::plugin_cache_path(), false, None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_plugin_host::scan::ChildCommand;

    /// **One scan at a time.** While a scan holds the claim, a second is
    /// refused and says so, and starts no thread and no child; once the
    /// first is done, the next one runs.
    #[test]
    fn a_second_scan_while_one_runs_is_refused() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let config = ScanConfig {
            search_paths: vec![dir.path().to_path_buf()],
            timeout: std::time::Duration::from_secs(1),
            child: ChildCommand {
                program: PathBuf::from("/nonexistent/scan-child"),
                args: Vec::new(),
            },
        };
        let cache = dir.path().join("plugins.toml");
        let progress = ScanProgress::default();

        let running = ScanClaim::take().expect("no scan is running yet");
        assert!(spawn_scan(config.clone(), cache.clone(), true, Some(progress.clone())).is_none());
        assert!(matches!(&*progress.lock().unwrap(), ScanState::Refused(why) if why.contains("already")));
        drop(running);

        let handle = spawn_scan(config, cache, true, Some(progress.clone())).expect("the scan starts");
        handle.join().expect("the scan thread did not panic");
        assert_eq!(*progress.lock().unwrap(), ScanState::Done { plugins: 0, failed: 0 });
        assert!(ScanClaim::take().is_some(), "a finished scan gives the claim back");
    }
}
