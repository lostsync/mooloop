//! Versioned mooloop directory bundles and sample-asset handling.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use mooloop_core::{
    ChannelSetup, ChannelSource, Kit, Project, SampleReference, SamplerParams, MAX_CHANNELS,
    MAX_CHOKE_GROUP, MAX_NOTES_PER_CHANNEL_PATTERN, MAX_PATTERNS, MAX_PATTERN_STEPS,
    MAX_PLAYLIST_PLACEMENTS, MAX_PLAYLIST_TICKS, MAX_SAMPLER_VOICES, TICKS_PER_STEP,
};
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "manifest.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetMode {
    Embedded,
    Referenced,
}

impl Default for AssetMode {
    fn default() -> Self {
        Self::Embedded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Song,
    Kit,
    Channel,
}

impl DocumentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Song => "song",
            Self::Kit => "kit",
            Self::Channel => "channel",
        }
    }
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadedDocument {
    Song(Project),
    Kit(Kit),
    Channel(ChannelSetup),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadReport {
    pub document: LoadedDocument,
    pub asset_mode: AssetMode,
    pub warnings: Vec<AssetWarning>,
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Encode(toml::ser::Error),
    Invalid(String),
    UnsupportedVersion(u32),
    UnsupportedDocument(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Parse(error) => write!(f, "invalid manifest: {error}"),
            Self::Encode(error) => write!(f, "could not encode manifest: {error}"),
            Self::Invalid(message) => write!(f, "invalid document: {message}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "unsupported format version {version}")
            }
            Self::UnsupportedDocument(kind) => write!(f, "unsupported document type {kind:?}"),
        }
    }
}

impl std::error::Error for Error {}

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
    document: T,
}

#[derive(Deserialize)]
struct Header {
    format_version: u32,
    document_type: String,
}

pub fn save_song(path: &Path, project: &Project, mode: AssetMode) -> Result<SaveReport, Error> {
    validate_project(project)?;
    save_with_assets(path, DocumentKind::Song, project.clone(), mode, |project| {
        project
            .channels
            .iter_mut()
            .map(|channel| &mut channel.setup)
            .collect()
    })
}

pub fn save_kit(path: &Path, kit: &Kit, mode: AssetMode) -> Result<SaveReport, Error> {
    validate_setups(&kit.channels)?;
    save_with_assets(path, DocumentKind::Kit, kit.clone(), mode, |kit| {
        kit.channels.iter_mut().collect()
    })
}

pub fn save_channel(
    path: &Path,
    channel: &ChannelSetup,
    mode: AssetMode,
) -> Result<SaveReport, Error> {
    validate_setups(std::slice::from_ref(channel))?;
    save_with_assets(
        path,
        DocumentKind::Channel,
        channel.clone(),
        mode,
        |channel| vec![channel],
    )
}

