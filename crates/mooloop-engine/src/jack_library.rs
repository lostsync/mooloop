//! Which `libjack.so.0` the `jack` crate gets (MOO-342).
//!
//! `jack-sys` opens the library by name, `libjack.so.0`, and the dynamic
//! linker answers with whatever its search path finds first. On Debian,
//! Ubuntu and Mint that is JACK2's client library even with `pipewire-jack`
//! installed: PipeWire's copy lives in `pipewire-0.3/jack/` beside the other
//! libraries and is reached only through `pw-jack` or an `ld.so.conf.d`
//! snippet the package ships as an example. JACK2's library then finds no
//! server, and mooloop comes up with no audio on a desktop where PipeWire is
//! playing everything else.
//!
//! So before the `jack` crate loads anything, [`choose`] may open PipeWire's
//! library **by its full path**. glibc matches a later `dlopen` of a bare
//! name against the SONAME of what is already loaded, so the `jack` crate's
//! `libjack.so.0` is then PipeWire's, and the process environment is never
//! touched. It does this only when nothing else has chosen:
//!
//! 1. `LD_LIBRARY_PATH` or `LD_PRELOAD` already reaches a libjack (`pw-jack`
//!    is this) -- the user's choice stands.
//! 2. A JACK server is answering -- a real `jackd` next to PipeWire is
//!    somebody's deliberate setup, and the default library is how it is
//!    reached.
//! 3. PipeWire is not running, or its JACK library is not installed --
//!    there is nothing better to load.
//!
//! Adam's ruling, 2026-09-29, on MOO-342: option 1, *"this might be what
//! bitwig does, more or less"*.
//!
//! [`loaded_library`] then reads back which file is actually mapped, which
//! is what the log and the Audio preferences say: on a machine with several
//! JACK installs, that one line is the diagnosis.

use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The name `jack-sys` asks the dynamic linker for.
const SONAME: &str = "libjack.so.0";

/// What [`plan`] decided, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Open this file before the `jack` crate loads the library.
    Preload(PathBuf),
    /// Leave the dynamic linker's answer alone.
    Leave(Why),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Why {
    /// `LD_LIBRARY_PATH` or `LD_PRELOAD` names a libjack; `pw-jack` sets the
    /// first.
    Chosen(&'static str),
    /// A JACK server answers on this socket.
    JackServer(PathBuf),
    NoPipeWire,
    NoPipeWireJack,
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Why::Chosen(var) => write!(f, "{var} chooses the library"),
            Why::JackServer(socket) => {
                write!(f, "a JACK server is running ({})", socket.display())
            }
            Why::NoPipeWire => f.write_str("PipeWire is not running"),
            Why::NoPipeWireJack => {
                f.write_str("PipeWire's JACK library (pipewire-jack) is not installed")
            }
        }
    }
}

/// What [`plan`] looks at, gathered by [`Machine::probe`] so a test can
/// give it any machine it likes.
#[derive(Debug, Default)]
pub(crate) struct Machine {
    /// A directory on `LD_LIBRARY_PATH` holds a `libjack.so.0`.
    pub ld_library_path_has_libjack: bool,
    /// `LD_PRELOAD` names a libjack.
    pub ld_preload_has_libjack: bool,
    /// A JACK server's socket that accepts a connection.
    pub jack_server: Option<PathBuf>,
    pub pipewire_running: bool,
    /// PipeWire's `libjack.so.0`, where one is installed.
    pub pipewire_libjack: Option<PathBuf>,
}

/// The rule, and nothing that touches the machine.
pub(crate) fn plan(machine: &Machine) -> Plan {
    if machine.ld_library_path_has_libjack {
        return Plan::Leave(Why::Chosen("LD_LIBRARY_PATH"));
    }
    if machine.ld_preload_has_libjack {
        return Plan::Leave(Why::Chosen("LD_PRELOAD"));
    }
    if let Some(socket) = &machine.jack_server {
        return Plan::Leave(Why::JackServer(socket.clone()));
    }
    if !machine.pipewire_running {
        return Plan::Leave(Why::NoPipeWire);
    }
    match &machine.pipewire_libjack {
        Some(path) => Plan::Preload(path.clone()),
        None => Plan::Leave(Why::NoPipeWireJack),
    }
}

