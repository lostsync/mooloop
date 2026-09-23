//! File choosers from the desktop.
//!
//! Questions are not asked here any more: the unsaved-changes, preset and
//! kit questions are the application's own `QuestionDialog` (MOO-91),
//! because a question program that would not start read as No -- so with
//! no zenity a song with unsaved changes could not be quit -- and one that
//! did start blocked the UI thread until it was answered.
//!
//! On Linux a file chooser is asked of the desktop's **file chooser portal**
//! (`org.freedesktop.portal.FileChooser`, over D-Bus), then of **zenity**,
//! then of **kdialog**, and the first that can show one answers. On macOS it
//! is `osascript`, whose standard additions draw the system's own open, save
//! and alert panels. Each call blocks its thread until the user answers, so
//! callers run them off the UI thread.
//!
//! **A chooser that could not be shown is not a cancel** (MOO-90). Until
//! 2026-09-22 a dialog program that would not start came back as `None`, the
//! same as the Cancel button, and every caller treated it as one: on a
//! desktop without zenity -- Kubuntu, Fedora KDE and Arch ship none, and the
//! AppImage cannot declare one -- the first Ctrl+S on the untitled starter
//! song cleared the status bar and did nothing, and so did Open and Export.
//! [`Picked::Unavailable`] carries what was tried, so the caller can say why
//! no chooser appeared.

use crate::audio_file;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a file chooser came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Picked {
    Path(PathBuf),
    /// The user cancelled, or closed the chooser without picking.
    Cancelled,
    /// No chooser could be shown at all.
    Unavailable(NoChooser),
}

impl Picked {
    fn map(self, f: impl FnOnce(PathBuf) -> PathBuf) -> Self {
        match self {
            Self::Path(path) => Self::Path(f(path)),
            other => other,
        }
    }

    /// The path, if one was picked.
    pub fn path(self) -> Option<PathBuf> {
        match self {
            Self::Path(path) => Some(path),
            _ => None,
        }
    }
}

/// Why no chooser appeared: each way of showing one that was tried, and
/// what stopped it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoChooser {
    pub tried: Vec<String>,
}

impl NoChooser {
    /// One line, for a status bar.
    pub fn one_line(&self) -> String {
        format!("No file chooser could be opened: {}", backend::INSTALL_HINT)
    }

    /// The whole answer, for the error dialog: what was tried, in order, and
    /// what would make one work.
    pub fn explain(&self) -> String {
        let mut text = format!("No file chooser could be opened. {}", backend::ORDER);
        for attempt in &self.tried {
            text.push_str("\n  - ");
            text.push_str(attempt);
        }
        text.push_str("\n\n");
        text.push_str(backend::INSTALL_HINT);
        text
    }
}

/// The patterns a chooser offers, named. Each backend spells a filter its own
/// way, so it is kept as data until one of them asks.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Filter {
    name: String,
    patterns: Vec<String>,
}

impl Filter {
    fn new(name: &str, patterns: impl IntoIterator<Item = String>) -> Self {
        Self {
            name: name.to_string(),
            patterns: patterns.into_iter().collect(),
        }
    }
}

/// What to ask a chooser for.
#[derive(Debug, Clone, Copy)]
enum Request<'a> {
    Open {
        title: &'a str,
        filter: Option<&'a Filter>,
    },
    Directory {
        title: &'a str,
    },
    Save {
        title: &'a str,
        suggested: &'a str,
        filter: Option<&'a Filter>,
    },
}

/// What one way of showing a chooser came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Attempt {
    Chosen(PathBuf),
    Cancelled,
    /// This way could not show one; the text says why, for the user.
    Unavailable(String),
}

/// The first answer from `attempts`, tried in order. An attempt that could
/// not show a chooser passes to the next; one that did -- whatever the user
/// then did with it -- is the answer.
fn first_answer<'a>(attempts: impl IntoIterator<Item = Box<dyn FnOnce() -> Attempt + 'a>>) -> Picked {
    let mut tried = Vec::new();
    for attempt in attempts {
        match attempt() {
            Attempt::Chosen(path) => return Picked::Path(path),
            Attempt::Cancelled => return Picked::Cancelled,
            Attempt::Unavailable(why) => {
                mooloop_core::log_warn!("dialog", "no file chooser from {why}");
                tried.push(why);
            }
        }
    }
    Picked::Unavailable(NoChooser { tried })
}

