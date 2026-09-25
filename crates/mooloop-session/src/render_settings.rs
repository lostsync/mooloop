//! Everything the export dialog sets, as one value (MOO-180).
//!
//! `RenderSettings` is independent of the window: the dialog writes one,
//! [`RenderSettings::job`] turns one into the engine's [`RenderJob`], and
//! the settings memory, presets and per-song settings (MOO-190, MOO-191,
//! MOO-194) serialize it as it is. Every field has a default, and a table
//! missing one loads with it, so a field added later leaves what was saved
//! before readable.

use std::path::{Path, PathBuf};

use mooloop_core::file_names;
use mooloop_core::{
    ChannelId, LoopRange, TrackId, BEATS_PER_BAR, MASTER_BUS, TICKS_PER_BAR, TICKS_PER_STEP,
};
use mooloop_engine::{
    ExistingFile, ExportFormat, Mp3Bitrate, OutputChannels, RenderJob, RenderOutput, RenderPass, RenderScope, RenderTap,
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
    #[serde(deserialize_with = "tolerant_source")]
    pub source: RenderSource,
    #[serde(deserialize_with = "tolerant_range")]
    pub range: RenderRange,
    #[serde(deserialize_with = "tolerant_format")]
    pub format: FileFormat,
    /// Stereo or mono, for every format (MOO-186).
    #[serde(deserialize_with = "tolerant_channels")]
    pub channels: Channels,
    pub tail: TailSettings,
    pub output: OutputSettings,
}

/// What is rendered.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderSource {
    /// The master mix, one file.
    #[default]
    Master,
    /// Stems (MOO-182): each checked track's own output -- after its rack,
    /// fader and balance, before what it feeds -- to its own file, all in
    /// one pass. Tracks are named by their durable identity, so a saved
    /// choice survives a reorder; one the song no longer has is dropped.
    Tracks {
        tracks: Vec<TrackId>,
        /// The master mix as well, on the same pass.
        #[serde(default)]
        with_master: bool,
        /// Write no file for a stem that stayed silent throughout.
        #[serde(default)]
        skip_silent: bool,
    },
    /// Channels straight out (MOO-183): each checked channel's own output
    /// -- its source, rack, fader and pan, and nothing the mixer does after
    /// -- to its own file, all in one pass. Named by durable identity, as
    /// tracks are.
    Channels {
        channels: Vec<ChannelId>,
        #[serde(default)]
        with_master: bool,
        #[serde(default)]
        skip_silent: bool,
    },
}

/// A channel as an export sees it (MOO-183).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportChannel {
    pub id: ChannelId,
    /// Its index in the song's channels, which the engine's tap names.
    pub index: u8,
    pub name: String,
}

/// The song's tracks and channels, which a stem export names its files
/// after (MOO-182, MOO-183).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExportParts {
    pub tracks: Vec<ExportTrack>,
    pub channels: Vec<ExportChannel>,
}

/// A track as an export sees it: the song's own, at its place in the bank.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportTrack {
    pub id: TrackId,
    /// Its index in the song's tracks, which the engine's tap names.
    pub index: u8,
    pub name: String,
    /// The track it feeds.
    pub output: u8,
}

impl ExportTrack {
    /// The tracks a stem export checks until told otherwise: the ones that
    /// feed the master straight. Their stems summed are the master's input;
    /// adding a bus's feeders as well would count their audio twice.
    pub fn default_selection(tracks: &[ExportTrack]) -> Vec<TrackId> {
        tracks
            .iter()
            .filter(|track| track.output == MASTER_BUS)
            .map(|track| track.id)
            .collect()
    }

    /// The tracks in the order the card lists them, each with how deep it
    /// sits: a track feeding the master at 0, and the tracks feeding a bus
    /// straight after it, one deeper, so checking a bus and its feeders
    /// both is plain to see. A track in a routing loop, which the mixer
    /// refuses, is listed at 0 at the end rather than lost.
    pub fn tree(tracks: &[ExportTrack]) -> Vec<(&ExportTrack, usize)> {
        let mut listed = Vec::with_capacity(tracks.len());
        let mut seen = vec![false; tracks.len()];
        fn walk<'a>(
            tracks: &'a [ExportTrack],
            into: u8,
            depth: usize,
            seen: &mut [bool],
            listed: &mut Vec<(&'a ExportTrack, usize)>,
        ) {
            for (position, track) in tracks.iter().enumerate() {
                if track.output == into && !seen[position] && track.index != into {
                    seen[position] = true;
                    listed.push((track, depth));
                    walk(tracks, track.index, depth + 1, seen, listed);
                }
            }
        }
        walk(tracks, MASTER_BUS, 0, &mut seen, &mut listed);
        for (position, track) in tracks.iter().enumerate() {
            if !seen[position] {
                listed.push((track, 0));
            }
        }
        listed
    }
}

