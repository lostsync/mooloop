//! The plugin scanner, out of process, with its cache
//! (`docs/plans/plugin-hosting/05-the-scanner.md`).
//!
//! **A plugin that crashes or hangs while it is being scanned must not crash
//! or hang mooloop.** So mooloop never loads a plugin library to find out
//! what is in it. Each candidate file is handed to a child process --
//! mooloop's own binary, run as `mooloop --scan-plugin <path>` -- which loads
//! it, lists what its factory holds, prints one JSON report between two
//! marker lines, and exits. A child that crashes, exits non-zero, prints
//! nothing usable or runs past the timeout is recorded as a failure of that
//! one file ([`FailureKind`]), and the scan moves on.
//!
//! The result is kept in [`PluginCache`], a TOML file under the config
//! directory (`<config>/plugins.toml`, `mooloop_ui::plugin_cache_path`).
//! Entries are keyed by canonical path and carry the file's modification time
//! and size; a file whose two have not changed is never launched again,
//! **including one that failed**, so a plugin that crashes on load costs one
//! child per change of the file rather than one per startup.
//!
//! The same module holds the rules for where a scanned plugin may go
//! ([`main_port_effect_refusal`], [`main_port_source_refusal`]), which the
//! CLAP adapter applies again to a live instance.
//!
//! Nothing here runs on the audio thread. [`scan`] and [`run_child`] block
//! for as long as their children run.

use std::ffi::{CStr, OsString};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use clack_extensions::audio_ports::{AudioPortFlags, AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::gui::PluginGui;
use clack_extensions::note_ports::PluginNotePorts;
use clack_host::entry::PluginEntryError;
use mooloop_core::plugin::{PluginFormat, PluginRef};
use serde::{Deserialize, Serialize};

use crate::{instantiate, load_entry};

/// The argument that turns a mooloop binary into a scan child. It must come
/// first: `mooloop --scan-plugin <path>`.
pub const SCAN_FLAG: &str = "--scan-plugin";

/// Optional, after the path: `--deadline-ms <n>`. The child exits on its own
/// after this long, so a hung plugin cannot outlive a parent that died
/// without killing it.
pub const DEADLINE_FLAG: &str = "--deadline-ms";

/// How long a child gets before it is killed, unless the settings say
/// otherwise.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// The child's own exit status when its deadline passes (the one `timeout(1)`
/// uses).
pub const EXIT_DEADLINE: i32 = 124;

/// The child's exit status when its arguments are wrong.
pub const EXIT_USAGE: i32 = 2;

/// The line before the report on the child's stdout. A plugin is free to
/// print whatever it likes to stdout while it loads; the markers are what
/// keep that from being read as the report.
const REPORT_BEGIN: &str = "@@mooloop-scan-report-begin@@";
/// The line after the report on the child's stdout.
const REPORT_END: &str = "@@mooloop-scan-report-end@@";

/// The cache file's own version. A file with any other is ignored and the
/// next scan rebuilds it.
pub const CACHE_VERSION: u32 = 1;

/// How much of a failed child's stderr is kept as the failure's reason, in
/// characters from the end.
const STDERR_TAIL: usize = 600;

/// One plugin, as a scan found it in a file. This is also its entry in the
/// cache file, so a field added here needs `serde(default)` for the caches
/// already written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ScannedPlugin {
    /// The file (or macOS bundle) it lives in: what to hand
    /// [`crate::load_entry`].
    pub path: PathBuf,
    /// What a song saves about it.
    #[serde(flatten)]
    pub plugin: PluginRef,
    /// The plugin's own one-line description, empty when it gives none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The plugin's feature strings as it declared them (for CLAP,
    /// `instrument`, `audio-effect`, `note-effect`, `analyzer`, `stereo`, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Channel count of each audio input port, in the plugin's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_inputs: Vec<u32>,
    /// Channel count of each audio output port, in the plugin's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_outputs: Vec<u32>,
    /// Which of `audio_inputs` is the main input: the port the plugin flags
    /// `CLAP_AUDIO_PORT_IS_MAIN`, or 0 when it flags none ([`main_port`]).
    /// It indexes past the end when there are no inputs; read it through
    /// [`main_input_channels`](Self::main_input_channels). An entry without
    /// the key reads 0, and 0 is never written.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub main_audio_input: u32,
    /// Which of `audio_outputs` is the main output, as `main_audio_input`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub main_audio_output: u32,
    /// How many note input ports it has.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub note_inputs: u32,
    /// How many note output ports it has.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub note_outputs: u32,
    /// Whether it declares a GUI of its own.
    #[serde(default, skip_serializing_if = "is_false")]
    pub has_gui: bool,
    /// Set when the factory lists the plugin but it could not be created.
    /// Such a plugin is recorded, so it can be named, and is not offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The index of a plugin's main audio port in one direction, from each
/// port's `CLAP_AUDIO_PORT_IS_MAIN` flag in the plugin's order: the first
/// flagged port, or 0 when none is flagged. The scan records it as
/// [`ScannedPlugin::main_audio_input`] and `main_audio_output`, and the CLAP
/// adapter finds a live instance's main ports by the same rule, so the two
/// always agree.
pub fn main_port(is_main: impl IntoIterator<Item = bool>) -> u32 {
    is_main
        .into_iter()
        .position(|main| main)
        .map_or(0, |index| u32::try_from(index).unwrap_or(0))
}

/// The channel count of port `main` among `channels` (one count per port),
/// or `None` when the plugin has no port there: no main port at all.
pub fn main_channels(channels: &[u32], main: u32) -> Option<u32> {
    usize::try_from(main).ok().and_then(|index| channels.get(index)).copied()
}

/// One direction of a live plugin's audio ports, as the scan records them:
/// the channel count of each port in the plugin's order (0 for a port it
/// would not describe, so the indices stay the plugin's) and the index of
/// the main one ([`main_port`]). The scan's [`describe_ports`] reads its
/// ports through this, and so does the CLAP adapter's `check_ports`, so
/// both find the same main port. Main thread only, as every CLAP
/// `audio-ports` call is.
pub(crate) fn read_audio_ports(
    ports: &PluginAudioPorts,
    handle: &clack_host::prelude::PluginMainThreadHandle<'_>,
    is_input: bool,
) -> (Vec<u32>, u32) {
    let mut main = Vec::new();
    let channels = (0..ports.count(handle, is_input))
        .map(|index| {
            let mut buffer = AudioPortInfoBuffer::new();
            let info = ports.get(handle, index, is_input, &mut buffer);
            main.push(info.as_ref().is_some_and(|info| info.flags.contains(AudioPortFlags::IS_MAIN)));
            info.map_or(0, |info| info.channel_count)
        })
        .collect();
    (channels, main_port(main))
}

/// Why a scanned plugin cannot go where it was asked to, of one of two kinds
/// a browser treats differently: an unsupported plugin may be hidden, a
/// failed one is always shown, so a plugin that breaks after an update
/// never quietly disappears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// It was created, but has something mooloop does not handle yet: a
    /// main port that is not one or two channels, an instrument that takes
    /// no notes, the other role's plugin ([`main_port_effect_refusal`],
    /// [`main_port_source_refusal`]). The reason, in words.
    Unsupported(String),
    /// It could not be created when it was scanned
    /// ([`ScannedPlugin::error`]). The reason, in words.
    Failed(String),
}

