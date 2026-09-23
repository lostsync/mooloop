//! The Rust side of the status bar: the held notice and the audio readout.
//!
//! The bar had one line, `status-message`, in one muted colour, and the next
//! ordinary event -- a device selected, a note copied -- overwrote whatever it
//! said. So a failure written there was on screen for as long as it took the
//! user to click anything, and most failures outside the document path were
//! not written there at all: a sample that would not decode, a load onto a
//! channel that is not a sampler, and every dropout went only to the log
//! (MOO-132).
//!
//! [`notify`] is the one door. An [`Severity::Info`] message is what
//! `status-message` always was. A warning or an error is *held*: it has its
//! own segment and colour, and it stays until it is clicked away or a notice
//! of at least its weight replaces it. The rule for that is here rather than
//! in the markup so there is one copy of it, and so a caller cannot write the
//! two properties by hand and get it wrong.

use crate::{MainWindow, NoticeLevel};
use mooloop_engine::load::{LoadSnapshot, RealtimeStatus};

/// How much a status-bar message weighs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The next event may overwrite it: "Device selected", "Copied 3 notes".
    Info,
    /// Held until dismissed or replaced by another warning or an error.
    Warning,
    /// Held until dismissed or replaced by a newer error. A warning does not
    /// displace it, because the error is the thing the user has not read yet.
    Error,
}

/// Say `text` in the status bar at `severity`.
///
/// This does not log. Most failures worth showing are worth logging with
/// more detail than a status line has room for, and the caller is the one
/// that has the detail.
pub fn notify(window: &MainWindow, severity: Severity, text: &str) {
    let level = match severity {
        Severity::Info => {
            window.set_status_message(text.into());
            return;
        }
        Severity::Warning => NoticeLevel::Warning,
        Severity::Error => NoticeLevel::Error,
    };
    let held = !window.get_status_notice().is_empty();
    if held
        && window.get_status_notice_level() == NoticeLevel::Error
        && level == NoticeLevel::Warning
    {
        return;
    }
    window.set_status_notice(text.into());
    window.set_status_notice_level(level);
}

/// Take a held notice down, but only if it still says `text`.
///
/// For a condition that ends by itself -- the audio coming back, a backlog
/// draining -- where the notice that reported it may since have been replaced
/// by something the user has not read. `StatusHint.clear` in `theme.slint` has
/// the same shape for the same reason.
pub fn withdraw(window: &MainWindow, text: &str) {
    if window.get_status_notice() == text {
        window.set_status_notice("".into());
    }
}

/// Publish one second of the audio callback's health to the readout.
///
/// `xruns` is what the driver reported over the same window. A dropout is
/// counted as the larger of that and the blocks the engine itself saw go
/// wrong ([`LoadSnapshot::had_trouble`]'s two counts), not their sum: an
/// over-budget block is usually the very xrun the driver reports, and
/// counting it twice would make the number mean less. Added to what the
/// readout shows rather than to a running total here, so clicking the
/// readout back to zero sticks.
pub fn show_audio_load(window: &MainWindow, load: &LoadSnapshot, xruns: u32) {
    if load.blocks == 0 {
        // Nothing rendered: the other fields are meaningless, and a number
        // left over from the last live window would say the audio is fine.
        window.set_audio_load(-1.0);
        window.set_audio_load_peak(0.0);
        return;
    }
    window.set_audio_load(load.mean_load);
    window.set_audio_load_peak(load.peak_load);
    let heard = load.over_budget.saturating_add(load.late_wakeups);
    let dropouts = i32::try_from(xruns.max(heard)).unwrap_or(i32::MAX);
    if dropouts > 0 {
        window.set_audio_dropouts(window.get_audio_dropouts().saturating_add(dropouts));
    }
    window.set_audio_time_shared(load.realtime == RealtimeStatus::TimeShared);
}
