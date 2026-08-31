//! Versioned mooloop documents and sample-asset handling.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mooloop_core::{ChannelSetup, ChannelSource, DeviceKind, Kit, Project, SampleReference};
use serde::{Deserialize, Serialize};

pub mod factory;
pub mod integrity;

pub use factory::{rescope_modulation, seed_ml1_bank};
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
}

impl DocumentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::Kit => "kit",
            Self::Channel => "channel",
            Self::Generator => "generator",
        }
    }
}

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

/// Summary of a preset bundle found by [`list_presets`], cheap to compute
/// because it only reads the manifest header and preset metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetSummary {
    pub path: PathBuf,
    pub name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub kind: DeviceKind,
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
    Generator(ChannelSource),
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
    document: T,
}

#[derive(Deserialize)]
struct Header {
    format_version: u32,
    document_type: String,
    #[serde(default)]
    preset: Option<PresetInfo>,
}

/// Writes `project` to `path`, correcting on the way out anything that can be
/// corrected without discarding work. The saved file is therefore the repaired
/// document, and [`SaveReport::repairs`] says what changed; only a problem
/// whose fix would delete notes or clips stops the write, and that comes back
/// as [`Error::InvalidDocument`] naming exactly where it is.
pub fn save_song(path: &Path, project: &Project, mode: AssetMode) -> Result<SaveReport, Error> {
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
    let mut report = save_with_assets(path, DocumentKind::Kit, kit, mode, None, |kit| {
        kit.channels
            .iter_mut()
            .map(|setup| &mut setup.source)
            .collect()
    })?;
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
        |source| vec![source],
    )?;
    report.repairs = diagnosis.issues;
    Ok(report)
}

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
    let staging_assets = parent.join(format!(".{stem}-assets.tmp-{}-{nonce}", std::process::id()));

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
                &staging_assets,
                mode,
                &mut copied,
                &mut report.warnings,
            )?;
        }
        let envelope = Envelope {
            format_version: FORMAT_VERSION,
            document_type: DocumentKind::Song.as_str().into(),
            asset_mode: mode,
            preset: None,
            document,
        };
        fs::write(&staging_file, toml::to_string_pretty(&envelope)?)?;
        replace_song_file(path, &staging_file, &target_assets, &staging_assets)?;
        Ok(report)
    })();

    if staging_file.exists() {
        let _ = fs::remove_file(&staging_file);
    }
    if staging_assets.exists() {
        let _ = fs::remove_dir_all(&staging_assets);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn prepare_song_asset(
    channel: usize,
    source: &mut ChannelSource,
    target: &Path,
    target_assets: &Path,
    staging_assets: &Path,
    mode: AssetMode,
    copied: &mut HashMap<PathBuf, PathBuf>,
    warnings: &mut Vec<AssetWarning>,
) -> Result<(), Error> {
    let ChannelSource::Sampler(sampler) = source else {
        return Ok(());
    };
    let SampleReference::File { path, embedded } = &mut sampler.sample else {
        return Ok(());
    };

    let source = path.clone();
    let keep_owned = *embedded && (source.starts_with(target) || source.starts_with(target_assets));
    let parent = target.parent().expect("validated song parent");
    if mode == AssetMode::Referenced && !keep_owned {
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
        let asset_name = target_assets
            .file_name()
            .expect("song assets path has a file name");
        let relative = PathBuf::from(asset_name)
            .join("samples")
            .join(format!("{channel:02}-{name}"));
        let destination = staging_assets
            .join("samples")
            .join(format!("{channel:02}-{name}"));
        fs::create_dir_all(destination.parent().expect("sample destination has parent"))?;
        fs::copy(&source, destination)?;
        copied.insert(canonical, relative.clone());
        relative
    };
    *path = relative;
    *embedded = true;
    Ok(())
}

fn song_assets_path(path: &Path) -> Result<PathBuf, Error> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Invalid("song path has no parent".into()))?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::Invalid("song path has no file name".into()))?;
    let mut assets_name = name.to_os_string();
    assets_name.push("-assets");
    Ok(parent.join(assets_name))
}