impl Machine {
    pub(crate) fn probe() -> Self {
        let env = |name: &str| std::env::var_os(name);
        let runtime_dir = env("XDG_RUNTIME_DIR").map(PathBuf::from);
        Self {
            ld_library_path_has_libjack: env("LD_LIBRARY_PATH")
                .is_some_and(|dirs| std::env::split_paths(&dirs).any(|d| d.join(SONAME).exists())),
            ld_preload_has_libjack: env("LD_PRELOAD")
                .is_some_and(|list| list.as_bytes().windows(7).any(|w| w == b"libjack")),
            jack_server: jack_server_socket(&jack_socket_dirs(runtime_dir.as_deref()), uid()),
            pipewire_running: pipewire_running(
                env("PIPEWIRE_REMOTE").as_deref(),
                runtime_dir.as_deref(),
            ),
            pipewire_libjack: pipewire_libjack_candidates()
                .into_iter()
                .find(|p| p.exists()),
        }
    }
}

fn uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// Where a JACK server puts its sockets. JACK2 uses `/dev/shm`, JACK1 and
/// some JACK2 builds `/tmp` or the runtime directory.
fn jack_socket_dirs(runtime_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/dev/shm"), PathBuf::from("/tmp")];
    dirs.extend(runtime_dir.map(Path::to_path_buf));
    dirs
}

/// A JACK server's socket in one of `dirs` that accepts a connection.
///
/// JACK2 names it `jack_<server>_<uid>_0`; JACK1 puts `jack_0` in a
/// `jack-<uid>/<server>/` directory. A socket left by a server that
/// crashed refuses the connection, so it does not count: taking it for a
/// running server would keep mooloop off PipeWire for nothing.
/// pipewire-jack makes none of these.
pub(crate) fn jack_server_socket(dirs: &[PathBuf], uid: u32) -> Option<PathBuf> {
    let jack2_suffix = format!("_{uid}_0");
    let jack1_dir = format!("jack-{uid}");
    let mut candidates = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("jack_") && name.ends_with(&jack2_suffix) {
                candidates.push(entry.path());
            } else if *name == *jack1_dir {
                let Ok(servers) = std::fs::read_dir(entry.path()) else {
                    continue;
                };
                candidates.extend(servers.flatten().map(|s| s.path().join("jack_0")));
            }
        }
    }
    candidates
        .into_iter()
        .find(|socket| UnixStream::connect(socket).is_ok())
}

/// PipeWire answers on `PIPEWIRE_REMOTE`, or on `pipewire-0` in the
/// runtime directory.
pub(crate) fn pipewire_running(remote: Option<&OsStr>, runtime_dir: Option<&Path>) -> bool {
    let socket = match (remote, runtime_dir) {
        (Some(remote), _) if Path::new(remote).is_absolute() => PathBuf::from(remote),
        (Some(remote), Some(dir)) => dir.join(remote),
        (None, Some(dir)) => dir.join("pipewire-0"),
        (_, None) => return false,
    };
    UnixStream::connect(socket).is_ok()
}

/// Where distributions install PipeWire's `libjack.so.0` off the default
/// search path. Fedora and Arch put it on the path already, and there the
/// default library is PipeWire's anyway.
fn pipewire_libjack_candidates() -> Vec<PathBuf> {
    let triplet = if cfg!(target_arch = "x86_64") {
        "x86_64-linux-gnu"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64-linux-gnu"
    } else {
        ""
    };
    let mut dirs = Vec::new();
    if !triplet.is_empty() {
        dirs.push(format!("/usr/lib/{triplet}/pipewire-0.3/jack"));
    }
    dirs.extend([
        "/usr/lib64/pipewire-0.3/jack".into(),
        "/usr/lib/pipewire-0.3/jack".into(),
    ]);
    dirs.into_iter()
        .map(|d| Path::new(&d).join(SONAME))
        .collect()
}

/// Open `path` and keep it open for the life of the process, so that the
/// `jack` crate's `dlopen("libjack.so.0")` is answered with it.
pub(crate) fn preload(path: &Path) -> Result<(), String> {
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: a NUL-terminated path; the handle is deliberately never closed.
    let handle = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        // SAFETY: dlerror returns a NUL-terminated string or null.
        let error = unsafe { libc::dlerror() };
        let detail = if error.is_null() {
            "unknown error".to_owned()
        } else {
            // SAFETY: non-null, from dlerror, read before any other dl call.
            unsafe { std::ffi::CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        };
        return Err(detail);
    }
    Ok(())
}

