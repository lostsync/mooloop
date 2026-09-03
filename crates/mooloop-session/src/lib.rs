//! The live application model: session state, edit logic, undo, and the
//! commands that reach the engine.
//!
//! Nothing here knows what a window is. `mooloop-ui` owns the window, the
//! models, the callbacks, and the projection into them; this crate owns
//! everything the application would still need if that view were replaced.
//! The boundary is enforced by the absence of `slint` from `Cargo.toml`
//! rather than by convention: the session speaks `String` and `PathBuf`, and
//! the view converts.

pub mod audio_file;
pub mod browser;
pub mod dialogs;
pub mod history;
pub mod sample;

pub use browser::{browser_display_name, has_playable_descendant, is_playable_sample,
                  scan_browser_dir};
pub use dialogs::{confirm_via_zenity, pick_bundle_via_zenity, pick_export_via_zenity,
                  pick_sample_via_zenity, pick_save_via_zenity, pick_song_via_zenity};
pub use sample::{adjacent_sample, inspect_sample, load_sample_at_path, sample_description,
                 sample_duration, sample_files_in_directory, sample_index, tune_label,
                 waveform_peaks, waveform_peaks_windowed, LoadedSample, SampleInspection};