/// Run a dialog program that prints the path it was given on stdout.
///
/// Both zenity and kdialog exit 0 with a path, and 1 for Cancel or a closed
/// window. A program that will not start is unavailable, and so is one that
/// exits any other way: both use other codes for "could not run at all" --
/// no display, a bad argument -- and reading those as a cancel is the defect
/// this module exists to stop.
fn run_program(mut command: Command) -> Attempt {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = match command.output() {
        Ok(output) => output,
        Err(error) => return Attempt::Unavailable(format!("{program}: would not start ({error})")),
    };
    match output.status.code() {
        Some(0) => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if value.is_empty() {
                Attempt::Cancelled
            } else {
                Attempt::Chosen(PathBuf::from(value))
            }
        }
        Some(1) => Attempt::Cancelled,
        status => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr.lines().find(|line| !line.trim().is_empty());
            Attempt::Unavailable(format!(
                "{program}: {}{}",
                status.map_or_else(|| "ended by a signal".to_string(), |code| format!("exited with {code}")),
                detail.map(|line| format!(" ({})", line.trim())).unwrap_or_default(),
            ))
        }
    }
}

pub fn pick_bundle_dialog(title: &str) -> Picked {
    backend::pick(Request::Directory { title })
}

pub fn pick_song_dialog(title: &str) -> Picked {
    let filter = Filter::new(
        "Mooloop songs",
        ["*.mooloop".to_string(), mooloop_project::MANIFEST_FILE.to_string()],
    );
    backend::pick(Request::Open {
        title,
        filter: Some(&filter),
    })
    .map(normalize_song_selection)
}

fn normalize_song_selection(path: PathBuf) -> PathBuf {
    let is_legacy_manifest = path
        .file_name()
        .is_some_and(|name| name == mooloop_project::MANIFEST_FILE)
        && path
            .parent()
            .and_then(Path::extension)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mooloop"));
    if is_legacy_manifest {
        path.parent()
            .expect("manifest selection has a parent")
            .into()
    } else {
        path
    }
}

pub fn pick_save_dialog(title: &str, suggested: &str) -> Picked {
    backend::pick(Request::Save {
        title,
        suggested,
        filter: None,
    })
}

pub fn pick_export_dialog(extension: &str) -> Picked {
    let filter = Filter::new(&extension.to_uppercase(), [format!("*.{extension}")]);
    backend::pick(Request::Save {
        title: "Export audio",
        suggested: &format!("mooloop-export.{extension}"),
        filter: Some(&filter),
    })
    .map(|mut path| {
        if path
            .extension()
            .and_then(|value| value.to_str())
            .is_none_or(|value| !value.eq_ignore_ascii_case(extension))
        {
            path.set_extension(extension);
        }
        path
    })
}

/// Pick a supported audio file.
pub fn pick_sample_dialog() -> Picked {
    let filter = Filter::new(
        "Audio samples",
        audio_file::SUPPORTED_EXTENSIONS.iter().flat_map(|extension| {
            [
                format!("*.{extension}"),
                format!("*.{}", extension.to_uppercase()),
            ]
        }),
    );
    backend::pick(Request::Open {
        title: "Load sample",
        filter: Some(&filter),
    })
}

/// The portal, then zenity, then kdialog.
#[cfg(not(target_os = "macos"))]
mod backend {
    use super::{first_answer, run_program, Attempt, Filter, Picked, Request};
    use std::path::PathBuf;
    use std::process::Command;

    pub(super) const ORDER: &str =
        "mooloop asks the desktop's file chooser portal first, then zenity, then kdialog:";
    pub(super) const INSTALL_HINT: &str = "install zenity or kdialog with your package manager, \
        or run mooloop on a desktop that provides xdg-desktop-portal.";

    pub(super) fn pick(request: Request<'_>) -> Picked {
        first_answer([
            Box::new(move || portal::pick(request)) as Box<dyn FnOnce() -> Attempt>,
            Box::new(move || run_program(zenity(request))),
            Box::new(move || run_program(kdialog(request))),
        ])
    }

