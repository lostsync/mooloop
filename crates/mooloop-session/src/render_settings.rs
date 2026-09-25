//! Everything the export dialog sets, as one value (MOO-180).
//!
//! `RenderSettings` is independent of the window: the dialog writes one,
//! [`RenderSettings::job`] turns one into the engine's [`RenderJob`], and
//! the settings memory, presets and per-song settings (MOO-190, MOO-191,
//! MOO-194) serialize it as it is. Every field has a default, and a table
//! missing one loads with it, so a field added later leaves what was saved
//! before readable.

use std::path::{Path, PathBuf};

use mooloop_engine::{
    ExportFormat, Mp3Bitrate, RenderJob, RenderOutput, RenderPass, RenderScope, RenderTap,
    WavEncoding,
};
use serde::{Deserialize, Serialize};

/// The name a file gets when the name field is empty and the song has never
/// been saved.
pub const UNTITLED_EXPORT_NAME: &str = "mooloop-export";

/// The longest tail the dialog offers, in seconds; the engine refuses more.
pub const MAX_TAIL_SECONDS: u32 = 30;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    pub source: RenderSource,
    pub range: RenderRange,
    pub format: FileFormat,
    pub tail: TailSettings,
    pub output: OutputSettings,
}

/// What is rendered. The master mix only, until stems (MOO-182) and direct
/// channels (MOO-183).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderSource {
    #[default]
    Master,
}

/// Which stretch of the timeline is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderRange {
    /// What the transport plays: the whole song in song mode, the current
    /// pattern in pattern mode. Exporting the song while the sequencer is in
    /// pattern mode would render something the user is not listening to.
    #[default]
    Transport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileFormat {
    Wav { depth: WavDepth },
    Mp3 { kbps: u16 },
}