/// Which stretch of the timeline is rendered (MOO-181).
///
/// Saved settings (MOO-190, MOO-194) must load whatever becomes of this
/// enum, so it loads tolerantly: a variant this build does not know, or one
/// missing its fields, is [`RenderRange::Song`], and a custom range that no
/// longer fits the song falls back to it too ([`RenderRange::loaded`]).
/// A custom range is held in ticks rather than as the text typed, so a
/// change of tempo cannot move it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderRange {
    /// What the transport plays: the whole song in song mode, the current
    /// pattern in pattern mode. Exporting the song while the sequencer is in
    /// pattern mode would render something the user is not listening to.
    #[default]
    Transport,
    /// The whole song, from the top to its last bar.
    Song,
    /// The loop selection's points, whether or not looping is switched on:
    /// the points are kept either way.
    Loop,
    /// Song ticks from `start_tick` up to, not including, `end_tick`.
    Custom { start_tick: u32, end_tick: u32 },
    /// The current pattern, one pass.
    Pattern,
}

/// What a range is resolved against: the song as it stands when the export
/// is confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeline {
    /// What the transport plays ([`RenderRange::Transport`]).
    pub transport: RenderScope,
    /// The current pattern, and its length in ticks.
    pub pattern: usize,
    pub pattern_ticks: u32,
    pub loop_range: LoopRange,
    /// The song's length in ticks, a whole number of bars.
    pub song_ticks: u32,
}

impl Timeline {
    /// The loop selection's points, cut to the song as the transport cuts
    /// them, or `None` when that leaves nothing. Looping need not be on.
    pub fn loop_span(&self) -> Option<(u32, u32)> {
        LoopRange {
            enabled: true,
            ..self.loop_range
        }
        .active(self.song_ticks)
    }
}

impl RenderRange {
    /// The engine's scope for this range, or why it cannot be rendered: an
    /// empty loop selection, or a custom range that ends before it starts or
    /// reaches past the song. Said on the card, which stays open.
    pub fn scope(self, timeline: &Timeline) -> Result<RenderScope, SettingsProblem> {
        match self {
            Self::Transport => Ok(timeline.transport),
            Self::Song => Ok(RenderScope::Song),
            Self::Pattern => Ok(RenderScope::Pattern {
                index: timeline.pattern,
            }),
            Self::Loop => timeline
                .loop_span()
                .map(|(start_tick, end_tick)| RenderScope::Range {
                    start_tick,
                    end_tick,
                })
                .ok_or_else(|| SettingsProblem("The song has no loop selection.".into())),
            Self::Custom {
                start_tick,
                end_tick,
            } => {
                if end_tick <= start_tick {
                    return Err(SettingsProblem("The range must end after it starts.".into()));
                }
                if end_tick > timeline.song_ticks {
                    return Err(SettingsProblem(format!(
                        "The song ends at {}.",
                        format_bar_beat(timeline.song_ticks)
                    )));
                }
                Ok(RenderScope::Range {
                    start_tick,
                    end_tick,
                })
            }
        }
    }

    /// The range as saved settings should open it on this song: a custom
    /// range the song no longer holds is the whole song, without a word.
    pub fn loaded(self, timeline: &Timeline) -> Self {
        match self {
            Self::Custom { .. } if self.scope(timeline).is_err() => Self::Song,
            other => other,
        }
    }

    /// The ticks this range covers, for the card to show: song ticks, or
    /// the pattern's own for a pattern. `None` when it covers nothing.
    pub fn span(self, timeline: &Timeline) -> Option<(u32, u32)> {
        match self.scope(timeline).ok()? {
            RenderScope::Song => Some((0, timeline.song_ticks)),
            RenderScope::Pattern { .. } => Some((0, timeline.pattern_ticks)),
            RenderScope::Range {
                start_tick,
                end_tick,
            } => Some((start_tick, end_tick)),
        }
    }
}

const TICKS_PER_BEAT: u32 = TICKS_PER_BAR / BEATS_PER_BAR;
const SIXTEENTHS_PER_BEAT: u32 = TICKS_PER_BEAT / TICKS_PER_STEP;

/// A position typed as `bar.beat` or `bar.beat.sixteenth`, each counted
/// from 1 as the ruler counts them, in ticks from the top of the song:
/// `3.1` is the downbeat of bar 3, and `3` is too. A beat or sixteenth
/// outside its bar or beat is refused rather than carried.
pub fn parse_bar_beat(text: &str) -> Option<u32> {
    let mut parts = text.trim().split('.');
    let mut next = |most: u32| -> Option<Option<u32>> {
        match parts.next() {
            None => Some(None),
            Some(part) => {
                let value: u32 = part.trim().parse().ok()?;
                let index = value.checked_sub(1).filter(|index| *index < most)?;
                Some(Some(index))
            }
        }
    };
    let bar = next(u32::MAX / TICKS_PER_BAR)??;
    let beat = next(BEATS_PER_BAR)?.unwrap_or(0);
    let sixteenth = next(SIXTEENTHS_PER_BEAT)?.unwrap_or(0);
    if parts.next().is_some() {
        return None;
    }
    Some(bar * TICKS_PER_BAR + beat * TICKS_PER_BEAT + sixteenth * TICKS_PER_STEP)
}

