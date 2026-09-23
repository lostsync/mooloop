//! Asking the desktop whether it is wearing light or dark.
//!
//! **Linux-first, and there is no single answer on Linux.** The portal setting
//! `org.freedesktop.appearance color-scheme` is the cross-desktop one and is
//! what GNOME, KDE and most of the modern shells publish, but reaching it
//! wants a D-Bus client and this crate has no business growing one for three
//! bytes of information. So the probes are external, ordered by how much they
//! are worth believing, and the first that answers wins:
//!
//! 1. the XDG desktop portal, via `gdbus` or `busctl`;
//! 2. GNOME's `gsettings` key, which predates the portal and is still what
//!    several shells set;
//! 3. KDE's `kdeglobals`, read as a file, because it is a file;
//! 4. macOS's `AppleInterfaceStyle`;
//! 5. dark, because that is what this program looks like.
//!
//! **Nothing here is on a hot path and nothing here blocks the UI for long.**
//! The answer is cached for the life of the process and re-probed when the
//! Appearance page asks.
//!
//! **Watching is Slint's job, not this file's** (MOO-156). Slint's winit
//! backend already holds a D-Bus connection to the portal and applies its
//! `SettingChanged` signal to `Palette.color-scheme`; on macOS winit reports
//! the system appearance the same way. `MainWindow.desktop-color-scheme`
//! mirrors that and fires a callback when it moves, which lands in
//! [`desktop_reported`]. So a desktop that changes its mind while mooloop is
//! running is followed within a frame, with no D-Bus client of our own and no
//! polling. The probes below remain for the first answer and for a platform
//! whose backend never reports one.

use std::process::{Command, Stdio};
use std::cell::Cell;

thread_local! {
    /// 0 = not probed, 1 = dark, 2 = light.
    ///
    /// Per thread rather than per process: every reader is the UI thread, and
    /// each test thread gets a cache of its own, so a test that feeds in a
    /// desktop report cannot race another test's probe. A second thread that
    /// ever asked would only cost one more probe.
    static CACHED: Cell<u8> = const { Cell::new(0) };
}

/// What the desktop is wearing, as far as anything here can tell.
pub(crate) fn prefers_dark() -> bool {
    match CACHED.get() {
        1 => true,
        2 => false,
        _ => {
            let dark = probe().unwrap_or(true);
            CACHED.set(if dark { 1 } else { 2 });
            dark
        }
    }
}

/// Throws away the cached answer, so the next [`prefers_dark`] asks again.
/// The Appearance page calls this when it opens.
pub(crate) fn forget() {
    CACHED.set(0);
}

/// The platform said what it is wearing now, as
/// `MainWindow.desktop-color-scheme` spells it: `1` dark, `2` light. That
/// replaces the cached answer and returns true. Anything else is the platform
/// not knowing, which must not overwrite what the probes found, so it changes
/// nothing and returns false.
pub(crate) fn desktop_reported(scheme: i32) -> bool {
    let Some(dark) = reported_dark(scheme) else {
        return false;
    };
    CACHED.set(if dark { 1 } else { 2 });
    true
}

fn reported_dark(scheme: i32) -> Option<bool> {
    match scheme {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

fn probe() -> Option<bool> {
    portal()
        .or_else(gsettings)
        .or_else(kdeglobals)
        .or_else(macos)
}

/// Runs a command and hands back its stdout, or nothing at all -- a missing
/// binary, a non-zero exit and unreadable output are one outcome here, which
/// is "this probe did not answer".
fn output(program: &str, args: &[&str]) -> Option<String> {
    let result = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !result.status.success() {
        return None;
    }
    String::from_utf8(result.stdout).ok()
}

/// `org.freedesktop.appearance color-scheme`: 0 no preference, 1 prefer dark,
/// 2 prefer light.
///
/// **`0` is not an answer and must not be read as one.** A desktop that says
/// "no preference" is declining to choose, and falling through to the next
/// probe is more likely to find what the user actually set than treating it as
/// light would be.
fn portal() -> Option<bool> {
    const ARGS: [&str; 8] = [
        "call",
        "--session",
        "--dest",
        "org.freedesktop.portal.Desktop",
        "--object-path",
        "/org/freedesktop/portal/desktop",
        "--method",
        "org.freedesktop.portal.Settings.ReadOne",
    ];
    let mut args = ARGS.to_vec();
    args.push("org.freedesktop.appearance");
    args.push("color-scheme");
    let reply = output("gdbus", &args).or_else(|| {
        output(
            "busctl",
            &[
                "--user",
                "call",
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Settings",
                "ReadOne",
                "ss",
                "org.freedesktop.appearance",
                "color-scheme",
            ],
        )
    })?;
    // `gdbus` prints `(<<uint32 1>>,)` and `busctl` prints `v u 1`; the last
    // bare integer in either is the value.
    let value: u32 = reply
        .split(|c: char| !c.is_ascii_digit())
        .rfind(|piece| !piece.is_empty())?
        .parse()
        .ok()?;
    match value {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

fn gsettings() -> Option<bool> {
    let reply = output(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "color-scheme"],
    )?;
    let reply = reply.trim().trim_matches('\'');
    match reply {
        "prefer-dark" => Some(true),
        "prefer-light" | "default" => Some(false),
        _ => None,
    }
}

/// KDE writes its choice into `kdeglobals`, and the readable half of it is the
/// scheme's name. Plasma 6 also sets the portal, so this is mostly the older
/// desktops and the case where no portal is running.
fn kdeglobals() -> Option<bool> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })?;
    let text = std::fs::read_to_string(base.join("kdeglobals")).ok()?;
    let name = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("ColorScheme="))?
        .trim()
        .to_ascii_lowercase();
    if name.contains("dark") {
        Some(true)
    } else if name.contains("light") || name.contains("breeze") {
        // Plain "Breeze" is the light one; "BreezeDark" was caught above.
        Some(false)
    } else {
        None
    }
}

fn macos() -> Option<bool> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    // The key is *absent* in light mode, which is why a failed read here is an
    // answer rather than a shrug -- but only on macOS, hence the guard above.
    Some(
        output("defaults", &["read", "-g", "AppleInterfaceStyle"])
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("Dark")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe runs whatever is on this machine, so the only thing a test
    /// can assert is that it terminates, answers, and keeps answering the
    /// same thing. A CI runner with no session bus takes the fallback, which
    /// is the path most worth knowing does not hang.
    #[test]
    fn the_probe_answers_and_caches() {
        forget();
        let first = prefers_dark();
        assert_eq!(first, prefers_dark());
        forget();
        assert_eq!(first, prefers_dark());
    }

    /// Only dark and light are answers. Slint's `unknown` (0) is a platform
    /// that has not said, and taking it as either side would override what
    /// the probes found.
    #[test]
    fn a_report_is_the_answer_and_unknown_is_not() {
        assert!(desktop_reported(2));
        assert!(!prefers_dark());
        assert!(!desktop_reported(0));
        assert!(!prefers_dark(), "0 left the light answer alone");
        assert!(desktop_reported(1));
        assert!(prefers_dark());
        assert!(!desktop_reported(3));
        assert!(!desktop_reported(-1));
        assert!(prefers_dark());
    }

    /// Every probe has to survive a binary that is not installed, which is the
    /// ordinary case for at least three of them on any one machine.
    #[test]
    fn a_missing_binary_is_not_an_answer() {
        assert!(output("mooloop-no-such-program-exists", &["--version"]).is_none());
    }
}
