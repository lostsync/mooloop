//! Versioned mooloop documents and sample-asset handling.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mooloop_core::{
    ChannelSetup, ChannelSource, DeviceKind, EffectKind, EffectRun, EffectSlotState, Kit,
    PluginSlotState, Project, SampleReference,
};
use serde::{Deserialize, Serialize};

pub mod factory;
pub mod integrity;

/// A kit or channel document brings no audio input.
///
/// An audio input names a track or channel of the song it was saved in, by
/// identity, and those identities mean nothing in the song a preset lands in
/// -- the same reason these documents carry no channel id
/// (`PROJECT_FORMAT.md`). Cleared on the way in rather than refused, and
/// silently, because it is normalization of a field the preset had no
/// business keeping rather than damage to report.
fn forget_audio_input(setup: &mut mooloop_core::ChannelSetup) {
    setup.channel.audio_input = mooloop_core::AudioInputSource::Off;
}

#[cfg(test)]
mod io_cost;

pub use factory::{
    rescope_modulation, seed_ds01_bank, seed_effect_bank, seed_effect_run_bank, seed_mlm1_bank,
    seed_mlp8_bank,
};
pub use integrity::{Diagnosis, Issue, Remedy};

pub const FORMAT_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "manifest.toml";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetMode {
    #[default]
    Embedded,
    Referenced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Song,
    Kit,
    Channel,
    Generator,
    /// One rack row: an [`EffectSlotState`] and nothing else.
    Effect,
    /// A container and everything inside it, in rack order.
    EffectRun,
}

impl DocumentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::Kit => "kit",
            Self::Channel => "channel",
            Self::Generator => "generator",
            Self::Effect => "effect",
            Self::EffectRun => "effect_run",
        }
    }
}

/// What an effect preset bundle holds, recorded in its manifest's `contains`
/// list. A reader that meets an entry it does not know refuses the bundle
/// rather than loading the half it understands.
///
/// This is the record `docs/plans/preset-system/00-status.md` made a
/// condition of building the device-level preset first: a later fragment
/// format can tell a one-row preset from a run of rows by reading this,
/// instead of guessing from the document type.
pub const EFFECT_PRESET_CONTAINS: &[&str] = &["effect_params"];

/// What a *container* preset holds.
///
/// It **adds** an entry rather than redefining `effect_params`, which is the
/// condition `docs/plans/preset-system/00-status.md` set on having built the
/// one-row preset first: a reader can tell a run from a row by reading this
/// instead of guessing from the document type, and a reader that predates
/// runs meets `effect_run`, does not know it, and refuses the bundle rather
/// than loading the first device and silently dropping the box.
///
/// The modulation a run drives is not in here, and the list is where it would
/// go if a modulator ever gets to live in a container.
pub const EFFECT_RUN_PRESET_CONTAINS: &[&str] = &["effect_params", "effect_run"];

/// What a hosted plugin device's preset holds (MOO-222): its row, and the
/// plugin behind it -- which plugin, its parameter list, its pinned ids and
/// its saved state -- in the envelope's `plugin` table.
///
/// Added rather than folded into `effect_params`, for the reason
/// [`EFFECT_RUN_PRESET_CONTAINS`] gives: 0.1.5 checks an effect bundle's list
/// against `["effect_params"]` alone, so it meets `effect_plugin`, does not
/// know it, and refuses the bundle rather than loading a row whose slot
/// names nothing in the song it lands in.
pub const EFFECT_PLUGIN_PRESET_CONTAINS: &[&str] = &["effect_params", "effect_plugin"];

/// The `contains` entry that marks a plugin device's preset.
const EFFECT_PLUGIN: &str = "effect_plugin";

/// Indexable metadata for a saved preset, carried alongside the document so
/// a future preset browser can list/group/filter without opening every
/// bundle's full document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetInfo {
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Which class of preset a bundle is, and which device it is for.
///
/// Three classes rather than a bare [`DeviceKind`], because a bare kind could
/// not name an effect at all and could not tell a whole-channel preset from a
/// generator-only one. This is the structural half of the browser taxonomy:
/// the list can be grouped by it before any browser exists to show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresetKind {
    Generator(DeviceKind),
    Channel(DeviceKind),
    Effect(EffectKind),
}