impl Refusal {
    /// The reason, in words, whichever kind it is.
    pub fn reason(&self) -> &str {
        match self {
            Self::Unsupported(reason) | Self::Failed(reason) => reason,
        }
    }

    /// Whether something went wrong, rather than mooloop not supporting it.
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// Why a plugin with these CLAP `features` and main audio ports (the
/// channels of its main input and main output, `None` for no such port)
/// cannot be a device on a chain, or `None` when it can. The reason is a
/// short clause for the browser to show.
///
/// **The role is the plugin's own word, and the main ports only what the
/// host can wire.** A plugin says what it is with its features
/// (`audio-effect`, `instrument`); an effect is one that declares
/// `audio-effect`, or declares neither role and has a main input. It needs
/// a main input and a main output of one or two channels each. A mono input
/// is fed the chain's `(L + R) / 2` and a mono output goes to both sides.
/// Any other port -- a sidechain, a second bus -- is no reason to refuse
/// it: the host feeds an extra input silence and throws an extra output
/// away. A note input is allowed and gets no notes: a chain carries none. A
/// plugin that declares both roles may go in either place.
///
/// A refusal here is [`Refusal::Unsupported`]'s reason (the plugin loads,
/// mooloop cannot wire it), never [`Refusal::Failed`]'s.
pub fn main_port_effect_refusal(
    features: &[String],
    main_input: Option<u32>,
    main_output: Option<u32>,
) -> Option<String> {
    let has = |feature: &str| features.iter().any(|f| f == feature);
    let effect = has("audio-effect") || (!has("instrument") && main_input.is_some());
    if !effect {
        return Some("an instrument: it plays as a channel's source".into());
    }
    match main_input {
        None => return Some("it has no audio input".into()),
        Some(1 | 2) => {}
        Some(n) => return Some(format!("its main input has {n} channels; 1 or 2 are hosted")),
    }
    main_output_refusal(main_output)
}

/// Why a plugin with these `features`, main audio output (its channels,
/// `None` for none) and note inputs cannot be a channel's source, or `None`
/// when it can; *unsupported*, never *failed*, as for
/// [`main_port_effect_refusal`]. The role as [`main_port_effect_refusal`]
/// reads it: `instrument`, or a note input when it declares neither role. A
/// source needs a note input and a main output of one or two channels (mono
/// is copied to both sides). Every audio input it has, the main one
/// included, is fed silence -- a channel's source has nothing upstream of
/// it -- and every output but the main one is thrown away, so neither is a
/// reason to refuse it.
pub fn main_port_source_refusal(features: &[String], main_output: Option<u32>, note_inputs: u32) -> Option<String> {
    let has = |feature: &str| features.iter().any(|f| f == feature);
    let neither = !has("instrument") && !has("audio-effect");
    let instrument = has("instrument") || (neither && note_inputs > 0);
    if !instrument {
        return Some("an effect: it goes in a chain, not as a channel's source".into());
    }
    if note_inputs == 0 {
        return Some("it takes no notes".into());
    }
    main_output_refusal(main_output)
}

fn main_output_refusal(main_output: Option<u32>) -> Option<String> {
    match main_output {
        None => Some("it has no audio output".into()),
        Some(1 | 2) => None,
        Some(n) => Some(format!("its main output has {n} channels; 1 or 2 are hosted")),
    }
}

impl ScannedPlugin {
    /// Whether it declares the given feature string.
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    /// Whether it is an instrument (it plays notes into audio).
    pub fn is_instrument(&self) -> bool {
        self.has_feature("instrument")
    }

    /// Whether it is an audio effect.
    pub fn is_effect(&self) -> bool {
        self.has_feature("audio-effect")
    }

    /// The main input's channel count, or `None` when it has no audio input
    /// (or a cache names a main input it does not have).
    pub fn main_input_channels(&self) -> Option<u32> {
        main_channels(&self.audio_inputs, self.main_audio_input)
    }