/// A song position as [`parse_bar_beat`] reads it: `bar.beat`, with the
/// sixteenth only when it is not the first. A position between sixteenths
/// shows the one before it.
pub fn format_bar_beat(tick: u32) -> String {
    let bar = tick / TICKS_PER_BAR + 1;
    let within = tick % TICKS_PER_BAR;
    let beat = within / TICKS_PER_BEAT + 1;
    let sixteenth = within % TICKS_PER_BEAT / TICKS_PER_STEP + 1;
    if sixteenth == 1 {
        format!("{bar}.{beat}")
    } else {
        format!("{bar}.{beat}.{sixteenth}")
    }
}

/// Reads a source from saved settings: anything unknown is the master mix.
fn tolerant_source<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<RenderSource, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Tolerant {
        Known(RenderSource),
        Unknown(serde::de::IgnoredAny),
    }
    Ok(match Tolerant::deserialize(deserializer)? {
        Tolerant::Known(source) => source,
        Tolerant::Unknown(_) => RenderSource::Master,
    })
}

/// Reads a format from saved settings, and never fails the settings over
/// it: one this build does not know is the default, 24-bit WAV.
fn tolerant_format<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<FileFormat, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Tolerant {
        Known(FileFormat),
        Unknown(serde::de::IgnoredAny),
    }
    Ok(match Tolerant::deserialize(deserializer)? {
        Tolerant::Known(format) => format,
        Tolerant::Unknown(_) => FileFormat::default(),
    })
}

/// Reads the channel count from saved settings: anything unknown is stereo.
fn tolerant_channels<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Channels, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Tolerant {
        Known(Channels),
        Unknown(serde::de::IgnoredAny),
    }
    Ok(match Tolerant::deserialize(deserializer)? {
        Tolerant::Known(channels) => channels,
        Tolerant::Unknown(_) => Channels::Stereo,
    })
}