/// Summary of a preset bundle found by [`list_presets`], cheap to compute
/// because it only reads the manifest header and preset metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetSummary {
    pub path: PathBuf,
    pub name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub kind: PresetKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetWarning {
    pub channel: usize,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SaveReport {
    pub warnings: Vec<AssetWarning>,
    /// Problems found on the way out and corrected before writing. The
    /// document on disk reflects these; the one in memory does not until the
    /// caller applies the same repair.
    pub repairs: Vec<Issue>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadedDocument {
    Song(Project),
    Kit(Kit),
    Channel(Box<ChannelSetup>),
    Generator(Box<ChannelSource>),
    Effect(Box<EffectSlotState>),
    EffectRun(Box<EffectRun>),
    /// A hosted plugin device's preset (MOO-222): its row, whose slot is
    /// unassigned, and the plugin that row runs. Loading one mints a slot
    /// for `plugin` in the song it lands in.
    PluginEffect {
        effect: Box<EffectSlotState>,
        plugin: Box<PluginSlotState>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadReport {
    pub document: LoadedDocument,
    pub asset_mode: AssetMode,
    pub warnings: Vec<AssetWarning>,
    /// Problems found on the way in and corrected before handing the document
    /// over. A file that needed these opens; one that needed more does not.
    pub repairs: Vec<Issue>,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Encode(toml::ser::Error),
    /// Something wrong with the *request* rather than the document: a path
    /// with no parent directory, a bundle target that is a plain file.
    Invalid(String),
    /// Something wrong with the document that a repair pass could not put
    /// right. Carries every problem found, where it is, and what correcting
    /// it would have cost -- see [`Diagnosis::report`] for the copyable form.
    InvalidDocument(Box<Diagnosis>),
    UnsupportedVersion(u32),
    UnsupportedDocument(String),
    /// The manifest's `contains` list names something this reader does not
    /// understand. Refused whole rather than loaded in part: a preset that
    /// arrives missing half of itself is worse than one that does not open.
    UnsupportedContents(String),
}

impl Error {
    /// The full diagnostic text when there is one, for a bug report or a
    /// clipboard button. `None` for errors that are already one line.
    pub fn report(&self) -> Option<String> {
        match self {
            Self::InvalidDocument(diagnosis) => Some(diagnosis.report()),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Parse(error) => write!(f, "invalid manifest: {error}"),
            Self::Encode(error) => write!(f, "could not encode manifest: {error}"),
            Self::Invalid(message) => write!(f, "invalid document: {message}"),
            Self::InvalidDocument(diagnosis) => write!(f, "{diagnosis}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported format version {version}")
            }
            Self::UnsupportedDocument(kind) => write!(f, "unsupported document type {kind:?}"),
            Self::UnsupportedContents(entry) => {
                write!(f, "this preset contains {entry:?}, which this version cannot load")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<Diagnosis> for Error {
    fn from(value: Diagnosis) -> Self {
        Self::InvalidDocument(Box::new(value))
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<toml::de::Error> for Error {
    fn from(value: toml::de::Error) -> Self {
        Self::Parse(value)
    }
}

impl From<toml::ser::Error> for Error {
    fn from(value: toml::ser::Error) -> Self {
        Self::Encode(value)
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    format_version: u32,
    document_type: String,
    asset_mode: AssetMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preset: Option<PresetInfo>,
    /// What the document holds, for readers that need to know before they
    /// parse it. Empty for the document kinds that predate the field: their
    /// contents are implied by `document_type` and nothing else has ever
    /// been written under it. See [`EFFECT_PRESET_CONTAINS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    contains: Vec<String>,
    document: T,
    /// The plugin a plugin device's preset runs (MOO-222), and nothing for
    /// every other document, which writes exactly as it did before the field.
    /// See [`EFFECT_PLUGIN_PRESET_CONTAINS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin: Option<PluginSlotState>,
}

#[derive(Deserialize)]
struct Header {
    format_version: u32,
    document_type: String,
    #[serde(default)]
    preset: Option<PresetInfo>,
    #[serde(default)]
    contains: Vec<String>,
}

impl Header {
    /// The two fields `load_bundle` needs before it knows what `T` is, taken
    /// as lookups in an already-parsed table.
    ///
    /// Deserializing the whole `Header` would work and is what this replaces:
    /// it re-ran the TOML parser over the entire file to read four fields.
    /// The derived `Deserialize` is still what the preset lister and the
    /// tests use, where the files are small and one parse is the only parse.
    ///
    /// All four, not the two `load_bundle` reads first. `contains` is how an
    /// effect preset holding something this build does not understand is
    /// refused, and leaving it empty here made that refusal silently stop
    /// happening -- which is what
    /// `an_effect_preset_containing_something_unknown_is_refused` is for.
    fn from_table(table: &toml::Table) -> Result<Self, Error> {
        let format_version = table
            .get("format_version")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(Error::UnsupportedVersion(0))?;
        let document_type = table
            .get("document_type")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| Error::UnsupportedDocument(String::new()))?
            .to_string();
        let preset = table
            .get("preset")
            .cloned()
            .map(|value| value.try_into::<PresetInfo>())
            .transpose()?;
        let contains = table
            .get("contains")
            .and_then(toml::Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            format_version,
            document_type,
            preset,
            contains,
        })
    }
}

/// Writes `project` to `path`, correcting on the way out anything that can be
/// corrected without discarding work. The saved file is therefore the repaired
/// document, and [`SaveReport::repairs`] says what changed; only a problem
/// whose fix would delete notes or clips stops the write, and that comes back
/// as [`Error::InvalidDocument`] naming exactly where it is.
pub fn save_song(path: &Path, project: &Project, mode: AssetMode) -> Result<SaveReport, Error> {
    // A generator address with no kind would be written as one that means
    // "whatever device is there" (MOO-135). Only a decode makes one, and
    // every load fills it before handing the song over, so a song reaching
    // here with one came in by a path that skipped `assign_channel_ids`.
    debug_assert!(
        !project.has_unidentified_source_kinds(),
        "a song is being saved with a generator address that names no kind"
    );
    let mut document = project.clone();
    let diagnosis = integrity::repair_project(&mut document);
    if !diagnosis.is_usable() {
        return Err(diagnosis.into());
    }
    let mut report = save_song_file(path, &document, mode)?;
    report.repairs = diagnosis.issues;
    Ok(report)
}

/// Writes `project` exactly as it stands -- no repair, no validation -- with
/// `report` alongside it as `<path>.txt` explaining why it could not be saved
/// the ordinary way.
///
/// This is the escape hatch for the one case [`save_song`] cannot rescue: a
/// document whose only fix would delete the user's work. Refusing that save is
/// right, but refusing it *and* dropping the document leaves nothing to look
/// at afterwards, and these problems have all turned up on unsaved songs that
/// then could not be reproduced. So the bytes go to disk regardless, and the
/// question of what to do about them is left for later.
///
/// Assets are referenced rather than embedded: this runs on a failed save, in
/// front of a user who is already stuck, and copying a sample library first
/// would turn a diagnostic into a wait. The samples stay wherever they already
/// were, which is enough to read the document's structure back.
pub fn quarantine_song(path: &Path, project: &Project, report: &str) -> Result<PathBuf, Error> {
    save_song_file(path, project, AssetMode::Referenced)?;
    fs::write(path.with_extension("txt"), report)?;
    Ok(path.to_path_buf())
}

pub fn save_kit(path: &Path, kit: &Kit, mode: AssetMode) -> Result<SaveReport, Error> {
    let mut kit = kit.clone();
    let diagnosis = integrity::repair_setups(DocumentKind::Kit, &mut kit.channels);
    if !diagnosis.is_usable() {
        return Err(diagnosis.into());
    }
    let mut report = save_with_assets(
        path,
        DocumentKind::Kit,
        kit,
        mode,
        None,
        Vec::new(),
        None,
        |kit| {
            kit.channels
                .iter_mut()
                .map(|setup| &mut setup.source)
                .collect()
        },
    )?;
    report.repairs = diagnosis.issues;
    Ok(report)
}

pub fn save_channel(
    path: &Path,
    channel: &ChannelSetup,
    mode: AssetMode,
) -> Result<SaveReport, Error> {
    save_channel_with_preset(path, channel, mode, None)
}

/// Saves a channel bundle the same way [`save_channel`] does, additionally
/// carrying indexable preset metadata in the manifest when `info` is set.
pub fn save_channel_preset(
    path: &Path,
    channel: &ChannelSetup,
    info: PresetInfo,
    mode: AssetMode,
) -> Result<SaveReport, Error> {
    save_channel_with_preset(path, channel, mode, Some(info))
}

fn save_channel_with_preset(
    path: &Path,
    channel: &ChannelSetup,
    mode: AssetMode,
    preset: Option<PresetInfo>,
) -> Result<SaveReport, Error> {
    let mut channel = channel.clone();
    let diagnosis =
        integrity::repair_setups(DocumentKind::Channel, std::slice::from_mut(&mut channel));
    if !diagnosis.is_usable() {
        return Err(diagnosis.into());
    }
    let mut report = save_with_assets(
        path,
        DocumentKind::Channel,
        channel,
        mode,
        preset,
        Vec::new(),
        None,
        |channel| vec![&mut channel.source],
    )?;
    report.repairs = diagnosis.issues;
    Ok(report)
}

/// Saves a generator-only preset: just the [`ChannelSource`] (params +
/// sample reference for a sampler), no mixer/channel fields.
pub fn save_generator_preset(
    path: &Path,
    source: &ChannelSource,
    info: PresetInfo,
    mode: AssetMode,
) -> Result<SaveReport, Error> {
    let mut source = source.clone();
    let diagnosis = integrity::repair_source(DocumentKind::Generator, &mut source);
    if !diagnosis.is_usable() {
        return Err(diagnosis.into());
    }
    let mut report = save_with_assets(
        path,
        DocumentKind::Generator,
        source,
        mode,
        Some(info),
        Vec::new(),
        None,
        |source| vec![source],
    )?;
    report.repairs = diagnosis.issues;
    Ok(report)
}

/// Saves one rack row as a preset: the [`EffectSlotState`] alone.
///
/// The effect's kind is not stored separately, because
/// [`EffectParams::kind`](mooloop_core::EffectParams::kind) derives it from
/// the payload and a preset whose kind is implied by its parameters cannot
/// disagree with itself. No route and no [`mooloop_core::EffectTarget`] is
/// carried either, so nothing in the bundle names a channel and nothing has
/// to be re-scoped when it lands on another one.
///
/// An effect references no samples -- [`mooloop_core::BufferParams`] holds a
/// length in bars, a read offset and a crossfade, not audio -- so the asset
/// closure has nothing to prepare and `mode` only records itself in the
/// manifest.
pub fn save_effect_preset(
    path: &Path,
    effect: &EffectSlotState,
    info: PresetInfo,
    mode: AssetMode,
) -> Result<SaveReport, Error> {
    // A preset is what a device sounds like, not which device it is --
    // `EffectSlotState::id`'s own documentation says a preset carries no
    // identity. `save_effect_run_preset` below strips it; this one relied on
    // every caller having done so first.
    //
    // A plugin device's row is only a slot number, which names nothing in
    // the song the preset lands in: it saves with its plugin, through
    // `save_plugin_effect_preset` (MOO-222).
    if effect.kind() == EffectKind::Plugin {
        return Err(Error::Invalid(
            "a plugin device saves with its plugin, as a plugin preset".into(),
        ));
    }
    let mut effect = effect.with_id(mooloop_core::DeviceId::UNASSIGNED);
    let diagnosis = integrity::repair_effect(DocumentKind::Effect, &mut effect);
    if !diagnosis.is_usable() {
        return Err(diagnosis.into());
    }
    let mut report = save_with_assets(
        path,
        DocumentKind::Effect,
        effect,
        mode,
        Some(info),
        EFFECT_PRESET_CONTAINS
            .iter()
            .map(|entry| (*entry).to_string())
            .collect(),
        None,
        |_| Vec::new(),
    )?;
    report.repairs = diagnosis.issues;
    Ok(report)
}

/// Saves a hosted plugin device as a preset (MOO-222): its row, and the
/// plugin it runs -- which plugin, its parameter list, its pinned ids and its
/// state -- in the envelope's `plugin` table, under
/// [`EFFECT_PLUGIN_PRESET_CONTAINS`].
///
/// Both identities are stripped: the device's, for `save_effect_preset`'s
/// reason, and the plugin slot's, which is a key into the song the device
/// was taken from. Loading it mints a slot in the song it lands in.
///
/// `plugin.state` should be what the plugin holds *now*, not what the song
/// last captured; the caller asks the live instance.
pub fn save_plugin_effect_preset(
    path: &Path,
    effect: &EffectSlotState,
    plugin: &PluginSlotState,
    info: PresetInfo,
    mode: AssetMode,
) -> Result<SaveReport, Error> {
    if effect.kind() != EffectKind::Plugin {
        return Err(Error::Invalid(format!(
            "a plugin preset holds a plugin device, not a {}",
            effect.kind().label()
        )));
    }
    let mut effect = effect.with_id(mooloop_core::DeviceId::UNASSIGNED);
    effect.params = mooloop_core::EffectParams::Plugin(mooloop_core::PluginSlotId::UNASSIGNED);
    let diagnosis = integrity::repair_effect(DocumentKind::Effect, &mut effect);
    if !diagnosis.is_usable() {
        return Err(diagnosis.into());
    }
    let mut report = save_with_assets(
        path,
        DocumentKind::Effect,
        effect,
        mode,
        Some(info),
        EFFECT_PLUGIN_PRESET_CONTAINS
            .iter()
            .map(|entry| (*entry).to_string())
            .collect(),
        Some(plugin.clone()),
        |_| Vec::new(),
    )?;
    report.repairs = diagnosis.issues;
    Ok(report)
}

/// Save a container and its run as one preset.
///
/// Identities are stripped on the way out: a preset is what a group of
/// devices sounds like, and which devices they *are* belongs to the chain
/// they were lifted from. `load_effect_run` mints fresh ones on the way in.
pub fn save_effect_run_preset(
    path: &Path,
    run: &EffectRun,
    info: PresetInfo,
    mode: AssetMode,
) -> Result<SaveReport, Error> {
    let mut run = EffectRun {
        effects: run
            .effects
            .iter()
            .map(|effect| effect.with_id(mooloop_core::DeviceId::UNASSIGNED))
            .collect(),
    };
    if run.effects.is_empty() {
        return Err(Error::Invalid("an effect run preset holds no devices".into()));
    }
    if !run.effects[0].kind().is_container() {
        return Err(Error::Invalid(
            "an effect run preset must start with the container".into(),
        ));
    }
    if let Some(problem) = mooloop_core::span_problem(&run.effects) {
        return Err(Error::Invalid(problem));
    }
    let mut diagnosis = None;
    for effect in &mut run.effects {
        let found = integrity::repair_effect(DocumentKind::EffectRun, effect);
        if !found.is_usable() {
            return Err(found.into());
        }
        let repairs = found.issues;
        let entry = diagnosis.get_or_insert_with(Vec::new);
        entry.extend(repairs);
    }
    let mut report = save_with_assets(
        path,
        DocumentKind::EffectRun,
        run,
        mode,
        Some(info),
        EFFECT_RUN_PRESET_CONTAINS
            .iter()
            .map(|entry| (*entry).to_string())
            .collect(),
        None,
        |_| Vec::new(),
    )?;
    report.repairs = diagnosis.unwrap_or_default();
    Ok(report)
}

/// The folder inside a song's sidecar that recorded takes are copied into,
/// and the name of the shared folder a take is written to before any song
/// owns it (`audio-recording/06`).
///
/// **One name for both, and that is how a save tells a take from a sample.**
/// A take is recorded into a folder called `recordings` and nothing else in
/// the application writes one, so an owned sample whose file sits in a
/// `recordings` folder -- the shared one, or another song's on a Save As --
/// is copied into this song's `recordings/` under the name it already has.
/// Everything else goes into `samples/`.
pub const RECORDINGS_DIR: &str = "recordings";

/// The folder inside a song's sidecar, or a directory bundle, that samples
/// are copied into.
const SAMPLES_DIR: &str = "samples";

/// Whether `rest`, a path inside a sidecar, names one of the two folders a
/// save writes into. The loader follows nothing else.
fn is_asset_folder_path(rest: &Path) -> bool {
    rest.starts_with(SAMPLES_DIR) || rest.starts_with(RECORDINGS_DIR)
}

/// The sidecar folder a song at `song` keeps its embedded samples and takes
/// in, whether or not it exists yet.
pub fn song_assets_dir(song: &Path) -> Option<PathBuf> {
    song_assets_path(song).ok()
}

/// The two folders inside a song's sidecar that a save writes files into.
pub fn song_asset_folders(song: &Path) -> Vec<PathBuf> {
    song_assets_dir(song)
        .map(|assets| vec![assets.join(SAMPLES_DIR), assets.join(RECORDINGS_DIR)])
        .unwrap_or_default()
}

/// Writes the song file, and copies into its sidecar whatever it owns that is
/// not there yet.
///
/// **The sidecar is added to, never rebuilt.** Until 2026-09-22 every save
/// staged a whole new assets folder from what that save referenced, renamed
/// it over the old one and deleted the old one: a 500 MB embedded kit copied
/// 500 MB per Ctrl+S (report finding D9), and a take that one save copied in
/// and a retake replaced was deleted by the next save while the undo history
/// still pointed at it (MOO-89). Adam, 2026-09-22: *"that rebuild is a
/// problem, too. it keeps rewriting the filenames every save."*
///
/// So a file already in the sidecar stays exactly where it is, under the name
/// it has, and is not copied again; a file new to the song is copied in once,
/// under a name nothing in the sidecar has, and never over an existing file.
/// A file the song stops using stays too, until the clean-up dialog
/// (`recording.clean-up`) is asked to move it to the trash.
///
/// A save that fails removes what it copied in, and leaves the song file and
/// everything already in the sidecar as they were.
fn save_song_file(path: &Path, project: &Project, mode: AssetMode) -> Result<SaveReport, Error> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("song path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    let target_assets = song_assets_path(path)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mooloop");
    let staging_file = parent.join(format!(".{stem}.tmp-{}-{nonce}", std::process::id()));

    let mut added = Vec::<PathBuf>::new();
    let result = (|| {
        let mut document = project.clone();
        let mut report = SaveReport::default();
        let mut copied = HashMap::<PathBuf, PathBuf>::new();
        for (index, channel) in document.channels.iter_mut().enumerate() {
            prepare_song_asset(
                index,
                &mut channel.setup.source,
                path,
                &target_assets,
                mode,
                &mut copied,
                &mut added,
                &mut report.warnings,
            )?;
        }
        let envelope = Envelope {
            format_version: FORMAT_VERSION,
            document_type: DocumentKind::Song.as_str().into(),
            asset_mode: mode,
            preset: None,
            contains: Vec::new(),
            plugin: None,
            document,
        };
        write_synced(&staging_file, toml::to_string_pretty(&envelope)?.as_bytes())?;
        // Read back from the disk and parsed before anything is renamed: a
        // save may only put in place a file the loader will open (MOO-92).
        parse_manifest(&fs::read_to_string(&staging_file)?)?;
        // What this save copied into the sidecar is on the disk too, and so
        // are the names it copied them under, before the song names them.
        for directory in added.iter().filter_map(|file| file.parent()) {
            sync_dir(directory)?;
        }
        replace_song_file(path, &staging_file, nonce)?;
        Ok(report)
    })();

    if staging_file.exists() {
        let _ = fs::remove_file(&staging_file);
    }
    if result.is_err() {
        // Only what this save added: the song file on disk still names
        // everything else in the sidecar.
        for file in &added {
            let _ = fs::remove_file(file);
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn prepare_song_asset(
    channel: usize,
    source: &mut ChannelSource,
    target: &Path,
    target_assets: &Path,
    mode: AssetMode,
    copied: &mut HashMap<PathBuf, PathBuf>,
    added: &mut Vec<PathBuf>,
    warnings: &mut Vec<AssetWarning>,
) -> Result<(), Error> {
    for reference in sample_references_mut(source) {
        prepare_song_reference(
            channel,
            reference,
            target,
            target_assets,
            mode,
            copied,
            added,
            warnings,
        )?;
    }
    Ok(())
}

/// Every sample reference a source holds: a sampler's own, then each of its
/// key zones' (MOO-14). The asset walkers take them all the same way, so a
/// zone's file is embedded, referenced and resolved by exactly the base's
/// rules.
fn sample_references_mut(source: &mut ChannelSource) -> Vec<&mut SampleReference> {
    match source {
        ChannelSource::Sampler(sampler) => std::iter::once(&mut sampler.sample)
            .chain(sampler.zones.iter_mut().map(|zone| &mut zone.sample))
            .collect(),
        _ => Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_song_reference(
    channel: usize,
    reference: &mut SampleReference,
    target: &Path,
    target_assets: &Path,
    mode: AssetMode,
    copied: &mut HashMap<PathBuf, PathBuf>,
    added: &mut Vec<PathBuf>,
    warnings: &mut Vec<AssetWarning>,
) -> Result<(), Error> {
    let SampleReference::File { path, embedded } = reference else {
        return Ok(());
    };

    let source = path.clone();
    // Compared resolved, not as spelled (MOO-179): a song loaded through one
    // spelling of its path and saved through another -- a `..`, a relative
    // path from the command line, a symlinked parent -- made a sample already
    // in its own sidecar look foreign, and it was copied in again under
    // another `NN-`.
    let resolved_source = resolved_path(&source);
    let resolved_assets = resolved_path(target_assets);
    let keep_owned = *embedded
        && (resolved_source.starts_with(resolved_path(target))
            || resolved_source.starts_with(&resolved_assets));
    let parent = target.parent().expect("validated song parent");
    // **A sample the bundle already owns cannot be un-embedded, and says
    // so.** Un-embedding for real means copying the bytes out to somewhere
    // the user has chosen, which is a gesture that does not exist;
    // `docs/LOOSE_ENDS.md` carries it. Before the warning, unticking "Embed
    // assets" and saving produced no warning, no status message and no
    // change, so `CURRENT.md`'s "embedded and referenced asset policies are
    // available per save" was true only of a song that had never been
    // embedded. A warning rather than a refusal, because the save itself is
    // correct and the rest of the document does follow the mode.
    //
    // **And a sample the song owns but has not stored yet is embedded too.**
    // `embedded` means *owned by the song*. A recorded take is owned from the
    // moment it lands (`audio-recording/04`), but it sits in the shared
    // recordings folder until a save copies it in. Referencing it there
    // instead would leave the song depending on a folder whose unused takes
    // can be moved to the trash (`audio-recording/06`). The same rule stops a
    // Save As in Referenced mode pointing into another song's sidecar.
    if mode == AssetMode::Referenced && *embedded {
        warnings.push(AssetWarning {
            channel,
            path: source.clone(),
            message: if keep_owned {
                "sample stays embedded: the bundle holds the only copy of it".into()
            } else {
                "sample stays embedded: the song owns it".into()
            },
        });
    }
    if mode == AssetMode::Referenced && !*embedded {
        if !source.is_file() {
            warnings.push(AssetWarning {
                channel,
                path: source.clone(),
                message: "referenced sample is missing".into(),
            });
        }
        *path = pathdiff::diff_paths(&source, parent).unwrap_or(source);
        *embedded = false;
        return Ok(());
    }

    if !source.is_file() {
        warnings.push(AssetWarning {
            channel,
            path: source.clone(),
            message: "sample could not be embedded because it is missing".into(),
        });
        *path = pathdiff::diff_paths(&source, parent).unwrap_or(source);
        *embedded = false;
        return Ok(());
    }

    let asset_name = PathBuf::from(
        target_assets
            .file_name()
            .expect("song assets path has a file name"),
    );
    // **Already in this song's sidecar: stays where it is, as it is.** No
    // copy and no new name. Before 2026-09-22 this is where a sample the
    // bundle owned was copied into a fresh folder under a name worked out
    // again, and a mistake in that working-out once grew `00-kick.wav` into
    // `00-00-kick.wav` by three bytes a Ctrl+S until the song could not be
    // saved at all. A file in the sidecar but outside its two folders is not
    // one a save wrote, and is copied in like any other.
    if let Some(rest) = resolved_source
        .strip_prefix(&resolved_assets)
        .ok()
        .filter(|rest| is_asset_folder_path(rest))
    {
        *path = asset_name.join(rest);
        *embedded = true;
        return Ok(());
    }

    let canonical = source.canonicalize().unwrap_or_else(|_| source.clone());
    let relative = if let Some(relative) = copied.get(&canonical) {
        relative.clone()
    } else {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .map(sanitize_preset_name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "sample.wav".into());
        let is_take = source
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|folder| folder == RECORDINGS_DIR);
        let folder = if is_take { RECORDINGS_DIR } else { SAMPLES_DIR };
        // A take keeps the name it was recorded under, which already says
        // when and on what. So does a file coming out of a legacy directory
        // bundle, which was given its `NN-` when it was embedded there.
        // Anything else is prefixed with its channel, once, as it is copied
        // in, so two `kick.wav`s from two folders start out apart.
        let wanted = if is_take || keep_owned {
            name
        } else {
            format!("{channel:02}-{name}")
        };
        let directory = target_assets.join(folder);
        fs::create_dir_all(&directory)?;
        let leaf = copy_new_file(&source, &directory, &wanted)?;
        added.push(directory.join(&leaf));
        let relative = asset_name.join(folder).join(&leaf);
        copied.insert(canonical, relative.clone());
        relative
    };
    *path = relative;
    *embedded = true;
    Ok(())
}

/// `path` with symlinks, `.` and `..` resolved, as far as it exists: a file
/// not written yet (a song on its first save, its sidecar) resolves its
/// nearest existing ancestor and keeps the rest as spelled. Only for
/// comparing; nothing is written under the resolved spelling.
fn resolved_path(path: &Path) -> PathBuf {
    if let Ok(resolved) = path.canonicalize() {
        return resolved;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            resolved_path(parent).join(name)
        }
        _ => path.to_path_buf(),
    }
}

/// `wanted`, or `wanted` with `-2`, `-3`, ... before its extension, whichever
/// is the first name nothing in `directory` has.
///
/// A sidecar keeps files the song no longer uses -- an undo may want them
/// back -- so a new file can arrive under a name an old one already has. It
/// must never be copied over it.
fn unclaimed_name(directory: &Path, wanted: &str) -> String {
    mooloop_core::file_names::candidates(wanted)
        .find(|candidate| !directory.join(candidate).exists())
        .expect("an unbounded search finds a free name")
}

/// Copy `source` into `directory` as `wanted`, or the first of its
/// [`candidates`](mooloop_core::file_names::candidates) nothing has, and
/// return the name it landed under.
///
/// Nothing ever sees a half-copied file under its final name: the bytes go
/// to a hidden sibling first and are moved into place. And nothing already
/// there is ever replaced (MOO-243): the move is
/// [`rename_no_replace`](mooloop_core::file_names::rename_no_replace), which
/// the OS refuses atomically when the name is taken, and a refusal moves on
/// to the next candidate. So a file that appears under a name after anything
/// checked it -- another process saving into the same sidecar -- keeps it.
fn copy_new_file(source: &Path, directory: &Path, wanted: &str) -> Result<String, Error> {
    use mooloop_core::file_names::{candidates, rename_no_replace};
    let partial = directory.join(format!(".{wanted}.part-{}", std::process::id()));
    let placed = fs::copy(source, &partial)
        .and_then(|_| fs::File::open(&partial)?.sync_all())
        .and_then(|_| {
            for leaf in candidates(wanted) {
                match rename_no_replace(&partial, &directory.join(&leaf)) {
                    Ok(()) => return Ok(leaf),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            unreachable!("the candidates never run out")
        });
    if placed.is_err() {
        let _ = fs::remove_file(&partial);
    }
    placed.map_err(Error::Io)
}

/// What a song's sidecar directory is called, after the song's own file name.
///
/// Spelled once because two functions need it from opposite ends:
/// [`song_assets_path`] builds *this* song's, and [`embedded_bundle_path`]
/// recognises the one a document was written against -- which, after a
/// rename, is a different name for the same directory.
const ASSETS_SUFFIX: &str = "-assets";

fn song_assets_path(path: &Path) -> Result<PathBuf, Error> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("song path has no parent".into()))?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::Invalid("song path has no file name".into()))?;
    let mut assets_name = name.to_os_string();
    assets_name.push(ASSETS_SUFFIX);
    Ok(parent.join(assets_name))
}

fn remove_path(path: &Path) -> Result<(), std::io::Error> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Put the staged song file at `target`, and keep what was there as
/// `<name>.bak`.
///
/// **One rename, and the song is never missing** (MOO-92). A plain file is
/// replaced by `rename(staging, target)`, which POSIX makes atomic: a reader,
/// a crash or a power cut sees the old song or the new one, never neither.
/// Until 2026-09-23 the old file was moved aside first and the staged one
/// renamed onto the now-empty path, so there was a window with no song at
/// all, and nothing had been flushed to the disk, so the "new" file a crash
/// left could be empty. The staged file is `sync_all`ed by the caller and the
/// folder is synced here, after the rename, so the rename itself survives.
///
/// The previous version is kept as a hard link, made before the rename under
/// a name this save alone uses and then renamed onto `<name>.bak`, so two
/// saves never share a backup path and a `.bak` is always a whole file.
///
/// The song file only. Its sidecar is written to in place and never swapped
/// out (see [`save_song_file`]). `target` may be a legacy directory-style
/// song, which a file cannot be renamed over; that one is moved to
/// `<name>.bak` first, and its samples have already been copied into the
/// sidecar by then.
fn replace_song_file(target: &Path, staging_file: &Path, nonce: u128) -> Result<(), Error> {
    let parent = target.parent().expect("validated song parent");
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mooloop");
    let kept = parent.join(format!("{name}.bak"));

    if target.is_dir() {
        if kept.exists() {
            remove_path(&kept)?;
        }
        fs::rename(target, &kept)?;
        let placed = injected_fault(Step::Place)
            .and_then(|()| fs::rename(staging_file, target).map_err(Error::Io));
        if let Err(error) = placed {
            let _ = fs::rename(&kept, target);
            return Err(error);
        }
        sync_dir(parent)?;
        return Ok(());
    }

    if target.exists() {
        let link = parent.join(format!(".{name}.bak-{}-{nonce}", std::process::id()));
        // A hard link costs nothing and leaves the old inode where the old
        // bytes are; a filesystem without them gets a copy.
        let linked = injected_fault(Step::KeepOld).and_then(|()| {
            fs::hard_link(target, &link)
                .or_else(|_| {
                    fs::copy(target, &link)?;
                    fs::File::open(&link)?.sync_all()
                })
                .map_err(Error::Io)
        });
        let named = linked.and_then(|()| {
            injected_fault(Step::NameBackup)?;
            fs::rename(&link, &kept).map_err(Error::Io)
        });
        if let Err(error) = named {
            let _ = fs::remove_file(&link);
            return Err(error);
        }
    }
    injected_fault(Step::Place)?;
    fs::rename(staging_file, target)?;
    injected_fault(Step::SyncDirectory)?;
    sync_dir(parent)?;
    Ok(())
}

/// Write `bytes` to a new file at `path`, and have them on the disk rather
/// than in the page cache before returning.
fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Flush a directory's entries, so a rename or a new file inside it survives
/// a crash. Unix only: elsewhere a directory cannot be opened to sync.
fn sync_dir(directory: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(directory)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Ok(())
    }
}

/// Every file under `directory`, synced, then every folder under it.
fn sync_tree(directory: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            sync_tree(&path)?;
        } else {
            fs::File::open(&path)?.sync_all()?;
        }
    }
    sync_dir(directory)
}

/// The steps of putting a staged file in place, each of which a test can
/// make fail (MOO-121). `Place` is the rename that makes the new version the
/// song; the steps before it must leave the previous version as the song,
/// and the one after it must leave the previous version as `.bak`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// Linking (or copying) the version being replaced to a private name.
    KeepOld,
    /// Renaming that copy to `<name>.bak`.
    NameBackup,
    /// The rename that puts the new version in place.
    Place,
    /// Syncing the directory so the rename survives a power cut.
    SyncDirectory,
}

#[cfg(test)]
thread_local! {
    /// Fails the next save at this step.
    static FAIL_AT: std::cell::Cell<Option<Step>> = const { std::cell::Cell::new(None) };
}

fn injected_fault(step: Step) -> Result<(), Error> {
    #[cfg(test)]
    if FAIL_AT.with(|fail| fail.get() == Some(step)) {
        FAIL_AT.with(|fail| fail.set(None));
        return Err(Error::Io(std::io::Error::other(format!("injected fault at {step:?}"))));
    }
    let _ = step;
    Ok(())
}

// One argument per envelope field it writes, and the asset closure; a
// struct holding three of them would only move the list.
#[allow(clippy::too_many_arguments)]
fn save_with_assets<T, F>(
    path: &Path,
    kind: DocumentKind,
    mut document: T,
    mode: AssetMode,
    preset: Option<PresetInfo>,
    contains: Vec<String>,
    plugin: Option<PluginSlotState>,
    setups: F,
) -> Result<SaveReport, Error>
where
    T: Serialize,
    F: FnOnce(&mut T) -> Vec<&mut ChannelSource>,
{
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("bundle path has no parent".into()))?;
    fs::create_dir_all(parent)?;
    if path.exists() && !path.is_dir() {
        return Err(Error::Invalid(format!(
            "bundle target is not a directory: {}",
            path.display()
        )));
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mooloop");
    let staging = parent.join(format!(".{stem}.tmp-{}-{nonce}", std::process::id()));
    fs::create_dir(&staging)?;

    let result = (|| {
        let mut report = SaveReport::default();
        let mut copied = HashMap::<PathBuf, PathBuf>::new();
        for (index, setup) in setups(&mut document).into_iter().enumerate() {
            prepare_setup_asset(
                index,
                setup,
                path,
                &staging,
                mode,
                &mut copied,
                &mut report.warnings,
            )?;
        }
        let envelope = Envelope {
            format_version: FORMAT_VERSION,
            document_type: kind.as_str().into(),
            asset_mode: mode,
            preset,
            contains,
            plugin,
            document,
        };
        let manifest = toml::to_string_pretty(&envelope)?;
        write_synced(&staging.join(MANIFEST_FILE), manifest.as_bytes())?;
        parse_manifest(&fs::read_to_string(staging.join(MANIFEST_FILE))?)?;
        sync_tree(&staging)?;
        replace_bundle(path, &staging, nonce)?;
        Ok(report)
    })();

    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn prepare_setup_asset(
    channel: usize,
    source: &mut ChannelSource,
    target: &Path,
    staging: &Path,
    mode: AssetMode,
    copied: &mut HashMap<PathBuf, PathBuf>,
    warnings: &mut Vec<AssetWarning>,
) -> Result<(), Error> {
    for reference in sample_references_mut(source) {
        prepare_setup_reference(channel, reference, target, staging, mode, copied, warnings)?;
    }
    Ok(())
}

fn prepare_setup_reference(
    channel: usize,
    reference: &mut SampleReference,
    target: &Path,
    staging: &Path,
    mode: AssetMode,
    copied: &mut HashMap<PathBuf, PathBuf>,
    warnings: &mut Vec<AssetWarning>,
) -> Result<(), Error> {
    let SampleReference::File { path, embedded } = reference else {
        return Ok(());
    };

    let source = path.clone();
    let keep_owned = *embedded && source.starts_with(target);
    if mode == AssetMode::Referenced && !keep_owned {
        if !source.is_file() {
            warnings.push(AssetWarning {
                channel,
                path: source.clone(),
                message: "referenced sample is missing".into(),
            });
        }
        let relative = pathdiff::diff_paths(&source, target).unwrap_or(source);
        *path = relative;
        *embedded = false;
        return Ok(());
    }

    if !source.is_file() {
        warnings.push(AssetWarning {
            channel,
            path: source.clone(),
            message: "sample could not be embedded because it is missing".into(),
        });
        *path = pathdiff::diff_paths(&source, target).unwrap_or(source);
        *embedded = false;
        return Ok(());
    }

    let canonical = source.canonicalize().unwrap_or_else(|_| source.clone());
    let relative = if let Some(relative) = copied.get(&canonical) {
        relative.clone()
    } else {
        let assets = staging.join("samples");
        fs::create_dir_all(&assets)?;
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .map(sanitize_preset_name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "sample.wav".into());
        // As in `prepare_song_asset`: a sample the bundle already owns keeps
        // its leaf, or the prefix accumulates on every save.
        let wanted = if keep_owned {
            name
        } else {
            format!("{channel:02}-{name}")
        };
        // Unclaimed rather than as wanted: a sampler's key zones (MOO-14)
        // are several files on one channel, and two of them may share a
        // name from two folders, which would otherwise copy over each other.
        let relative = PathBuf::from("samples").join(unclaimed_name(&assets, &wanted));
        fs::copy(&source, staging.join(&relative))?;
        copied.insert(canonical, relative.clone());
        relative
    };
    *path = relative;
    *embedded = true;
    Ok(())
}

/// Sanitizes a user-facing name (preset name, sample file name) into a
/// filesystem-safe string: only alphanumerics, `.`, `-`, and `_` survive.
pub fn sanitize_preset_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// Put a staged kit or preset bundle at `target`.
///
/// A directory cannot be renamed over a directory that has files in it, so
/// the old bundle is moved aside first -- under a name this save alone uses
/// (MOO-92: two saves once shared `.name.backup-PID` and deleted each other's
/// backup) -- and removed once the new one is in place and synced.
fn replace_bundle(target: &Path, staging: &Path, nonce: u128) -> Result<(), Error> {
    let parent = target.parent().expect("validated bundle parent");
    if !target.exists() {
        injected_fault(Step::Place)?;
        fs::rename(staging, target)?;
        sync_dir(parent)?;
        return Ok(());
    }

    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mooloop");
    let backup = parent.join(format!(".{name}.backup-{}-{nonce}", std::process::id()));
    fs::rename(target, &backup)?;
    let placed =
        injected_fault(Step::Place).and_then(|()| fs::rename(staging, target).map_err(Error::Io));
    if let Err(error) = placed {
        let _ = fs::rename(&backup, target);
        return Err(error);
    }
    sync_dir(parent)?;
    fs::remove_dir_all(backup)?;
    Ok(())
}

/// A manifest's text, parsed and checked as far as it can be without the
/// files it names: the header, the format version and the typed document.
///
/// Shared by [`load_bundle`] and by the save path, which reads a staged
/// manifest back through it before anything is renamed (MOO-92), so a save
/// can only put in place a file the loader will open.
fn parse_manifest(manifest: &str) -> Result<(LoadedDocument, AssetMode), Error> {
    // Parsed once, not twice. The header has to be read before the document
    // type is known and therefore before `T` is, and this used to mean
    // running the whole file through the TOML parser for the two fields and
    // then again for everything -- which on a three-megabyte song was most of
    // the time spent opening it. The table is the parse; the header fields
    // are two lookups in it, and the envelope is deserialized from the same
    // table rather than from the text again.
    let table: toml::Table = manifest.parse()?;
    let header = Header::from_table(&table)?;
    if header.format_version != FORMAT_VERSION {
        return Err(Error::UnsupportedVersion(header.format_version));
    }

    let parsed = match header.document_type.as_str() {
        "song" => {
            let envelope: Envelope<Project> = table.try_into()?;
            validate_envelope(&envelope, "song")?;
            (LoadedDocument::Song(envelope.document), envelope.asset_mode)
        }
        "kit" => {
            let envelope: Envelope<Kit> = table.try_into()?;
            validate_envelope(&envelope, "kit")?;
            (LoadedDocument::Kit(envelope.document), envelope.asset_mode)
        }
        "channel" => {
            let envelope: Envelope<ChannelSetup> = table.try_into()?;
            validate_envelope(&envelope, "channel")?;
            (
                LoadedDocument::Channel(Box::new(envelope.document)),
                envelope.asset_mode,
            )
        }
        "generator" => {
            let envelope: Envelope<ChannelSource> = table.try_into()?;
            validate_envelope(&envelope, "generator")?;
            (
                LoadedDocument::Generator(Box::new(envelope.document)),
                envelope.asset_mode,
            )
        }
        "effect" => {
            // Checked before the document is parsed at all: a `contains`
            // entry this reader does not know means the bundle holds more
            // than an `EffectSlotState`, and parsing that part alone would
            // be exactly the partial load the list exists to prevent.
            let known = effect_contains(&header.contains);
            validate_contains(&header.contains, known)?;
            let envelope: Envelope<EffectSlotState> = table.try_into()?;
            validate_envelope(&envelope, "effect")?;
            let document = if known == EFFECT_PLUGIN_PRESET_CONTAINS {
                plugin_effect_document(envelope.document, envelope.plugin)?
            } else {
                LoadedDocument::Effect(Box::new(envelope.document))
            };
            (document, envelope.asset_mode)
        }
        "effect_run" => {
            validate_contains(&header.contains, EFFECT_RUN_PRESET_CONTAINS)?;
            let envelope: Envelope<EffectRun> = table.try_into()?;
            validate_envelope(&envelope, "effect_run")?;
            (
                LoadedDocument::EffectRun(Box::new(envelope.document)),
                envelope.asset_mode,
            )
        }
        other => return Err(Error::UnsupportedDocument(other.into())),
    };
    Ok(parsed)
}

pub fn load_bundle(path: &Path) -> Result<LoadReport, Error> {
    let manifest_path = if path.is_dir() {
        path.join(MANIFEST_FILE)
    } else {
        path.to_path_buf()
    };
    let manifest = fs::read_to_string(&manifest_path)?;
    let (mut document, asset_mode) = parse_manifest(&manifest)?;

    // Repair runs after the padding steps below rather than before them,
    // because a bank that is merely short is the legitimate on-disk shape of
    // an older song rather than damage, and padding it first keeps the two
    // from being confused. A file that needs more than repair can offer does
    // not open -- but that is now the only case that does not.
    let mut warnings = Vec::new();
    let diagnosis = match &mut document {
        LoadedDocument::Song(project) => {
            // Before the repair pass, because that pass judges every route
            // and lane against the chain it names, and until this has run a
            // chain written by an older version holds no identities at all.
            project.assign_device_ids();
            // Beside it, and for the same reason: until this has run a song
            // written before channels had identities holds none, and every
            // saved address that names another channel resolves to nothing.
            // A song with no ids takes positions, which is what those
            // addresses already meant.
            project.assign_channel_ids();
            // Tracks the same way: a song written before tracks had
            // identities takes its positions.
            project.assign_track_ids();
            // And after that, because it looks a lane's buffer up by identity
            // to find how many bars of history the old offset was a fraction
            // of. Before the repair pass, because that pass judges a lane
            // against the descriptor table, where id 0 no longer exists.
            project.migrate_retired_buffer_offset();
            // Before the repair pass for the same reason: until this has run,
            // a song written under the Linear strip-volume curve reads every
            // volume lane, route and binding 6 dB and more off (MOO-131).
            project.migrate_linear_strip_volume();
            for (index, channel) in project.channels.iter_mut().enumerate() {
                resolve_setup_asset(path, index, &mut channel.setup.source, &mut warnings)?;
                channel.normalize_automation();
                channel.recompute_next_note_id();
            }
            integrity::repair_project(project)
        }
        LoadedDocument::Kit(kit) => {
            for (index, setup) in kit.channels.iter_mut().enumerate() {
                setup.assign_device_ids();
                forget_audio_input(setup);
                resolve_setup_asset(path, index, &mut setup.source, &mut warnings)?;
            }
            integrity::repair_setups(DocumentKind::Kit, &mut kit.channels)
        }
        LoadedDocument::Channel(setup) => {
            setup.assign_device_ids();
            forget_audio_input(setup);
            resolve_setup_asset(path, 0, &mut setup.source, &mut warnings)?;
            integrity::repair_setups(DocumentKind::Channel, std::slice::from_mut(setup.as_mut()))
        }
        LoadedDocument::Generator(source) => {
            resolve_setup_asset(path, 0, source, &mut warnings)?;
            integrity::repair_source(DocumentKind::Generator, source)
        }
        // Nothing to resolve: an effect carries no sample reference.
        LoadedDocument::Effect(effect) => integrity::repair_effect(DocumentKind::Effect, effect),
        // The plugin's own state is its own, and never judged here: only the
        // row's host settings are (MOO-222).
        LoadedDocument::PluginEffect { effect, .. } => {
            integrity::repair_effect(DocumentKind::Effect, effect)
        }
        LoadedDocument::EffectRun(run) => integrity::repair_effect_run(run),
    };
    if !diagnosis.is_usable() {
        return Err(diagnosis.into());
    }
    Ok(LoadReport {
        document,
        asset_mode,
        warnings,
        repairs: diagnosis.issues,
    })
}

/// Scans `dir` for one level of preset bundles (`*.mooloop-generator` /
/// `*.mooloop-channel` / `*.mooloop-effect` directories) and returns a
/// summary for each, sorted by `(category, name)`. Bundles that fail to
/// parse, are missing preset metadata, or aren't a
/// `generator`/`channel`/`effect` document are silently skipped rather than
/// failing the whole scan. Returns an empty list if `dir` doesn't exist yet.
pub fn list_presets(dir: &Path) -> Vec<PresetSummary> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut summaries: Vec<PresetSummary> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| summarize_preset(&entry.path()))
        .collect();
    summaries.sort_by(|a, b| (&a.category, &a.name).cmp(&(&b.category, &b.name)));
    summaries
}

fn summarize_preset(path: &Path) -> Option<PresetSummary> {
    if !path.is_dir() {
        return None;
    }
    let manifest = fs::read_to_string(path.join(MANIFEST_FILE)).ok()?;
    let header: Header = toml::from_str(&manifest).ok()?;
    if header.format_version != FORMAT_VERSION {
        return None;
    }
    let preset = header.preset?;
    let kind = match header.document_type.as_str() {
        "generator" => {
            let envelope: Envelope<ChannelSource> = toml::from_str(&manifest).ok()?;
            PresetKind::Generator(envelope.document.kind())
        }
        "channel" => {
            let envelope: Envelope<ChannelSetup> = toml::from_str(&manifest).ok()?;
            PresetKind::Channel(envelope.document.kind())
        }
        "effect" => {
            // A bundle this version could not load is left out of the list
            // rather than offered and then refused.
            let known = effect_contains(&header.contains);
            validate_contains(&header.contains, known).ok()?;
            let envelope: Envelope<EffectSlotState> = toml::from_str(&manifest).ok()?;
            if known == EFFECT_PLUGIN_PRESET_CONTAINS {
                plugin_effect_document(envelope.document, envelope.plugin).ok()?;
                PresetKind::Effect(EffectKind::Plugin)
            } else {
                PresetKind::Effect(envelope.document.kind())
            }
        }
        "effect_run" => {
            validate_contains(&header.contains, EFFECT_RUN_PRESET_CONTAINS).ok()?;
            let envelope: Envelope<EffectRun> = toml::from_str(&manifest).ok()?;
            // A run preset belongs to the container it starts with, so it
            // lists on that container's rail -- a chain's beside the chain's
            // presets, a layer's beside the layer's (`containers/10`).
            // Nothing else can be at the head of a well-formed run, and a
            // bundle whose head is something else is left out rather than
            // offered and then refused.
            let head = envelope.document.effects.first()?.kind();
            head.is_container().then_some(PresetKind::Effect(head))?
        }
        _ => return None,
    };
    Some(PresetSummary {
        path: path.to_path_buf(),
        name: preset.name,
        category: preset.category,
        tags: preset.tags,
        kind,
    })
}

/// The `contains` list an `effect` bundle is checked against: a plugin
/// device's when it says it holds a plugin, the one-row list otherwise.
fn effect_contains(contains: &[String]) -> &'static [&'static str] {
    if contains.iter().any(|entry| entry == EFFECT_PLUGIN) {
        EFFECT_PLUGIN_PRESET_CONTAINS
    } else {
        EFFECT_PRESET_CONTAINS
    }
}

/// A plugin device's preset, from its row and its `plugin` table, or why it
/// is not one. Refused whole, never half loaded: a row with no plugin would
/// land as a device that runs nothing, and a plugin whose row is some other
/// kind has nowhere to run.
fn plugin_effect_document(
    effect: EffectSlotState,
    plugin: Option<PluginSlotState>,
) -> Result<LoadedDocument, Error> {
    let Some(plugin) = plugin else {
        return Err(Error::Invalid(
            "this plugin preset does not say which plugin it is for".into(),
        ));
    };
    if effect.kind() != EffectKind::Plugin {
        return Err(Error::Invalid(format!(
            "this plugin preset holds a {}, not a plugin device",
            effect.kind().label()
        )));
    }
    if plugin.plugin.id.is_empty() {
        return Err(Error::Invalid(
            "this plugin preset names a plugin with no identifier".into(),
        ));
    }
    Ok(LoadedDocument::PluginEffect {
        effect: Box::new(effect),
        plugin: Box::new(plugin),
    })
}

/// Every entry in a manifest's `contains` list must be one this reader
/// understands. An empty list is accepted for compatibility with bundles
/// written before the field existed, though every effect preset carries one.
fn validate_contains(contains: &[String], known: &[&str]) -> Result<(), Error> {
    match contains
        .iter()
        .find(|entry| !known.contains(&entry.as_str()))
    {
        Some(unknown) => Err(Error::UnsupportedContents(unknown.clone())),
        None => Ok(()),
    }
}

fn validate_envelope<T>(envelope: &Envelope<T>, expected: &str) -> Result<(), Error> {

    if envelope.format_version != FORMAT_VERSION {
        return Err(Error::UnsupportedVersion(envelope.format_version));
    }
    if envelope.document_type != expected {
        return Err(Error::UnsupportedDocument(envelope.document_type.clone()));
    }
    Ok(())
}

fn resolve_setup_asset(
    bundle: &Path,
    channel: usize,
    source: &mut ChannelSource,
    warnings: &mut Vec<AssetWarning>,
) -> Result<(), Error> {
    for reference in sample_references_mut(source) {
        resolve_reference(bundle, channel, reference, warnings)?;
    }
    Ok(())
}

fn resolve_reference(
    bundle: &Path,
    channel: usize,
    reference: &mut SampleReference,
    warnings: &mut Vec<AssetWarning>,
) -> Result<(), Error> {
    let SampleReference::File { path, embedded } = reference else {
        return Ok(());
    };
    if *embedded {
        match embedded_bundle_path(bundle, path) {
            Some(inside) => {
                if inside != *path {
                    warnings.push(AssetWarning {
                        channel,
                        path: path.clone(),
                        message: format!(
                            "embedded sample read from this song's own assets \
                             folder, {}, rather than the one the document names",
                            inside.display()
                        ),
                    });
                    *path = inside;
                }
            }
            None => {
                return Err(Error::Invalid(format!(
                    "channel {channel} has unsafe embedded path {}",
                    path.display()
                )))
            }
        }
    }
    let resolved = if path.is_absolute() {
        path.clone()
    } else if bundle.is_dir() {
        bundle.join(&*path)
    } else {
        bundle
            .parent()
            .expect("loaded song file has a parent")
            .join(&*path)
    };
    if !resolved.is_file() {
        warnings.push(AssetWarning {
            channel,
            path: resolved.clone(),
            message: "sample is missing".into(),
        });
    }
    *path = resolved;
    Ok(())
}

/// Where inside `bundle` an embedded reference actually points, or `None` if
/// it points somewhere this loader may not follow.
///
/// **A song that was renamed still opens.** The check used to require the
/// stored path to begin with *this song's exact file name* followed by
/// `-assets`, which is precisely what renaming breaks: rename the pair the
/// only sane way, in a file manager and both together, and the song refused
/// to open at all -- `Error::Invalid`, so the whole document was rejected
/// rather than one sample warned about, and recovery meant hand-editing TOML.
/// The name equality was never what made the path safe. The
/// `Component::Normal | CurDir` filter is: it admits no `..`, no root and no
/// prefix, so the path cannot leave the directory it is joined to, whatever
/// the first component is called.
///
/// So the shape is checked and the *name* is substituted: a first component
/// ending in `-assets`, a second of `samples` or `recordings`, and the
/// answer is that path with the first component replaced by the sidecar this
/// song actually has.
/// A rename therefore self-repairs, which is what a user expects and is the
/// only option that leaves the song playing -- merely relaxing the equality
/// would open the song with every sample missing, turning a brick into a
/// silent loss. The caller reports the substitution, and the next save writes
/// the corrected path.
///
/// The check stays **lexical**: a symlink under `samples/` still escapes,
/// which `PROJECT_FORMAT.md` records and `LOOSE_ENDS.md` still carries.
fn embedded_bundle_path(bundle: &Path, path: &Path) -> Option<PathBuf> {
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    // A directory bundle *is* the song, so its assets sit directly inside it
    // and there is no sidecar name to substitute.
    if bundle.is_dir() {
        return path.starts_with(SAMPLES_DIR).then(|| path.to_path_buf());
    }
    let sidecar = song_assets_path(bundle)
        .ok()
        .and_then(|assets| assets.file_name().map(PathBuf::from))?;
    let mut components = path.components().filter(|component| {
        !matches!(component, Component::CurDir)
    });
    let stored = components.next()?.as_os_str().to_str()?.to_string();
    if !stored.ends_with(ASSETS_SUFFIX) {
        return None;
    }
    let rest: PathBuf = components.collect();
    is_asset_folder_path(&rest).then(|| sidecar.join(rest))
}

/// Whether `project` is already exactly what the format stores, with no
/// correction needed. Saving does not require this -- it repairs first -- so
/// this is for callers that want to know the document is pristine.
pub fn validate_project(project: &Project) -> Result<(), Error> {
    let diagnosis = integrity::inspect_project(project);
    if diagnosis.is_clean() {
        Ok(())
    } else {
        Err(diagnosis.into())
    }
}

/// The channel-bank counterpart of [`validate_project`].
pub fn validate_setups(setups: &[ChannelSetup]) -> Result<(), Error> {
    let diagnosis = integrity::inspect_setups(DocumentKind::Kit, setups);
    if diagnosis.is_clean() {
        Ok(())
    } else {
        Err(diagnosis.into())
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{
        AutomationLane, AutomationPoint, DrumSynthParams, MlM1Params, MlP8Params, MonoSynthParams,
        NoteEvent,
        ParamAddr, PatternMeta, PatternPlacement, ProjectColor, MAX_CHOKE_GROUP,
    };
    use tempfile::tempdir;

    /// **An asset never replaces a file that took its name after the name
    /// was chosen** (MOO-243). The name is picked the way a save used to
    /// pick it, then another writer puts a file there, then the copy lands:
    /// the other file is untouched, and the asset is under the next free
    /// name, which the copy reports.
    #[test]
    fn an_asset_copy_moves_past_a_file_that_appeared_under_its_name() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("kick.wav");
        std::fs::write(&source, b"the new asset").unwrap();
        let directory = temp.path().join("samples");
        std::fs::create_dir_all(&directory).unwrap();
        let wanted = "01-kick.wav";

        let chosen = unclaimed_name(&directory, wanted);
        assert_eq!(chosen, wanted);
        std::fs::write(directory.join(&chosen), b"someone else's file").unwrap();

        let landed = copy_new_file(&source, &directory, wanted).unwrap();
        assert_eq!(landed, "01-kick-2.wav");
        assert_eq!(
            std::fs::read(directory.join(&chosen)).unwrap(),
            b"someone else's file",
            "the file that appeared was replaced"
        );
        assert_eq!(std::fs::read(directory.join(&landed)).unwrap(), b"the new asset");
        // No part file is left behind.
        let names: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "stray files: {names:?}");
    }

    /// **Saving twice must not rename the sample twice.**
    ///
    /// The app reloads the bundle after a save and writes the resolved paths
    /// back into the live session, so the next save sees a path that is
    /// already inside the sidecar and already carries its `NN-`. Prefixing
    /// again grew the name three bytes per Ctrl+S -- `00-kick.wav`,
    /// `00-00-kick.wav`, ... -- until it hit `NAME_MAX` and the song could
    /// not be saved at all, permanently, because the long name was in the
    /// manifest and a restart reloaded it.
    ///
    /// Two saves, not one: a single round trip is what the old test checked,
    /// and a single round trip is exactly what this bug survives.
    #[test]
    fn an_embedded_sample_keeps_its_name_across_repeated_saves() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("kick.wav");
        std::fs::write(&source, b"RIFF....WAVEfmt ").unwrap();

        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.sampler_state_mut().unwrap().sample =
            mooloop_core::SampleReference::File {
                path: source.clone(),
                embedded: true,
            };

        let leaf_of = |project: &Project| {
            let mooloop_core::SampleReference::File { path, .. } =
                &project.channels[0].setup.sampler_state().unwrap().sample
            else {
                panic!("the channel holds a file reference");
            };
            path.file_name().unwrap().to_string_lossy().into_owned()
        };

        for round in 1..=4 {
            save_song(&bundle, &project, AssetMode::Embedded).unwrap();
            // What the application does after every save: reload, and take the
            // resolved paths back into the document it will save next.
            let LoadedDocument::Song(reloaded) = load_bundle(&bundle).unwrap().document else {
                panic!("a song bundle loads as a song");
            };
            project = reloaded;
            assert_eq!(
                leaf_of(&project),
                "00-kick.wav",
                "the prefix accumulated on round {round}"
            );
        }
    }

    /// **MOO-179: the same file, spelled another way, is still the song's
    /// own.** The containment checks in `prepare_song_asset` are lexical, so
    /// the suspect was a song loaded through one spelling of its path and
    /// saved through another -- a `..` in it, or a symlinked parent (a
    /// portal, `/home` -> `/var/home`) -- making a sample already in the
    /// sidecar look foreign and earn another `NN-`. And a name that grew
    /// before the fix stays as it is and stops growing.
    #[test]
    fn an_embedded_sample_keeps_its_name_whichever_way_the_path_is_spelled() {
        let temp = tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let source = root.join("kick.wav");
        std::fs::write(&source, b"RIFF....WAVEfmt ").unwrap();
        let plain = root.join("song.mooloop");
        let dotted = root.join("sub").join("..").join("song.mooloop");
        let mut spellings = vec![plain.clone(), dotted];
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&root, root.join("link")).unwrap();
            spellings.push(root.join("link").join("song.mooloop"));
        }

        let leaf_of = |project: &Project| {
            let mooloop_core::SampleReference::File { path, .. } =
                &project.channels[0].setup.sampler_state().unwrap().sample
            else {
                panic!("the channel holds a file reference");
            };
            path.file_name().unwrap().to_string_lossy().into_owned()
        };
        let samples_in_sidecar = || {
            std::fs::read_dir(root.join("song.mooloop-assets").join("samples"))
                .unwrap()
                .count()
        };

        let mut project = Project::default();
        project.channels[0].setup.sampler_state_mut().unwrap().sample =
            mooloop_core::SampleReference::File {
                path: source,
                embedded: true,
            };
        save_song(&plain, &project, AssetMode::Embedded).unwrap();
        // Every pair: loaded through one spelling, saved through another.
        for load in &spellings {
            for save in &spellings {
                let LoadedDocument::Song(reloaded) = load_bundle(load).unwrap().document else {
                    panic!("a song");
                };
                save_song(save, &reloaded, AssetMode::Embedded).unwrap();
                let LoadedDocument::Song(saved) = load_bundle(&plain).unwrap().document else {
                    panic!("a song");
                };
                assert_eq!(
                    leaf_of(&saved),
                    "00-kick.wav",
                    "loaded as {}, saved as {}",
                    load.display(),
                    save.display()
                );
                assert_eq!(samples_in_sidecar(), 1, "and no second copy");
            }
        }

        // A song whose name had already grown before the fix: it keeps the
        // long name and gets no longer.
        let grown = root.join("song.mooloop-assets").join("samples").join("08-08-08-ohh.wav");
        std::fs::write(&grown, b"RIFF....WAVEfmt ").unwrap();
        project.channels[0].setup.sampler_state_mut().unwrap().sample =
            mooloop_core::SampleReference::File {
                path: grown,
                embedded: true,
            };
        for _ in 0..3 {
            save_song(&plain, &project, AssetMode::Embedded).unwrap();
            let LoadedDocument::Song(reloaded) = load_bundle(&plain).unwrap().document else {
                panic!("a song");
            };
            project = reloaded;
            assert_eq!(leaf_of(&project), "08-08-08-ohh.wav");
        }
        assert_eq!(samples_in_sidecar(), 2);
    }

    #[test]
    fn song_round_trip_retains_hidden_notes_and_playlist() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project {
            swing_percent: 66,
            ..Project::default()
        };
        project.pattern_lengths.push(8);
        project.channels[0]
            .notes
            .push(vec![NoteEvent::new(7, 200, 24, 64, 91)]);
        project.channels[0].recompute_next_note_id();
        project.channels[0].normalize_automation();
        project.playlist.push(PatternPlacement::new(1, 384));
        project.current_pattern = 1;

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert!(bundle.is_file());
        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.document, LoadedDocument::Song(project));
    }

    /// **Names and colours are content, so they survive the round trip.**
    ///
    /// The pattern name half of this is a fix rather than a feature: patterns
    /// could be renamed from 2026-09-07, `Session::pattern_names` held the
    /// name, and `Project` had nowhere to put it -- so every reopened song
    /// came back with "Pattern 1", and nothing failed.
    #[test]
    fn channel_and_pattern_identity_round_trips() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.channel.name = "Kick".into();
        project.channels[0].setup.channel.color = Some(ProjectColor::new(0x84, 0xCC, 0x16));
        project.pattern_lengths.push(16);
        project.channels[0].notes.push(Vec::new());
        project.channels[0].automation.push(Vec::new());
        project.pattern_meta = vec![
            PatternMeta { name: "Verse".into(), color: None },
            PatternMeta {
                name: "Chorus".into(),
                color: Some(ProjectColor::new(0xEA, 0xB3, 0x08)),
            },
        ];

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.document, LoadedDocument::Song(project));
    }

    /// **A song written before channels had identities needs no migration.**
    /// Both fields are defaulted and skipped when unset, so such a song is
    /// byte-identical to one saved now with none; on the way in each channel
    /// takes its **position**, which is what every address in that song that
    /// said `channel = 3` already meant. `FORMAT_VERSION` does not move.
    ///
    /// Written through `save_song_file` rather than `save_song` on purpose:
    /// the repair pass is the thing under test on the way back out, so the
    /// file has to be made without it.
    #[test]
    fn a_song_with_no_channel_identities_loads_with_its_positions() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("old.mooloop");
        let project = Project {
            channels: vec![
                mooloop_core::ProjectChannel::sampler(0, 1),
                mooloop_core::ProjectChannel::sampler(1, 1),
                mooloop_core::ProjectChannel::sampler(2, 1),
            ],
            next_channel_id: 0,
            // Tracks without identities too, which is what makes the
            // manifest below the file an older version wrote.
            buses: (0..2).map(mooloop_core::BusSetup::new).collect(),
            next_track_id: 0,
            ..Project::default()
        };
        save_song_file(&bundle, &project, AssetMode::Embedded).unwrap();

        let manifest = fs::read_to_string(&bundle).unwrap();
        assert!(
            !manifest.contains("next_channel_id"),
            "the mint was written into a file that predates it:\n{manifest}"
        );
        assert!(
            !manifest.contains("\nid = "),
            "an identity was written into a file that predates it:\n{manifest}"
        );

        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected a song");
        };
        for (index, channel) in loaded.channels.iter().enumerate() {
            assert_eq!(channel.id, mooloop_core::ChannelId(index as u32));
        }
        assert_eq!(loaded.next_channel_id, 3, "and the mint is past them");
        for (index, track) in loaded.buses.iter().enumerate() {
            assert_eq!(track.id, mooloop_core::TrackId(index as u32));
        }
        assert_eq!(loaded.next_track_id, 2, "and the track mint is past them");

        // Saving it then writes them, so the second open reads them rather
        // than deriving them again.
        save_song(&bundle, &loaded, AssetMode::Embedded).unwrap();
        let rewritten = fs::read_to_string(&bundle).unwrap();
        assert!(rewritten.contains("next_channel_id = 3"), "{rewritten}");
        assert!(rewritten.contains("next_track_id = 2"), "{rewritten}");
        let LoadedDocument::Song(reopened) = load_bundle(&bundle).unwrap().document else {
            panic!("expected a song");
        };
        assert_eq!(reopened, loaded);
    }

    /// **Opening a song and saving it must not rewrite it.**
    ///
    /// A song nobody has coloured writes no colour and no pattern list at
    /// all, which is what makes the defaulted-field rule hold in both
    /// directions: the field is absent from an old song *and* from a new one
    /// that has not used it, so the two are the same document.
    #[test]
    fn a_song_with_no_colours_writes_nothing_about_them() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.pattern_lengths.push(16);
        project.channels[0].notes.push(Vec::new());
        project.channels[0].automation.push(Vec::new());
        // What the session hands the snapshot: one entry per pattern, every
        // one of them saying nothing.
        project.pattern_meta = vec![PatternMeta::default(), PatternMeta::default()];
        project.pattern_meta = mooloop_core::trim_pattern_meta(&project.pattern_meta);

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        // A song with no assets to embed saves as one file, and that file is
        // the manifest.
        let manifest = fs::read_to_string(&bundle).unwrap();
        assert!(!manifest.contains("pattern_meta"), "an empty list was written:\n{manifest}");
        assert!(!manifest.contains("color"), "a colour nobody chose was written:\n{manifest}");
    }

    /// A colour that is not a colour reads as "none chosen" rather than
    /// refusing the song. It is a cosmetic field, and losing a whole document
    /// to a hand-edited one would be the wrong trade -- `ProjectColor`'s
    /// lenient deserializer is where this is decided.
    #[test]
    fn a_malformed_colour_loses_the_colour_and_not_the_song() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.channel.color = Some(ProjectColor::new(1, 2, 3));
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();

        let manifest_path = bundle.clone();
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("#010203", "octarine");
        fs::write(&manifest_path, manifest).unwrap();

        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("the song was refused over a colour");
        };
        assert_eq!(loaded.channels[0].setup.channel.color, None);
    }

    #[test]
    fn automation_lanes_round_trip_and_older_songs_load_without_them() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        let mut lane = AutomationLane::new(ParamAddr::strip(
            mooloop_core::EffectTarget::Channel(0),
            mooloop_core::STRIP_PARAM_PAN,
        ));
        assert!(lane.upsert(AutomationPoint::new(1, 0, 0.0)));
        assert!(lane.upsert(AutomationPoint::new(2, 96, 0.75)));
        project.channels[0].automation[0].push(lane);

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert_eq!(loaded.document, LoadedDocument::Song(project.clone()));

        // A song written before clip automation has no `automation` key at
        // all. It must load, and load with one empty bank per pattern.
        let mut legacy = project.clone();
        legacy.channels[0].automation.clear();
        validate_project(&legacy).expect("a missing automation bank is not an error");
        legacy.channels[0].normalize_automation();
        assert_eq!(
            legacy.channels[0].automation.len(),
            legacy.channels[0].notes.len()
        );
    }

    /// A channel's MIDI input and the project's control bindings survive a
    /// save, and -- the part that matters -- a song written before either
    /// existed loads unchanged.
    ///
    /// Both are skipped when they are at their defaults, so an unconfigured
    /// song is byte-identical to one written before the fields existed. That
    /// is asserted here against the serialized text rather than against a
    /// struct, because a `skip_serializing_if` nobody checks the output of is
    /// not evidence of anything.
    /// An audio input saves and reloads as it was, an old song with only a
    /// MIDI input writes nothing about audio, and a channel preset never
    /// brings one -- it would name a channel of another song.
    #[test]
    fn an_audio_input_round_trips_and_a_preset_never_brings_one() {
        use mooloop_core::AudioInputSource;
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert!(!fs::read_to_string(&bundle).unwrap().contains("audio_input"));

        let source = project.channels[0].id;
        project.channels[0].setup.channel.audio_input = AudioInputSource::Channel(source);
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected a song");
        };
        assert_eq!(
            loaded.channels[0].setup.channel.audio_input,
            AudioInputSource::Channel(source)
        );

        let preset = temp.path().join("channel.mooloop");
        save_channel(&preset, &project.channels[0].setup, AssetMode::Embedded).unwrap();
        let LoadedDocument::Channel(setup) = load_bundle(&preset).unwrap().document else {
            panic!("expected a channel");
        };
        assert!(setup.channel.audio_input.is_off());
    }

    /// A song written before the strip volume took the fader taper has no
    /// `strip_volume_taper` key, and loads with its volume lane converted:
    /// a point at a quarter of the old 0..+12 dB range was unity, and unity
    /// is now three-quarter travel (MOO-131). Asserted against the file with
    /// the key physically absent, because that is what an older song is.
    #[test]
    fn a_song_from_before_the_fader_taper_loads_at_the_same_volume() {
        use mooloop_core::{AutomationLane, AutomationPoint, EffectTarget, ParamAddr};

        let temp = tempdir().unwrap();
        let bundle = temp.path().join("linear.mooloop");
        let mut project = Project::default();
        project.channels[0].normalize_automation();
        let mut lane = AutomationLane::new(ParamAddr::strip(
            EffectTarget::Channel(0),
            mooloop_core::STRIP_PARAM_VOLUME,
        ));
        lane.reserve_points();
        lane.reset_points([AutomationPoint::new(1, 0, 0.25)]);
        project.channels[0].automation[0].push(lane);
        save_song_file(&bundle, &project, AssetMode::Embedded).unwrap();

        let manifest = fs::read_to_string(&bundle).unwrap();
        let key = format!("strip_volume_taper = {}\n", mooloop_core::STRIP_VOLUME_TAPER);
        assert!(manifest.contains(&key), "a new song records its taper:\n{manifest}");
        fs::write(&bundle, manifest.replace(&key, "")).unwrap();

        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected a song");
        };
        assert_eq!(loaded.strip_volume_taper, mooloop_core::STRIP_VOLUME_TAPER);
        let value = loaded.channels[0].automation[0][0].points()[0].value;
        assert!((value - 0.75).abs() < 1e-4, "unity converted to {value}");
    }

    #[test]
    fn midi_input_and_control_bindings_round_trip_and_cost_nothing_unused() {
        use mooloop_core::{
            ChannelMidiInput, ControlBinding, ControlSource, ControlTarget, MidiChannelFilter,
            MidiInputSource, MidiPortFilter,
        };

        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");

        // A song nobody has configured writes neither key.
        let plain = Project::default();
        save_song(&bundle, &plain, AssetMode::Embedded).unwrap();
        let written = std::fs::read_to_string(temp.path().join("song.mooloop").join("project.toml"))
            .or_else(|_| std::fs::read_to_string(&bundle))
            .expect("the song's document is readable");
        assert!(
            !written.contains("midi_input"),
            "an unconfigured channel should not write a MIDI input"
        );
        assert!(
            !written.contains("control_map"),
            "a song with no bindings should not write a control map"
        );

        let mut project = Project::default();
        project.channels[0].setup.channel.midi_input = ChannelMidiInput {
            source: MidiInputSource::Port("Launchkey MK3".to_owned()),
            channel: MidiChannelFilter::One(9),
        };
        // A binding names its channel by identity, so the song has to have
        // handed one out before there is anything to learn onto.
        project.assign_channel_ids();
        project.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Named("Faderfox".to_owned()),
                channel: MidiChannelFilter::Omni,
                controller: 74,
            },
            ControlTarget::Param(mooloop_core::ParamKey::strip(
                mooloop_core::ChainKey::Channel(project.channels[0].id),
                mooloop_core::STRIP_PARAM_VOLUME,
            )),
        ));

        let bundle = temp.path().join("configured.mooloop");
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert_eq!(loaded.document, LoadedDocument::Song(project.clone()));
    }

    /// Stretch settings survive a save, and -- the part that actually
    /// matters -- a song written before they existed still loads, with
    /// stretch off and the ratio at unity.
    ///
    /// Asserted against TOML with the keys physically absent rather than
    /// against a struct with defaults filled in, because a `#[serde(default)]`
    /// that is never exercised by a document missing the key is not evidence
    /// of anything.
    #[test]
    fn stretch_settings_round_trip_and_older_songs_load_without_them() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        {
            let state = project.channels[0].setup.sampler_state_mut().unwrap();
            state.params.stretch_enabled = true;
            state.params.stretch_mode = mooloop_core::StretchMode::Grain;
            state.params.stretch_ratio = 6.5;
            state.params.stretch_grain = 192;
        }

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert_eq!(loaded.document, LoadedDocument::Song(project.clone()));

        // What a song saved before this feature actually looks like: no
        // stretch keys at all.
        let source = toml::to_string(&project.channels[0].setup.source).unwrap();
        let legacy: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("stretch_"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !legacy.contains("stretch_"),
            "the legacy fixture still mentions stretch"
        );

        let restored: ChannelSource = toml::from_str(&legacy).unwrap();
        let params = match &restored {
            ChannelSource::Sampler(state) => state.params,
            other => panic!("expected a sampler, got {other:?}"),
        };
        assert!(!params.stretch_enabled, "an old song must not start stretching");
        assert_eq!(params.stretch_ratio, 1.0);
        assert_eq!(params.stretch_grain, 1024);
        assert_eq!(params.stretch_mode, mooloop_core::StretchMode::Music);
    }

    /// Slice markers and a commit spec are project data, not a view: they
    /// survive a save/load intact, and a song written before either existed
    /// comes back as an ordinary pitched sampler.
    #[test]
    fn an_old_project_without_slices_or_a_commit_loads_as_pitched() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        {
            let state = project.channels[0].setup.sampler_state_mut().unwrap();
            state.params.play_mode = mooloop_core::PlayMode::Slice;
            state.params.slice_base_note = 48;
            state.slices.divide_evenly(4, 0, 4_000);
            state.commit = Some(Box::new(mooloop_core::SampleCommit {
                mode: mooloop_core::StretchMode::Drums,
                ratio: 2.5,
                grain: 512,
                source_markers: (0..4)
                    .map(|index| mooloop_core::SliceMarker {
                        id: index + 1,
                        frame: index as u32 * 1_000,
                        hand: true,
                    })
                    .collect(),
                source_start: 0.0,
                source_end: 1.0,
                source_loop_start: 0.0,
                source_loop_end: 1.0,
            }));
        }

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert_eq!(loaded.document, LoadedDocument::Song(project.clone()));

        // What a song saved before slicing actually looks like: no play mode,
        // no base note, and no slice table at all.
        let source = toml::to_string(&project.channels[0].setup.source).unwrap();
        let mut legacy = Vec::new();
        let mut in_slices = false;
        for line in source.lines() {
            if line.trim_start().starts_with('[') {
                in_slices = line.contains("slices") || line.contains("commit");
            }
            if in_slices
                || line.trim_start().starts_with("play_mode")
                || line.trim_start().starts_with("slice_base_note")
            {
                continue;
            }
            legacy.push(line);
        }
        let legacy = legacy.join("\n");
        assert!(!legacy.contains("slice"), "the legacy fixture still mentions slices");

        let restored: ChannelSource = toml::from_str(&legacy).unwrap();
        let ChannelSource::Sampler(state) = &restored else {
            panic!("expected a sampler, got {restored:?}");
        };
        assert_eq!(state.params.play_mode, mooloop_core::PlayMode::Pitched);
        assert_eq!(
            state.params.slice_base_note,
            mooloop_core::DEFAULT_SLICE_BASE_NOTE
        );
        assert!(state.slices.is_empty());
        assert_eq!(state.commit, None, "an old song's buffer is its source");
    }

    /// A hand-edited or corrupted document must be repaired rather than
    /// refused, like every other out-of-range field.
    #[test]
    fn an_out_of_range_stretch_setting_is_repaired() {
        let mut project = Project::default();
        {
            let state = project.channels[0].setup.sampler_state_mut().unwrap();
            state.params.stretch_ratio = 900.0;
            state.params.stretch_grain = 3;
        }
        let diagnosis = crate::integrity::repair_project(&mut project);
        assert!(
            diagnosis
                .repairs()
                .any(|issue| issue.code.starts_with("channel.sampler.")),
            "the out-of-range stretch settings should have been repaired"
        );

        let params = project.channels[0].setup.sampler_state().unwrap().params;
        assert_eq!(params.stretch_ratio, mooloop_core::MAX_STRETCH_RATIO);
        assert_eq!(params.stretch_grain, mooloop_core::MIN_STRETCH_GRAIN);
    }

    #[test]
    fn two_lanes_on_one_destination_are_rejected() {
        let mut project = Project::default();
        let target = ParamAddr::effect(
            mooloop_core::EffectTarget::Channel(0),
            mooloop_core::DeviceId(0),
            1,
        );
        project.channels[0].automation[0].push(AutomationLane::new(target));
        project.channels[0].automation[0].push(AutomationLane::new(target));
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));
    }

    #[test]
    fn embedded_assets_are_copied_and_deduplicated() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("kick.wav");
        fs::write(&source, b"wav bytes").unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels.push(project.channels[0].clone());
        for channel in &mut project.channels {
            channel.setup.sampler_state_mut().unwrap().sample = SampleReference::File {
                path: source.clone(),
                embedded: false,
            };
        }

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert_eq!(
            fs::read_dir(song_assets_path(&bundle).unwrap().join("samples"))
                .unwrap()
                .count(),
            1
        );
        let loaded = load_bundle(&bundle).unwrap();
        let LoadedDocument::Song(project) = loaded.document else {
            panic!("expected song")
        };
        let first = &project.channels[0].setup.sampler_state().unwrap().sample;
        let second = &project.channels[1].setup.sampler_state().unwrap().sample;
        assert_eq!(first, second);
    }

    /// A sampler with key zones (MOO-14) that has never been saved, with
    /// two zone files that share a name from two folders.
    fn zoned_project(root: &Path) -> Project {
        use mooloop_core::{KeyRange, SampleZone};
        let file = |folder: &str, name: &str| {
            let dir = root.join(folder);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            fs::write(&path, format!("{folder} bytes")).unwrap();
            SampleReference::File { path, embedded: false }
        };
        let mut project = Project::default();
        let state = project.channels[0].setup.sampler_state_mut().unwrap();
        state.sample = file("base", "kick.wav");
        state.keys = KeyRange::new(0, 59);
        state.zones = vec![
            SampleZone {
                keys: KeyRange::new(60, 71),
                root_note: 64,
                sample: file("low", "tone.wav"),
                ..SampleZone::default()
            },
            SampleZone {
                keys: KeyRange::new(72, 127),
                root_note: 76,
                sample: file("high", "tone.wav"),
                ..SampleZone::default()
            },
        ];
        project
    }

    fn zone_files(state: &mooloop_core::SamplerState) -> Vec<PathBuf> {
        state
            .zones
            .iter()
            .map(|zone| match &zone.sample {
                SampleReference::File { path, .. } => path.clone(),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// **Every zone's file travels like the base's (MOO-14).** Embedded,
    /// each is copied into the bundle, two files with one name stay two, and
    /// the load resolves them to the copies with nothing repaired.
    #[test]
    fn a_zoned_sampler_embeds_every_zone_and_round_trips() {
        let temp = tempdir().unwrap();
        let project = zoned_project(temp.path());
        let bundle = temp.path().join("song.mooloop");
        let report = save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.repairs.is_empty(), "{:?}", loaded.repairs);
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let LoadedDocument::Song(song) = loaded.document else {
            panic!("expected song")
        };
        let state = song.channels[0].setup.sampler_state().unwrap();
        let original = project.channels[0].setup.sampler_state().unwrap();
        assert_eq!(state.keys, original.keys);
        assert_eq!(state.zones.len(), 2);
        for (zone, was) in state.zones.iter().zip(&original.zones) {
            assert_eq!((zone.keys, zone.velocity, zone.root_note), (was.keys, was.velocity, was.root_note));
        }
        let files = zone_files(state);
        assert_ne!(files[0], files[1], "two zones' same-named files became one");
        assert_eq!(fs::read_to_string(&files[0]).unwrap(), "low bytes");
        assert_eq!(fs::read_to_string(&files[1]).unwrap(), "high bytes");
        assert!(files.iter().all(|file| file.starts_with(song_assets_path(&bundle).unwrap())));
    }

    /// The same in a channel preset, whose walker names its copies on its
    /// own and used to take one name per channel as enough.
    #[test]
    fn a_zoned_channel_preset_keeps_both_same_named_zone_files() {
        let temp = tempdir().unwrap();
        let project = zoned_project(temp.path());
        let bundle = temp.path().join("zoned.mooloop-channel");
        save_channel(&bundle, &project.channels[0].setup, AssetMode::Embedded).unwrap();
        let LoadedDocument::Channel(setup) = load_bundle(&bundle).unwrap().document else {
            panic!("expected a channel")
        };
        let files = zone_files(setup.sampler_state().unwrap());
        assert_ne!(files[0], files[1]);
        assert_eq!(fs::read_to_string(&files[0]).unwrap(), "low bytes");
        assert_eq!(fs::read_to_string(&files[1]).unwrap(), "high bytes");
    }

    /// A missing zone file is a sample warning on load, as a missing base
    /// file is, and the zone keeps its place in the map.
    #[test]
    fn a_missing_zone_file_warns_and_keeps_the_zone() {
        let temp = tempdir().unwrap();
        let project = zoned_project(temp.path());
        let bundle = temp.path().join("song.mooloop");
        save_song(&bundle, &project, AssetMode::Referenced).unwrap();
        fs::remove_file(temp.path().join("high").join("tone.wav")).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
        assert!(loaded.repairs.is_empty());
        let LoadedDocument::Song(song) = loaded.document else {
            panic!("expected song")
        };
        assert_eq!(song.channels[0].setup.sampler_state().unwrap().zones.len(), 2);
    }

    /// A sampler with no zones writes neither field, so a song saved before
    /// zones is saved byte-identical (MOO-14).
    #[test]
    fn a_sampler_without_zones_writes_no_zone_fields() {
        let state = mooloop_core::SamplerState::default();
        let text = toml::to_string(&state).unwrap();
        assert!(!text.contains("zones") && !text.contains("keys"), "{text}");
        let back: mooloop_core::SamplerState = toml::from_str(&text).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn embedded_bundle_can_be_saved_again_without_the_external_source() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("kick.wav");
        fs::write(&source, b"wav bytes").unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::File {
            path: source.clone(),
            embedded: false,
        };

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let LoadedDocument::Song(saved) = load_bundle(&bundle).unwrap().document else {
            panic!("expected song")
        };
        fs::remove_file(source).unwrap();

        let report = save_song(&bundle, &saved, AssetMode::Embedded).unwrap();
        assert!(report.warnings.is_empty());
        let LoadedDocument::Song(saved_again) = load_bundle(&bundle).unwrap().document else {
            panic!("expected song")
        };
        let SampleReference::File { path, embedded } = &saved_again.channels[0]
            .setup
            .sampler_state()
            .unwrap()
            .sample
        else {
            panic!("expected file sample")
        };
        assert!(*embedded);
        assert!(path.is_file());
    }

    /// **Unticking "Embed assets" on an already-embedded song says so.**
    ///
    /// The guard that keeps the sample in the bundle is necessary: without it
    /// `replace_song_file` deletes the sidecar the new reference would point
    /// at, destroying the only copy. What was wrong was everything around it
    /// -- the save produced no warning, no status message and no change, and
    /// reopening set the checkbox from the *document-level* mode, so the box
    /// showed unticked on a bundle whose samples are all embedded and the
    /// state never converged.
    ///
    /// Un-embedding for real means copying the bytes out to somewhere the
    /// user has chosen, which is a gesture that does not exist. This is the
    /// honest refusal, which is what `LOOSE_ENDS.md` called the cheap half.
    /// **A recorded take is the song's, whatever the save mode.** It sits in
    /// the shared recordings folder, owned (`embedded`) but not yet stored,
    /// and either kind of save copies it into the bundle -- so a song never
    /// depends on a folder whose unused takes can be deleted
    /// (`audio-recording/04` and `06`).
    #[test]
    fn a_take_is_copied_into_the_song_in_either_mode() {
        for mode in [AssetMode::Embedded, AssetMode::Referenced] {
            let temp = tempdir().unwrap();
            let recordings = temp.path().join("recordings");
            fs::create_dir_all(&recordings).unwrap();
            let take = recordings.join("20260918-120000-Sampler_1.wav");
            fs::write(&take, b"take bytes").unwrap();
            let bundle = temp.path().join("song.mooloop");
            let mut project = Project::default();
            project.channels[0].setup.sampler_state_mut().unwrap().sample =
                SampleReference::File {
                    path: take.clone(),
                    embedded: true,
                };

            save_song(&bundle, &project, mode).unwrap();
            fs::remove_dir_all(&recordings).unwrap();

            let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
                panic!("expected song")
            };
            let SampleReference::File { path, embedded } =
                &loaded.channels[0].setup.sampler_state().unwrap().sample
            else {
                panic!("a file reference");
            };
            assert!(embedded, "{mode:?}");
            assert_eq!(fs::read(path).unwrap(), b"take bytes", "{mode:?}");
        }
    }

    #[test]
    fn unticking_embed_on_an_embedded_song_is_refused_out_loud() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("kick.wav");
        fs::write(&source, b"wav bytes").unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::File {
            path: source.clone(),
            embedded: false,
        };

        // Embed it, and take back the document the save produced -- which is
        // where the bundle-owned path lives.
        assert!(save_song(&bundle, &project, AssetMode::Embedded)
            .unwrap()
            .warnings
            .is_empty());
        let LoadedDocument::Song(embedded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected song")
        };

        // Now untick the box. The sample stays, and the report says why.
        let report = save_song(&bundle, &embedded, AssetMode::Referenced).unwrap();
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(
            report.warnings[0].message.contains("stays embedded"),
            "{:?}",
            report.warnings[0]
        );

        let LoadedDocument::Song(after) = load_bundle(&bundle).unwrap().document else {
            panic!("expected song")
        };
        let SampleReference::File { path, embedded: still } =
            &after.channels[0].setup.sampler_state().unwrap().sample
        else {
            panic!("expected file sample")
        };
        assert!(*still, "the flag stopped saying what the bundle holds");
        assert!(path.is_file(), "the only copy was deleted: {}", path.display());
    }

    /// A sample that is *not* in the bundle un-embeds silently, because that
    /// one really can: the external file is still there to point at. The
    /// warning above has to be about the impossible case only, or every
    /// referenced save would carry it.
    #[test]
    fn a_referenced_save_of_an_external_sample_says_nothing() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("kick.wav");
        fs::write(&source, b"wav bytes").unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::File {
            path: source,
            embedded: false,
        };

        let report = save_song(&bundle, &project, AssetMode::Referenced).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    /// **A sample the song stops using stays in its sidecar.** Until
    /// 2026-09-22 this test was the opposite -- the old sidecar was removed --
    /// because every save rebuilt the folder from what that save referenced.
    /// An undo can bring a sample back, so the save that dropped it must not
    /// delete it; moving an unused one to the trash is the clean-up dialog's
    /// job.
    #[test]
    fn resaving_without_a_sample_leaves_it_in_the_sidecar() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("kick.wav");
        fs::write(&source, b"wav bytes").unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::File {
            path: source,
            embedded: false,
        };

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let assets = song_assets_path(&bundle).unwrap();
        let kept = assets.join("samples/00-kick.wav");
        assert!(kept.is_file());

        project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::default();
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert_eq!(fs::read(&kept).unwrap(), b"wav bytes");
        assert_eq!(
            load_bundle(&bundle).unwrap().document,
            LoadedDocument::Song(project)
        );
    }

    /// A take the song plays, as a save would find it: owned, and still in
    /// the shared recordings folder under `root`.
    fn record_take(root: &Path, name: &str, bytes: &[u8]) -> SampleReference {
        let recordings = root.join(RECORDINGS_DIR);
        fs::create_dir_all(&recordings).unwrap();
        let take = recordings.join(name);
        fs::write(&take, bytes).unwrap();
        SampleReference::File {
            path: take,
            embedded: true,
        }
    }

    fn sample_of(project: &Project) -> PathBuf {
        let SampleReference::File { path, .. } =
            &project.channels[0].setup.sampler_state().unwrap().sample
        else {
            panic!("the channel holds a file reference");
        };
        path.clone()
    }

    /// Save, then take back the document the save produced, as the
    /// application does after every save.
    fn save_and_reload(bundle: &Path, project: &Project) -> Project {
        save_song(bundle, project, AssetMode::Embedded).unwrap();
        let LoadedDocument::Song(saved) = load_bundle(bundle).unwrap().document else {
            panic!("a song bundle loads as a song");
        };
        saved
    }

    /// **A take goes into the song's own `recordings/`, under the name it
    /// was recorded with** (Adam, 2026-09-22, MOO-38: *"having a recordings/
    /// doesnt sound like a terrible idea"*). Before, it went into `samples/`
    /// with a channel prefix, so a file called `20260922-...-Sampler_1.wav`
    /// in the recordings folder turned up as `00-20260922-...` in the song.
    #[test]
    fn a_take_lands_in_the_songs_recordings_folder_under_its_own_name() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.sampler_state_mut().unwrap().sample =
            record_take(temp.path(), "20260922-120000-Sampler_1.wav", b"take");

        let saved = save_and_reload(&bundle, &project);

        assert_eq!(
            sample_of(&saved),
            song_assets_path(&bundle)
                .unwrap()
                .join("recordings/20260922-120000-Sampler_1.wav")
        );
        let manifest = fs::read_to_string(&bundle).unwrap();
        assert!(
            manifest.contains("song.mooloop-assets/recordings/20260922-120000-Sampler_1.wav"),
            "{manifest}"
        );
    }

    /// **The sequence MOO-89 suspected, at the level of the files.** Record
    /// a take and save; record a retake over it and save again; go back to
    /// the first take, as an undo does, and save a third time. The first
    /// take's copy in the song has to still be there at every step: the
    /// history holds its path, and until 2026-09-22 the second save rebuilt
    /// the sidecar without it.
    #[test]
    fn an_earlier_take_survives_the_save_after_the_retake_that_replaced_it() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.sampler_state_mut().unwrap().sample =
            record_take(temp.path(), "20260922-120000-Sampler_1.wav", b"first take");
        let after_first = save_and_reload(&bundle, &project);
        // What the "Record Take" entry for the retake holds as its `before`.
        let undo_to = after_first.clone();

        let mut retaken = after_first;
        retaken.channels[0].setup.sampler_state_mut().unwrap().sample =
            record_take(temp.path(), "20260922-120100-Sampler_1.wav", b"second take");
        let after_second = save_and_reload(&bundle, &retaken);
        assert_eq!(fs::read(sample_of(&after_second)).unwrap(), b"second take");
        assert!(
            sample_of(&undo_to).is_file(),
            "the save after the retake deleted the take an undo goes back to"
        );

        let after_undo = save_and_reload(&bundle, &undo_to);
        assert_eq!(fs::read(sample_of(&after_undo)).unwrap(), b"first take");
        assert_eq!(sample_of(&after_undo), sample_of(&undo_to));
    }

    fn song_at(bpm: u16) -> Project {
        Project {
            bpm,
            ..Project::default()
        }
    }

    fn bpm_of(path: &Path) -> u16 {
        let LoadedDocument::Song(song) = load_bundle(path).unwrap().document else {
            panic!("a song loads as a song");
        };
        song.bpm
    }

    /// Hidden files a save leaves beside the song: staging, links, backups.
    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect()
    }

    /// **A save that fails after staging leaves the previous version
    /// readable** (MOO-92). The fault lands between the staged file being
    /// written, synced and read back and the rename that would put it in
    /// place: the old song is still the song, and nothing is left behind.
    #[test]
    fn a_save_that_fails_after_staging_leaves_the_previous_version() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        save_song(&bundle, &song_at(97), AssetMode::Embedded).unwrap();

        FAIL_AT.with(|fail| fail.set(Some(Step::Place)));
        assert!(save_song(&bundle, &song_at(141), AssetMode::Embedded).is_err());

        assert_eq!(bpm_of(&bundle), 97);
        assert_eq!(leftovers(temp.path()), Vec::<String>::new());
    }

    /// **Every step of putting a song in place can fail, and none of them
    /// loses the previous version** (MOO-121). Before the rename that places
    /// the new song, the old one is still the song; after it, the old one is
    /// the `.bak`. No step leaves a hidden file behind.
    #[test]
    fn a_fault_at_any_step_of_the_replace_keeps_the_previous_version() {
        for step in [Step::KeepOld, Step::NameBackup, Step::Place, Step::SyncDirectory] {
            let temp = tempdir().unwrap();
            let bundle = temp.path().join("song.mooloop");
            save_song(&bundle, &song_at(97), AssetMode::Embedded).unwrap();

            FAIL_AT.with(|fail| fail.set(Some(step)));
            let result = save_song(&bundle, &song_at(141), AssetMode::Embedded);
            assert!(result.is_err(), "{step:?}: the save reported success");

            let now = bpm_of(&bundle);
            let kept = temp.path().join("song.mooloop.bak");
            if step == Step::SyncDirectory {
                // Renamed but not synced: the new song is in place, and the
                // old one is whole beside it.
                assert_eq!(now, 141, "{step:?}");
                assert_eq!(bpm_of(&kept), 97, "{step:?}");
            } else {
                assert_eq!(now, 97, "{step:?}: the previous version is no longer the song");
            }
            assert_eq!(leftovers(temp.path()), Vec::<String>::new(), "{step:?}");
        }
    }

    /// **The previous version is kept as `<name>.bak`, and a save leaves no
    /// hidden file behind** (MOO-92): the backup is made under a name that
    /// one save alone uses and renamed into place, so two saves never share
    /// one and a `.bak` is always a whole song.
    #[test]
    fn a_save_keeps_the_version_it_replaced_as_bak() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        save_song(&bundle, &song_at(97), AssetMode::Embedded).unwrap();
        assert!(!temp.path().join("song.mooloop.bak").exists(), "a first save replaces nothing");

        save_song(&bundle, &song_at(120), AssetMode::Embedded).unwrap();
        save_song(&bundle, &song_at(141), AssetMode::Embedded).unwrap();

        assert_eq!(bpm_of(&bundle), 141);
        assert_eq!(bpm_of(&temp.path().join("song.mooloop.bak")), 120);
        assert_eq!(leftovers(temp.path()), Vec::<String>::new());
    }

    /// The same fault on a kit, whose bundle is a directory and so cannot be
    /// replaced by one rename: the old kit is put back.
    #[test]
    fn a_kit_save_that_fails_after_staging_leaves_the_previous_kit() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("drums.mooloop-kit");
        let kit = |names: &[&str]| Kit {
            channels: names.iter().map(|name| ChannelSetup::drum_synth(*name)).collect(),
        };
        save_kit(&path, &kit(&["A"]), AssetMode::Embedded).unwrap();

        FAIL_AT.with(|fail| fail.set(Some(Step::Place)));
        assert!(save_kit(&path, &kit(&["A", "B"]), AssetMode::Embedded).is_err());

        let LoadedDocument::Kit(loaded) = load_bundle(&path).unwrap().document else {
            panic!("a kit loads as a kit");
        };
        assert_eq!(loaded.channels.len(), 1);
        assert_eq!(leftovers(temp.path()), Vec::<String>::new());
    }

    /// **A file already in the sidecar is not copied again.** The bytes in
    /// the sidecar are changed behind the save's back, and the next save
    /// leaves them changed: it did not rewrite the file from anywhere.
    #[test]
    fn a_sample_already_in_the_sidecar_is_left_as_it_is() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("kick.wav");
        fs::write(&source, b"original").unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.sampler_state_mut().unwrap().sample = SampleReference::File {
            path: source,
            embedded: false,
        };
        let saved = save_and_reload(&bundle, &project);
        let stored = sample_of(&saved);
        fs::write(&stored, b"touched in place").unwrap();

        let saved_again = save_and_reload(&bundle, &saved);

        assert_eq!(sample_of(&saved_again), stored);
        assert_eq!(fs::read(&stored).unwrap(), b"touched in place");
    }

    /// **A new file never lands on an old one's name.** The sidecar keeps
    /// what the song stopped using, so a later sample can want a name that is
    /// taken; it gets a fresh one and the old file is untouched.
    #[test]
    fn a_new_sample_is_never_copied_over_one_the_sidecar_kept() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let first_dir = temp.path().join("a");
        let second_dir = temp.path().join("b");
        fs::create_dir_all(&first_dir).unwrap();
        fs::create_dir_all(&second_dir).unwrap();
        fs::write(first_dir.join("kick.wav"), b"first kick").unwrap();
        fs::write(second_dir.join("kick.wav"), b"second kick").unwrap();

        let mut project = Project::default();
        project.channels[0].setup.sampler_state_mut().unwrap().sample = SampleReference::File {
            path: first_dir.join("kick.wav"),
            embedded: false,
        };
        let first = save_and_reload(&bundle, &project);
        let first_copy = sample_of(&first);

        let mut replaced = first;
        replaced.channels[0].setup.sampler_state_mut().unwrap().sample = SampleReference::File {
            path: second_dir.join("kick.wav"),
            embedded: false,
        };
        let second = save_and_reload(&bundle, &replaced);

        assert_ne!(sample_of(&second), first_copy);
        assert!(sample_of(&second).ends_with("samples/00-kick-2.wav"));
        assert_eq!(fs::read(sample_of(&second)).unwrap(), b"second kick");
        assert_eq!(fs::read(&first_copy).unwrap(), b"first kick");
    }

    #[test]
    fn kit_and_channel_setup_round_trip() {
        let temp = tempdir().unwrap();
        let mut setup = ChannelSetup::sampler("Closed Hat");
        setup.channel.muted = true;
        setup.channel.volume = 0.42;
        setup.channel.pan = -0.25;
        setup.sampler_state_mut().unwrap().params.reverse = true;
        setup.sampler_state_mut().unwrap().params.choke_group = 3;
        let kit = Kit {
            channels: vec![setup.clone()],
        };

        let kit_path = temp.path().join("drums.mooloop-kit");
        save_kit(&kit_path, &kit, AssetMode::Embedded).unwrap();
        let loaded_kit = load_bundle(&kit_path).unwrap();
        assert_eq!(loaded_kit.document, LoadedDocument::Kit(kit));

        let channel_path = temp.path().join("hat.mooloop-channel");
        save_channel(&channel_path, &setup, AssetMode::Referenced).unwrap();
        let loaded_channel = load_bundle(&channel_path).unwrap();
        assert_eq!(loaded_channel.asset_mode, AssetMode::Referenced);
        assert_eq!(loaded_channel.document, LoadedDocument::Channel(Box::new(setup)));
    }

    /// The whole point of the repair pass, checked end to end: a song that
    /// used to be refused now reaches disk, and what comes back is the
    /// corrected version rather than the broken one.
    #[test]
    fn a_song_with_an_out_of_range_setting_saves_and_reloads_corrected() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::mono_synth(0, 1).with_id(id);
        project.channels[0]
            .setup
            .mono_synth_state_mut()
            .unwrap()
            .params
            .lfo
            .rate_hz = 400.0;

        let saved = save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert_eq!(saved.repairs.len(), 1);
        assert_eq!(saved.repairs[0].code, "channel.synth.lfo");

        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.repairs.is_empty(), "the file on disk is already fixed");
        let LoadedDocument::Song(reloaded) = loaded.document else {
            panic!("expected a song");
        };
        assert_eq!(
            reloaded.channels[0]
                .setup
                .mono_synth_state()
                .unwrap()
                .params
                .lfo
                .rate_hz,
            20.0
        );
    }

    /// A song already on disk in a shape the old validator refused was
    /// unopenable, which is worse than unsaveable. It must open.
    #[test]
    fn a_song_written_with_a_bad_value_still_opens() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.channel.pan = 9.0;
        // Straight past `save_song`, so the manifest keeps the bad value the
        // way a hand edit or an older build would have left it.
        let envelope = Envelope {
            format_version: FORMAT_VERSION,
            document_type: "song".into(),
            asset_mode: AssetMode::Embedded,
            preset: None,
            contains: Vec::new(),
            plugin: None,
            document: project,
        };
        fs::write(&bundle, toml::to_string_pretty(&envelope).unwrap()).unwrap();

        let loaded = load_bundle(&bundle).unwrap();
        assert_eq!(loaded.repairs.len(), 1);
        let LoadedDocument::Song(project) = loaded.document else {
            panic!("expected a song");
        };
        assert_eq!(project.channels[0].setup.channel.pan, 1.0);
    }

    /// What is left when repair cannot help has to say where it is and what it
    /// would cost, in both the sentence and the copyable report.
    #[test]
    fn an_unrepairable_song_reports_the_place_and_the_price() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.channel.name = "Lead".into();
        project.channels[0].notes[0] = (0..mooloop_core::MAX_NOTES_PER_CHANNEL_PATTERN + 2)
            .map(|index| NoteEvent::new(index as u32 + 1, 0, 24, 60, 100))
            .collect();

        let error = save_song(&bundle, &project, AssetMode::Embedded).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Channel 1 \"Lead\""), "{message}");
        assert!(message.contains("pattern 1"), "{message}");
        assert!(message.contains("delete 2 notes"), "{message}");

        let report = error.report().expect("a document error carries a report");
        assert!(report.contains("channel.notes.count"), "{report}");
        assert!(report.contains("format version"), "{report}");
        assert!(!bundle.exists(), "a refused save leaves no partial file");
    }

    #[test]
    fn a_song_that_cannot_be_saved_can_still_be_set_aside_and_read_back() {
        let temp = tempdir().unwrap();
        let mut project = Project::default();
        project.channels[0].setup.channel.name = "Lead".into();
        project.channels[0].notes[0] = (0..mooloop_core::MAX_NOTES_PER_CHANNEL_PATTERN + 2)
            .map(|index| NoteEvent::new(index as u32 + 1, 0, 24, 60, 100))
            .collect();
        let error = save_song(&temp.path().join("song.mooloop"), &project, AssetMode::Embedded)
            .expect_err("the note ceiling cannot be repaired");

        // The whole point of the escape hatch: the document the ordinary save
        // refused is the document that reaches disk here, unaltered.
        let parked = quarantine_song(
            &temp.path().join("quarantine/20260831-142203.mooloop"),
            &project,
            &error.report().unwrap(),
        )
        .expect("park the refused song");
        assert!(parked.exists());

        let notes = quarantine_song_notes(&parked);
        assert_eq!(notes, mooloop_core::MAX_NOTES_PER_CHANNEL_PATTERN + 2);

        let explanation =
            fs::read_to_string(parked.with_extension("txt")).expect("the report next to it");
        assert!(explanation.contains("channel.notes.count"), "{explanation}");
    }

    /// Reads a parked song back the only way one can be read: as the raw
    /// envelope. `load_bundle` repairs, and repairing is exactly what would
    /// destroy the evidence.
    fn quarantine_song_notes(path: &Path) -> usize {
        let text = fs::read_to_string(path).expect("read the parked song");
        let envelope: Envelope<Project> = toml::from_str(&text).expect("parse the parked song");
        envelope.document.channels[0].notes[0].len()
    }

    #[test]
    fn missing_reference_loads_with_warning() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::File {
            path: temp.path().join("gone.wav"),
            embedded: false,
        };
        let saved = save_song(&bundle, &project, AssetMode::Referenced).unwrap();
        assert_eq!(saved.warnings.len(), 1);
        let report = load_bundle(&bundle).unwrap();
        assert_eq!(report.warnings.len(), 1);
    }

    /// **Renaming a song in a file manager must not brick it.**
    ///
    /// Rename the pair the only sane way -- the song and its `-assets`
    /// sidecar, together -- and the song used to refuse to open at all:
    /// `safe_embedded_path` required the stored path to begin with *this
    /// song's exact file name*, so it answered false about a path that is
    /// present, relative, traversal-free and sitting right beside the file.
    /// It was an `Error::Invalid`, so the whole document was rejected rather
    /// than one sample warned about, and recovery meant hand-editing TOML.
    ///
    /// It now opens, plays, and says what it did. The sample is *found*,
    /// which is the half that a mere relaxation of the name check would have
    /// missed: dropping the equality alone would have opened the song with
    /// every sample missing, turning a brick into a silent loss.
    #[test]
    fn a_renamed_song_opens_with_its_samples() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("kick.wav");
        fs::write(&source, b"wav bytes").unwrap();
        let bundle = temp.path().join("before.mooloop");
        let mut project = Project::default();
        project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::File {
            path: source.clone(),
            embedded: false,
        };
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();

        // What a file manager does: both, together, nothing else touched.
        let renamed = temp.path().join("after.mooloop");
        fs::rename(&bundle, &renamed).unwrap();
        fs::rename(
            temp.path().join("before.mooloop-assets"),
            temp.path().join("after.mooloop-assets"),
        )
        .unwrap();

        let report = load_bundle(&renamed).expect("a renamed song must still open");
        let LoadedDocument::Song(song) = report.document else {
            panic!("expected song")
        };
        let SampleReference::File { path, embedded } =
            &song.channels[0].setup.sampler_state().unwrap().sample
        else {
            panic!("expected file sample")
        };
        assert!(*embedded);
        assert!(
            path.is_file(),
            "the sample was not found under the renamed sidecar: {}",
            path.display()
        );
        assert!(
            path.starts_with(temp.path().join("after.mooloop-assets")),
            "the path was not repointed at this song's own sidecar: {}",
            path.display()
        );

        // Said out loud rather than silently repaired: the document named a
        // directory that is not there, and the loader read a different one.
        assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
        assert!(
            report.warnings[0].message.contains("assets folder"),
            "{:?}",
            report.warnings[0]
        );

        // And the correction is what the next save writes, so it is a
        // one-time warning rather than a permanent one.
        let resaved = save_song(&renamed, &song, AssetMode::Embedded).unwrap();
        assert!(resaved.warnings.is_empty(), "{:?}", resaved.warnings);
        assert!(load_bundle(&renamed).unwrap().warnings.is_empty());
    }

    /// A rename is forgiven; leaving the bundle is not. The sidecar name is
    /// substituted, and every traversal property the old check had is kept by
    /// the `Component::Normal` filter that was always doing that work.
    #[test]
    fn a_repointed_path_still_cannot_leave_the_bundle() {
        let song = Path::new("/songs/after.mooloop");
        let sidecar = PathBuf::from("after.mooloop-assets");

        assert_eq!(
            embedded_bundle_path(song, Path::new("before.mooloop-assets/samples/00-kick.wav")),
            Some(sidecar.join("samples/00-kick.wav")),
            "a renamed sidecar is substituted"
        );
        assert_eq!(
            embedded_bundle_path(song, Path::new("after.mooloop-assets/samples/00-kick.wav")),
            Some(sidecar.join("samples/00-kick.wav")),
            "and an unrenamed one is unchanged"
        );

        for refused in [
            "../escape.wav",
            "/etc/passwd",
            "before.mooloop-assets/../../escape.wav",
            // The shape is still checked: a first component that is not a
            // sidecar, or a second that is not `samples`, is not a path into
            // any song's assets.
            "elsewhere/samples/00-kick.wav",
            "before.mooloop-assets/secrets/00-kick.wav",
            "before.mooloop-assets",
        ] {
            assert_eq!(
                embedded_bundle_path(song, Path::new(refused)),
                None,
                "{refused} was accepted"
            );
        }
    }

    #[test]
    fn rejects_unsafe_embedded_path() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        fs::create_dir(&bundle).unwrap();
        let mut project = Project::default();
        project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::File {
            path: PathBuf::from("../escape.wav"),
            embedded: true,
        };
        let envelope = Envelope {
            format_version: FORMAT_VERSION,
            document_type: "song".into(),
            asset_mode: AssetMode::Embedded,
            preset: None,
            contains: Vec::new(),
            plugin: None,
            document: project,
        };
        fs::write(
            bundle.join(MANIFEST_FILE),
            toml::to_string_pretty(&envelope).unwrap(),
        )
        .unwrap();
        // A path escaping the bundle is a problem with the request, not with
        // the document's own contents, so it stays an `Invalid` rather than
        // going through the repair pass.
        assert!(matches!(load_bundle(&bundle), Err(Error::Invalid(_))));
    }

    #[test]
    fn rejects_future_format_without_changing_document() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        fs::create_dir(&bundle).unwrap();
        fs::write(
            bundle.join(MANIFEST_FILE),
            "format_version = 99\ndocument_type = \"song\"\n",
        )
        .unwrap();
        assert!(matches!(
            load_bundle(&bundle),
            Err(Error::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn legacy_directory_song_loads_and_migrates_to_a_file() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("legacy.mooloop");
        fs::create_dir(&bundle).unwrap();
        let project = Project::default();
        let envelope = Envelope {
            format_version: FORMAT_VERSION,
            document_type: "song".into(),
            asset_mode: AssetMode::Referenced,
            preset: None,
            contains: Vec::new(),
            plugin: None,
            document: project.clone(),
        };
        fs::write(
            bundle.join(MANIFEST_FILE),
            toml::to_string_pretty(&envelope).unwrap(),
        )
        .unwrap();

        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected song")
        };
        assert_eq!(loaded, project);
        save_song(&bundle, &loaded, AssetMode::Referenced).unwrap();
        assert!(bundle.is_file());
        assert_eq!(
            load_bundle(&bundle).unwrap().document,
            LoadedDocument::Song(project)
        );
    }

    #[test]
    fn synth_sources_round_trip_without_sample_assets() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("starter.mooloop");
        let project = Project::starter_kit();

        let report = save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert!(report.warnings.is_empty());
        assert!(bundle.is_file());
        assert!(!song_assets_path(&bundle).unwrap().exists());
        let manifest = fs::read_to_string(&bundle).unwrap();
        assert!(manifest.contains("type = \"ds01\""));
        assert_eq!(
            load_bundle(&bundle).unwrap().document,
            LoadedDocument::Song(project)
        );
    }

    #[test]
    fn sampler_drum_and_mono_sources_round_trip_for_kit_and_channel() {
        let temp = tempdir().unwrap();
        let kit = Kit {
            channels: vec![
                ChannelSetup::sampler("Sample"),
                ChannelSetup::drum_synth("Drum"),
                ChannelSetup::mono_synth("Mono"),
            ],
        };
        let kit_path = temp.path().join("mixed.mooloop-kit");
        save_kit(&kit_path, &kit, AssetMode::Embedded).unwrap();
        assert!(!kit_path.join("samples").exists());
        assert_eq!(
            load_bundle(&kit_path).unwrap().document,
            LoadedDocument::Kit(kit)
        );

        let channel = ChannelSetup::mono_synth("Bass");
        let channel_path = temp.path().join("bass.mooloop-channel");
        save_channel(&channel_path, &channel, AssetMode::Embedded).unwrap();
        assert!(!channel_path.join("samples").exists());
        assert_eq!(
            load_bundle(&channel_path).unwrap().document,
            LoadedDocument::Channel(Box::new(channel))
        );
    }

    /// The Drum Synth became the DS-SX on 2026-09-26 in the interface only.
    /// A song saved before then, with a channel it named "Drum Synth 1", loads
    /// with that name and that tag, and saves back to the same bytes: a
    /// device's label is not an on-disk identifier, and a stored channel
    /// name is never re-derived from it.
    #[test]
    fn a_drum_synth_channel_named_before_the_rename_round_trips_untouched() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("before-ds-sx.mooloop");
        let mut project = Project::starter_kit();
        let drum = 0;
        project.channels[drum].setup = ChannelSetup::drum_synth("Drum Synth 1");
        assert_eq!(project.channels[drum].setup.kind(), DeviceKind::DrumSynth);

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let first = fs::read(&bundle).unwrap();
        let manifest = String::from_utf8(first.clone()).unwrap();
        assert!(manifest.contains("name = \"Drum Synth 1\""));
        assert!(manifest.contains("type = \"drum_synth\""));
        assert!(!manifest.contains("DS-SX"), "the new label leaked into the file");

        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected song")
        };
        assert_eq!(loaded.channels[drum].setup.channel.name, "Drum Synth 1");
        assert_eq!(loaded, project);
        save_song(&bundle, &loaded, AssetMode::Embedded).unwrap();
        assert_eq!(fs::read(&bundle).unwrap(), first, "a load and save changed the bytes");
    }

    #[test]
    fn legacy_sampler_source_shape_remains_loadable() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("legacy.mooloop");
        fs::create_dir(&bundle).unwrap();
        fs::write(
            bundle.join(MANIFEST_FILE),
            r#"format_version = 1
document_type = "song"
asset_mode = "embedded"

[document]
bpm = 120
ppq = 96
beats_per_bar = 4
playback_mode = "pattern"
current_pattern = 0
selected_channel = 0
pattern_lengths = [16]
playlist = []

[[document.channels]]
notes = [[]]
next_note_id = 1

[document.channels.setup.channel]
name = "Sampler 1"
kind = "sampler"
muted = false
volume = 0.8
pan = 0.0

[document.channels.setup.source]
type = "sampler"

[document.channels.setup.source.state.params]
voice_mode = "one_shot"
polyphony = 1
retrigger_mode = "restart"
choke_group = 0
start = 0.0
end = 1.0
reverse = false
root_note = 60
tune_semitones = 0.0
tune_cents = 0.0
loop_start = 0.0
loop_end = 1.0
loop_mode = "off"
attack = 0.001
decay = 0.25
sustain = 1.0
release = 0.05
filter_cutoff = 1.0
filter_resonance = 0.0
filter_env_amount = 0.0
drive = 0.0
bit_reduction = 0.0
rate_reduction = 0.0

[document.channels.setup.source.state.sample]
kind = "builtin"
id = "default_kick"
"#,
        )
        .unwrap();
        // The manifest predates the mixer: it names no buses and no channel
        // bus assignment. Both must default rather than fail the load.
        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected a song");
        };
        // The manifest explicitly names the legacy builtin-kick sample
        // reference, which the loader must still honor even though a fresh
        // project now defaults to no sample at all.
        let mut expected = Project::default();
        expected.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::Builtin {
            id: "default_kick".into(),
        };
        // The manifest stores the pre-reference-level default of 0.8, which
        // the loader must honor rather than replace with today's unity
        // default. Asserting it here is the point: a stored gain survives a
        // change to what a fresh channel starts at.
        expected.channels[0].setup.channel.volume = 0.8;
        // The manifest also predates the sampler's output trim. That mix was
        // balanced against a sampler at unity, so the missing field has to
        // deserialize to unity -- not to the -12 dB a sampler created today
        // starts at, which would quieten every old song by 12 dB.
        expected.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .params
            .output_gain = 1.0;
        assert_eq!(loaded, expected);
        // A manifest written before the mixer existed opens with the master
        // and nothing else, rather than sixteen empty tracks nobody made.
        assert_eq!(loaded.buses.len(), 1);
        assert_eq!(
            loaded.channels[0].setup.channel.bus,
            mooloop_core::MASTER_BUS
        );
    }

    /// Buses carry their own effect chains, so the round trip has to survive
    /// the same tagged-params path a channel's chain does.
    #[test]
    fn bus_routing_and_bus_effects_round_trip() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("routed.mooloop");
        let mut project = Project::default();
        project.ensure_tracks(5);
        project.channels[0].setup.channel.bus = 4;
        project.buses[4].bus.name = "Drums".into();
        project.buses[4].bus.output = 2;
        project.buses[4].bus.volume = 0.5;
        project.buses[4].push_effect(mooloop_core::EffectSlotState::of_kind(
            mooloop_core::EffectKind::Compressor,
        ));
        project.buses[mooloop_core::MASTER_BUS as usize].push_effect(
            mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Limiter),
        );

        save_song(&bundle, &project, AssetMode::Referenced).unwrap();
        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected a song");
        };
        assert_eq!(loaded, project);
    }

    #[test]
    fn complete_channel_and_effect_address_spaces_are_valid() {
        let mut project = Project::default();
        let channel = project.channels[0].clone();
        project.channels = vec![channel; mooloop_core::MAX_CHANNELS];
        // Thirty-two clones of one channel are thirty-two copies of its
        // identity, which is exactly the state `channel.id.duplicate`
        // reports. A full bank is thirty-two *channels*, so they are given
        // the identities a full bank would really have.
        for (index, channel) in project.channels.iter_mut().enumerate() {
            channel.id = mooloop_core::ChannelId(index as u32);
        }
        project.next_channel_id = mooloop_core::MAX_CHANNELS as u32;
        let effect = mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Filter);
        project.channels[0].setup.effects = vec![effect; mooloop_core::MAX_EFFECTS_PER_CHANNEL];
        project.buses[0].effects = vec![effect; mooloop_core::MAX_EFFECTS_PER_CHANNEL];
        project.buses[0].assign_device_ids();

        validate_project(&project).expect("the complete realtime address space is supported");
    }

    #[test]
    fn accepts_the_ui_output_gain_ceiling() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("loud.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.channel.volume = mooloop_core::MAX_LINEAR_GAIN;

        save_song(&bundle, &project, AssetMode::Referenced)
            .expect("the UI's +12 dB output setting must be saveable");
        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected a song");
        };
        assert_eq!(
            loaded.channels[0].setup.channel.volume,
            mooloop_core::MAX_LINEAR_GAIN
        );
    }

    #[test]
    fn rejects_mismatched_or_invalid_synth_sources() {
        let mut project = Project {
            swing_percent: 49,
            ..Project::default()
        };
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));
        project.swing_percent = 76;
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));

        let mut project = Project::starter_kit();
        project.channels[0].setup.channel.kind = mooloop_core::DeviceKind::Sampler;
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));

        // The v1 drum synth's choke group, which is the one validation
        // bounds. The starter kit used to be four of them; it is DS-01 now,
        // so the channel is made a v1 drum synth here.
        let mut project = Project::starter_kit();
        project.channels[0].setup = mooloop_core::ChannelSetup::drum_synth("Kick");
        project.channels[0]
            .setup
            .drum_synth_state_mut()
            .unwrap()
            .params
            .choke_group = MAX_CHOKE_GROUP + 1;
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));

        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::mono_synth(0, 1).with_id(id);
        project.channels[0]
            .setup
            .mono_synth_state_mut()
            .unwrap()
            .params
            .filter_cutoff = f32::NAN;
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));

        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::mono_synth(0, 1).with_id(id);
        project.channels[0]
            .setup
            .mono_synth_state_mut()
            .unwrap()
            .params
            .lfo
            .rate_hz = 400.0;
        let error = validate_project(&project).unwrap_err().to_string();
        assert!(error.contains("LFO rate"), "{error}");
        assert!(error.contains("range 0 to 20"), "{error}");
        assert!(error.contains("Mono Synth 1"), "{error}");

        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::poly_synth(0, 1).with_id(id);
        project.channels[0]
            .setup
            .poly_synth_state_mut()
            .unwrap()
            .params
            .polyphony = 0;
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));
    }

    #[test]
    fn mono_synth_params_without_an_lfo_table_keep_the_default_lfo() {
        let written = toml::to_string(&MonoSynthParams::default()).unwrap();
        let (before_lfo, _) = written.split_once("[lfo]").unwrap();
        let loaded: MonoSynthParams = toml::from_str(before_lfo).unwrap();
        assert_eq!(loaded, MonoSynthParams::default());
    }

    /// The ML-M1 carries `#[serde(default)]` from the start, so a
    /// manifest written by a build that predates any given field still loads.
    /// Asserted by truncating the table at the filter envelope, which is the
    /// shape a project saved before that block existed would have.
    #[test]
    fn mlm1_params_load_from_a_manifest_missing_later_fields() {
        let written = toml::to_string(&MlM1Params::default()).unwrap();
        let (before_filter_env, _) = written.split_once("filter_attack").unwrap();
        let loaded: MlM1Params = toml::from_str(before_filter_env).unwrap();
        assert_eq!(loaded, MlM1Params::default());
    }

    /// The ML-P8 carries `#[serde(default)]` from the start too. Truncated at
    /// the network amounts, which is the shape a project saved before step 02
    /// of the plan filled them in would have.
    #[test]
    fn mlp8_params_load_from_a_manifest_missing_later_fields() {
        let written = toml::to_string(&MlP8Params::default()).unwrap();
        let (before_network, _) = written.split_once("xmod").unwrap();
        let loaded: MlP8Params = toml::from_str(before_network).unwrap();
        assert_eq!(loaded, MlP8Params::default());
    }

    #[test]
    fn mlp8_source_round_trips_in_a_song() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("mlp8.mooloop");
        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::mlp8(0, 1).with_id(id);
        let params = &mut project.channels[0].setup.mlp8_state_mut().unwrap().params;
        params.xmod[mooloop_core::mlp8::xmod_index(1, 0)] = -62.5;
        params.osc_feedback[2] = 41.0;
        params.noise_to_osc[0] = 18.0;
        params.sync_source[0] = mooloop_core::SyncSource::Osc3;
        params.sub_level = 0.4;
        params.sub_octave = mooloop_core::SubOctave::Minus2;
        params.noise_color = -30.0;

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let manifest = fs::read_to_string(&bundle).unwrap();
        assert!(manifest.contains("type = \"mlp8\""), "{manifest}");
        assert_eq!(
            load_bundle(&bundle).unwrap().document,
            LoadedDocument::Song(project)
        );
    }

    #[test]
    fn ds01_source_round_trips_in_a_song() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("ds01.mooloop");
        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::ds01(0, 1).with_id(id);
        let params = &mut project.channels[0].setup.ds01_state_mut().unwrap().params;
        // One value from every band, because the bands are what the on-disk
        // form is made of: a field that failed to serialize would otherwise
        // hide behind its own default.
        params.tune = -12.0;
        params.retrigger = mooloop_core::Ds01Retrigger::Mono;
        params.tone_partials = 5;
        params.tone_spread = 0.75;
        params.noise_color = mooloop_core::Ds01NoiseColor::Velvet;
        params.body_ratio = 0.8;
        params.body_decay = 1.5;
        params.amp.gate = true;
        params.amp.sustain = 0.6;
        params.amp.curve = -0.4;
        params.pitch.depth = -36.0;
        params.noise_env.hold = 0.02;
        params.mod_env.decay = 2.0;
        params.burst_repeats = 4;
        params.burst_spread = -0.5;
        params.character = mooloop_core::Ds01Character::Crush;
        params.bits = 6.0;
        params.matrix[2] = mooloop_core::Ds01Route {
            source: mooloop_core::Ds01ModSource::HitAlternator,
            dest: mooloop_core::ds01::PARAM_FILTER_CUTOFF,
            amount: -0.65,
            curve: 0.3,
        };

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let manifest = fs::read_to_string(&bundle).unwrap();
        // The tag is `ds01`, and it is written down here because it is an
        // on-disk identifier: `rename_all` would have produced the same
        // string today, which is exactly why it is pinned rather than left to
        // depend on the variant's spelling.
        assert!(manifest.contains("type = \"ds01\""), "{manifest}");
        assert_eq!(
            load_bundle(&bundle).unwrap().document,
            LoadedDocument::Song(project)
        );
    }

    /// A row pointed at something that cannot be modulated is switched off
    /// rather than left to be applied at the trigger. Only reachable from a
    /// hand-edited file — the face cannot author one — which is exactly why
    /// the doctor has to see it.
    #[test]
    fn ds01_validation_switches_off_a_row_aimed_at_a_stepped_control() {
        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::ds01(0, 1).with_id(id);
        project.channels[0]
            .setup
            .ds01_state_mut()
            .unwrap()
            .params
            .matrix[0] = mooloop_core::Ds01Route {
            source: mooloop_core::Ds01ModSource::Velocity,
            dest: mooloop_core::ds01::PARAM_NOISE_COLOR,
            amount: 1.0,
            curve: 0.0,
        };

        assert!(integrity::repair_project(&mut project).is_usable());
        let route = project.channels[0].setup.ds01_state().unwrap().params.matrix[0];
        assert_eq!(route.source, mooloop_core::Ds01ModSource::None);
        assert!(mooloop_core::ds01::destination_index(route.dest).is_some());
    }

    /// A value out of range is repaired rather than refused, like every other
    /// range in the document.
    #[test]
    fn ds01_validation_clamps_an_out_of_range_value() {
        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::ds01(0, 1).with_id(id);
        project.channels[0]
            .setup
            .ds01_state_mut()
            .unwrap()
            .params
            .body_decay = 900.0;

        assert!(integrity::repair_project(&mut project).is_usable());
        assert_eq!(
            project.channels[0].setup.ds01_state().unwrap().params.body_decay,
            8.0
        );
    }

    /// A signed percent out of range is repaired rather than refused, like
    /// every other range in the document.
    #[test]
    fn mlp8_validation_clamps_an_out_of_range_route_amount() {
        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::mlp8(0, 1).with_id(id);
        project.channels[0]
            .setup
            .mlp8_state_mut()
            .unwrap()
            .params
            .xmod[0] = 900.0;

        assert!(integrity::repair_project(&mut project).is_usable());
        assert_eq!(
            project.channels[0].setup.mlp8_state().unwrap().params.xmod[0],
            100.0
        );
    }

    #[test]
    fn mlm1_source_round_trips_in_a_song() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("mlm1.mooloop");
        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::mlm1(0, 1).with_id(id);
        let params = &mut project.channels[0]
            .setup
            .mlm1_state_mut()
            .unwrap()
            .params;
        params.filter_decay = 0.08;
        params.filter_sustain = 0.0;
        params.filter_keytrack = 0.75;
        params.filter_env_amount = 0.6;

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let manifest = fs::read_to_string(&bundle).unwrap();
        assert!(manifest.contains("type = \"ml1\""));
        assert_eq!(
            load_bundle(&bundle).unwrap().document,
            LoadedDocument::Song(project)
        );
    }

    /// The device shipped as "ML-1" and was renamed to "ML-M1" after songs and
    /// channel presets had already been saved. Those files tag the source
    /// `ml1`, and the round-trip above cannot catch a break here because it
    /// writes and reads with the same build — rename both ends and it still
    /// passes. This pins the reader against a literal old manifest instead.
    #[test]
    fn a_source_saved_under_the_old_ml1_name_still_loads() {
        let source: ChannelSource = toml::from_str(
            r#"
            type = "ml1"

            [state.params]
            filter_decay = 0.08
            "#,
        )
        .expect("a source tagged with the pre-rename name must still load");

        assert_eq!(source.kind(), DeviceKind::MlM1);
        let ChannelSource::MlM1(state) = source else {
            panic!("`type = \"ml1\"` must load as the ML-M1");
        };
        assert_eq!(state.params.filter_decay, 0.08);
    }

    /// A buffer saved before 2026-09-16 holds `offset_beats`, and its lane
    /// holds id 0. Both mean beats behind a moving writer; neither exists any
    /// more.
    ///
    /// **The test is that the head lands on the same audio**, not that a
    /// number survived. One beat behind an eight-bar ring is one thirty-second
    /// of the way back from the writer, so `Position` has to come back at
    /// `31/32` -- and the lane, whose stored `0.0625` was a sixteenth of the
    /// old sixteen-beat range and therefore also one beat, has to come back at
    /// the same place.
    ///
    /// The fixture is built by writing the *modern* shape and rewriting the
    /// text, which is backwards from how it reads and is the only way round
    /// that works: `save_song` repairs before it writes, and a lane naming a
    /// retired id is exactly what repair deletes. Writing it by hand instead
    /// would mean spelling `ParamAddr`'s serde shape in a string literal,
    /// where a rename would make the fixture stop testing anything without
    /// failing.
    #[test]
    fn a_buffer_offset_saved_before_position_existed_lands_on_the_same_audio() {
        use mooloop_core::{
            AutomationLane, AutomationPoint, EffectTarget, ParamAddr, ParamOwner,
        };

        // Distinctive enough that the substitution below cannot hit anything
        // else in the manifest, and asserted to occur exactly once anyway.
        const LANE_MARKER: &str = "0.1234567";

        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");

        let mut project = Project::default();
        let mut slot = mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Buffer);
        slot.params = mooloop_core::EffectParams::Buffer(mooloop_core::BufferParams {
            bars: 8,
            ..Default::default()
        });
        project.channels[0].setup.push_effect(slot);
        project.channels[0].setup.assign_device_ids();
        let device = project.channels[0].setup.effects[0].id;
        project.channels[0].normalize_automation();
        let mut lane = AutomationLane::new(ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Effect { device },
            param: mooloop_core::BUFFER_PARAM_POSITION,
        });
        lane.reserve_points();
        lane.reset_points([AutomationPoint::new(1, 0, 0.1234567)]);
        project.channels[0].automation[0].push(lane);
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();

        // Now put the old spelling back: the device's key, the lane's id, and
        // the lane's value, which was a sixteenth of a 0..16 beat range and so
        // was one beat.
        let manifest = fs::read_to_string(&bundle).unwrap();
        for (what, needle) in [
            ("the device's position", "position = 1.0"),
            (
                "the lane's id",
                &format!("param = {}", mooloop_core::BUFFER_PARAM_POSITION),
            ),
            ("the lane's value", LANE_MARKER),
        ] {
            assert_eq!(
                manifest.matches(needle).count(),
                1,
                "{what} has to appear exactly once for the rewrite to be exact"
            );
        }
        let legacy = manifest
            .replace("position = 1.0", "offset_beats = 1.0")
            .replace(
                &format!("param = {}", mooloop_core::BUFFER_PARAM_POSITION),
                &format!("param = {}", mooloop_core::BUFFER_PARAM_OFFSET_BEATS),
            )
            .replace(LANE_MARKER, "0.0625");
        fs::write(&bundle, legacy).unwrap();

        let loaded = load_bundle(&bundle).unwrap();
        let LoadedDocument::Song(reopened) = loaded.document else {
            panic!("a song loads as a song");
        };

        // One beat behind an eight-bar ring is 1/32 of it.
        let expected = 1.0 - 1.0 / 32.0;
        let params = reopened.channels[0].setup.effects[0]
            .params
            .buffer()
            .expect("the slot is still a buffer");
        assert!(
            (params.position - expected).abs() < 1e-5,
            "the saved offset reopened at position {}, expected {expected}",
            params.position
        );

        let lanes = &reopened.channels[0].automation[0];
        assert_eq!(
            lanes.len(),
            1,
            "the lane must survive: repair deletes one that still names a \
             retired id, so losing it here means the migration did not run"
        );
        assert_eq!(lanes[0].target.param, mooloop_core::BUFFER_PARAM_POSITION);
        assert!(
            (lanes[0].points()[0].value - expected).abs() < 1e-5,
            "the lane reopened at {}, expected {expected}",
            lanes[0].points()[0].value
        );
    }

    /// A delay saved before 2026-09-08 named the five-entry
    /// `DelayTimeDivision`, whose serde names are a subset of the
    /// twenty-one-entry `ModTimeDivision` grid that replaced it. Those five
    /// strings are on-disk identifiers now, and the round-trip tests cannot
    /// see a break here because they write and read with the same build --
    /// rename both ends and they still pass. Same reason as
    /// `a_source_saved_under_the_old_ml1_name_still_loads`, so the same
    /// shape: decode from a literal manifest.
    ///
    /// `half` is deliberately not an identity. It was worth half a beat
    /// under the old five and is worth two under the grid it now shares, so
    /// such a delay reopens four times slower -- playing the half note its
    /// own label always claimed. `docs/PROJECT_FORMAT.md` records that as
    /// intended, and this pins it so nobody "fixes" it by accident.
    #[test]
    fn a_delay_division_saved_under_the_old_five_entry_grid_still_decodes() {
        #[derive(serde::Deserialize)]
        struct Held {
            division: mooloop_core::ModTimeDivision,
        }

        for (name, expected, beats) in [
            ("half", mooloop_core::ModTimeDivision::Half, 2.0),
            ("quarter", mooloop_core::ModTimeDivision::Quarter, 1.0),
            (
                "dotted_eighth",
                mooloop_core::ModTimeDivision::DottedEighth,
                0.75,
            ),
            (
                "eighth_triplet",
                mooloop_core::ModTimeDivision::EighthTriplet,
                1.0 / 3.0,
            ),
            ("sixteenth", mooloop_core::ModTimeDivision::Sixteenth, 0.25),
        ] {
            let held: Held = toml::from_str(&format!("division = \"{name}\""))
                .unwrap_or_else(|error| panic!("`{name}` must still decode: {error}"));
            assert_eq!(held.division, expected, "`{name}` decoded to the wrong division");
            assert!(
                (held.division.beats() - beats).abs() < 1e-6,
                "`{name}` is worth {} beats, expected {beats}",
                held.division.beats()
            );
        }
    }

    #[test]
    fn mlm1_validation_rejects_an_out_of_range_filter_envelope() {
        let mut project = Project::default();
        // The channel keeps its identity: swapping the generator changes
        // what channel 0 plays, not which channel it is.
        let id = project.channels[0].id;
        project.channels[0] = mooloop_core::ProjectChannel::mlm1(0, 1).with_id(id);
        project.channels[0]
            .setup
            .mlm1_state_mut()
            .unwrap()
            .params
            .filter_keytrack = 4.0;
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));
    }

    #[test]
    fn generator_presets_round_trip_for_every_kind() {
        let temp = tempdir().unwrap();
        let sources = [
            ChannelSource::Sampler(mooloop_core::SamplerState::default()),
            ChannelSource::DrumSynth(mooloop_core::DrumSynthState::default()),
            ChannelSource::MonoSynth(mooloop_core::MonoSynthState::default()),
            ChannelSource::PolySynth(mooloop_core::PolySynthState::default()),
            ChannelSource::MlM1(mooloop_core::MlM1State::default()),
            ChannelSource::MlP8(mooloop_core::MlP8State::default()),
        ];
        for (index, source) in sources.into_iter().enumerate() {
            let info = PresetInfo {
                name: format!("Preset {index}"),
                category: "Bass".into(),
                tags: vec!["warm".into(), "analog".into()],
            };
            let path = temp
                .path()
                .join(format!("preset-{index}.mooloop-generator"));
            save_generator_preset(&path, &source, info.clone(), AssetMode::Embedded).unwrap();
            let loaded = load_bundle(&path).unwrap();
            assert!(loaded.warnings.is_empty());
            assert_eq!(loaded.document, LoadedDocument::Generator(Box::new(source)));

            let manifest = fs::read_to_string(path.join(MANIFEST_FILE)).unwrap();
            let header: Header = toml::from_str(&manifest).unwrap();
            assert_eq!(header.preset, Some(info));
        }
    }

    #[test]
    fn channel_preset_round_trips_with_metadata() {
        let temp = tempdir().unwrap();
        let setup = ChannelSetup::mono_synth("Lead");
        let info = PresetInfo {
            name: "Screamer".into(),
            category: "Lead".into(),
            tags: vec!["aggressive".into()],
        };
        let path = temp.path().join("screamer.mooloop-channel");
        save_channel_preset(&path, &setup, info.clone(), AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&path).unwrap();
        assert_eq!(
            loaded.document,
            LoadedDocument::Channel(Box::new(setup.clone()))
        );

        let manifest = fs::read_to_string(path.join(MANIFEST_FILE)).unwrap();
        let header: Header = toml::from_str(&manifest).unwrap();
        assert_eq!(header.preset, Some(info));

        // A plain (non-preset) channel save leaves preset metadata unset.
        let plain_path = temp.path().join("plain.mooloop-channel");
        save_channel(&plain_path, &setup, AssetMode::Embedded).unwrap();
        let manifest = fs::read_to_string(plain_path.join(MANIFEST_FILE)).unwrap();
        let header: Header = toml::from_str(&manifest).unwrap();
        assert_eq!(header.preset, None);
    }

    #[test]
    fn list_presets_skips_corrupt_bundles_and_sorts_by_category_then_name() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("presets");
        fs::create_dir_all(&dir).unwrap();

        save_generator_preset(
            &dir.join("b.mooloop-generator"),
            &ChannelSource::MonoSynth(mooloop_core::MonoSynthState::default()),
            PresetInfo {
                name: "Zeta".into(),
                category: "Bass".into(),
                tags: vec![],
            },
            AssetMode::Embedded,
        )
        .unwrap();
        save_generator_preset(
            &dir.join("a.mooloop-generator"),
            &ChannelSource::MonoSynth(mooloop_core::MonoSynthState::default()),
            PresetInfo {
                name: "Alpha".into(),
                category: "Bass".into(),
                tags: vec![],
            },
            AssetMode::Embedded,
        )
        .unwrap();

        // A bundle with no manifest at all should be skipped, not error.
        fs::create_dir_all(dir.join("corrupt.mooloop-generator")).unwrap();
        fs::write(dir.join("not-a-bundle.txt"), "ignored").unwrap();

        let summaries = list_presets(&dir);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "Alpha");
        assert_eq!(summaries[1].name, "Zeta");
        assert_eq!(
            summaries[0].kind,
            PresetKind::Generator(mooloop_core::DeviceKind::MonoSynth)
        );

    }

    #[test]
    fn list_presets_on_missing_directory_returns_empty() {
        let temp = tempdir().unwrap();
        assert!(list_presets(&temp.path().join("does-not-exist")).is_empty());
    }

    #[test]
    fn drum_synth_params_without_new_punch_fields_keep_defaults() {
        let written = toml::to_string(&DrumSynthParams::default()).unwrap();
        let stripped = written
            .lines()
            .filter(|line| {
                !line.starts_with("kick_character")
                    && !line.starts_with("snare_character")
                    && !line.starts_with("hat_character")
                    && !line.starts_with("punch")
                    && !line.starts_with("snare_tone2_hz")
                    && !line.starts_with("snare_tone2_mix")
                    && !line.starts_with("snare_noise_color")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let loaded: DrumSynthParams = toml::from_str(&stripped).unwrap();
        assert_eq!(loaded, DrumSynthParams::default());
    }

    #[test]
    fn song_round_trip_retains_effect_chain() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0]
            .setup
            .push_effect(mooloop_core::EffectSlotState::filter(
                mooloop_core::FilterParams {
                    cutoff_hz: 1_250.0,
                    resonance: 0.6,
                    mode: mooloop_core::FilterMode::HighPass,
                    ..mooloop_core::FilterParams::default()
                },
            ));

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.document, LoadedDocument::Song(project));
    }

    #[test]
    fn song_round_trip_retains_a_mixed_effect_chain() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        // One slot per kind, so a kind whose tagged serde shape is broken
        // fails here rather than silently round-tripping as its default.
        for kind in mooloop_core::EffectKind::ALL {
            let mut slot = mooloop_core::EffectSlotState::of_kind(kind);
            for descriptor in kind.descriptors() {
                // Move every parameter off its default.
                let shifted = descriptor
                    .from_normalized((descriptor.to_normalized(descriptor.default) + 0.37) % 1.0);
                slot.params.set(descriptor.id, shifted);
            }
            slot.bypassed = kind == mooloop_core::EffectKind::Drive;
            project.channels[0].setup.push_effect(slot);
        }

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.document, LoadedDocument::Song(project));
    }

    /// A song with a hosted plugin in it, for the two tests below: one
    /// plugin device on channel 0 naming a slot whose plugin nobody here has
    /// installed, with sparse parameter ids and a megabyte of state.
    fn song_with_a_plugin() -> Project {
        let mut project = Project::default();
        let mut slot = mooloop_core::PluginSlotState::new(mooloop_core::PluginRef {
            format: mooloop_core::PluginFormat::Clap,
            id: "com.example.not-installed-anywhere".to_owned(),
            name: "Nowhere".to_owned(),
            vendor: "Example".to_owned(),
            version: "1.2.3".to_owned(),
        });
        slot.params = [7u32, 1000, 4_000_000_000]
            .into_iter()
            .map(|id| mooloop_core::PluginParamInfo {
                id,
                name: format!("Param {id}"),
                module: String::new(),
                min: 0.0,
                max: 1.0,
                default: 0.5,
                stepped: None,
                automatable: true,
                modulatable: true,
                hidden: false,
            })
            .collect();
        // A megabyte that is not all one byte, so the encoding is exercised
        // rather than compressed away by anything clever downstream.
        let data: Vec<u8> = (0..1_048_576u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        slot.state = mooloop_core::PluginStateText(mooloop_core::PluginState {
            chunks: vec![mooloop_core::PluginStateChunk { tag: "clap".to_owned(), data }],
        });
        let id = project.add_plugin_slot(slot);
        let mut device = mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Plugin);
        device.params = mooloop_core::EffectParams::Plugin(id);
        project.channels[0].setup.push_effect(device);
        project
    }

    /// `docs/plans/plugin-hosting/02-the-neutral-contract.md`: a song with a
    /// plugin slot round-trips exactly, a megabyte of state included, and the
    /// state is written as wrapped base64 rather than one enormous line.
    #[test]
    fn a_song_with_a_plugin_slot_round_trips_exactly() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("plugin.mooloop");
        let project = song_with_a_plugin();
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();

        let manifest = fs::read_to_string(&bundle).unwrap();
        assert!(manifest.contains("next_plugin_slot = 1"), "the mint was not saved");
        let longest = manifest.lines().map(str::len).max().unwrap_or(0);
        assert!(longest < 200, "a line of {longest} characters: the state was not wrapped");

        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.document, LoadedDocument::Song(project));
    }

    /// A plugin nobody has installed does not stop the song opening, and
    /// nothing about it is lost: its slot, parameters and state survive a
    /// save, and the second save is byte-identical to the first. The engine
    /// side of "missing" is `build_effect`'s pass-through placeholder,
    /// `a_plugin_device_with_no_plugin_passes_the_signal_through`.
    #[test]
    fn a_song_whose_plugin_is_missing_loads_and_saves_back_unchanged() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("missing.mooloop");
        save_song(&bundle, &song_with_a_plugin(), AssetMode::Embedded).unwrap();
        let first = fs::read(&bundle).unwrap();

        let LoadedDocument::Song(loaded) = load_bundle(&bundle).unwrap().document else {
            panic!("expected a song");
        };
        save_song(&bundle, &loaded, AssetMode::Embedded).unwrap();
        assert!(fs::read(&bundle).unwrap() == first, "the second save changed the file");
    }

    /// A channel whose source is a plugin (MOO-84) saves as the slot it
    /// plays, `type = "plugin"`, beside the slot's plugin, parameters and
    /// state in the song's plugin table; it loads with no repairs, equal to
    /// what was saved, and a second save of it is byte-identical. Nothing on
    /// disk needs the plugin installed.
    #[test]
    fn a_song_whose_source_is_a_plugin_round_trips() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("plugin-source.mooloop");
        let mut project = song_with_a_plugin();
        let state = project.plugins.values().next().unwrap().clone();
        let slot = project.add_plugin_slot(state);
        let channel = &mut project.channels[0].setup;
        channel.source = mooloop_core::ChannelSource::Plugin(slot);
        channel.channel.kind = mooloop_core::DeviceKind::Plugin;
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let first = fs::read(&bundle).unwrap();
        let manifest = String::from_utf8(first.clone()).unwrap();
        assert!(manifest.contains("type = \"plugin\""), "the source is not tagged plugin");

        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert!(loaded.repairs.is_empty(), "{:?}", loaded.repairs);
        let LoadedDocument::Song(loaded) = loaded.document else {
            panic!("expected a song");
        };
        assert_eq!(loaded, project);
        assert_eq!(loaded.channels[0].setup.source.kind(), mooloop_core::DeviceKind::Plugin);
        save_song(&bundle, &loaded, AssetMode::Embedded).unwrap();
        assert!(fs::read(&bundle).unwrap() == first, "the second save changed the file");
    }

    /// A plugin parameter's lanes and routes survive a save and a load byte
    /// for byte (MOO-78, Adam's MOO-74 ruling): the device is the pass-through
    /// placeholder a missing plugin plays as, one id is in the slot's
    /// reported list and one is in no list at all, and the integrity pass
    /// that runs on load judges neither id. `owner.plugin_param` is the
    /// spelling on disk.
    #[test]
    fn plugin_parameter_lanes_and_routes_save_back_unchanged() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("plugin-lanes.mooloop");
        let mut project = song_with_a_plugin();
        let setup = &mut project.channels[0].setup;
        setup
            .modulation
            .install(0, mooloop_core::ModulatorParams::Lfo(Default::default()));
        let device = setup.effects.last().unwrap().id;
        assert!(device.is_assigned(), "test setup: the plugin device has no id");
        let here = mooloop_core::EffectTarget::Channel(0);
        let reported = mooloop_core::ParamAddr::plugin_param(here, device, 4_000_000_000);
        let unknown = mooloop_core::ParamAddr::plugin_param(here, device, 123_456);
        for destination in [reported, unknown] {
            setup
                .modulation
                .add_route(mooloop_core::ModRoute::to_slot(
                    0,
                    destination,
                    0.5,
                    mooloop_core::ModPolarity::Bipolar,
                ))
                .unwrap();
        }
        project.channels[0].automation[0].push(mooloop_core::AutomationLane::new(unknown));

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let first = fs::read(&bundle).unwrap();
        let manifest = String::from_utf8(first.clone()).unwrap();
        assert!(manifest.contains("plugin_param"), "the owner is not spelled on disk");

        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.repairs.is_empty(), "the load repaired something: {:?}", loaded.repairs);
        let LoadedDocument::Song(loaded) = loaded.document else {
            panic!("expected a song");
        };
        assert_eq!(loaded, project);
        save_song(&bundle, &loaded, AssetMode::Embedded).unwrap();
        assert!(fs::read(&bundle).unwrap() == first, "the second save changed the file");
    }

    /// A song with no plugins says nothing about plugins, so it is
    /// byte-identical to one written before the table existed.
    #[test]
    fn a_song_with_no_plugins_writes_no_plugin_keys() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("plain.mooloop");
        save_song(&bundle, &Project::default(), AssetMode::Embedded).unwrap();
        let manifest = fs::read_to_string(&bundle).unwrap();
        assert!(!manifest.contains("plugin"), "{manifest}");
    }

    #[test]
    fn song_manifest_with_untagged_filter_params_still_loads() {
        // Songs written while `Filter` was the only effect kind stored
        // `params` as a bare `FilterParams` table with a sibling `kind` key.
        // `EffectParams` is tagged now; the old shape must still load.
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("legacy-effects.mooloop");
        let mut project = Project::default();
        project.channels[0]
            .setup
            .push_effect(mooloop_core::EffectSlotState::filter(
                mooloop_core::FilterParams {
                    cutoff_hz: 1_250.0,
                    resonance: 0.6,
                    mode: mooloop_core::FilterMode::HighPass,
                    ..mooloop_core::FilterParams::default()
                },
            ));
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();

        // Rewrite the tagged effect table back into the pre-tag shape.
        let manifest = fs::read_to_string(&bundle).unwrap();
        let legacy = manifest
            .replace(
                "[document.channels.setup.effects.params]\ntype = \"filter\"\n\n\
                 [document.channels.setup.effects.params.state]",
                "[document.channels.setup.effects.params]",
            )
            .replace("bypassed = false", "kind = \"filter\"\nbypassed = false");
        assert_ne!(manifest, legacy, "test setup: nothing was rewritten");
        assert!(
            !legacy.contains("params.state"),
            "test setup: the tagged table survived the rewrite"
        );
        fs::write(&bundle, legacy).unwrap();

        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.document, LoadedDocument::Song(project));
    }

    #[test]
    fn song_manifest_without_effects_field_still_loads() {
        // Songs written before effects existed have no `effects` key on their
        // channels; `#[serde(default)]` must fill in an empty chain.
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("legacy.mooloop");
        fs::create_dir(&bundle).unwrap();
        let project = Project::default();
        let envelope = Envelope {
            format_version: FORMAT_VERSION,
            document_type: "song".into(),
            asset_mode: AssetMode::Referenced,
            preset: None,
            contains: Vec::new(),
            plugin: None,
            document: project.clone(),
        };
        let manifest = toml::to_string_pretty(&envelope).unwrap();
        let stripped = manifest
            .lines()
            .filter(|line| !line.starts_with("effects"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_ne!(manifest, stripped, "test setup: nothing stripped");
        fs::write(bundle.join(MANIFEST_FILE), stripped).unwrap();

        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.document, LoadedDocument::Song(project));
    }

    // --- Effect presets (docs/plans/preset-system/01) ---------------------

    fn effect_info(name: &str) -> PresetInfo {
        PresetInfo {
            name: name.into(),
            category: "Space".into(),
            tags: vec!["wide".into()],
        }
    }

    /// The whole point of the format: a row saved from one channel loads on
    /// any other with nothing to re-scope, because nothing in it names a
    /// channel to begin with.
    #[test]
    fn an_effect_preset_round_trips() {
        let temp = tempdir().unwrap();
        let mut effect = EffectSlotState::of_kind(EffectKind::Delay);
        if let mooloop_core::EffectParams::Delay(delay) = &mut effect.params {
            delay.feedback = 0.71;
            delay.mode = mooloop_core::DelayMode::Tape;
            delay.tempo_sync = true;
            delay.time_division = mooloop_core::ModTimeDivision::DottedEighth;
        }
        let path = temp.path().join("tape.mooloop-effect");
        let report =
            save_effect_preset(&path, &effect, effect_info("Tape"), AssetMode::Embedded).unwrap();
        assert!(report.warnings.is_empty());
        assert!(report.repairs.is_empty());

        let loaded = load_bundle(&path).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.document, LoadedDocument::Effect(Box::new(effect)));
    }

    /// One loop over every kind is what catches a `serde` attribute missing
    /// from one params struct, which no single-kind test would.
    #[test]
    fn effect_presets_round_trip_for_every_kind() {
        let temp = tempdir().unwrap();
        for kind in EffectKind::ALL {
            let effect = EffectSlotState::of_kind(kind);
            let path = temp.path().join(format!("{kind:?}.mooloop-effect"));
            save_effect_preset(&path, &effect, effect_info("Default"), AssetMode::Embedded)
                .unwrap_or_else(|error| panic!("{kind:?} did not save: {error}"));
            let loaded = load_bundle(&path).unwrap_or_else(|error| panic!("{kind:?}: {error}"));
            let LoadedDocument::Effect(back) = loaded.document else {
                panic!("{kind:?} did not load as an effect");
            };
            assert_eq!(*back, effect, "{kind:?} changed on the way through disk");
            assert_eq!(back.kind(), kind);
        }
    }

    /// These four default on load, so a bug that dropped them would be
    /// silent: the preset would open, at the wrong settings.
    #[test]
    fn host_settings_survive_an_effect_preset() {
        let temp = tempdir().unwrap();
        let effect = EffectSlotState {
            bypassed: true,
            wet_dry: 0.4,
            input_trim: 0.5,
            output_trim: 1.75,
            ..EffectSlotState::of_kind(EffectKind::Reverb)
        };
        let path = temp.path().join("hosted.mooloop-effect");
        save_effect_preset(&path, &effect, effect_info("Hosted"), AssetMode::Embedded).unwrap();
        let LoadedDocument::Effect(back) = load_bundle(&path).unwrap().document else {
            panic!("not an effect");
        };
        assert!(back.bypassed);
        assert_eq!(back.wet_dry, 0.4);
        assert_eq!(back.input_trim, 0.5);
        assert_eq!(back.output_trim, 1.75);
    }

    #[test]
    fn list_presets_names_an_effect_preset_by_its_kind() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("effects");
        for (name, kind) in [("Bright", EffectKind::Filter), ("Warm", EffectKind::Drive)] {
            save_effect_preset(
                &dir.join(format!("{name}.mooloop-effect")),
                &EffectSlotState::of_kind(kind),
                PresetInfo {
                    name: name.into(),
                    category: String::new(),
                    tags: Vec::new(),
                },
                AssetMode::Embedded,
            )
            .unwrap();
        }
        let listed = list_presets(&dir);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "Bright");
        assert_eq!(listed[0].kind, PresetKind::Effect(EffectKind::Filter));
        assert_eq!(listed[1].name, "Warm");
        assert_eq!(listed[1].kind, PresetKind::Effect(EffectKind::Drive));
    }

    /// Matches `summarize_preset`'s contract for the other kinds: a bundle
    /// from another format version is left out of the list, not an error.
    #[test]
    fn list_presets_skips_an_effect_preset_from_another_format_version() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("effects");
        let path = dir.join("future.mooloop-effect");
        save_effect_preset(
            &path,
            &EffectSlotState::of_kind(EffectKind::Gate),
            effect_info("Future"),
            AssetMode::Embedded,
        )
        .unwrap();
        let manifest_path = path.join(MANIFEST_FILE);
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        let bumped = manifest.replacen(
            &format!("format_version = {FORMAT_VERSION}"),
            &format!("format_version = {}", FORMAT_VERSION + 1),
            1,
        );
        assert_ne!(manifest, bumped, "the manifest did not carry its version");
        fs::write(&manifest_path, bumped).unwrap();

        assert!(list_presets(&dir).is_empty());
        assert!(matches!(
            load_bundle(&path),
            Err(Error::UnsupportedVersion(version)) if version == FORMAT_VERSION + 1
        ));
    }

    /// The explicit record that lets a later fragment reader tell one row
    /// from a run of rows. Read back as data rather than through the loader,
    /// because the loader is what a future reader will not be.
    #[test]
    fn an_effect_preset_records_what_it_contains() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("recorded.mooloop-effect");
        save_effect_preset(
            &path,
            &EffectSlotState::of_kind(EffectKind::Compressor),
            effect_info("Recorded"),
            AssetMode::Embedded,
        )
        .unwrap();
        let manifest = fs::read_to_string(path.join(MANIFEST_FILE)).unwrap();
        let header: Header = toml::from_str(&manifest).unwrap();
        assert_eq!(header.document_type, "effect");
        assert_eq!(header.contains, vec!["effect_params".to_string()]);
        assert_eq!(header.preset, Some(effect_info("Recorded")));
    }

    /// The forward-compatibility promise: a bundle holding more than this
    /// version understands is refused whole, not opened at half strength.
    /// The generator and channel loaders are not made to honour it, because
    /// nothing has ever written the field for them.
    #[test]
    fn an_effect_preset_containing_something_unknown_is_refused() {
        let temp = tempdir().unwrap();
        let dir = temp.path().join("effects");
        let path = dir.join("fragment.mooloop-effect");
        save_effect_preset(
            &path,
            &EffectSlotState::of_kind(EffectKind::Plate),
            effect_info("Fragment"),
            AssetMode::Embedded,
        )
        .unwrap();
        let manifest_path = path.join(MANIFEST_FILE);
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        let widened = manifest.replacen(
            "contains = [\"effect_params\"]",
            "contains = [\"effect_params\", \"mod_routes\"]",
            1,
        );
        assert_ne!(manifest, widened, "the manifest did not carry its contains list");
        fs::write(&manifest_path, widened).unwrap();

        match load_bundle(&path) {
            Err(Error::UnsupportedContents(entry)) => assert_eq!(entry, "mod_routes"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        // And it is not offered in the list only to be refused on the click.
        assert!(list_presets(&dir).is_empty());
    }

    /// A plugin as a device preset would carry it (MOO-222): moved off its
    /// default, pinned, and holding state bytes of its own.
    fn saved_plugin() -> PluginSlotState {
        let mut plugin = PluginSlotState::new(mooloop_core::PluginRef {
            format: mooloop_core::PluginFormat::Clap,
            id: "org.example.gain".into(),
            name: "Gain".into(),
            vendor: "Example Audio".into(),
            version: "1.2.0".into(),
        });
        plugin.params.push(mooloop_core::PluginParamInfo {
            id: 4_000_000_000,
            name: "Gain".into(),
            module: String::new(),
            min: 0.0,
            max: 2.0,
            default: 1.0,
            stepped: None,
            automatable: true,
            modulatable: true,
            hidden: false,
        });
        plugin.pinned.push(4_000_000_000);
        plugin.state = mooloop_core::PluginStateText(mooloop_core::PluginState {
            chunks: vec![mooloop_core::PluginStateChunk {
                tag: "clap".into(),
                data: (0..=255u8).collect(),
            }],
        });
        plugin
    }

    fn plugin_row() -> EffectSlotState {
        let mut effect = EffectSlotState::of_kind(EffectKind::Plugin);
        effect.params = mooloop_core::EffectParams::Plugin(mooloop_core::PluginSlotId(3));
        effect
    }

    /// **A plugin device saves as a preset with its plugin, and comes back
    /// with none of the song it came from** (MOO-222): the plugin, its list,
    /// its pins and its state bytes intact, the row's host settings with it,
    /// and neither the device's identity nor the slot it had.
    #[test]
    fn a_plugin_preset_round_trips_its_plugin_and_no_identity() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("warm.mooloop-effect");
        let mut effect = plugin_row();
        effect.wet_dry = 0.25;
        let effect = effect.with_id(mooloop_core::DeviceId(7));
        let plugin = saved_plugin();
        save_plugin_effect_preset(&path, &effect, &plugin, effect_info("Warm"), AssetMode::Embedded)
            .unwrap();

        let manifest = fs::read_to_string(path.join(MANIFEST_FILE)).unwrap();
        let header: Header = toml::from_str(&manifest).unwrap();
        assert_eq!(header.contains, EFFECT_PLUGIN_PRESET_CONTAINS);
        let LoadedDocument::PluginEffect {
            effect: back,
            plugin: back_plugin,
        } = load_bundle(&path).unwrap().document
        else {
            panic!("a plugin preset did not load as one");
        };
        assert_eq!(*back_plugin, plugin);
        assert_eq!(back.wet_dry, 0.25);
        assert_eq!(back.id, mooloop_core::DeviceId::UNASSIGNED);
        assert_eq!(
            back.params,
            mooloop_core::EffectParams::Plugin(mooloop_core::PluginSlotId::UNASSIGNED)
        );
        // Listed as a plugin device's preset, so a plugin row can offer it.
        let listed = list_presets(temp.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].kind, PresetKind::Effect(EffectKind::Plugin));
    }

    /// **0.1.5 refuses a plugin preset rather than half loading it**
    /// (MOO-222). Its reader checks an `effect` bundle's list against
    /// `["effect_params"]` -- `EFFECT_PRESET_CONTAINS`, unchanged since, and
    /// read from the `v0.1.5` tag's `parse_manifest` -- before it parses the
    /// document. This is that check, run on the manifest a save writes now.
    #[test]
    fn an_older_reader_refuses_a_plugin_preset() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("warm.mooloop-effect");
        save_plugin_effect_preset(
            &path,
            &plugin_row(),
            &saved_plugin(),
            effect_info("Warm"),
            AssetMode::Embedded,
        )
        .unwrap();
        let manifest = fs::read_to_string(path.join(MANIFEST_FILE)).unwrap();
        let header: Header = toml::from_str(&manifest).unwrap();
        const V0_1_5_EFFECT_CONTAINS: &[&str] = &["effect_params"];
        assert_eq!(EFFECT_PRESET_CONTAINS, V0_1_5_EFFECT_CONTAINS);
        match validate_contains(&header.contains, V0_1_5_EFFECT_CONTAINS) {
            Err(Error::UnsupportedContents(entry)) => assert_eq!(entry, "effect_plugin"),
            other => panic!("0.1.5 would have opened it: {other:?}"),
        }
    }

    /// A plugin row cannot be saved as a plain effect preset: its slot is a
    /// number that names nothing in the song it would land in. Nor can a
    /// native row be saved as a plugin preset.
    #[test]
    fn a_plugin_row_saves_only_as_a_plugin_preset() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("bare.mooloop-effect");
        assert!(matches!(
            save_effect_preset(&path, &plugin_row(), effect_info("Bare"), AssetMode::Embedded),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            save_plugin_effect_preset(
                &path,
                &EffectSlotState::of_kind(EffectKind::Delay),
                &saved_plugin(),
                effect_info("Bare"),
                AssetMode::Embedded
            ),
            Err(Error::Invalid(_))
        ));
        assert!(!path.exists());
    }

    /// A bundle that says it holds a plugin and has none is refused whole,
    /// and is not listed.
    #[test]
    fn a_plugin_preset_missing_its_plugin_is_refused() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("gone.mooloop-effect");
        save_plugin_effect_preset(
            &path,
            &plugin_row(),
            &saved_plugin(),
            effect_info("Gone"),
            AssetMode::Embedded,
        )
        .unwrap();
        let manifest_path = path.join(MANIFEST_FILE);
        let manifest = fs::read_to_string(&manifest_path).unwrap();
        let cut = manifest.find("[plugin").expect("the plugin table was written");
        fs::write(&manifest_path, &manifest[..cut]).unwrap();
        assert!(matches!(load_bundle(&path), Err(Error::Invalid(_))));
        assert!(list_presets(temp.path()).is_empty());
    }

    /// A non-finite host setting is corrected on the way out, as it is for
    /// every other document, and the correction is reported.
    #[test]
    fn an_effect_preset_with_a_bad_trim_is_repaired_on_save() {
        let temp = tempdir().unwrap();
        let effect = EffectSlotState {
            wet_dry: f32::NAN,
            ..EffectSlotState::of_kind(EffectKind::Bitcrush)
        };
        let path = temp.path().join("nan.mooloop-effect");
        let report =
            save_effect_preset(&path, &effect, effect_info("NaN"), AssetMode::Embedded).unwrap();
        assert_eq!(report.repairs.len(), 1);
        let LoadedDocument::Effect(back) = load_bundle(&path).unwrap().document else {
            panic!("not an effect");
        };
        assert_eq!(back.wet_dry, 1.0);
    }
}