/// Decide, and act on it, once per process; every later call returns the
/// first decision. Called before the `jack` crate first loads the library.
pub(crate) fn choose() -> &'static Plan {
    static CHOSEN: OnceLock<Plan> = OnceLock::new();
    CHOSEN.get_or_init(|| {
        let plan = plan(&Machine::probe());
        match &plan {
            Plan::Preload(path) => match preload(path) {
                Ok(()) => mooloop_core::log_info!(
                    "audio",
                    "loading PipeWire's JACK library directly: {}",
                    path.display()
                ),
                Err(error) => mooloop_core::log_warn!(
                    "audio",
                    "could not load PipeWire's JACK library {} ({error}); using the default",
                    path.display()
                ),
            },
            Plan::Leave(why) => {
                mooloop_core::log_info!("audio", "using the default JACK library: {why}")
            }
        }
        plan
    })
}

/// The libjack file this process has mapped, read from `/proc/self/maps`:
/// what was actually loaded, whoever chose it.
pub(crate) fn loaded_library() -> Option<PathBuf> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    loaded_library_in(&maps)
}

pub(crate) fn loaded_library_in(maps: &str) -> Option<PathBuf> {
    maps.lines()
        .filter_map(|line| line.split_whitespace().nth(5))
        .map(Path::new)
        .find(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.starts_with("libjack.so"))
        })
        .map(Path::to_path_buf)
}