/// Reads a range from saved settings, and never fails the settings over it:
/// anything that is not a range this build knows is the whole song.
fn tolerant_range<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<RenderRange, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Tolerant {
        Known(RenderRange),
        Unknown(serde::de::IgnoredAny),
    }
    Ok(match Tolerant::deserialize(deserializer)? {
        Tolerant::Known(range) => range,
        Tolerant::Unknown(_) => RenderRange::Song,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileFormat {
    Wav {
        depth: WavDepth,
        /// TPDF dither before rounding (MOO-186). `None` is the depth's
        /// default ([`WavDepth::dithers_by_default`]); a float file takes
        /// none whatever this says.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dither: Option<bool>,
    },
    Mp3 { kbps: u16 },
}

impl Default for FileFormat {
    fn default() -> Self {
        Self::Wav {
            depth: WavDepth::Pcm24,
            dither: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WavDepth {
    /// 16-bit PCM (MOO-186).
    Pcm16,
    #[default]
    Pcm24,
    Float32,
}

impl WavDepth {
    /// Dither is on by default at 16 bits, where rounding is audible on a
    /// quiet fade, and off at 24, where it sits under any playback chain's
    /// own noise (MOO-186).
    pub fn dithers_by_default(self) -> bool {
        self == Self::Pcm16
    }
}

/// How many channels the files hold (MOO-186).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channels {
    #[default]
    Stereo,
    /// `(L + R) / 2`; see `docs/GAIN_STRUCTURE.md`, "Mono files".
    Mono,
}

impl Channels {
    pub fn engine(self) -> OutputChannels {
        match self {
            Self::Stereo => OutputChannels::Stereo,
            Self::Mono => OutputChannels::Mono,
        }
    }
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

    /// Whether the file is dithered: the choice made, else the depth's
    /// default. Never for float or MP3.
    pub fn dither(self) -> bool {
        match self {
            Self::Wav {
                depth: WavDepth::Float32,
                ..
            }
            | Self::Mp3 { .. } => false,
            Self::Wav { depth, dither } => dither.unwrap_or(depth.dithers_by_default()),
        }
    }

    /// The engine's format. A bitrate the encoder does not offer -- a hand
    /// edited settings file, say -- is the nearest one it does.
    pub fn engine(self) -> ExportFormat {
        match self {
            Self::Wav {
                depth: WavDepth::Pcm16,
                ..
            } => ExportFormat::Wav(WavEncoding::Pcm16),
            Self::Wav {
                depth: WavDepth::Pcm24,
                ..
            } => ExportFormat::Wav(WavEncoding::Pcm24),
            Self::Wav {
                depth: WavDepth::Float32,
                ..
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
    /// Replace a file already there, after asking. Off, the default, is the
    /// card's "Don't overwrite: number it": a name that is taken is written
    /// as `song-001.wav`, `song-002.wav`, and nothing is ever replaced
    /// (MOO-188).
    pub replace_existing: bool,
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

    /// The job these settings describe, rendering `scope`, for a song named
    /// `song_name` whose default folder is `default_folder` and whose tracks
    /// and channels are `parts`. The scope is the range resolved against the
    /// song ([`RenderRange::scope`]), which the caller does because only it
    /// has the song.
    pub fn job(
        &self,
        scope: RenderScope,
        song_name: Option<&str>,
        default_folder: &Path,
        parts: &ExportParts,
    ) -> Result<RenderJob, SettingsProblem> {
        let tracks = parts.tracks.as_slice();
        let folder = self.folder(default_folder);
        if !folder.is_dir() {
            return Err(SettingsProblem(format!(
                "The folder {} doesn't exist.",
                folder.display()
            )));
        }
        let stem = self.stem(song_name)?;
        let extension = self.format.extension();
        let output = |name: &str, tap: RenderTap, skip_silent: bool| RenderOutput {
            channels: self.channels.engine(),
            dither: self.format.dither(),
            skip_silent,
            ..RenderOutput::new(
                folder.join(format!("{name}.{extension}")),
                tap,
                self.format.engine(),
            )
        };
        let mut outputs = match &self.source {
            RenderSource::Master => vec![output(&stem, RenderTap::Master, false)],
            RenderSource::Tracks {
                tracks: checked,
                with_master,
                skip_silent,
            } => {
                let mut outputs = Vec::new();
                if *with_master {
                    outputs.push(output(&stem, RenderTap::Master, false));
                }
                for track in tracks.iter().filter(|track| checked.contains(&track.id)) {
                    outputs.push(output(
                        &format!("{stem}-{}", stem_name(track, tracks)),
                        RenderTap::Track(track.index),
                        *skip_silent,
                    ));
                }
                if outputs.is_empty() {
                    return Err(SettingsProblem("Check at least one track.".into()));
                }
                outputs
            }
            RenderSource::Channels {
                channels: checked,
                with_master,
                skip_silent,
            } => {
                let mut outputs = Vec::new();
                if *with_master {
                    outputs.push(output(&stem, RenderTap::Master, false));
                }
                let channels = parts.channels.as_slice();
                for channel in channels.iter().filter(|channel| checked.contains(&channel.id)) {
                    outputs.push(output(
                        &format!("{stem}-{}", channel_file_name(channel, channels)),
                        RenderTap::Channel(channel.index),
                        *skip_silent,
                    ));
                }
                if outputs.is_empty() {
                    return Err(SettingsProblem("Check at least one channel.".into()));
                }
                outputs
            }
        };
        // Every file of the job, stems included, takes the one number
        // (MOO-188).
        if !self.output.replace_existing {
            number_outputs(&mut outputs, file_names::is_taken);
        }
        Ok(RenderJob {
            passes: vec![RenderPass {
                scope,
                tail_seconds: self.tail.max_seconds.min(MAX_TAIL_SECONDS) as f32,
                outputs,
            }],
        })
    }
}

/// Number a job's outputs so that none of them replaces a file `taken`
/// says is there (MOO-188). The paths come in as typed and go out with
/// **one** number for the whole job, the lowest at which every one of them
/// is free, so a set of files keeps one number even when only one of them
/// clashed: nothing taken is the names as typed, then `-001`, `-002`, ...
/// Each output is marked to be placed without replacing, and to take the
/// next free number of its own name if a file appears at its path while it
/// renders ([`ExistingFile::Number`]).
pub fn number_outputs(outputs: &mut [RenderOutput], taken: impl Fn(&Path) -> bool) {
    let bases: Vec<PathBuf> = outputs.iter().map(|output| output.path.clone()).collect();
    let number = file_names::free_number(&bases, taken);
    for (output, base) in outputs.iter_mut().zip(bases) {
        output.path = file_names::numbered(&base, number);
        output.existing = ExistingFile::Number { base };
    }
}

/// A track's part of its stem's file name: its own name with anything a
/// file name cannot hold replaced, or `track N` for one with none, and its
/// number added when another track has the same name, so no two stems of
/// one export share a file.
fn stem_name(track: &ExportTrack, tracks: &[ExportTrack]) -> String {
    part_name(
        &track.name,
        "track",
        u32::from(track.index),
        tracks.iter().map(|other| other.name.as_str()),
    )
}

/// [`stem_name`] for a channel: its number is counted from 1, as the rack
/// shows it.
fn channel_file_name(channel: &ExportChannel, channels: &[ExportChannel]) -> String {
    part_name(
        &channel.name,
        "channel",
        u32::from(channel.index) + 1,
        channels.iter().map(|other| other.name.as_str()),
    )
}

/// A part's name as a file name can hold it, `<kind> <number>` for one with
/// none, and its number added when another part of the same kind shares it.
fn part_name<'a>(
    name: &str,
    kind: &str,
    number: u32,
    all: impl Iterator<Item = &'a str>,
) -> String {
    let clean = |name: &str| -> String {
        let replaced: String = name
            .chars()
            .map(|c| if matches!(c, '/' | '\\' | ':' | '\0') { '-' } else { c })
            .collect();
        replaced
            .trim_matches(|c: char| c == '-' || c.is_whitespace())
            .to_string()
    };
    let name = clean(name);
    if name.is_empty() {
        return format!("{kind} {number}");
    }
    let shared = all
        .filter(|other| clean(other).eq_ignore_ascii_case(&name))
        .count()
        > 1;
    if shared {
        format!("{name} {number}")
    } else {
        name
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
                ..OutputSettings::default()
            },
            ..RenderSettings::default()
        };
        let job = settings
            .job(RenderScope::Song, Some("song"), Path::new("/nowhere"), &ExportParts::default())
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
            .job(RenderScope::Pattern { index: 2 }, Some("ok then"), temp.path(), &ExportParts::default())
            .unwrap();
        assert_eq!(only_output(&job).path, temp.path().join("ok then.wav"));

        let untitled = settings.job(RenderScope::Song, None, temp.path(), &ExportParts::default()).unwrap();
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
        assert!(settings.job(RenderScope::Song, None, temp.path(), &ExportParts::default()).is_err());

        settings.output.name = "take".into();
        settings.output.folder = Some(temp.path().join("not here"));
        let problem = settings
            .job(RenderScope::Song, None, temp.path(), &ExportParts::default())
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

    fn timeline() -> Timeline {
        Timeline {
            transport: RenderScope::Pattern { index: 1 },
            pattern: 1,
            pattern_ticks: TICKS_PER_BAR,
            loop_range: LoopRange {
                start_tick: 2 * TICKS_PER_BAR,
                end_tick: 4 * TICKS_PER_BAR,
                enabled: false,
            },
            song_ticks: 8 * TICKS_PER_BAR,
        }
    }

    /// **Each range resolves to the stretch it names** (MOO-181): the loop
    /// selection to exactly its points, switched on or not.
    #[test]
    fn each_range_resolves_to_the_stretch_it_names() {
        let timeline = timeline();
        assert_eq!(RenderRange::Transport.scope(&timeline), Ok(timeline.transport));
        assert_eq!(RenderRange::Song.scope(&timeline), Ok(RenderScope::Song));
        assert_eq!(
            RenderRange::Pattern.scope(&timeline),
            Ok(RenderScope::Pattern { index: 1 })
        );
        assert_eq!(
            RenderRange::Loop.scope(&timeline),
            Ok(RenderScope::Range {
                start_tick: 2 * TICKS_PER_BAR,
                end_tick: 4 * TICKS_PER_BAR,
            })
        );
        let custom = RenderRange::Custom {
            start_tick: TICKS_PER_BAR + 96,
            end_tick: 3 * TICKS_PER_BAR,
        };
        assert_eq!(
            custom.scope(&timeline),
            Ok(RenderScope::Range {
                start_tick: TICKS_PER_BAR + 96,
                end_tick: 3 * TICKS_PER_BAR,
            })
        );
        assert_eq!(custom.span(&timeline), Some((TICKS_PER_BAR + 96, 3 * TICKS_PER_BAR)));
        assert_eq!(RenderRange::Pattern.span(&timeline), Some((0, TICKS_PER_BAR)));
    }

    /// **A range that cannot be rendered is refused on the card**
    /// (MOO-181): a custom end at or before its start, one past the song,
    /// and a loop selection with nothing in it.
    #[test]
    fn an_empty_or_backwards_range_is_refused() {
        let mut timeline = timeline();
        for (start_tick, end_tick) in [(TICKS_PER_BAR, TICKS_PER_BAR), (TICKS_PER_BAR, 0)] {
            let problem = RenderRange::Custom {
                start_tick,
                end_tick,
            }
            .scope(&timeline)
            .unwrap_err();
            assert!(problem.0.contains("end after it starts"), "{problem}");
        }
        let past = RenderRange::Custom {
            start_tick: 0,
            end_tick: 9 * TICKS_PER_BAR,
        };
        assert_eq!(
            past.scope(&timeline).unwrap_err().0,
            "The song ends at 9.1."
        );
        timeline.loop_range = LoopRange::default();
        assert!(RenderRange::Loop.scope(&timeline).is_err());
        assert_eq!(timeline.loop_span(), None);
        // A loop reaching past the song is cut to it, as the transport cuts it.
        timeline.loop_range.start_tick = 6 * TICKS_PER_BAR;
        timeline.loop_range.end_tick = 12 * TICKS_PER_BAR;
        assert_eq!(timeline.loop_span(), Some((6 * TICKS_PER_BAR, 8 * TICKS_PER_BAR)));
    }

    #[test]
    fn bar_beat_reads_as_the_ruler_counts() {
        let beat = TICKS_PER_BAR / BEATS_PER_BAR;
        assert_eq!(parse_bar_beat("1.1"), Some(0));
        assert_eq!(parse_bar_beat(" 3 "), Some(2 * TICKS_PER_BAR));
        assert_eq!(parse_bar_beat("3.2"), Some(2 * TICKS_PER_BAR + beat));
        assert_eq!(parse_bar_beat("3.2.3"), Some(2 * TICKS_PER_BAR + beat + 2 * TICKS_PER_STEP));
        for refused in ["", "0", "3.5", "3.0", "3.1.5", "3.1.1.1", "x", "-1", "3."] {
            assert_eq!(parse_bar_beat(refused), None, "{refused:?}");
        }
        for tick in [0, beat, TICKS_PER_BAR * 7 + 3 * beat + TICKS_PER_STEP] {
            assert_eq!(parse_bar_beat(&format_bar_beat(tick)), Some(tick));
        }
        assert_eq!(format_bar_beat(2 * TICKS_PER_BAR + beat), "3.2");
        assert_eq!(format_bar_beat(TICKS_PER_STEP), "1.1.2");
    }

    /// **Saved settings load whatever the range says** (MOO-181, for
    /// MOO-190 and MOO-194): a range this build does not know, or one
    /// missing its fields, is the whole song and the rest of the table
    /// still loads; a custom range the song no longer holds opens as the
    /// whole song; and a custom range is saved in ticks.
    #[test]
    fn a_range_this_build_cannot_use_loads_as_the_whole_song() {
        for json in [
            r#"{"range":{"kind":"marker","name":"chorus"},"tail":{"max_seconds":3}}"#,
            r#"{"range":{"kind":"custom","start_tick":5},"tail":{"max_seconds":3}}"#,
            r#"{"range":"loop","tail":{"max_seconds":3}}"#,
            r#"{"range":7,"tail":{"max_seconds":3}}"#,
        ] {
            let settings: RenderSettings = serde_json::from_str(json).unwrap();
            assert_eq!(settings.range, RenderRange::Song, "{json}");
            assert_eq!(settings.tail.max_seconds, 3, "{json}");
        }
        let custom = RenderRange::Custom {
            start_tick: 4 * TICKS_PER_BAR,
            end_tick: 10 * TICKS_PER_BAR,
        };
        let json = serde_json::to_string(&RenderSettings {
            range: custom,
            ..RenderSettings::default()
        })
        .unwrap();
        assert!(
            json.contains(r#""range":{"kind":"custom","start_tick":1536,"end_tick":3840}"#),
            "{json}"
        );
        let settings: RenderSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(settings.range, custom);
        assert_eq!(settings.range.loaded(&timeline()), RenderRange::Song);
        let fits = RenderRange::Custom {
            start_tick: 0,
            end_tick: TICKS_PER_BAR,
        };
        assert_eq!(fits.loaded(&timeline()), fits);
    }

    /// **16-bit is dithered by default, 24-bit is not, float never is, and
    /// mono reaches the job** (MOO-186). A table saved before either field
    /// existed loads as stereo with the depth's default.
    #[test]
    fn depth_dither_and_channels_reach_the_job() {
        let wav = |depth, dither| FileFormat::Wav { depth, dither };
        assert!(wav(WavDepth::Pcm16, None).dither());
        assert!(!wav(WavDepth::Pcm24, None).dither());
        assert!(wav(WavDepth::Pcm24, Some(true)).dither());
        assert!(!wav(WavDepth::Pcm16, Some(false)).dither());
        assert!(!wav(WavDepth::Float32, Some(true)).dither());
        assert!(!FileFormat::Mp3 { kbps: 320 }.dither());
        assert_eq!(
            wav(WavDepth::Pcm16, None).engine(),
            ExportFormat::Wav(WavEncoding::Pcm16)
        );

        let temp = tempdir().unwrap();
        let settings = RenderSettings {
            format: wav(WavDepth::Pcm16, None),
            channels: Channels::Mono,
            ..RenderSettings::default()
        };
        let job = settings.job(RenderScope::Song, None, temp.path(), &ExportParts::default()).unwrap();
        let output = only_output(&job);
        assert_eq!(output.channels, OutputChannels::Mono);
        assert!(output.dither);

        let old: RenderSettings =
            serde_json::from_str(r#"{"format":{"kind":"wav","depth":"pcm24"}}"#).unwrap();
        assert_eq!(old.format, FileFormat::default());
        assert_eq!(old.channels, Channels::Stereo);
        let unknown: RenderSettings = serde_json::from_str(
            r#"{"format":{"kind":"flac","level":8},"channels":"surround","tail":{"max_seconds":2}}"#,
        )
        .unwrap();
        assert_eq!(unknown.format, FileFormat::default());
        assert_eq!(unknown.channels, Channels::Stereo);
        assert_eq!(unknown.tail.max_seconds, 2);
    }

    /// MOO-188's resolver cases: the job's outputs share the lowest number at
    /// which none of them is taken, and each is marked never to replace.
    #[test]
    fn a_taken_name_is_numbered_and_a_job_shares_one_number() {
        let wav = ExportFormat::Wav(WavEncoding::Pcm24);
        let resolve = |names: &[&str], taken: &[&str]| -> Vec<PathBuf> {
            let mut outputs: Vec<RenderOutput> = names
                .iter()
                .map(|name| RenderOutput::new(PathBuf::from(name), RenderTap::Master, wav))
                .collect();
            number_outputs(&mut outputs, |path| {
                taken.iter().any(|name| path == Path::new(name))
            });
            for (output, name) in outputs.iter().zip(names) {
                assert_eq!(
                    output.existing,
                    ExistingFile::Number {
                        base: PathBuf::from(name)
                    }
                );
            }
            outputs.into_iter().map(|output| output.path).collect()
        };
        assert_eq!(resolve(&["song.mp3"], &[]), [PathBuf::from("song.mp3")], "nothing there");
        assert_eq!(resolve(&["song.mp3"], &["song.mp3"]), [PathBuf::from("song-001.mp3")]);
        assert_eq!(
            resolve(&["song.mp3"], &["song.mp3", "song-001.mp3", "song-002.mp3"]),
            [PathBuf::from("song-003.mp3")]
        );
        assert_eq!(
            resolve(&["drums.wav", "bass.wav", "keys.wav"], &["bass.wav"]),
            ["drums-001.wav", "bass-001.wav", "keys-001.wav"].map(PathBuf::from),
            "one stem of three taken numbers all three"
        );
    }

    /// The job numbers by default and replaces only when asked to, and a
    /// numbered job has nothing for the dialog to ask about.
    #[test]
    fn the_job_numbers_unless_told_to_replace() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("mix.wav"), b"old").unwrap();
        let mut settings = RenderSettings {
            output: OutputSettings {
                folder: Some(temp.path().to_path_buf()),
                name: "mix".into(),
                ..OutputSettings::default()
            },
            ..RenderSettings::default()
        };
        let job = settings.job(RenderScope::Song, None, Path::new("/nowhere"), &ExportParts::default()).unwrap();
        assert_eq!(only_output(&job).path, temp.path().join("mix-001.wav"));
        assert!(job.existing_targets().is_empty());

        settings.output.replace_existing = true;
        let job = settings.job(RenderScope::Song, None, Path::new("/nowhere"), &ExportParts::default()).unwrap();
        let output = only_output(&job);
        assert_eq!(output.path, temp.path().join("mix.wav"));
        assert_eq!(output.existing, ExistingFile::Replace);
        assert_eq!(job.existing_targets(), [temp.path().join("mix.wav")]);
    }

    fn parts(tracks: Vec<ExportTrack>) -> ExportParts {
        ExportParts {
            tracks,
            channels: Vec::new(),
        }
    }

    /// **Channels straight out: one file a checked channel, named after the
    /// song and the channel, on one pass** (MOO-183).
    #[test]
    fn channel_stems_write_one_file_a_checked_channel() {
        let temp = tempdir().unwrap();
        let channel = |index: u8, name: &str| ExportChannel {
            id: ChannelId(u32::from(index) + 10),
            index,
            name: name.into(),
        };
        let parts = ExportParts {
            tracks: Vec::new(),
            channels: vec![channel(0, "Kick"), channel(1, "Hat"), channel(2, ""), channel(3, "hat")],
        };
        let settings = RenderSettings {
            source: RenderSource::Channels {
                channels: [10, 11, 12, 13].map(ChannelId).to_vec(),
                with_master: false,
                skip_silent: false,
            },
            ..RenderSettings::default()
        };
        let job = settings.job(RenderScope::Song, Some("mix"), temp.path(), &parts).unwrap();
        assert_eq!(job.passes.len(), 1);
        let named: Vec<_> = job.passes[0]
            .outputs
            .iter()
            .map(|output| {
                (
                    output.path.file_name().unwrap().to_string_lossy().into_owned(),
                    output.tap,
                )
            })
            .collect();
        assert_eq!(
            named,
            [
                ("mix-Kick.wav".to_string(), RenderTap::Channel(0)),
                ("mix-Hat 2.wav".to_string(), RenderTap::Channel(1)),
                ("mix-channel 3.wav".to_string(), RenderTap::Channel(2)),
                ("mix-hat 4.wav".to_string(), RenderTap::Channel(3)),
            ]
        );
        let none = RenderSettings {
            source: RenderSource::Channels {
                channels: Vec::new(),
                with_master: false,
                skip_silent: false,
            },
            ..RenderSettings::default()
        };
        assert_eq!(
            none.job(RenderScope::Song, None, temp.path(), &parts).unwrap_err().0,
            "Check at least one channel."
        );
    }

    fn bank() -> Vec<ExportTrack> {
        let track = |index: u8, name: &str, output: u8| ExportTrack {
            id: TrackId(u32::from(index) + 100),
            index,
            name: name.into(),
            output,
        };
        vec![
            track(1, "Drums", MASTER_BUS),
            track(2, "Kick", 1),
            track(3, "Bass", MASTER_BUS),
            track(4, "Verb", MASTER_BUS),
            track(5, "bass", MASTER_BUS),
            track(6, " / ", MASTER_BUS),
        ]
    }

    /// **Stems: one file a checked track, named after the song and the
    /// track, all on one pass, the master too when asked** (MOO-182).
    #[test]
    fn stems_write_one_file_a_checked_track_on_one_pass() {
        let temp = tempdir().unwrap();
        let tracks = bank();
        let settings = RenderSettings {
            source: RenderSource::Tracks {
                tracks: vec![TrackId(101), TrackId(103), TrackId(105), TrackId(106), TrackId(999)],
                with_master: true,
                skip_silent: true,
            },
            format: FileFormat::Wav {
                depth: WavDepth::Float32,
                dither: None,
            },
            ..RenderSettings::default()
        };
        let job = settings
            .job(RenderScope::Song, Some("ok then"), temp.path(), &parts(tracks.clone()))
            .unwrap();
        assert_eq!(job.passes.len(), 1, "stems are one pass");
        let outputs = &job.passes[0].outputs;
        let named: Vec<_> = outputs
            .iter()
            .map(|output| {
                (
                    output.path.file_name().unwrap().to_string_lossy().into_owned(),
                    output.tap,
                    output.skip_silent,
                )
            })
            .collect();
        assert_eq!(
            named,
            [
                ("ok then.wav".to_string(), RenderTap::Master, false),
                ("ok then-Drums.wav".to_string(), RenderTap::Track(1), true),
                ("ok then-Bass 3.wav".to_string(), RenderTap::Track(3), true),
                ("ok then-bass 5.wav".to_string(), RenderTap::Track(5), true),
                ("ok then-track 6.wav".to_string(), RenderTap::Track(6), true),
            ]
        );

        let none = RenderSettings {
            source: RenderSource::Tracks {
                tracks: vec![TrackId(999)],
                with_master: false,
                skip_silent: false,
            },
            ..RenderSettings::default()
        };
        let problem = none
            .job(RenderScope::Song, None, temp.path(), &parts(tracks))
            .unwrap_err();
        assert_eq!(problem.0, "Check at least one track.");
    }

    /// The default selection is the tracks feeding the master, and the card
    /// lists a bus's feeders under it, one deeper.
    #[test]
    fn the_default_stems_are_the_tracks_feeding_the_master() {
        let tracks = bank();
        assert_eq!(
            ExportTrack::default_selection(&tracks),
            [101, 103, 104, 105, 106].map(TrackId)
        );
        let tree: Vec<_> = ExportTrack::tree(&tracks)
            .into_iter()
            .map(|(track, depth)| (track.index, depth))
            .collect();
        assert_eq!(tree, [(1, 0), (2, 1), (3, 0), (4, 0), (5, 0), (6, 0)]);
        let saved: RenderSettings = serde_json::from_str(
            r#"{"source":{"kind":"tracks","tracks":[101,103]},"tail":{"max_seconds":1}}"#,
        )
        .unwrap();
        assert_eq!(
            saved.source,
            RenderSource::Tracks {
                tracks: vec![TrackId(101), TrackId(103)],
                with_master: false,
                skip_silent: false,
            }
        );
        let unknown: RenderSettings =
            serde_json::from_str(r#"{"source":{"kind":"busses"},"tail":{"max_seconds":1}}"#).unwrap();
        assert_eq!(unknown.source, RenderSource::Master);
        assert_eq!(unknown.tail.max_seconds, 1);
    }

    /// The settings are a serde value, and a table missing fields still
    /// loads: what MOO-190 saves stays readable as fields are added.
    #[test]
    fn settings_round_trip_and_a_partial_table_loads() {
        let settings = RenderSettings {
            range: RenderRange::Loop,
            format: FileFormat::Wav {
                depth: WavDepth::Float32,
                dither: None,
            },
            tail: TailSettings { max_seconds: 0 },
            output: OutputSettings {
                folder: Some(PathBuf::from("/tmp/renders")),
                name: "mix".into(),
                ..OutputSettings::default()
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