impl Default for FileFormat {
    fn default() -> Self {
        Self::Wav {
            depth: WavDepth::Pcm24,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WavDepth {
    #[default]
    Pcm24,
    Float32,
}

/// The MP3 bitrates the encoder offers, in kbps.
pub const MP3_KBPS: [u16; 3] = [192, 256, 320];

impl FileFormat {
    /// The file extension this format writes, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav { .. } => "wav",
            Self::Mp3 { .. } => "mp3",
        }
    }

    /// The engine's format. A bitrate the encoder does not offer -- a hand
    /// edited settings file, say -- is the nearest one it does.
    pub fn engine(self) -> ExportFormat {
        match self {
            Self::Wav {
                depth: WavDepth::Pcm24,
            } => ExportFormat::Wav(WavEncoding::Pcm24),
            Self::Wav {
                depth: WavDepth::Float32,
            } => ExportFormat::Wav(WavEncoding::Float32),
            Self::Mp3 { kbps } => ExportFormat::Mp3(match kbps {
                0..=223 => Mp3Bitrate::Kbps192,
                224..=287 => Mp3Bitrate::Kbps256,
                _ => Mp3Bitrate::Kbps320,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TailSettings {
    /// The most tail there may be after the last bar. The render stops as
    /// soon as every device has fallen silent, so this is a cap.
    pub max_seconds: u32,
}

impl Default for TailSettings {
    fn default() -> Self {
        Self { max_seconds: 10 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    /// The folder the files go in. `None` is the default: the song's own
    /// folder, or the music folder for a song never saved.
    pub folder: Option<PathBuf>,
    /// The file name, without its extension. Empty is the song's name.
    pub name: String,
}

/// Why a job could not be built from the settings: one sentence for the
/// dialog, which stays open so it can be corrected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsProblem(pub String);

impl std::fmt::Display for SettingsProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl RenderSettings {
    /// The folder the files go in: the one chosen, else `default`. A typed
    /// `~` at its start is the home folder, as it is in a shell.
    pub fn folder(&self, default: &Path) -> PathBuf {
        match &self.output.folder {
            Some(folder) if !folder.as_os_str().is_empty() => {
                let home = std::env::var_os("HOME").map(PathBuf::from);
                match (folder.strip_prefix("~"), home) {
                    (Ok(rest), Some(home)) => home.join(rest),
                    _ => folder.clone(),
                }
            }
            _ => default.to_path_buf(),
        }
    }

    /// The file's name without its extension: the one typed, else the
    /// song's, else [`UNTITLED_EXPORT_NAME`]. An audio extension typed on
    /// the end is dropped, so `take.wav` exported as MP3 is `take.mp3`
    /// rather than `take.wav.mp3`.
    pub fn stem(&self, song_name: Option<&str>) -> Result<String, SettingsProblem> {
        let typed = self.output.name.trim();
        let typed = strip_audio_extension(typed);
        let name = if typed.is_empty() {
            song_name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(UNTITLED_EXPORT_NAME)
        } else {
            typed
        };
        if name.contains(['/', '\\']) {
            return Err(SettingsProblem(
                "A file name can't contain a slash. Choose the folder with Browse.".into(),
            ));
        }
        if name == "." || name == ".." {
            return Err(SettingsProblem(format!("\"{name}\" isn't a file name.")));
        }
        Ok(name.to_string())
    }

    /// The job these settings describe, for a song named `song_name` whose
    /// default folder is `default_folder`. The transport's scope is given
    /// separately: the range follows it until a range is chosen (MOO-181).
    pub fn job(
        &self,
        scope: RenderScope,
        song_name: Option<&str>,
        default_folder: &Path,
    ) -> Result<RenderJob, SettingsProblem> {
        let folder = self.folder(default_folder);
        if !folder.is_dir() {
            return Err(SettingsProblem(format!(
                "The folder {} doesn't exist.",
                folder.display()
            )));
        }
        let stem = self.stem(song_name)?;
        let path = folder.join(format!("{stem}.{}", self.format.extension()));
        let RenderSource::Master = self.source;
        let RenderRange::Transport = self.range;
        Ok(RenderJob {
            passes: vec![RenderPass {
                scope,
                tail_seconds: self.tail.max_seconds.min(MAX_TAIL_SECONDS) as f32,
                outputs: vec![RenderOutput {
                    path,
                    tap: RenderTap::Master,
                    format: self.format.engine(),
                }],
            }],
        })
    }
}

fn strip_audio_extension(name: &str) -> &str {
    for extension in ["wav", "mp3"] {
        let Some(split) = name.len().checked_sub(extension.len() + 1) else {
            continue;
        };
        if !name.is_char_boundary(split) {
            continue;
        }
        let (stem, tail) = name.split_at(split);
        if tail.starts_with('.') && tail[1..].eq_ignore_ascii_case(extension) {
            return stem;
        }
    }
    name
}

/// The folder an export goes to by default: the song's own, or the music
/// folder, or home, for a song never saved.
pub fn default_export_folder(song: Option<&Path>) -> PathBuf {
    song.and_then(Path::parent)
        .filter(|folder| !folder.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(crate::dialogs::music_dir)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn only_output(job: &RenderJob) -> &RenderOutput {
        assert_eq!(job.passes.len(), 1);
        assert_eq!(job.passes[0].outputs.len(), 1);
        &job.passes[0].outputs[0]
    }

    #[test]
    fn the_default_is_todays_export() {
        let settings = RenderSettings::default();
        assert_eq!(settings.format.engine(), ExportFormat::Wav(WavEncoding::Pcm24));
        assert_eq!(settings.tail.max_seconds, 10);
        assert_eq!(settings.output, OutputSettings::default());
    }

    #[test]
    fn the_job_writes_the_typed_name_in_the_chosen_folder() {
        let temp = tempdir().unwrap();
        let settings = RenderSettings {
            format: FileFormat::Mp3 { kbps: 256 },
            tail: TailSettings { max_seconds: 4 },
            output: OutputSettings {
                folder: Some(temp.path().to_path_buf()),
                name: "  take two.WAV ".into(),
            },
            ..RenderSettings::default()
        };
        let job = settings
            .job(RenderScope::Song, Some("song"), Path::new("/nowhere"))
            .unwrap();
        let output = only_output(&job);
        assert_eq!(output.path, temp.path().join("take two.mp3"));
        assert_eq!(output.format, ExportFormat::Mp3(Mp3Bitrate::Kbps256));
        assert_eq!(output.tap, RenderTap::Master);
        assert_eq!(job.passes[0].scope, RenderScope::Song);
        assert_eq!(job.passes[0].tail_seconds, 4.0);
    }

    #[test]
    fn an_empty_name_and_folder_are_the_songs() {
        let temp = tempdir().unwrap();
        let settings = RenderSettings::default();
        let job = settings
            .job(RenderScope::Pattern { index: 2 }, Some("ok then"), temp.path())
            .unwrap();
        assert_eq!(only_output(&job).path, temp.path().join("ok then.wav"));

        let untitled = settings.job(RenderScope::Song, None, temp.path()).unwrap();
        assert_eq!(
            only_output(&untitled).path,
            temp.path().join(format!("{UNTITLED_EXPORT_NAME}.wav"))
        );
    }

    #[test]
    fn a_name_with_a_slash_or_a_missing_folder_is_refused() {
        let temp = tempdir().unwrap();
        let mut settings = RenderSettings::default();
        settings.output.name = "sub/take".into();
        assert!(settings.job(RenderScope::Song, None, temp.path()).is_err());

        settings.output.name = "take".into();
        settings.output.folder = Some(temp.path().join("not here"));
        let problem = settings
            .job(RenderScope::Song, None, temp.path())
            .unwrap_err();
        assert!(problem.0.contains("doesn't exist"), "{problem}");
    }

    #[test]
    fn the_default_folder_is_the_songs_own() {
        assert_eq!(
            default_export_folder(Some(Path::new("/music/songs/ok then.mooloop"))),
            PathBuf::from("/music/songs")
        );
    }

    /// The settings are a serde value, and a table missing fields still
    /// loads: what MOO-190 saves stays readable as fields are added.
    #[test]
    fn settings_round_trip_and_a_partial_table_loads() {
        let settings = RenderSettings {
            format: FileFormat::Wav {
                depth: WavDepth::Float32,
            },
            tail: TailSettings { max_seconds: 0 },
            output: OutputSettings {
                folder: Some(PathBuf::from("/tmp/renders")),
                name: "mix".into(),
            },
            ..RenderSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(serde_json::from_str::<RenderSettings>(&json).unwrap(), settings);

        let partial: RenderSettings =
            serde_json::from_str(r#"{"format":{"kind":"mp3","kbps":320}}"#).unwrap();
        assert_eq!(partial.format, FileFormat::Mp3 { kbps: 320 });
        assert_eq!(partial.tail, TailSettings::default());
        assert_eq!(partial.output, OutputSettings::default());
    }
}