    /// zenity's command line. Filters use its `--file-filter` syntax,
    /// `Name | *.a *.b`.
    pub(super) fn zenity(request: Request<'_>) -> Command {
        let mut command = Command::new("zenity");
        command.arg("--file-selection");
        let filter = match request {
            Request::Open { title, filter } => {
                command.arg(format!("--title={title}"));
                filter
            }
            Request::Directory { title } => {
                command.arg(format!("--title={title}")).arg("--directory");
                None
            }
            Request::Save {
                title,
                suggested,
                filter,
            } => {
                command
                    .arg(format!("--title={title}"))
                    .arg("--save")
                    .arg("--confirm-overwrite")
                    .arg(format!("--filename={suggested}"));
                filter
            }
        };
        if let Some(filter) = filter {
            command.arg(format!(
                "--file-filter={} | {}",
                filter.name,
                filter.patterns.join(" ")
            ));
        }
        command
    }

    /// kdialog's command line. It wants a starting folder before a filter,
    /// and spells the filter `Name (*.a *.b)`. Its save dialog asks before
    /// replacing a file, as zenity's `--confirm-overwrite` does.
    pub(super) fn kdialog(request: Request<'_>) -> Command {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let spelled = |filter: Option<&Filter>| {
            filter.map(|filter| format!("{} ({})", filter.name, filter.patterns.join(" ")))
        };
        let mut command = Command::new("kdialog");
        match request {
            Request::Open { title, filter } => {
                command.arg("--title").arg(title).arg("--getopenfilename").arg(&home);
                command.args(spelled(filter));
            }
            Request::Directory { title } => {
                command
                    .arg("--title")
                    .arg(title)
                    .arg("--getexistingdirectory")
                    .arg(&home);
            }
            Request::Save {
                title,
                suggested,
                filter,
            } => {
                command
                    .arg("--title")
                    .arg(title)
                    .arg("--getsavefilename")
                    .arg(home.join(suggested));
                command.args(spelled(filter));
            }
        }
        command
    }

    /// `org.freedesktop.portal.FileChooser`, which is what every desktop's
    /// own chooser answers to: KDE's, GNOME's, and the ones a tiling
    /// compositor installs. Spoken over D-Bus with the `zbus` the window
    /// system already links, so it costs no new dependency.
    #[cfg(target_os = "linux")]
    mod portal {
        use super::super::{Attempt, Filter, Request};
        use std::collections::HashMap;
        use std::path::PathBuf;
        use zbus::blocking::{Connection, Proxy};
        use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

        const DESTINATION: &str = "org.freedesktop.portal.Desktop";
        const PATH: &str = "/org/freedesktop/portal/desktop";
        const INTERFACE: &str = "org.freedesktop.portal.FileChooser";
        const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";

        pub(in super::super) fn pick(request: Request<'_>) -> Attempt {
            match ask(request) {
                Ok(attempt) => attempt,
                Err(error) => Attempt::Unavailable(format!("the file chooser portal: {error}")),
            }
        }

        fn ask(request: Request<'_>) -> Result<Attempt, String> {
            let connection = Connection::session().map_err(|error| format!("no session bus ({error})"))?;
            let chooser = Proxy::new(&connection, DESTINATION, PATH, INTERFACE)
                .map_err(|error| error.to_string())?;
            // Asked first because it also starts the portal if it is only
            // activatable, and a signal cannot be subscribed to on a name
            // nobody owns yet. A desktop with no chooser behind the portal
            // fails here, and zenity is asked instead.
            let version: u32 = chooser
                .get_property("version")
                .map_err(|error| format!("not offered here ({error})"))?;
            if matches!(request, Request::Directory { .. }) && version < 3 {
                return Err(format!("version {version} cannot pick a folder"));
            }

            // The reply names a Request object whose `Response` signal
            // carries the answer. Its path is predictable from our bus name
            // and a token we choose, so the subscription is in place before
            // the call: subscribing after the reply could miss a chooser the
            // user answered very fast.
            let token = format!("mooloop_{}_{}", std::process::id(), next_token());
            let sender = connection
                .unique_name()
                .map(|name| name.trim_start_matches(':').replace('.', "_"))
                .ok_or("no bus name")?;
            let expected = format!("{PATH}/request/{sender}/{token}");
            let listener = Proxy::new(&connection, DESTINATION, expected.as_str(), REQUEST_INTERFACE)
                .map_err(|error| error.to_string())?;
            let mut responses = listener
                .receive_signal("Response")
                .map_err(|error| error.to_string())?;

            let mut options: HashMap<&str, Value<'_>> = HashMap::new();
            options.insert("handle_token", Value::from(token.as_str()));
            options.insert("modal", Value::from(true));
            let (method, title) = match request {
                Request::Open { title, filter } => {
                    insert_filter(&mut options, filter);
                    ("OpenFile", title)
                }
                Request::Directory { title } => {
                    options.insert("directory", Value::from(true));
                    ("OpenFile", title)
                }
                Request::Save {
                    title,
                    suggested,
                    filter,
                } => {
                    options.insert("current_name", Value::from(suggested));
                    insert_filter(&mut options, filter);
                    ("SaveFile", title)
                }
            };
            let handle: OwnedObjectPath = chooser
                .call(method, &("", title, options))
                .map_err(|error| format!("{method} refused ({error})"))?;
            // A portal older than the token convention answers on a path of
            // its own choosing.
            let mut late;
            let responses: &mut dyn Iterator<Item = zbus::Message> = if handle.as_str() == expected {
                &mut responses
            } else {
                late = Proxy::new(&connection, DESTINATION, handle.as_str(), REQUEST_INTERFACE)
                    .and_then(|proxy| proxy.receive_signal("Response"))
                    .map_err(|error| error.to_string())?;
                &mut late
            };
            let message = responses.next().ok_or("the chooser went away without answering")?;
            let (response, results): (u32, HashMap<String, OwnedValue>) = message
                .body()
                .deserialize()
                .map_err(|error| format!("an answer it could not read ({error})"))?;
            match response {
                0 => {
                    let uri = results
                        .get("uris")
                        .and_then(|uris| Vec::<String>::try_from(uris.clone()).ok())
                        .and_then(|uris| uris.into_iter().next());
                    match uri.as_deref().and_then(path_from_uri) {
                        Some(path) => Ok(Attempt::Chosen(path)),
                        None => Ok(Attempt::Cancelled),
                    }
                }
                1 => Ok(Attempt::Cancelled),
                // "Ended some other way": the portal could not show one.
                other => Err(format!("the chooser ended with response {other}")),
            }
        }

        fn insert_filter(options: &mut HashMap<&str, Value<'_>>, filter: Option<&Filter>) {
            let Some(filter) = filter else {
                return;
            };
            // `a(sa(us))`: one named filter, each pattern a glob (kind 0).
            let patterns: Vec<(u32, String)> =
                filter.patterns.iter().map(|pattern| (0, pattern.clone())).collect();
            let filters = vec![(filter.name.clone(), patterns)];
            options.insert("filters", Value::from(filters.clone()));
            options.insert("current_filter", Value::from(filters[0].clone()));
        }

        fn next_token() -> u64 {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            NEXT.fetch_add(1, Ordering::Relaxed)
        }

        /// A local file's path from the `file://` URI the portal answers
        /// with. Anything else -- another scheme, a remote host -- is not a
        /// file this application can open.
        pub(in super::super) fn path_from_uri(uri: &str) -> Option<PathBuf> {
            let rest = uri.strip_prefix("file://")?;
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            if !rest.starts_with('/') {
                return None;
            }
            let bytes = rest.as_bytes();
            let mut decoded = Vec::with_capacity(bytes.len());
            let mut index = 0;
            while index < bytes.len() {
                if bytes[index] == b'%' {
                    let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
                    decoded.push(u8::from_str_radix(hex, 16).ok()?);
                    index += 3;
                } else {
                    decoded.push(bytes[index]);
                    index += 1;
                }
            }
            use std::os::unix::ffi::OsStringExt;
            Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
        }
    }

    /// No portal off Linux; the BSDs go straight to zenity.
    #[cfg(not(target_os = "linux"))]
    mod portal {
        use super::super::{Attempt, Request};

        pub(in super::super) fn pick(_: Request<'_>) -> Attempt {
            Attempt::Unavailable("the file chooser portal: not on this platform".into())
        }
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(super) use portal::path_from_uri;
}