    /// The main output's channel count, or `None` when it has no audio
    /// output, as [`main_input_channels`](Self::main_input_channels).
    pub fn main_output_channels(&self) -> Option<u32> {
        main_channels(&self.audio_outputs, self.main_audio_output)
    }

    /// Why it cannot be a device on a chain, or `None` when it can: when it
    /// could not be created, that ([`error`](Self::error)) as
    /// [`Refusal::Failed`]; otherwise [`main_port_effect_refusal`] on the
    /// main ports the scan recorded, as [`Refusal::Unsupported`].
    pub fn effect_refusal(&self) -> Option<Refusal> {
        self.creation_refusal().or_else(|| {
            main_port_effect_refusal(&self.features, self.main_input_channels(), self.main_output_channels())
                .map(Refusal::Unsupported)
        })
    }

    /// Why it cannot be a channel's source, or `None` when it can, of the
    /// kinds [`effect_refusal`](Self::effect_refusal) gives: the creation
    /// error first, then [`main_port_source_refusal`] on the main output and
    /// note inputs the scan recorded. `None` is what a browser offers as an
    /// instrument.
    pub fn source_refusal(&self) -> Option<Refusal> {
        self.creation_refusal().or_else(|| {
            main_port_source_refusal(&self.features, self.main_output_channels(), self.note_inputs)
                .map(Refusal::Unsupported)
        })
    }

    fn creation_refusal(&self) -> Option<Refusal> {
        self.error
            .as_ref()
            .map(|error| Refusal::Failed(format!("could not be created: {error}")))
    }

    /// Whether it could be created when it was scanned.
    pub fn is_usable(&self) -> bool {
        self.error.is_none()
    }
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Why a file yielded no plugins: the library would not load, it is not a
/// plugin this host can use, or the child running it failed. Every kind,
/// `Incompatible` included, is [`Refusal::Failed`]
/// ([`ScanFailure::refusal`]), never [`Refusal::Unsupported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    /// The child could not be started at all.
    Launch,
    /// The library would not load, or its entry refused to initialise.
    Load,
    /// It loaded, but is not something this host can use: another CLAP
    /// version, no plugin factory, or a factory that lists nothing.
    Incompatible,
    /// The child died -- a signal, or a non-zero exit -- or the parent lost
    /// track of it.
    Crashed,
    /// The child ran past the timeout and was killed.
    TimedOut,
    /// The child exited cleanly but printed no report that could be read.
    BadOutput,
}

/// A file that yielded no plugins, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFailure {
    /// Which of the ways to fail it was.
    pub kind: FailureKind,
    /// One line for a person: the child's own reason, or for a crash the
    /// exit status and the end of its stderr.
    pub reason: String,
}

impl ScanFailure {
    fn new(kind: FailureKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
        }
    }

    /// What the file's failure is as a refusal: [`Refusal::Failed`] with
    /// its reason, for every kind. `Incompatible` too: a plugin that an
    /// update left the wrong CLAP version is one that broke, not one mooloop
    /// chose not to host, and a browser must keep showing it.
    pub fn refusal(&self) -> Refusal {
        match self.kind {
            FailureKind::Launch
            | FailureKind::Load
            | FailureKind::Incompatible
            | FailureKind::Crashed
            | FailureKind::TimedOut
            | FailureKind::BadOutput => Refusal::Failed(self.reason.clone()),
        }
    }
}

/// What the child prints, as JSON between the markers ([`parse_report`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ChildReport {
    /// The file's factory listed its plugins.
    Scanned {
        /// The factory's plugins, at least one. A plugin that could not be
        /// created is here too, with its `error` set.
        plugins: Vec<ScannedPlugin>,
    },
    /// The file as a whole failed, as the child saw it.
    Failed {
        /// `Load` or `Incompatible`; the other kinds are the parent's to
        /// judge.
        kind: FailureKind,
        /// What went wrong, for the Plugins page.
        reason: String,
    },
}

/// One candidate file in the cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CachedFile {
    /// Canonical path: the key.
    pub path: PathBuf,
    /// Modification time in nanoseconds since the Unix epoch. For a macOS
    /// bundle, the newest of anything inside it.
    pub modified_ns: u64,
    /// Size in bytes. For a macOS bundle, the sum of what is inside it.
    pub size: u64,
    /// Set when the file yielded nothing; `plugins` is then empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<ScanFailure>,
    /// What the file holds, in its factory's order, usable or not.
    #[serde(default, rename = "plugin", skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<ScannedPlugin>,
}

#[derive(Serialize, Deserialize)]
struct CacheText {
    version: u32,
    #[serde(default, rename = "file")]
    files: Vec<CachedFile>,
}

/// Everything the last scan found, in search-path order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginCache {
    files: Vec<CachedFile>,
}