/// One line for a person: which JACK library, and what it talks to.
pub(crate) fn describe(path: &Path) -> String {
    let through = if path
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with("pipewire"))
    {
        "PipeWire's JACK"
    } else {
        "JACK"
    };
    format!("{through} ({})", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn pipewire_desktop() -> Machine {
        Machine {
            pipewire_running: true,
            pipewire_libjack: Some(PathBuf::from(
                "/usr/lib/x86_64-linux-gnu/pipewire-0.3/jack/libjack.so.0",
            )),
            ..Machine::default()
        }
    }

    #[test]
    fn a_pipewire_desktop_gets_pipewires_library() {
        assert_eq!(
            plan(&pipewire_desktop()),
            Plan::Preload(PathBuf::from(
                "/usr/lib/x86_64-linux-gnu/pipewire-0.3/jack/libjack.so.0"
            ))
        );
    }

    #[test]
    fn pw_jack_and_ld_preload_are_left_alone() {
        let machine = Machine {
            ld_library_path_has_libjack: true,
            ..pipewire_desktop()
        };
        assert_eq!(plan(&machine), Plan::Leave(Why::Chosen("LD_LIBRARY_PATH")));
        let machine = Machine {
            ld_preload_has_libjack: true,
            ..pipewire_desktop()
        };
        assert_eq!(plan(&machine), Plan::Leave(Why::Chosen("LD_PRELOAD")));
    }

    #[test]
    fn a_running_jack_server_keeps_the_default_library() {
        let socket = PathBuf::from("/dev/shm/jack_default_1000_0");
        let machine = Machine {
            jack_server: Some(socket.clone()),
            ..pipewire_desktop()
        };
        assert_eq!(plan(&machine), Plan::Leave(Why::JackServer(socket)));
    }

    #[test]
    fn nothing_to_load_keeps_the_default_library() {
        let machine = Machine {
            pipewire_libjack: None,
            ..pipewire_desktop()
        };
        assert_eq!(plan(&machine), Plan::Leave(Why::NoPipeWireJack));
        let machine = Machine {
            pipewire_running: false,
            ..pipewire_desktop()
        };
        assert_eq!(plan(&machine), Plan::Leave(Why::NoPipeWire));
    }

    /// A live JACK2 socket counts, a stale one (nothing listening) does
    /// not, and another user's does not.
    #[test]
    fn only_a_listening_socket_is_a_server() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = [dir.path().to_path_buf()];
        std::fs::write(dir.path().join("jack_default_1000_0"), b"").unwrap();
        assert_eq!(
            jack_server_socket(&dirs, 1000),
            None,
            "a stale file is not a server"
        );

        let live = dir.path().join("jack_default_1001_0");
        let _listener = UnixListener::bind(&live).unwrap();
        assert_eq!(jack_server_socket(&dirs, 1001), Some(live));
        assert_eq!(jack_server_socket(&dirs, 1002), None);
    }

    #[test]
    fn a_jack1_server_directory_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let server = dir.path().join("jack-1000").join("default");
        std::fs::create_dir_all(&server).unwrap();
        let socket = server.join("jack_0");
        let _listener = UnixListener::bind(&socket).unwrap();
        assert_eq!(
            jack_server_socket(&[dir.path().to_path_buf()], 1000),
            Some(socket)
        );
    }

    #[test]
    fn pipewire_is_found_by_its_socket() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!pipewire_running(None, Some(dir.path())));
        let _listener = UnixListener::bind(dir.path().join("pipewire-0")).unwrap();
        assert!(pipewire_running(None, Some(dir.path())));
        assert!(!pipewire_running(
            Some(OsStr::new("elsewhere")),
            Some(dir.path())
        ));
        assert!(!pipewire_running(None, None));
    }

    /// The mechanism itself: a library opened by its full path answers the
    /// bare `dlopen("libjack.so.0")` the `jack` crate makes. Run in a child
    /// process, because a libjack another test already loaded would answer
    /// first. The system's JACK2 library, copied under a `pipewire-0.3`
    /// directory, stands in for PipeWire's.
    #[test]
    fn a_preloaded_library_answers_the_bare_name() {
        const CHILD: &str = "MOOLOOP_JACK_PRELOAD_CHILD";
        let Some(child_dir) = std::env::var_os(CHILD) else {
            let Some(system) = [
                "/usr/lib/x86_64-linux-gnu",
                "/usr/lib/aarch64-linux-gnu",
                "/usr/lib64",
                "/usr/lib",
            ]
            .iter()
            .map(|dir| Path::new(dir).join(SONAME))
            .find(|path| path.exists()) else {
                eprintln!("no libjack.so.0 on this machine; the mechanism is not exercised");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let jack_dir = dir.path().join("pipewire-0.3").join("jack");
            std::fs::create_dir_all(&jack_dir).unwrap();
            std::fs::copy(&system, jack_dir.join(SONAME)).unwrap();
            let exe = std::env::current_exe().expect("the test binary has a path");
            let output = std::process::Command::new(exe)
                .args([
                    "--exact",
                    "jack_library::tests::a_preloaded_library_answers_the_bare_name",
                    "--nocapture",
                ])
                .env(CHILD, &jack_dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "child failed:\n{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        };
        let copy = PathBuf::from(child_dir).join(SONAME);
        assert_eq!(loaded_library(), None, "nothing has loaded a libjack yet");
        preload(&copy).unwrap();
        let name = CString::new(SONAME).unwrap();
        // SAFETY: a NUL-terminated name; the handle is left open.
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY) };
        assert!(!handle.is_null(), "the bare name resolves");
        let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
        let mapped: std::collections::BTreeSet<_> = maps
            .lines()
            .filter_map(|line| line.split_whitespace().nth(5))
            .filter(|path| path.contains("libjack.so"))
            .collect();
        assert_eq!(
            mapped.into_iter().collect::<Vec<_>>(),
            [copy.to_str().unwrap()],
            "only the preloaded copy is mapped"
        );
    }

    #[test]
    fn the_loaded_library_is_read_from_the_maps() {
        let maps = "\
7f00-7f01 r--p 00000000 08:01 1 /usr/lib/x86_64-linux-gnu/libjackserver.so.0.1.0
7f01-7f02 r--p 00000000 08:01 2 /usr/lib/x86_64-linux-gnu/pipewire-0.3/jack/libjack.so.0.3.1005
7f02-7f03 rw-p 00000000 00:00 0
";
        let path = loaded_library_in(maps).unwrap();
        assert_eq!(
            path,
            Path::new("/usr/lib/x86_64-linux-gnu/pipewire-0.3/jack/libjack.so.0.3.1005")
        );
        assert_eq!(
            describe(&path),
            "PipeWire's JACK (/usr/lib/x86_64-linux-gnu/pipewire-0.3/jack/libjack.so.0.3.1005)"
        );
        assert_eq!(
            describe(Path::new("/usr/lib/x86_64-linux-gnu/libjack.so.0.1.0")),
            "JACK (/usr/lib/x86_64-linux-gnu/libjack.so.0.1.0)"
        );
        assert_eq!(loaded_library_in("7f02-7f03 rw-p 00000000 00:00 0\n"), None);
    }
}