/// `osascript`, which every Mac has. The panels show every file: a filter is
/// ignored here, and every caller already refuses what it cannot use -- a
/// song that will not load, a sample that will not decode, an export whose
/// extension it corrects.
#[cfg(target_os = "macos")]
mod backend {
    use super::{first_answer, Attempt, Picked, Request};
    use std::path::PathBuf;
    use std::process::Command;

    pub(super) const ORDER: &str = "mooloop asks osascript for the system's own panels:";
    pub(super) const INSTALL_HINT: &str =
        "osascript is part of macOS; check that /usr/bin/osascript is there and runs.";

    pub(super) fn pick(request: Request<'_>) -> Picked {
        first_answer([Box::new(move || run(request)) as Box<dyn FnOnce() -> Attempt>])
    }

    /// osascript reports Cancel as AppleScript error -128, a failing exit
    /// status, and does the same for most other errors -- so on a Mac only a
    /// program that will not start is told apart from a cancel.
    fn run(request: Request<'_>) -> Attempt {
        let mut command = chooser(request);
        let program = command.get_program().to_string_lossy().into_owned();
        match command.output() {
            Err(error) => Attempt::Unavailable(format!("{program}: would not start ({error})")),
            Ok(output) if output.status.success() => {
                let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if value.is_empty() {
                    Attempt::Cancelled
                } else {
                    Attempt::Chosen(PathBuf::from(value))
                }
            }
            Ok(_) => Attempt::Cancelled,
        }
    }