fn remove_path(path: &Path) -> Result<(), std::io::Error> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn replace_song_file(
    target: &Path,
    staging_file: &Path,
    target_assets: &Path,
    staging_assets: &Path,
) -> Result<(), Error> {
    let parent = target.parent().expect("validated song parent");
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mooloop");
    let backup = parent.join(format!(".{name}.backup-{}", std::process::id()));
    let assets_backup = parent.join(format!(".{name}-assets.backup-{}", std::process::id()));
    for stale in [&backup, &assets_backup] {
        if stale.exists() {
            remove_path(stale)?;
        }
    }

    let had_target = target.exists();
    let had_assets = target_assets.exists();
    if had_target {
        fs::rename(target, &backup)?;
    }
    if had_assets {
        if let Err(error) = fs::rename(target_assets, &assets_backup) {
            if had_target {
                let _ = fs::rename(&backup, target);
            }
            return Err(Error::Io(error));
        }
    }

    let installs_assets = staging_assets.exists();
    if installs_assets {
        if let Err(error) = fs::rename(staging_assets, target_assets) {
            if had_assets {
                let _ = fs::rename(&assets_backup, target_assets);
            }
            if had_target {
                let _ = fs::rename(&backup, target);
            }
            return Err(Error::Io(error));
        }
    }
    if let Err(error) = fs::rename(staging_file, target) {
        if installs_assets {
            let _ = fs::rename(target_assets, staging_assets);
        }
        if had_assets {
            let _ = fs::rename(&assets_backup, target_assets);
        }
        if had_target {
            let _ = fs::rename(&backup, target);
        }
        return Err(Error::Io(error));
    }

    if had_target {
        remove_path(&backup)?;
    }
    if had_assets {
        remove_path(&assets_backup)?;
    }
    Ok(())
}

