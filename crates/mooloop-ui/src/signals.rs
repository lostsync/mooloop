//! SIGTERM, SIGINT and SIGHUP, routed to the quit path.
//!
//! A logout, `kill`, a closed terminal or Ctrl+C used to end the process
//! where it stood: no line in the log saying why, and no quit path, so a take
//! still recording was left with a header that says it holds nothing (P5 in
//! `reports/teams-2026-09-22.md`). Now the handler only records which signal
//! arrived -- a handler may do almost nothing else -- and the pump, which
//! reads [`take`] on every tick, leaves the event loop the way Quit does.
//! `main` then finishes the takes and logs the exit.
//!
//! Each handler is installed once and resets itself when it runs
//! (`SA_RESETHAND`), so a second Ctrl+C, or the session manager's follow-up
//! signal, ends the process at once rather than waiting on a quit that is
//! stuck.

use std::sync::atomic::{AtomicI32, Ordering};

/// The signal that arrived, or zero. Written by the handler, read by the pump.
static RECEIVED: AtomicI32 = AtomicI32::new(0);

/// The signals that mean "leave now", which the quit path answers.
#[cfg(unix)]
const QUIT_SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

#[cfg(unix)]
extern "C" fn on_quit_signal(signal: libc::c_int) {
    // An atomic store is async-signal-safe; logging, allocating or taking a
    // lock here is not.
    RECEIVED.store(signal, Ordering::Relaxed);
}

/// Route the quit signals to [`take`]. Call once, before the event loop.
pub(crate) fn install() {
    #[cfg(unix)]
    for signal in QUIT_SIGNALS {
        // SAFETY: `sigaction` is given a zeroed struct with its handler, mask
        // and flags filled in, and the handler only stores to an atomic.
        let installed = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_quit_signal as extern "C" fn(libc::c_int) as usize;
            action.sa_flags = libc::SA_RESTART | libc::SA_RESETHAND;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut()) == 0
        };
        if !installed {
            mooloop_core::log_warn!(
                "app",
                "could not route {} to the quit path; it will end mooloop without one",
                name(signal)
            );
        }
    }
}

/// The quit signal that has arrived since the last call, if any.
pub(crate) fn take() -> Option<&'static str> {
    match RECEIVED.swap(0, Ordering::Relaxed) {
        0 => None,
        signal => Some(name(signal)),
    }
}

fn name(signal: i32) -> &'static str {
    #[cfg(unix)]
    match signal {
        libc::SIGTERM => return "SIGTERM",
        libc::SIGINT => return "SIGINT",
        libc::SIGHUP => return "SIGHUP",
        _ => {}
    }
    let _ = signal;
    "a signal"
}

#[cfg(all(test, unix))]
mod tests {
    use super::{install, take};

    /// Raised at this very process: without the handler installed, SIGTERM
    /// would end the test binary, which is the failure this reports.
    #[test]
    fn a_sigterm_reaches_the_pump_instead_of_ending_the_process() {
        install();
        assert_eq!(take(), None);
        // SAFETY: raising a signal whose handler was just installed.
        assert_eq!(unsafe { libc::raise(libc::SIGTERM) }, 0);
        assert_eq!(take(), Some("SIGTERM"));
        assert_eq!(take(), None, "read once, then cleared");
    }
}