impl PluginCache {
    /// The cache at `path`. A file that is missing, unreadable, of another
    /// version or not valid is an empty cache: the next scan rebuilds it, and
    /// nothing a scan writes is worth refusing to start over.
    pub fn load(path: &Path) -> Self {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                mooloop_core::log_warn!(
                    "plugins",
                    "ignoring the plugin cache at {}: {error}",
                    path.display()
                );
                return Self::default();
            }
        };
        match Self::from_toml(&text) {
            Ok(cache) => cache,
            Err(why) => {
                mooloop_core::log_warn!(
                    "plugins",
                    "ignoring the plugin cache at {}: {why}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    /// Parse a cache file's text.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let parsed: CacheText = toml::from_str(text).map_err(|e| e.to_string())?;
        if parsed.version != CACHE_VERSION {
            return Err(format!(
                "version {} is not {CACHE_VERSION}",
                parsed.version
            ));
        }
        Ok(Self {
            files: parsed.files,
        })
    }

    /// The cache as a file's text.
    pub fn to_toml(&self) -> Result<String, String> {
        toml::to_string_pretty(&CacheText {
            version: CACHE_VERSION,
            files: self.files.clone(),
        })
        .map_err(|e| e.to_string())
    }

    /// Write the cache to `path`, through a temporary file and a rename, so
    /// a reader never sees half of one.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = self.to_toml().map_err(io::Error::other)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("toml.tmp");
        fs::write(&temporary, text)?;
        fs::rename(&temporary, path)
    }

    /// Every candidate file, in the order the last scan met them.
    pub fn files(&self) -> &[CachedFile] {
        &self.files
    }

    /// Whether it holds no files at all, failed ones included.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Every plugin found, usable or not ([`ScannedPlugin::is_usable`]).
    pub fn plugins(&self) -> impl Iterator<Item = &ScannedPlugin> {
        self.files.iter().flat_map(|file| file.plugins.iter())
    }

    /// Every file that yielded nothing, and why.
    pub fn failures(&self) -> impl Iterator<Item = (&Path, &ScanFailure)> {
        self.files
            .iter()
            .filter_map(|file| file.failed.as_ref().map(|f| (file.path.as_path(), f)))
    }

    /// Forget everything, so the next scan launches a child for every file
    /// rather than trusting a fingerprint. This is Rescan All, and the one
    /// way to pick up a plugin replaced by a build of the same size and
    /// modification time.
    pub fn clear(&mut self) {
        self.files.clear();
    }

    /// Forget every failure and keep every success, so the next scan tries
    /// only the files that failed again. Rescan All uses [`clear`](Self::clear),
    /// not this.
    pub fn clear_failures(&mut self) {
        self.files.retain(|file| file.failed.is_none());
    }

    /// The installed plugin a song's [`PluginRef`] names: same format and
    /// id, and usable. With more than one copy, the newest version wins, then
    /// the one met first in search-path order; the choice is logged when the
    /// copies' versions differ.
    pub fn resolve(&self, wanted: &PluginRef) -> Option<&ScannedPlugin> {
        let mut copies = self.plugins().filter(|found| {
            found.is_usable()
                && found.plugin.format == wanted.format
                && found.plugin.id == wanted.id
        });
        let mut best = copies.next()?;
        let mut differ = false;
        for copy in copies {
            if copy.plugin.version != best.plugin.version {
                differ = true;
            }
            if compare_versions(&copy.plugin.version, &best.plugin.version).is_gt() {
                best = copy;
            }
        }
        if differ {
            mooloop_core::log_info!(
                "plugins",
                "{} is installed more than once in different versions; using {} from {}",
                wanted.id,
                best.plugin.version,
                best.path.display()
            );
        }
        Some(best)
    }
}

/// Order two free-form version strings by their runs of digits, so
/// `1.10.0` is newer than `1.9.2`. Text between the numbers is ignored; a
/// version with more numbers is newer when the shared ones are equal.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn numbers(version: &str) -> Vec<u64> {
        version
            .split(|c: char| !c.is_ascii_digit())
            .filter(|run| !run.is_empty())
            .map(|run| run.parse().unwrap_or(u64::MAX))
            .collect()
    }
    numbers(a).cmp(&numbers(b))
}

/// Where CLAP plugins are looked for before any the user adds: CLAP's own
/// defaults, the distribution directories, then `CLAP_PATH`.
///
/// Linux: `~/.clap`, `/usr/lib/clap`, `/usr/lib64/clap` (Fedora's
/// `lsp-plugins-clap` and `clap-zam-plugins` install there),
/// `/usr/local/lib/clap`, and inside a Flatpak `/app/extensions/Plugins/clap`.
/// macOS: `~/Library/Audio/Plug-Ins/CLAP` and `/Library/Audio/Plug-Ins/CLAP`.
/// A directory that does not exist is skipped when scanned, not here.
pub fn default_search_paths() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut paths = Vec::new();
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = &home {
            paths.push(home.join("Library/Audio/Plug-Ins/CLAP"));
        }
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(home) = &home {
            paths.push(home.join(".clap"));
        }
        for dir in ["/usr/lib/clap", "/usr/lib64/clap", "/usr/local/lib/clap"] {
            paths.push(PathBuf::from(dir));
        }
        if Path::new("/.flatpak-info").exists() {
            paths.push(PathBuf::from("/app/extensions/Plugins/clap"));
        }
    }
    if let Some(clap_path) = std::env::var_os("CLAP_PATH") {
        paths.extend(std::env::split_paths(&clap_path).filter(|p| !p.as_os_str().is_empty()));
    }
    paths
}

/// The program a scan launches for each file, and what goes before the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildCommand {
    /// The executable.
    pub program: PathBuf,
    /// Its arguments before the path, ending in [`SCAN_FLAG`] for any binary
    /// that dispatches through [`run_child_from_args`].
    pub args: Vec<OsString>,
}

impl ChildCommand {
    /// The running binary, as `<it> --scan-plugin`. What the app uses: the
    /// child is the same file the user installed, so every package that
    /// ships mooloop ships the scanner.
    pub fn current_exe() -> io::Result<Self> {
        Ok(Self {
            program: std::env::current_exe()?,
            args: vec![SCAN_FLAG.into()],
        })
    }
}