    /// One `on run` handler around `body`. `activate` brings osascript, and so
    /// its panel, in front of mooloop's window. Titles and questions arrive
    /// through `argv` rather than being spliced into the script, so nothing in
    /// them can be read as AppleScript.
    fn osascript(body: &str, args: &[&str]) -> Command {
        let mut command = Command::new("osascript");
        command
            .args(["-e", "on run argv", "-e", "activate", "-e", body, "-e", "end run"])
            .args(args);
        command
    }

    pub(super) fn chooser(request: Request<'_>) -> Command {
        match request {
            Request::Directory { title } => osascript(
                "POSIX path of (choose folder with prompt (item 1 of argv))",
                &[title],
            ),
            Request::Open { title, .. } => osascript(
                "POSIX path of (choose file with prompt (item 1 of argv))",
                &[title],
            ),
            // The save panel asks before replacing a file itself, as zenity's
            // `--confirm-overwrite` does.
            Request::Save {
                title, suggested, ..
            } => osascript(
                "POSIX path of (choose file name with prompt (item 1 of argv) default name (item 2 of argv))",
                &[title, suggested],
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn song_selection_accepts_new_files_and_legacy_manifests() {
        let file = PathBuf::from("/songs/beat.mooloop");
        assert_eq!(normalize_song_selection(file.clone()), file);

        let legacy_manifest = PathBuf::from("/songs/old.mooloop/manifest.toml");
        assert_eq!(
            normalize_song_selection(legacy_manifest),
            PathBuf::from("/songs/old.mooloop")
        );
    }

    /// **A dialog program that will not start is not a cancel** (MOO-90).
    /// Before, both came back as `None`, and every caller read a missing
    /// zenity as the user pressing Cancel -- so Save As, Open and Export did
    /// nothing and said nothing.
    #[test]
    fn a_program_that_will_not_start_is_not_a_cancel() {
        let missing = run_program(Command::new("/nonexistent/mooloop-no-such-dialog"));
        assert!(
            matches!(&missing, Attempt::Unavailable(why) if why.contains("would not start")),
            "{missing:?}"
        );

        // And the Cancel button still is one.
        let cancelled = run_program(Command::new("false"));
        assert_eq!(cancelled, Attempt::Cancelled);
    }

    /// A program that started and could not run -- zenity with no display
    /// exits 255 -- is unavailable too, not a cancel.
    #[cfg(unix)]
    #[test]
    fn a_program_that_fails_to_run_is_not_a_cancel() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo 'cannot open display' >&2; exit 255"]);
        let attempt = run_program(command);
        assert!(
            matches!(&attempt, Attempt::Unavailable(why) if why.contains("255") && why.contains("cannot open display")),
            "{attempt:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_prints_a_path_chose_it() {
        let mut command = Command::new("sh");
        command.args(["-c", "echo /songs/beat.mooloop"]);
        assert_eq!(
            run_program(command),
            Attempt::Chosen(PathBuf::from("/songs/beat.mooloop"))
        );
    }

    /// The chain passes an unavailable chooser to the next one, stops at the
    /// first that showed one -- a cancel included -- and, when none could,
    /// reports every reason in the order tried.
    #[test]
    fn the_first_chooser_that_can_show_one_answers() {
        let unavailable = |why: &'static str| {
            Box::new(move || Attempt::Unavailable(why.into())) as Box<dyn FnOnce() -> Attempt>
        };
        let never = || -> Box<dyn FnOnce() -> Attempt> {
            Box::new(|| panic!("asked after a chooser had already answered"))
        };

        let picked = first_answer([
            unavailable("portal: none"),
            Box::new(|| Attempt::Chosen(PathBuf::from("/a.wav"))),
            never(),
        ]);
        assert_eq!(picked, Picked::Path(PathBuf::from("/a.wav")));

        let cancelled = first_answer([unavailable("portal: none"), Box::new(|| Attempt::Cancelled), never()]);
        assert_eq!(cancelled, Picked::Cancelled, "a cancel is an answer, not a reason to ask again");

        let none = first_answer([unavailable("portal: none"), unavailable("zenity: would not start")]);
        let Picked::Unavailable(no_chooser) = none else {
            panic!("nothing could show a chooser: {none:?}");
        };
        assert_eq!(no_chooser.tried, ["portal: none", "zenity: would not start"]);
        let explained = no_chooser.explain();
        assert!(explained.contains("zenity: would not start"), "{explained}");
        assert!(explained.contains(backend::INSTALL_HINT), "{explained}");
    }

    /// The zenity command lines are the ones Linux has always run; adding the
    /// portal and kdialog was not a reason for them to change.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn zenity_is_asked_what_it_always_was() {
        assert_eq!(
            args(&backend::zenity(Request::Save {
                title: "Save mooloop song",
                suggested: "Untitled.mooloop",
                filter: None
            })),
            [
                "--file-selection",
                "--title=Save mooloop song",
                "--save",
                "--confirm-overwrite",
                "--filename=Untitled.mooloop",
            ]
        );
        let songs = Filter::new("Mooloop songs", ["*.mooloop".into(), "manifest.toml".into()]);
        assert_eq!(
            args(&backend::zenity(Request::Open {
                title: "Open mooloop song",
                filter: Some(&songs)
            })),
            [
                "--file-selection",
                "--title=Open mooloop song",
                "--file-filter=Mooloop songs | *.mooloop manifest.toml",
            ]
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn kdialog_is_given_a_folder_before_its_filter() {
        let songs = Filter::new("Mooloop songs", ["*.mooloop".into()]);
        let open = args(&backend::kdialog(Request::Open {
            title: "Open mooloop song",
            filter: Some(&songs),
        }));
        assert_eq!(open[..3], ["--title", "Open mooloop song", "--getopenfilename"]);
        assert_eq!(open.last().unwrap(), "Mooloop songs (*.mooloop)");
        assert_eq!(open.len(), 5, "{open:?}");

        let save = args(&backend::kdialog(Request::Save {
            title: "Save mooloop song",
            suggested: "Untitled.mooloop",
            filter: None,
        }));
        assert_eq!(save[2], "--getsavefilename");
        assert!(save[3].ends_with("/Untitled.mooloop"), "{save:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_portal_uri_becomes_a_local_path() {
        use backend::path_from_uri;
        assert_eq!(
            path_from_uri("file:///home/a/My%20Songs/beat.mooloop"),
            Some(PathBuf::from("/home/a/My Songs/beat.mooloop"))
        );
        assert_eq!(
            path_from_uri("file://localhost/tmp/x.wav"),
            Some(PathBuf::from("/tmp/x.wav"))
        );
        assert_eq!(path_from_uri("sftp://host/tmp/x.wav"), None);
        assert_eq!(path_from_uri("file://otherhost/tmp/x.wav"), None);
    }

    /// A title is data. Quotes in it must not reach the script's source, where
    /// they would end a string literal and turn the rest into code.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_title_reaches_applescript_as_an_argument() {
        let title = r#"Save "quoted" song" & (do shell script "true")"#;
        let command = backend::chooser(Request::Save {
            title,
            suggested: "Untitled.mooloop",
            filter: None,
        });
        let args = args(&command);
        assert_eq!(&args[args.len() - 2..], [title, "Untitled.mooloop"]);
        assert!(args[..args.len() - 2].iter().all(|arg| !arg.contains("quoted")));
    }
}