fn save_with_assets<T, F>(
    path: &Path,
    kind: DocumentKind,
    mut document: T,
    mode: AssetMode,
    preset: Option<PresetInfo>,
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
            document,
        };
        let manifest = toml::to_string_pretty(&envelope)?;
        fs::write(staging.join(MANIFEST_FILE), manifest)?;
        replace_bundle(path, &staging)?;
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
    let ChannelSource::Sampler(sampler) = source else {
        return Ok(());
    };
    let SampleReference::File { path, embedded } = &mut sampler.sample else {
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
        let relative = PathBuf::from("samples").join(format!("{channel:02}-{name}"));
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

fn replace_bundle(target: &Path, staging: &Path) -> Result<(), Error> {
    if !target.exists() {
        fs::rename(staging, target)?;
        return Ok(());
    }

    let parent = target.parent().expect("validated bundle parent");
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mooloop");
    let backup = parent.join(format!(".{name}.backup-{}", std::process::id()));
    if backup.exists() {
        fs::remove_dir_all(&backup)?;
    }
    fs::rename(target, &backup)?;
    if let Err(error) = fs::rename(staging, target) {
        let _ = fs::rename(&backup, target);
        return Err(Error::Io(error));
    }
    fs::remove_dir_all(backup)?;
    Ok(())
}

pub fn load_bundle(path: &Path) -> Result<LoadReport, Error> {
    let manifest_path = if path.is_dir() {
        path.join(MANIFEST_FILE)
    } else {
        path.to_path_buf()
    };
    let manifest = fs::read_to_string(&manifest_path)?;
    let header: Header = toml::from_str(&manifest)?;
    if header.format_version != FORMAT_VERSION {
        return Err(Error::UnsupportedVersion(header.format_version));
    }

    let (mut document, asset_mode) = match header.document_type.as_str() {
        "song" => {
            let envelope: Envelope<Project> = toml::from_str(&manifest)?;
            validate_envelope(&envelope, "song")?;
            (LoadedDocument::Song(envelope.document), envelope.asset_mode)
        }
        "kit" => {
            let envelope: Envelope<Kit> = toml::from_str(&manifest)?;
            validate_envelope(&envelope, "kit")?;
            (LoadedDocument::Kit(envelope.document), envelope.asset_mode)
        }
        "channel" => {
            let envelope: Envelope<ChannelSetup> = toml::from_str(&manifest)?;
            validate_envelope(&envelope, "channel")?;
            (
                LoadedDocument::Channel(Box::new(envelope.document)),
                envelope.asset_mode,
            )
        }
        "generator" => {
            let envelope: Envelope<ChannelSource> = toml::from_str(&manifest)?;
            validate_envelope(&envelope, "generator")?;
            (
                LoadedDocument::Generator(envelope.document),
                envelope.asset_mode,
            )
        }
        other => return Err(Error::UnsupportedDocument(other.into())),
    };

    // Repair runs after the padding steps below rather than before them,
    // because a bank that is merely short is the legitimate on-disk shape of
    // an older song rather than damage, and padding it first keeps the two
    // from being confused. A file that needs more than repair can offer does
    // not open -- but that is now the only case that does not.
    let mut warnings = Vec::new();
    let diagnosis = match &mut document {
        LoadedDocument::Song(project) => {
            for (index, channel) in project.channels.iter_mut().enumerate() {
                resolve_setup_asset(path, index, &mut channel.setup.source, &mut warnings)?;
                channel.normalize_automation();
                channel.recompute_next_note_id();
            }
            integrity::repair_project(project)
        }
        LoadedDocument::Kit(kit) => {
            for (index, setup) in kit.channels.iter_mut().enumerate() {
                resolve_setup_asset(path, index, &mut setup.source, &mut warnings)?;
            }
            integrity::repair_setups(DocumentKind::Kit, &mut kit.channels)
        }
        LoadedDocument::Channel(setup) => {
            resolve_setup_asset(path, 0, &mut setup.source, &mut warnings)?;
            integrity::repair_setups(DocumentKind::Channel, std::slice::from_mut(setup.as_mut()))
        }
        LoadedDocument::Generator(source) => {
            resolve_setup_asset(path, 0, source, &mut warnings)?;
            integrity::repair_source(DocumentKind::Generator, source)
        }
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
/// `*.mooloop-channel` directories) and returns a summary for each,
/// sorted by `(category, name)`. Bundles that fail to parse, are missing
/// preset metadata, or aren't a `generator`/`channel` document are
/// silently skipped rather than failing the whole scan. Returns an empty
/// list if `dir` doesn't exist yet.
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
            envelope.document.kind()
        }
        "channel" => {
            let envelope: Envelope<ChannelSetup> = toml::from_str(&manifest).ok()?;
            envelope.document.kind()
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
    let ChannelSource::Sampler(sampler) = source else {
        return Ok(());
    };
    let SampleReference::File { path, embedded } = &mut sampler.sample else {
        return Ok(());
    };
    if *embedded && !safe_embedded_path(bundle, path) {
        return Err(Error::Invalid(format!(
            "channel {channel} has unsafe embedded path {}",
            path.display()
        )));
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

fn safe_embedded_path(bundle: &Path, path: &Path) -> bool {
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return false;
    }
    if bundle.is_dir() {
        return path.starts_with("samples");
    }
    song_assets_path(bundle)
        .ok()
        .and_then(|assets| assets.file_name().map(PathBuf::from))
        .is_some_and(|assets| path.starts_with(assets.join("samples")))
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
        AutomationLane, AutomationPoint, DrumSynthParams, Ml1Params, MonoSynthParams, NoteEvent,
        ParamAddr, PatternPlacement, MAX_CHOKE_GROUP,
    };
    use tempfile::tempdir;

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

    #[test]
    fn automation_lanes_round_trip_and_older_songs_load_without_them() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        let mut lane = AutomationLane::new(ParamAddr::effect(
            mooloop_core::EffectTarget::Channel(0),
            2,
            7,
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

    #[test]
    fn two_lanes_on_one_destination_are_rejected() {
        let mut project = Project::default();
        let target = ParamAddr::effect(mooloop_core::EffectTarget::Channel(0), 0, 1);
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

    #[test]
    fn resaving_without_embedded_samples_removes_the_old_sidecar() {
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
        assert!(assets.is_dir());

        project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .sample = SampleReference::default();
        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert!(!assets.exists());
        assert_eq!(
            load_bundle(&bundle).unwrap().document,
            LoadedDocument::Song(project)
        );
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
        project.channels[0] = mooloop_core::ProjectChannel::mono_synth(0, 1);
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
        let project = Project::starter_kit(7);

        let report = save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert!(report.warnings.is_empty());
        assert!(bundle.is_file());
        assert!(!song_assets_path(&bundle).unwrap().exists());
        let manifest = fs::read_to_string(&bundle).unwrap();
        assert!(manifest.contains("type = \"drum_synth\""));
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
        assert_eq!(loaded, expected);
        assert_eq!(loaded.buses.len(), mooloop_core::MAX_BUSES);
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
        project.channels[0].setup.channel.bus = 4;
        project.buses[4].bus.name = "Drums".into();
        project.buses[4].bus.output = 2;
        project.buses[4].bus.volume = 0.5;
        project.buses[4]
            .effects
            .push(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Compressor,
            ));
        project.buses[mooloop_core::MASTER_BUS as usize]
            .effects
            .push(mooloop_core::EffectSlotState::of_kind(
                mooloop_core::EffectKind::Limiter,
            ));

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
        let effect = mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Filter);
        project.channels[0].setup.effects = vec![effect; mooloop_core::MAX_EFFECTS_PER_CHANNEL];
        project.buses[0].effects = vec![effect; mooloop_core::MAX_EFFECTS_PER_CHANNEL];

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

        let mut project = Project::starter_kit(1);
        project.channels[0].setup.channel.kind = mooloop_core::DeviceKind::Sampler;
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));

        let mut project = Project::starter_kit(1);
        project.channels[0]
            .setup
            .drum_synth_state_mut()
            .unwrap()
            .params
            .choke_group = MAX_CHOKE_GROUP + 1;
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));

        let mut project = Project::default();
        project.channels[0] = mooloop_core::ProjectChannel::mono_synth(0, 1);
        project.channels[0]
            .setup
            .mono_synth_state_mut()
            .unwrap()
            .params
            .filter_cutoff = f32::NAN;
        assert!(matches!(validate_project(&project), Err(Error::InvalidDocument(_))));

        let mut project = Project::default();
        project.channels[0] = mooloop_core::ProjectChannel::mono_synth(0, 1);
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
        project.channels[0] = mooloop_core::ProjectChannel::poly_synth(0, 1);
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

    /// The ML-1 carries `#[serde(default)]` from the start, so a
    /// manifest written by a build that predates any given field still loads.
    /// Asserted by truncating the table at the filter envelope, which is the
    /// shape a project saved before that block existed would have.
    #[test]
    fn ml1_params_load_from_a_manifest_missing_later_fields() {
        let written = toml::to_string(&Ml1Params::default()).unwrap();
        let (before_filter_env, _) = written.split_once("filter_attack").unwrap();
        let loaded: Ml1Params = toml::from_str(before_filter_env).unwrap();
        assert_eq!(loaded, Ml1Params::default());
    }

    #[test]
    fn ml1_source_round_trips_in_a_song() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("ml1.mooloop");
        let mut project = Project::default();
        project.channels[0] = mooloop_core::ProjectChannel::ml1(0, 1);
        let params = &mut project.channels[0]
            .setup
            .ml1_state_mut()
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

    #[test]
    fn ml1_validation_rejects_an_out_of_range_filter_envelope() {
        let mut project = Project::default();
        project.channels[0] = mooloop_core::ProjectChannel::ml1(0, 1);
        project.channels[0]
            .setup
            .ml1_state_mut()
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
            ChannelSource::Ml1(mooloop_core::Ml1State::default()),
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
            assert_eq!(loaded.document, LoadedDocument::Generator(source));

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
        assert_eq!(summaries[0].kind, mooloop_core::DeviceKind::MonoSynth);
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
            .effects
            .push(mooloop_core::EffectSlotState::filter(
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
            project.channels[0].setup.effects.push(slot);
        }

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.document, LoadedDocument::Song(project));
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
            .effects
            .push(mooloop_core::EffectSlotState::filter(
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
}