/// What to scan, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanConfig {
    /// Searched in order, recursively. A directory named `*.clap` is a macOS
    /// bundle and is a candidate, not a directory to search.
    pub search_paths: Vec<PathBuf>,
    /// How long one child gets.
    pub timeout: Duration,
    /// What to launch for each file.
    pub child: ChildCommand,
}

impl ScanConfig {
    /// [`default_search_paths`] then `extra_paths`, launching the running
    /// binary.
    pub fn new(extra_paths: impl IntoIterator<Item = PathBuf>, timeout: Duration) -> io::Result<Self> {
        let mut search_paths = default_search_paths();
        search_paths.extend(extra_paths);
        Ok(Self {
            search_paths,
            timeout,
            child: ChildCommand::current_exe()?,
        })
    }
}

/// What one scan did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Candidate files found on the search paths.
    pub candidates: usize,
    /// Children launched: files that were new or had changed.
    pub launched: usize,
    /// Files whose cached result was kept because they had not changed.
    pub reused: usize,
    /// Plugins in the cache afterwards, usable or not.
    pub plugins: usize,
    /// Files in the cache afterwards that yielded nothing.
    pub failed: usize,
    /// Cache entries dropped because their file is gone.
    pub removed: usize,
}

/// Scan every candidate on `config.search_paths` into `cache`, launching a
/// child only for files that are new or have changed. Blocks until done, up
/// to `config.timeout` per launched file, so run it on a thread of its own.
/// `progress` is called before each file with its index, the total, and its
/// path.
///
/// Afterwards `cache` holds exactly the candidates, in search-path order; an
/// entry for a file that is gone is dropped. Does not save the cache; the
/// caller does, with [`PluginCache::save`].
pub fn scan(
    config: &ScanConfig,
    cache: &mut PluginCache,
    mut progress: impl FnMut(usize, usize, &Path),
) -> ScanSummary {
    let candidates = find_candidates(&config.search_paths);
    let mut summary = ScanSummary {
        candidates: candidates.len(),
        ..ScanSummary::default()
    };
    let mut previous = std::mem::take(&mut cache.files);
    for (index, path) in candidates.iter().enumerate() {
        progress(index, candidates.len(), path);
        let (modified_ns, size) = fingerprint(path).unwrap_or((0, 0));
        let cached = previous
            .iter()
            .position(|file| &file.path == path)
            .map(|at| previous.swap_remove(at));
        let entry = match cached {
            Some(file) if file.modified_ns == modified_ns && file.size == size => {
                summary.reused += 1;
                file
            }
            _ => {
                summary.launched += 1;
                let (plugins, failed) = match run_child(&config.child, path, config.timeout) {
                    Ok(plugins) => (plugins, None),
                    Err(failure) => {
                        mooloop_core::log_warn!(
                            "plugins",
                            "scanning {} failed ({:?}): {}",
                            path.display(),
                            failure.kind,
                            failure.reason
                        );
                        (Vec::new(), Some(failure))
                    }
                };
                CachedFile {
                    path: path.clone(),
                    modified_ns,
                    size,
                    failed,
                    plugins,
                }
            }
        };
        cache.files.push(entry);
    }
    summary.removed = previous.len();
    summary.plugins = cache.plugins().count();
    summary.failed = cache.failures().count();
    summary
}

/// Every `*.clap` under `roots`, canonical, each once, in the order met.
/// Directories are searched recursively (following symlinks, each real
/// directory once, at most eight deep); a `*.clap` directory is a bundle and
/// is not searched.
pub fn find_candidates(roots: &[PathBuf]) -> Vec<PathBuf> {
    fn is_clap(path: &Path) -> bool {
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("clap"))
    }
    fn walk(dir: &Path, depth: u32, seen_dirs: &mut Vec<PathBuf>, found: &mut Vec<PathBuf>) {
        let Ok(canonical) = fs::canonicalize(dir) else {
            return;
        };
        if depth > 8 || seen_dirs.contains(&canonical) {
            return;
        }
        seen_dirs.push(canonical);
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for path in entries {
            if is_clap(&path) {
                if let Ok(canonical) = fs::canonicalize(&path) {
                    if !found.contains(&canonical) {
                        found.push(canonical);
                    }
                }
            } else if path.is_dir() {
                walk(&path, depth + 1, seen_dirs, found);
            }
        }
    }
    let mut seen_dirs = Vec::new();
    let mut found = Vec::new();
    for root in roots {
        if is_clap(root) && root.exists() {
            if let Ok(canonical) = fs::canonicalize(root) {
                if !found.contains(&canonical) {
                    found.push(canonical);
                }
            }
        } else if root.is_dir() {
            walk(root, 0, &mut seen_dirs, &mut found);
        }
    }
    found
}

/// A file's modification time (ns since the epoch) and size. For a bundle
/// directory, the newest time and the total size of what is inside it, down
/// to six levels, so replacing the library inside a bundle counts as a
/// change.
fn fingerprint(path: &Path) -> io::Result<(u64, u64)> {
    fn modified_ns(meta: &fs::Metadata) -> u64 {
        meta.modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }
    fn visit(path: &Path, depth: u32, acc: &mut (u64, u64)) -> io::Result<()> {
        let meta = fs::metadata(path)?;
        acc.0 = acc.0.max(modified_ns(&meta));
        if meta.is_dir() {
            if depth < 6 {
                for entry in fs::read_dir(path)? {
                    visit(&entry?.path(), depth + 1, acc)?;
                }
            }
        } else {
            acc.1 = acc.1.saturating_add(meta.len());
        }
        Ok(())
    }
    let mut acc = (0, 0);
    visit(path, 0, &mut acc)?;
    Ok(acc)
}