fn save_with_assets<T, F>(
    path: &Path,
    kind: DocumentKind,
    mut document: T,
    mode: AssetMode,
    setups: F,
) -> Result<SaveReport, Error>
where
    T: Serialize,
    F: FnOnce(&mut T) -> Vec<&mut ChannelSetup>,
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
    setup: &mut ChannelSetup,
    target: &Path,
    staging: &Path,
    mode: AssetMode,
    copied: &mut HashMap<PathBuf, PathBuf>,
    warnings: &mut Vec<AssetWarning>,
) -> Result<(), Error> {
    let ChannelSource::Sampler(sampler) = &mut setup.source;
    let SampleReference::File { path, embedded } = &mut sampler.sample else {
        return Ok(());
    };

    let source = path.clone();
    let keep_owned = *embedded && source.starts_with(target);
    if mode == AssetMode::Referenced && !keep_owned {
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
            .map(sanitize_name)
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

fn sanitize_name(name: &str) -> String {
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
    let manifest_path = path.join(MANIFEST_FILE);
    let manifest = fs::read_to_string(&manifest_path)?;
    let header: Header = toml::from_str(&manifest)?;
    if header.format_version != FORMAT_VERSION {
        return Err(Error::UnsupportedVersion(header.format_version));
    }

    let (mut document, asset_mode) = match header.document_type.as_str() {
        "song" => {
            let envelope: Envelope<Project> = toml::from_str(&manifest)?;
            validate_envelope(&envelope, "song")?;
            validate_project(&envelope.document)?;
            (LoadedDocument::Song(envelope.document), envelope.asset_mode)
        }
        "kit" => {
            let envelope: Envelope<Kit> = toml::from_str(&manifest)?;
            validate_envelope(&envelope, "kit")?;
            validate_setups(&envelope.document.channels)?;
            (LoadedDocument::Kit(envelope.document), envelope.asset_mode)
        }
        "channel" => {
            let envelope: Envelope<ChannelSetup> = toml::from_str(&manifest)?;
            validate_envelope(&envelope, "channel")?;
            validate_setups(std::slice::from_ref(&envelope.document))?;
            (
                LoadedDocument::Channel(envelope.document),
                envelope.asset_mode,
            )
        }
        other => return Err(Error::UnsupportedDocument(other.into())),
    };

    let mut warnings = Vec::new();
    match &mut document {
        LoadedDocument::Song(project) => {
            for (index, channel) in project.channels.iter_mut().enumerate() {
                resolve_setup_asset(path, index, &mut channel.setup, &mut warnings)?;
                channel.recompute_next_note_id();
            }
        }
        LoadedDocument::Kit(kit) => {
            for (index, setup) in kit.channels.iter_mut().enumerate() {
                resolve_setup_asset(path, index, setup, &mut warnings)?;
            }
        }
        LoadedDocument::Channel(setup) => resolve_setup_asset(path, 0, setup, &mut warnings)?,
    }
    Ok(LoadReport {
        document,
        asset_mode,
        warnings,
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
    setup: &mut ChannelSetup,
    warnings: &mut Vec<AssetWarning>,
) -> Result<(), Error> {
    let ChannelSource::Sampler(sampler) = &mut setup.source;
    let SampleReference::File { path, embedded } = &mut sampler.sample else {
        return Ok(());
    };
    if *embedded && !safe_embedded_path(path) {
        return Err(Error::Invalid(format!(
            "channel {channel} has unsafe embedded path {}",
            path.display()
        )));
    }
    let resolved = if path.is_absolute() {
        path.clone()
    } else {
        bundle.join(&*path)
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

fn safe_embedded_path(path: &Path) -> bool {
    path.starts_with("samples")
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

pub fn validate_project(project: &Project) -> Result<(), Error> {
    if project.ppq != 96 || project.beats_per_bar != 4 {
        return Err(Error::Invalid("v1 requires PPQ 96 and 4/4 meter".into()));
    }
    if !(1..=999).contains(&project.bpm) {
        return Err(Error::Invalid("tempo must be in 1..=999 BPM".into()));
    }
    if project.channels.is_empty() || project.channels.len() > MAX_CHANNELS {
        return Err(Error::Invalid(format!(
            "channel count must be in 1..={MAX_CHANNELS}"
        )));
    }
    if project.pattern_lengths.is_empty() || project.pattern_lengths.len() > MAX_PATTERNS {
        return Err(Error::Invalid(format!(
            "pattern count must be in 1..={MAX_PATTERNS}"
        )));
    }
    if project.current_pattern as usize >= project.pattern_lengths.len() {
        return Err(Error::Invalid("current pattern is out of range".into()));
    }
    if project.selected_channel as usize >= project.channels.len() {
        return Err(Error::Invalid("selected channel is out of range".into()));
    }
    if project.playlist.len() > MAX_PLAYLIST_PLACEMENTS {
        return Err(Error::Invalid("playlist has too many placements".into()));
    }
    for (index, length) in project.pattern_lengths.iter().enumerate() {
        if !(1..=MAX_PATTERN_STEPS).contains(length) {
            return Err(Error::Invalid(format!(
                "pattern {index} length must be in 1..={MAX_PATTERN_STEPS}"
            )));
        }
    }
    for placement in &project.playlist {
        if placement.pattern as usize >= project.pattern_lengths.len()
            || placement.start_tick >= MAX_PLAYLIST_TICKS
        {
            return Err(Error::Invalid("playlist placement is out of range".into()));
        }
    }
    validate_setups(
        &project
            .channels
            .iter()
            .map(|channel| channel.setup.clone())
            .collect::<Vec<_>>(),
    )?;
    let capacity_ticks = u32::from(MAX_PATTERN_STEPS) * TICKS_PER_STEP;
    for (channel_index, channel) in project.channels.iter().enumerate() {
        if channel.notes.len() != project.pattern_lengths.len() {
            return Err(Error::Invalid(format!(
                "channel {channel_index} note bank does not match pattern count"
            )));
        }
        for (pattern_index, notes) in channel.notes.iter().enumerate() {
            if notes.len() > MAX_NOTES_PER_CHANNEL_PATTERN {
                return Err(Error::Invalid(format!(
                    "channel {channel_index} pattern {pattern_index} has too many notes"
                )));
            }
            let mut ids = HashSet::with_capacity(notes.len());
            for note in notes {
                if note.id == 0
                    || !ids.insert(note.id)
                    || note.start_tick >= capacity_ticks
                    || note.duration_ticks == 0
                    || note.note > 127
                    || !(1..=127).contains(&note.velocity)
                {
                    return Err(Error::Invalid(format!(
                        "channel {channel_index} pattern {pattern_index} has an invalid note"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_setups(setups: &[ChannelSetup]) -> Result<(), Error> {
    if setups.is_empty() || setups.len() > MAX_CHANNELS {
        return Err(Error::Invalid(format!(
            "channel count must be in 1..={MAX_CHANNELS}"
        )));
    }
    for (index, setup) in setups.iter().enumerate() {
        let channel = &setup.channel;
        if channel.name.trim().is_empty() || channel.name.len() > 128 {
            return Err(Error::Invalid(format!(
                "channel {index} has an invalid name"
            )));
        }
        if !channel.volume.is_finite()
            || !(0.0..=1.0).contains(&channel.volume)
            || !channel.pan.is_finite()
            || !(-1.0..=1.0).contains(&channel.pan)
        {
            return Err(Error::Invalid(format!(
                "channel {index} has invalid mixer values"
            )));
        }
        match &setup.source {
            ChannelSource::Sampler(sampler) => validate_sampler(index, sampler.params)?,
        }
    }
    Ok(())
}

fn validate_sampler(channel: usize, params: SamplerParams) -> Result<(), Error> {
    let normalized = [
        params.start,
        params.end,
        params.loop_start,
        params.loop_end,
        params.sustain,
        params.filter_cutoff,
        params.filter_resonance,
        params.drive,
        params.bit_reduction,
        params.rate_reduction,
    ];
    let finite = normalized.iter().all(|value| value.is_finite())
        && [
            params.tune_semitones,
            params.tune_cents,
            params.attack,
            params.decay,
            params.release,
            params.filter_env_amount,
        ]
        .iter()
        .all(|value| value.is_finite());
    let valid = finite
        && normalized.iter().all(|value| (0.0..=1.0).contains(value))
        && params.start < params.end
        && params.loop_start <= params.loop_end
        && params.root_note <= 127
        && (1..=MAX_SAMPLER_VOICES).contains(&params.polyphony)
        && params.choke_group <= MAX_CHOKE_GROUP
        && (-48.0..=48.0).contains(&params.tune_semitones)
        && (-100.0..=100.0).contains(&params.tune_cents)
        && params.attack >= 0.0
        && params.decay >= 0.0
        && params.release >= 0.0
        && (-1.0..=1.0).contains(&params.filter_env_amount);
    if !valid {
        return Err(Error::Invalid(format!(
            "channel {channel} has invalid sampler parameters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{NoteEvent, PatternPlacement};
    use tempfile::tempdir;

    #[test]
    fn song_round_trip_retains_hidden_notes_and_playlist() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.pattern_lengths.push(8);
        project.channels[0]
            .notes
            .push(vec![NoteEvent::new(7, 200, 24, 64, 91)]);
        project.channels[0].recompute_next_note_id();
        project.playlist.push(PatternPlacement::new(1, 384));
        project.current_pattern = 1;

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        let loaded = load_bundle(&bundle).unwrap();
        assert!(loaded.warnings.is_empty());
        assert_eq!(loaded.document, LoadedDocument::Song(project));
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
            channel.setup.sampler_state_mut().sample = SampleReference::File {
                path: source.clone(),
                embedded: false,
            };
        }

        save_song(&bundle, &project, AssetMode::Embedded).unwrap();
        assert_eq!(fs::read_dir(bundle.join("samples")).unwrap().count(), 1);
        let loaded = load_bundle(&bundle).unwrap();
        let LoadedDocument::Song(project) = loaded.document else {
            panic!("expected song")
        };
        let first = &project.channels[0].setup.sampler_state().sample;
        let second = &project.channels[1].setup.sampler_state().sample;
        assert_eq!(first, second);
    }

    #[test]
    fn missing_reference_loads_with_warning() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        let mut project = Project::default();
        project.channels[0].setup.sampler_state_mut().sample = SampleReference::File {
            path: temp.path().join("gone.wav"),
            embedded: false,
        };
        save_song(&bundle, &project, AssetMode::Referenced).unwrap();
        let report = load_bundle(&bundle).unwrap();
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn rejects_unsafe_embedded_path() {
        let temp = tempdir().unwrap();
        let bundle = temp.path().join("song.mooloop");
        fs::create_dir(&bundle).unwrap();
        let mut project = Project::default();
        project.channels[0].setup.sampler_state_mut().sample = SampleReference::File {
            path: PathBuf::from("../escape.wav"),
            embedded: true,
        };
        let envelope = Envelope {
            format_version: FORMAT_VERSION,
            document_type: "song".into(),
            asset_mode: AssetMode::Embedded,
            document: project,
        };
        fs::write(
            bundle.join(MANIFEST_FILE),
            toml::to_string_pretty(&envelope).unwrap(),
        )
        .unwrap();
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
}
