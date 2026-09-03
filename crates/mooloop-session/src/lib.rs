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
pub mod automation;
pub mod browser;
pub mod channel;
pub mod command;
pub mod dialogs;
pub mod document;
pub mod effects;
pub mod history;
pub mod mixer;
pub mod modulation;
pub mod notes;
pub mod project;
pub mod rack;
pub mod roll;
pub mod sample;
pub mod session;
pub mod steps;
pub mod transport;
pub mod values;