/// Launch one child on `path` and read its report: the plugins it holds, each
/// with its `path` set to `path`, or why it yielded none. Blocks for as long
/// as the child runs, at most `timeout` (then the child is killed and the
/// failure is [`FailureKind::TimedOut`]), plus up to a second for each of
/// its two pipes to close.
pub fn run_child(
    command: &ChildCommand,
    path: &Path,
    timeout: Duration,
) -> Result<Vec<ScannedPlugin>, ScanFailure> {
    // The child's own deadline is later than ours, so the parent's kill is
    // what normally ends a hang; the deadline is for a parent that is gone.
    let deadline = timeout.saturating_mul(2) + Duration::from_secs(5);
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .arg(path)
        .arg(DEADLINE_FLAG)
        .arg(deadline.as_millis().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            ScanFailure::new(
                FailureKind::Launch,
                format!("could not start {}: {e}", command.program.display()),
            )
        })?;

    // Both pipes are drained on threads of their own: a plugin factory with
    // hundreds of entries writes more than a pipe holds, and a child blocked
    // on a full pipe would look exactly like a hang.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());

    let started = Instant::now();
    let status: ExitStatus = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ScanFailure::new(
                    FailureKind::Crashed,
                    format!("lost track of the scan process: {e}"),
                ));
            }
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ScanFailure::new(
                FailureKind::TimedOut,
                format!(
                    "no answer within {:.1} s; the scan process was killed",
                    timeout.as_secs_f64()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    // The pipes close when the child exits, unless something it started
    // still holds them; a second is plenty either way.
    let out = stdout.recv_timeout(Duration::from_secs(1)).unwrap_or_default();
    let err = stderr.recv_timeout(Duration::from_secs(1)).unwrap_or_default();

    if !status.success() {
        let tail = stderr_tail(&err);
        let reason = if tail.is_empty() {
            format!("the scan process ended with {status}")
        } else {
            format!("the scan process ended with {status}: {tail}")
        };
        return Err(ScanFailure::new(FailureKind::Crashed, reason));
    }
    match parse_report(&String::from_utf8_lossy(&out)) {
        Some(ChildReport::Scanned { mut plugins }) => {
            for plugin in &mut plugins {
                plugin.path = path.to_path_buf();
            }
            Ok(plugins)
        }
        Some(ChildReport::Failed { kind, reason }) => Err(ScanFailure::new(kind, reason)),
        None => Err(ScanFailure::new(
            FailureKind::BadOutput,
            "the scan process exited without a readable report",
        )),
    }
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (send, receive) = mpsc::channel();
    if let Some(mut pipe) = pipe {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            let _ = send.send(bytes);
        });
    }
    receive
}

fn stderr_tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    let start = text
        .char_indices()
        .rev()
        .nth(STDERR_TAIL)
        .map(|(at, _)| at)
        .unwrap_or(0);
    text[start..].trim().replace('\n', " / ")
}

/// The report in a child's stdout: the text between the last begin marker and
/// the end marker after it.
pub fn parse_report(stdout: &str) -> Option<ChildReport> {
    let begin = stdout.rfind(REPORT_BEGIN)? + REPORT_BEGIN.len();
    let end = begin + stdout[begin..].find(REPORT_END)?;
    serde_json::from_str(stdout[begin..end].trim()).ok()
}

/// If this process was started as a scan child (`<exe> --scan-plugin <path>
/// [--deadline-ms <n>]`), scan and return the exit status to leave with.
/// `None` for any other command line.
///
/// A binary calls this **first**, before logging, settings, audio or a
/// window: a scan child must touch nothing but the one file it was given.
pub fn run_child_from_args() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != SCAN_FLAG {
        return None;
    }
    let Some(path) = args.next() else {
        eprintln!("usage: mooloop {SCAN_FLAG} <path> [{DEADLINE_FLAG} <ms>]");
        return Some(EXIT_USAGE);
    };
    let mut deadline = None;
    while let Some(arg) = args.next() {
        if arg == DEADLINE_FLAG {
            deadline = args
                .next()
                .and_then(|ms| ms.to_str().and_then(|ms| ms.parse().ok()))
                .map(Duration::from_millis);
        }
    }
    Some(child_main(Path::new(&path), deadline))
}

/// The scan child: load `path`, describe what it holds, print the report
/// between its markers, and return the exit status. With a `deadline`, the
/// process exits on its own with [`EXIT_DEADLINE`] once it passes.
///
/// The library is never unloaded: the entry is leaked on purpose, because a
/// plugin that crashes in its `deinit` would otherwise turn a good report
/// into a failed scan, and the process is about to end anyway.
pub fn child_main(path: &Path, deadline: Option<Duration>) -> i32 {
    if let Some(deadline) = deadline {
        std::thread::spawn(move || {
            std::thread::sleep(deadline);
            std::process::exit(EXIT_DEADLINE);
        });
    }
    let report = describe_file(path);
    let Ok(json) = serde_json::to_string(&report) else {
        return 1;
    };
    let mut out = io::stdout().lock();
    let written = write!(out, "\n{REPORT_BEGIN}\n{json}\n{REPORT_END}\n").and_then(|()| out.flush());
    if written.is_ok() { 0 } else { 1 }
}

fn text(value: Option<&CStr>) -> String {
    value
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn describe_file(path: &Path) -> ChildReport {
    // SAFETY: this is the scan child. Loading an unknown library is exactly
    // what it is for, and whatever the library does happens in this process,
    // not in mooloop's.
    let entry = match unsafe { load_entry(path) } {
        Ok(entry) => entry,
        Err(PluginEntryError::IncompatibleClapVersion { plugin_version }) => {
            return ChildReport::Failed {
                kind: FailureKind::Incompatible,
                reason: format!("built for CLAP {plugin_version:?}, which this host cannot load"),
            };
        }
        Err(error) => {
            return ChildReport::Failed {
                kind: FailureKind::Load,
                reason: format!("the library would not load: {error}"),
            };
        }
    };
    let report = match entry.get_plugin_factory() {
        None => ChildReport::Failed {
            kind: FailureKind::Incompatible,
            reason: "the library has no CLAP plugin factory".into(),
        },
        Some(factory) => {
            let listed: Vec<_> = factory
                .plugin_descriptors()
                .filter_map(|descriptor| {
                    let id = text(Some(descriptor.id()?));
                    let name = text(descriptor.name());
                    Some(ScannedPlugin {
                        path: path.to_path_buf(),
                        plugin: PluginRef {
                            format: PluginFormat::Clap,
                            name: if name.is_empty() { id.clone() } else { name },
                            id,
                            vendor: text(descriptor.vendor()),
                            version: text(descriptor.version()),
                        },
                        description: text(descriptor.description()),
                        features: descriptor
                            .features()
                            .map(|f| f.to_string_lossy().into_owned())
                            .collect(),
                        audio_inputs: Vec::new(),
                        audio_outputs: Vec::new(),
                        main_audio_input: 0,
                        main_audio_output: 0,
                        note_inputs: 0,
                        note_outputs: 0,
                        has_gui: false,
                        error: None,
                    })
                })
                .collect();
            if listed.is_empty() {
                ChildReport::Failed {
                    kind: FailureKind::Incompatible,
                    reason: "the plugin factory lists no plugins".into(),
                }
            } else {
                ChildReport::Scanned {
                    plugins: listed.into_iter().map(|p| describe_ports(&entry, p)).collect(),
                }
            }
        }
    };
    std::mem::forget(entry);
    report
}

/// Create the plugin once to read what only an instance can say: its ports
/// and whether it has a GUI. It is never activated.
fn describe_ports(entry: &clack_host::prelude::PluginEntry, mut plugin: ScannedPlugin) -> ScannedPlugin {
    let Ok(id) = std::ffi::CString::new(plugin.plugin.id.clone()) else {
        plugin.error = Some("its id holds a NUL".into());
        return plugin;
    };
    let mut instance = match instantiate(entry, &id) {
        Ok(instance) => instance,
        Err(error) => {
            plugin.error = Some(format!("could not be created: {error}"));
            return plugin;
        }
    };
    let audio: Option<PluginAudioPorts> = instance.plugin_shared_handle().get_extension();
    let notes: Option<PluginNotePorts> = instance.plugin_shared_handle().get_extension();
    let gui: Option<PluginGui> = instance.plugin_shared_handle().get_extension();
    plugin.has_gui = gui.is_some();
    let handle = instance.plugin_handle();
    if let Some(audio) = audio {
        (plugin.audio_inputs, plugin.main_audio_input) = read_audio_ports(&audio, &handle, true);
        (plugin.audio_outputs, plugin.main_audio_output) = read_audio_ports(&audio, &handle, false);
    }
    if let Some(notes) = notes {
        plugin.note_inputs = notes.count(&handle, true);
        plugin.note_outputs = notes.count(&handle, false);
    }
    plugin
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plugin() -> ScannedPlugin {
        ScannedPlugin {
            path: PathBuf::from("/usr/lib64/clap/example.clap"),
            plugin: PluginRef {
                format: PluginFormat::Clap,
                id: "org.example.comp".into(),
                name: "Comp".into(),
                vendor: "Example".into(),
                version: "1.2.0".into(),
            },
            description: String::new(),
            features: vec!["audio-effect".into(), "stereo".into()],
            audio_inputs: vec![2],
            audio_outputs: vec![2],
            main_audio_input: 0,
            main_audio_output: 0,
            note_inputs: 0,
            note_outputs: 0,
            has_gui: true,
            error: None,
        }
    }

    /// The role comes from the features, the ports only for what can be
    /// wired, each place has its reason, and a plugin that declares both
    /// roles goes in either place.
    #[test]
    fn a_plugins_places_come_from_its_features_and_what_its_ports_can_wire() {
        let plugin = |features: &[&str], ins: &[u32], outs: &[u32], notes: u32| ScannedPlugin {
            features: features.iter().map(|f| (*f).to_owned()).collect(),
            audio_inputs: ins.to_vec(),
            audio_outputs: outs.to_vec(),
            note_inputs: notes,
            ..sample_plugin()
        };
        let places = |p: &ScannedPlugin| (p.effect_refusal().is_none(), p.source_refusal().is_none());

        // A plain effect, and one taking MIDI (a vocoder, a gated effect).
        assert_eq!(places(&plugin(&["audio-effect"], &[2], &[2], 0)), (true, false));
        assert_eq!(places(&plugin(&["audio-effect"], &[2], &[2], 1)), (true, false));
        // A synth; one with a sidechain input fed silence; a mono one.
        assert_eq!(places(&plugin(&["instrument"], &[], &[2], 1)), (false, true));
        assert_eq!(places(&plugin(&["instrument"], &[2], &[2], 1)), (false, true));
        assert_eq!(places(&plugin(&["instrument"], &[], &[1], 1)), (false, true));
        // Mono effects: every mix of one and two channels, by feature or by
        // an input and no role.
        for (ins, outs) in [([1], [1]), ([1], [2]), ([2], [1])] {
            assert_eq!(places(&plugin(&["audio-effect"], &ins, &outs, 0)), (true, false));
            assert_eq!(places(&plugin(&[], &ins, &outs, 0)), (true, false));
        }
        // Both roles: either place.
        assert_eq!(places(&plugin(&["instrument", "audio-effect"], &[2], &[2], 1)), (true, true));
        // Neither role: a note input makes it an instrument, none an effect.
        assert_eq!(places(&plugin(&[], &[], &[2], 1)), (false, true));
        assert_eq!(places(&plugin(&[], &[2], &[2], 0)), (true, false));
        // What the host cannot wire, with the reason.
        let no_notes = plugin(&["instrument"], &[], &[2], 0);
        let unsupported = |why: &str| Some(Refusal::Unsupported(why.into()));
        assert_eq!(no_notes.source_refusal(), unsupported("it takes no notes"));
        let surround = plugin(&["audio-effect"], &[2], &[6], 0);
        assert_eq!(
            surround.effect_refusal(),
            unsupported("its main output has 6 channels; 1 or 2 are hosted")
        );
        let generator = plugin(&["audio-effect"], &[], &[2], 0);
        assert_eq!(generator.effect_refusal(), unsupported("it has no audio input"));
        let silent = plugin(&["instrument"], &[], &[], 1);
        assert_eq!(silent.source_refusal(), unsupported("it has no audio output"));
        let synth = plugin(&["instrument"], &[], &[2], 1);
        assert_eq!(synth.effect_refusal(), unsupported("an instrument: it plays as a channel's source"));
        let failed = ScannedPlugin {
            error: Some("boom".into()),
            ..synth
        };
        // A plugin that could not be created has failed in either place,
        // whatever its ports would have allowed, and says so in the words
        // the browser always showed.
        for refusal in [failed.source_refusal(), failed.effect_refusal()] {
            assert_eq!(refusal, Some(Refusal::Failed("could not be created: boom".into())));
        }
    }

    /// Every way a whole file can fail is a failure a browser always shows,
    /// `Incompatible` (an update to another CLAP version) as much as a crash,
    /// with the scanner's reason as it was.
    #[test]
    fn every_file_failure_is_failed_never_unsupported() {
        use FailureKind::*;
        for kind in [Launch, Load, Incompatible, Crashed, TimedOut, BadOutput] {
            let failure = ScanFailure::new(kind, "no clap_entry");
            assert_eq!(failure.refusal(), Refusal::Failed("no clap_entry".into()), "{kind:?}");
        }
    }

    #[test]
    fn versions_compare_by_their_numbers() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("1.10.0", "1.9.2"), Greater);
        assert_eq!(compare_versions("1.2", "1.2.0"), Less);
        assert_eq!(compare_versions("v2.0-beta", "2.0"), Equal);
        assert_eq!(compare_versions("", "0.1"), Less);
    }

    #[test]
    fn a_cache_round_trips_through_its_toml() {
        let mut cache = PluginCache::default();
        cache.files.push(CachedFile {
            path: PathBuf::from("/usr/lib64/clap/example.clap"),
            modified_ns: 1_790_000_000_123_456_789,
            size: 14_000_000,
            failed: None,
            plugins: vec![sample_plugin()],
        });
        cache.files.push(CachedFile {
            path: PathBuf::from("/home/u/.clap/broken.clap"),
            modified_ns: 1,
            size: 0,
            failed: Some(ScanFailure::new(FailureKind::Crashed, "signal: 11")),
            plugins: Vec::new(),
        });
        let text = cache.to_toml().expect("serializes");
        assert!(text.contains("[[file.plugin]]"), "{text}");
        assert_eq!(PluginCache::from_toml(&text).expect("parses"), cache);
    }

    #[test]
    fn a_cache_of_another_version_is_refused() {
        assert!(PluginCache::from_toml("version = 99\n").is_err());
        assert!(PluginCache::from_toml("version = 1\n").expect("empty").is_empty());
    }

    #[test]
    fn resolve_prefers_the_newest_usable_copy() {
        let old = sample_plugin();
        let mut new = sample_plugin();
        new.path = PathBuf::from("/home/u/.clap/example.clap");
        new.plugin.version = "1.10.0".into();
        let mut broken = sample_plugin();
        broken.plugin.version = "9.0".into();
        broken.error = Some("could not be created".into());
        let mut cache = PluginCache::default();
        for (index, plugin) in [old, new.clone(), broken].into_iter().enumerate() {
            cache.files.push(CachedFile {
                path: PathBuf::from(format!("/f{index}")),
                modified_ns: 0,
                size: 0,
                failed: None,
                plugins: vec![plugin],
            });
        }
        let wanted = PluginRef {
            version: "1.0".into(),
            ..new.plugin.clone()
        };
        assert_eq!(cache.resolve(&wanted).map(|p| &p.path), Some(&new.path));
        let other = PluginRef {
            id: "org.example.other".into(),
            ..wanted
        };
        assert!(cache.resolve(&other).is_none());
    }

    #[test]
    fn a_report_is_found_between_its_markers_whatever_else_is_printed() {
        let report = ChildReport::Scanned {
            plugins: vec![sample_plugin()],
        };
        let json = serde_json::to_string(&report).expect("serializes");
        let stdout = format!(
            "plugin says hello\n{REPORT_BEGIN}\n{json}\n{REPORT_END}\nplugin says bye at exit\n"
        );
        assert_eq!(parse_report(&stdout), Some(report));
        assert_eq!(parse_report("nothing here"), None);
        assert_eq!(parse_report(&format!("{REPORT_BEGIN}\n{{\"half")), None);
    }
}
