//! Opening, saving, and failing to do either.
//!
//! A document that cannot be saved is still a document the user must not
//! lose, so the failure paths here carry as much as the success ones.

use crate::audio_file;
use crate::session::{PresetSaveTarget, Session};
use mooloop_core::{
    log_error, log_warn, ChannelSetup, EffectSlotState, Project, SampleReference,
};
use mooloop_dsp::SampleData;
use mooloop_engine::{
    ExportError, ExportProgress, OfflineRenderer, RenderJob, RenderScope, RenderedFile,
};
use crate::render_settings::{
    default_export_folder, ExportTrack, RenderSettings, SettingsProblem, Timeline,
};
use mooloop_project::{AssetMode, AssetWarning, Issue, LoadReport, LoadedDocument, SaveReport};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ResolvedDocument {
    pub report: LoadReport,
    pub samples: Vec<Option<Arc<SampleData>>>,
    /// Every key-zone file the document names, decoded here on the worker
    /// (MOO-14), for [`Session::admit_zone_audio`] before the install. A
    /// file that failed is not here, and is one of `report.warnings`.
    pub zone_audio: Vec<(PathBuf, Arc<SampleData>)>,
}

/// A failure the user has to be told about in full. `message` is the plain
/// language they act on; `report` is the codes and counts they copy into a bug
/// report, empty when the error has nothing more to say than `message` does.
/// Kept together so no failure path can show one and drop the other.
pub struct DocumentProblem {
    pub message: String,
    pub report: String,
}

impl From<mooloop_project::Error> for DocumentProblem {
    fn from(error: mooloop_project::Error) -> Self {
        Self {
            report: error.report().unwrap_or_default(),
            message: error.to_string(),
        }
    }
}

impl From<String> for DocumentProblem {
    fn from(message: String) -> Self {
        Self {
            message,
            report: String::new(),
        }
    }
}

impl DocumentProblem {
    /// Everything the problem knows, flattened onto one line for the log.
    /// Both halves, because the plain-language message and the codes are the
    /// two things a report needs and a log entry that carries one without the
    /// other is the situation this whole pass exists to remove.
    pub fn one_line(&self) -> String {
        let joined = if self.report.is_empty() {
            self.message.clone()
        } else {
            format!("{} | {}", self.message, self.report)
        };
        joined.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// Runs a document operation off the UI thread and **always** reports back.
///
/// The UI marks itself `document-busy` when an operation starts and clears it
/// only when a [`DocumentResult`] arrives (MOO-92). A worker that panicked --
/// the open path decodes arbitrary audio -- used to send nothing, and the File
/// menu stayed disabled for the rest of the session (MOO-103). Here a panic
/// becomes `Failed { action, .. }`, and so does a thread that cannot be
/// started, so every operation that began ends with exactly one result.
///
/// `action` completes "Could not ...", as in [`DocumentResult::Failed`].
pub fn spawn_document_worker(
    tx: std::sync::mpsc::Sender<DocumentResult>,
    action: &'static str,
    work: impl FnOnce() -> DocumentResult + Send + 'static,
) {
    let fallback = tx.clone();
    let started = std::thread::Builder::new()
        .name("document".into())
        .spawn(move || {
            let _ = tx.send(run_document_work(action, work));
        });
    if let Err(error) = started {
        log_error!("project", "could not start a worker to {action}: {error}");
        let _ = fallback.send(DocumentResult::Failed {
            action,
            problem: format!("mooloop could not start the work: {error}").into(),
        });
    }
}

/// `work`'s result, or `Failed` if it panicked. The panic itself has already
/// been through the process's panic hook (log and crash report) by the time
/// it is caught here, so this only has to tell the user.
pub fn run_document_work(
    action: &'static str,
    work: impl FnOnce() -> DocumentResult,
) -> DocumentResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        Ok(result) => result,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "no message".to_owned());
            log_error!("project", "the worker to {action} panicked: {detail}");
            DocumentResult::Failed {
                action,
                problem: DocumentProblem {
                    message: "Something inside mooloop went wrong. This is a bug, not \
                              something you did; the song you have open is unchanged."
                        .to_owned(),
                    report: format!("worker panicked: {detail}"),
                },
            }
        }
    }
}

/// Parks a song that could not be saved, returning where it went.
///
/// The user has already lost the save they asked for; what they must not also
/// lose is the document, because a problem nobody can reopen is a problem
/// nobody can fix. The written report holds the same text as the dialog, so a
/// file found weeks later still says what was wrong with it.
///
/// Failing to park is reported to the log and otherwise swallowed: this runs
/// inside the handler for an error that is already on its way to the user, and
/// a second error stacked on the first would only bury the first.
///
/// `directory` and `build` come from the caller because the config paths
/// and the version string belong to the running application, not to this
/// layer.
pub fn quarantine_song(
    directory: &Path,
    build: &str,
    project: &Project,
    problem: &DocumentProblem,
) -> Option<PathBuf> {
    let path = directory.join(format!("{}.mooloop", mooloop_core::log::file_stamp()));
    let report = format!(
        "mooloop {}\n\n{}\n\n{}\n",
        build, problem.message, problem.report
    );
    match mooloop_project::quarantine_song(&path, project, &report) {
        Ok(path) => {
            log_warn!("project", "song set aside at {}", path.display());
            Some(path)
        }
        Err(error) => {
            log_error!(
                "project",
                "could not set the song aside at {}: {error}",
                path.display()
            );
            None
        }
    }
}

/// A device that should wear the name a preset was saved under: resolved when
/// the dialog is confirmed, applied when the write comes back.
///
/// **The name used to appear before the save was known to have worked.**
/// `set_effect_preset_name` and `set_source_preset_name` ran synchronously on
/// confirm while the write happened on a worker thread, and a write can fail
/// -- a name too long for the filesystem, a permission, a full disk. The error
/// dialog opened and the rack row went on showing the name of a preset that
/// was never written.
///
/// Carrying it here rather than re-deriving it in the result handler is what
/// makes the *generator* case right as well: the channel is the one that was
/// selected when the dialog was confirmed, not whichever one is selected when
/// the disk finishes. The effect case is already safe either way, because
/// `DeviceId` is durable -- a device removed while the write was in flight is
/// simply not found, and nothing is renamed.
/// Which device wears the name a save is writing, resolved when the dialog is
/// confirmed and applied when the file has landed.
///
/// **Both arms name their channel durably**, and this is the clearest case in
/// the session for why: the resolve and the apply are separated by a disk
/// write, and a channel can be pasted, deleted or dragged in between. Keyed by
/// a seat, a slow save could put a preset label on a channel the user never
/// saved from.
pub enum PresetNaming {
    Effect {
        target: crate::session::ChainKey,
        device: mooloop_core::DeviceId,
        name: String,
    },
    Source {
        channel: mooloop_core::ChannelId,
        name: String,
    },
}

pub enum DocumentResult {
    Cancelled,
    NewSong(Project),
    SavedSong {
        path: PathBuf,
        mode: AssetMode,
        revision: u64,
        /// The `Session::document_generation` the save was started in.
        generation: u64,
        report: SaveReport,
        sample_references: Vec<Option<SampleReference>>,
        /// Each channel's key-zone references as the save left them
        /// (MOO-14), for `apply_zone_references`.
        zone_references: Vec<Vec<SampleReference>>,
    },
    SavedOther {
        label: &'static str,
        report: SaveReport,
    },
    SavedPreset {
        label: &'static str,
        report: SaveReport,
        /// The device that should wear the name, applied here rather than on
        /// confirm. See [`PresetNaming`].
        named: Option<PresetNaming>,
    },
    /// `action` completes "Could not ...", e.g. `save this song`.
    Failed {
        action: &'static str,
        problem: DocumentProblem,
    },
    Loaded {
        path: PathBuf,
        target: LoadTarget,
        document: ResolvedDocument,
    },
    /// An export's files, each with what its render found, shown in the
    /// export dialog (MOO-125). `cancelled` is a job stopped partway whose
    /// earlier files stay (MOO-180); a job cancelled before any file had
    /// finished is [`DocumentResult::Cancelled`].
    Exported {
        files: Vec<RenderedFile>,
        cancelled: bool,
    },
}

/// The export dialog's account of a finished export, one sentence per line:
/// how long the file is and at what rate, then anything that makes it not
/// quite the project -- overs the safety limiter held, samples a broken
/// device produced as NaN, automation that found no room, and any clamp the
/// PCM encoder still made (MOO-125, MOO-94).
pub fn export_result_detail(summary: &mooloop_engine::RenderSummary) -> String {
    let rate = f64::from(summary.sample_rate.max(1));
    let seconds = summary.total_frames as f64 / rate;
    let tail = summary.tail_frames as f64 / rate;
    let khz = |hz: u32| f64::from(hz) / 1000.0;
    let mut lines = vec![if summary.file_sample_rate == summary.sample_rate {
        format!(
            "{seconds:.1} s at {} kHz, {tail:.1} s of it tail.",
            khz(summary.sample_rate)
        )
    } else {
        format!(
            "{seconds:.1} s, {tail:.1} s of it tail. Rendered at {} kHz, written at {} kHz.",
            khz(summary.sample_rate),
            khz(summary.file_sample_rate)
        )
    }];
    if summary.overs > 0 {
        lines.push(format!(
            "The mix went over 0 dBFS ({} samples); the safety limiter held it at the ceiling.",
            summary.overs
        ));
    }
    if summary.non_finite_samples > 0 {
        lines.push(format!(
            "A device produced invalid audio (NaN): {} samples were written as silence.",
            summary.non_finite_samples
        ));
    }
    if summary.refused_events > 0 {
        lines.push(format!(
            "{} automation or modulation events found no room and are missing.",
            summary.refused_events
        ));
    }
    if summary.clipped_samples > 0 {
        lines.push(format!(
            "{} samples were clipped at full scale by the PCM encoder.",
            summary.clipped_samples
        ));
    }
    if lines.len() == 1 {
        lines.push("Nothing was over, clipped or lost.".into());
    }
    lines.join("\n")
}

/// The export dialog's title and account of a finished job (MOO-180): one
/// file is named with [`export_result_detail`] under it; several are
/// counted, each named with its own account.
pub fn export_job_result(files: &[RenderedFile], cancelled: bool) -> (String, String) {
    let name = |file: &RenderedFile| {
        file.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.path.display().to_string())
    };
    let mut title = match files {
        [one] => name(one),
        _ => format!("{} files", files.len()),
    };
    if cancelled {
        title = format!("Cancelled; kept {title}");
    }
    let detail = match files {
        [one] => export_result_detail(&one.summary),
        _ => files
            .iter()
            .map(|file| format!("{}: {}", name(file), export_result_detail(&file.summary)))
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    (title, detail)
}

/// Render `request`'s job, reporting into `progress`: what an export worker
/// runs, and what it sends back (MOO-180). A cancel keeps the files that had
/// finished; a failure names them too, since they are on disk.
pub fn run_export(
    request: ExportRequest,
    sample_rate: u32,
    progress: &ExportProgress,
    plugins: std::collections::BTreeMap<
        mooloop_core::PluginSlotId,
        Box<dyn mooloop_dsp::AudioNode + Send>,
    >,
) -> DocumentResult {
    let mut plugins = Some(plugins);
    match OfflineRenderer::render_job_with_plugins(
        &request.project,
        &request.samples,
        &request.zones,
        sample_rate,
        &request.job,
        progress,
        // Every job the dialog builds so far is one pass, and a pass is
        // the only one that gets the processors.
        &mut |_| plugins.take().unwrap_or_default(),
    ) {
        Ok(files) => DocumentResult::Exported {
            files,
            cancelled: false,
        },
        Err(failure) => match failure.error {
            ExportError::Cancelled if failure.written.is_empty() => DocumentResult::Cancelled,
            ExportError::Cancelled => DocumentResult::Exported {
                files: failure.written,
                cancelled: true,
            },
            error => {
                let mut message = error.to_string();
                if !failure.written.is_empty() {
                    let kept: Vec<String> = failure
                        .written
                        .iter()
                        .map(|file| file.path.display().to_string())
                        .collect();
                    message.push_str(&format!(
                        "\n\nThe files that had finished were kept: {}",
                        kept.join(", ")
                    ));
                }
                DocumentResult::Failed {
                    action: "export this song",
                    problem: message.into(),
                }
            }
        },
    }
}

/// The path a chooser picked, or the result to send instead.
///
/// A cancel is [`DocumentResult::Cancelled`], which clears the status bar.
/// **A chooser that could not be shown at all is a failure** (MOO-90), and
/// gets the error dialog with everything that was tried: it used to be a
/// cancel, so on a desktop without zenity Save As, Open and Export did
/// nothing and said nothing. `action` completes "Could not ...".
///
/// The large `Err` is the value the caller sends on as it is, once per
/// chooser, on a worker thread: boxing it would only be unboxed again.
#[allow(clippy::result_large_err)]
pub fn chosen_path(
    picked: crate::dialogs::Picked,
    action: &'static str,
) -> Result<PathBuf, DocumentResult> {
    match picked {
        crate::dialogs::Picked::Path(path) => Ok(path),
        crate::dialogs::Picked::Cancelled => Err(DocumentResult::Cancelled),
        crate::dialogs::Picked::Unavailable(none) => Err(DocumentResult::Failed {
            action,
            problem: none.explain().into(),
        }),
    }
}

#[derive(Clone, Debug)]
pub enum LoadTarget {
    Song,
    Kit,
    Channel,
    /// A generator preset, carrying the name it is offered under so the
    /// device it lands on can wear it. The name travels with the load rather
    /// than being stashed beside it: two loads in flight would otherwise race
    /// for one field, and the label would name the wrong patch.
    Generator {
        preset_name: String,
    },
}


pub fn resolve_document(path: &Path) -> Result<ResolvedDocument, DocumentProblem> {
    let mut report = mooloop_project::load_bundle(path)?;
    let sample_references = match &report.document {
        LoadedDocument::Song(project) => project
            .channels
            .iter()
            .map(|channel| {
                channel
                    .setup
                    .source
                    .sampler_state()
                    .map(|sampler| sampler.sample.clone())
            })
            .collect::<Vec<_>>(),
        LoadedDocument::Kit(kit) => kit
            .channels
            .iter()
            .map(|channel| {
                channel
                    .source
                    .sampler_state()
                    .map(|sampler| sampler.sample.clone())
            })
            .collect(),
        LoadedDocument::Channel(channel) => vec![channel
            .source
            .sampler_state()
            .map(|sampler| sampler.sample.clone())],
        LoadedDocument::Generator(source) => {
            vec![source.sampler_state().map(|sampler| sampler.sample.clone())]
        }
        // Neither an effect nor a run of them references audio; there is
        // nothing to decode.
        LoadedDocument::Effect(_) | LoadedDocument::EffectRun(_) => Vec::new(),
    };
    let mut samples = Vec::with_capacity(sample_references.len());
    for (channel, reference) in sample_references.into_iter().enumerate() {
        match reference {
            None | Some(SampleReference::Builtin { .. } | SampleReference::Empty) => {
                samples.push(None)
            }
            Some(SampleReference::File { path, .. }) if path.is_file() => {
                match audio_file::decode(&path) {
                    Ok(decoded) => samples.push(Some(decoded.sample)),
                    Err(error) => {
                        report.warnings.push(AssetWarning {
                            channel,
                            path,
                            message: error,
                        });
                        samples.push(None);
                    }
                }
            }
            Some(SampleReference::File { .. }) => samples.push(None),
        }
    }
    let samplers: Vec<(usize, &mooloop_core::SamplerState)> = match &report.document {
        LoadedDocument::Song(project) => project
            .channels
            .iter()
            .enumerate()
            .filter_map(|(index, channel)| Some((index, channel.setup.source.sampler_state()?)))
            .collect(),
        LoadedDocument::Kit(kit) => kit
            .channels
            .iter()
            .enumerate()
            .filter_map(|(index, channel)| Some((index, channel.source.sampler_state()?)))
            .collect(),
        LoadedDocument::Channel(channel) => channel.source.sampler_state().map(|s| (0, s)).into_iter().collect(),
        LoadedDocument::Generator(source) => source.sampler_state().map(|s| (0, s)).into_iter().collect(),
        LoadedDocument::Effect(_) | LoadedDocument::EffectRun(_) => Vec::new(),
    };
    let mut zone_warnings = Vec::new();
    let zone_audio = crate::sample::decode_zone_files(samplers, &mut zone_warnings);
    report.warnings.extend(zone_warnings);
    Ok(ResolvedDocument {
        report,
        samples,
        zone_audio,
    })
}

pub fn warning_suffix(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!(
            " ({count} sample warning{})",
            if count == 1 { "" } else { "s" }
        )
    }
}

/// Saving and loading repair what they can rather than refusing, so a clean
/// run still has something to say when it corrected anything on the way.
pub fn repair_suffix(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        format!(
            " ({count} problem{} corrected)",
            if count == 1 { "" } else { "s" }
        )
    }
}

/// Writes every correction to the log, one line each.
///
/// The status bar has room for a count and nothing else, but the count is not
/// the useful part: the code says which invariant the edit layer let through,
/// and that is the only lead there is on a bug that corrected itself. At
/// `warn` because a document needing repair is never expected, even though the
/// user's save went through.
pub fn log_repairs(what: &str, repairs: &[Issue]) {
    for issue in repairs {
        log_warn!("project", "{what}: [{}] {issue}", issue.code);
    }
}

/// The same, for the asset warnings the status bar can only count.
///
/// `warning_suffix` above says "(1 sample warning)" and stops, and an
/// `AssetWarning` carries the channel, the path and the message -- so a song
/// opened with a missing sample played silence and the name of the file it
/// wanted reached nothing. `CURRENT.md` says a missing sample is "recoverable
/// by loading a replacement audio file", which needs knowing which one it is.
pub fn log_asset_warnings(what: &str, warnings: &[AssetWarning]) {
    for warning in warnings {
        log_warn!(
            "project",
            "{what}: channel {} - {} ({})",
            warning.channel + 1,
            warning.message,
            warning.path.display()
        );
    }
}

/// Everything an offline render needs, resolved from the session in one go.
///
/// The render runs on a worker thread, so it takes the project and its audio
/// by value rather than borrowing a session that is still being edited.
pub struct ExportRequest {
    pub project: Project,
    pub samples: Vec<Option<Arc<SampleData>>>,
    /// Each sampler's key-zone buffers, parallel to its zones (MOO-14).
    pub zones: Vec<Vec<Option<Arc<SampleData>>>>,
    /// The files to write, built from the dialog's [`RenderSettings`].
    pub job: RenderJob,
}

/// The channel a preset is being saved from, and which kind of preset it is.
pub struct PresetSource {
    pub target: PresetSaveTarget,
    pub setup: ChannelSetup,
    /// The rack row an effect save was started from, when `target` is one.
    /// A bus row has no channel setup worth saving, so this is what an effect
    /// save writes and `setup` is only what the selected channel happens to
    /// be.
    pub effect: Option<EffectSlotState>,
    /// The container's whole run, when the row an effect save was started
    /// from is a container. `effect` still carries its first row, so a caller
    /// that does not know about containers writes the box on its own rather
    /// than something wrong.
    pub run: Option<mooloop_core::EffectRun>,
}

impl Session {
    /// What the transport plays, which is what an export's
    /// [`RenderRange::Transport`] renders: exporting the song while the
    /// sequencer is in pattern mode would render something the user is not
    /// listening to.
    ///
    /// [`RenderRange::Transport`]: crate::render_settings::RenderRange::Transport
    pub fn export_scope(&self) -> RenderScope {
        if self.song_mode {
            RenderScope::Song
        } else {
            RenderScope::Pattern {
                index: self.current_pattern,
            }
        }
    }

    /// The song as an export's range is resolved against (MOO-181): what
    /// the transport plays, the current pattern, the loop selection and the
    /// song's length.
    pub fn export_timeline(&self) -> Timeline {
        let pattern_steps = self
            .pattern_lengths
            .get(self.current_pattern)
            .copied()
            .unwrap_or(0);
        Timeline {
            transport: self.export_scope(),
            pattern: self.current_pattern,
            pattern_ticks: (pattern_steps as u32).saturating_mul(mooloop_core::TICKS_PER_STEP),
            loop_range: self.loop_range,
            song_ticks: self.song_length_ticks(),
        }
    }

    /// The song's name, which an export's file takes when none is typed:
    /// its file's, or `None` for a song never saved.
    pub fn export_song_name(&self) -> Option<String> {
        self.bundle_path
            .as_deref()
            .and_then(Path::file_stem)
            .map(|stem| stem.to_string_lossy().into_owned())
    }

    /// The song's tracks, the master left out, as a stem export names them
    /// (MOO-182).
    pub fn export_tracks(&self) -> Vec<ExportTrack> {
        self.buses
            .iter()
            .enumerate()
            .skip(1)
            .map(|(index, track)| ExportTrack {
                id: track.id,
                index: index as u8,
                name: track.bus.name.clone(),
                output: track.bus.output,
            })
            .collect()
    }

    /// The folder an export writes to when none is chosen: the song's own,
    /// or the music folder for a song never saved.
    pub fn export_default_folder(&self) -> PathBuf {
        default_export_folder(self.bundle_path.as_deref())
    }

    /// Resolves an export from the dialog's settings and the current
    /// transport state, or says why it can't be exported as it is.
    pub fn export_request(
        &self,
        bpm: i32,
        swing_percent: i32,
        settings: &RenderSettings,
    ) -> Result<ExportRequest, SettingsProblem> {
        let scope = settings.range.scope(&self.export_timeline())?;
        let job = settings.job(
            scope,
            self.export_song_name().as_deref(),
            &self.export_default_folder(),
            &self.export_tracks(),
        )?;
        Ok(ExportRequest {
            project: self.project_snapshot(bpm, swing_percent),
            samples: self.sample_snapshots(),
            zones: self.zone_sample_snapshots(),
            job,
        })
    }

    /// Takes the pending preset save, with the channel setup it applies to.
    ///
    /// Taking rather than reading: a dialog that has been confirmed is spent,
    /// and leaving it armed would let a second confirmation save again.
    pub fn take_preset_save(&mut self, bpm: i32, swing_percent: i32) -> Option<PresetSource> {
        let target = self.pending_preset_save.take()?;
        let snapshot = self.project_snapshot(bpm, swing_percent);
        let setup = snapshot
            .channels
            .get(snapshot.selected_index())?
            .setup
            .clone();
        let effect = match target {
            PresetSaveTarget::Effect {
                target: chain,
                device,
            } => {
                let chain = self.effect_chain_of(self.chain_target(chain)?)?;
                let slot = mooloop_core::device_slot(chain, device)?;
                // Stripped of its identity on the way out: a preset is what a
                // device sounds like, and identity belongs to the chain it
                // was taken from rather than to the patch.
                Some(chain[slot].with_id(mooloop_core::DeviceId::UNASSIGNED))
            }
            _ => None,
        };
        // A container saves as its run: the box, and everything in it, with
        // every identity stripped. `load_effect_run` mints fresh ones,
        // because which devices these are belongs to the chain they land on.
        let run = match target {
            PresetSaveTarget::Effect {
                target: chain,
                device,
            } if effect.is_some_and(|effect| effect.kind().is_container()) => {
                let chain = self.effect_chain_of(self.chain_target(chain)?)?;
                let slot = mooloop_core::device_slot(chain, device)?;
                Some(mooloop_core::EffectRun {
                    effects: chain[mooloop_core::run_of(chain, slot)]
                        .iter()
                        .map(|effect| effect.with_id(mooloop_core::DeviceId::UNASSIGNED))
                        .collect(),
                })
            }
            _ => None,
        };
        Some(PresetSource {
            target,
            setup,
            effect,
            run,
        })
    }

}

#[cfg(test)]
mod tests {
    use super::{
        export_job_result, export_result_detail, run_export, spawn_document_worker,
        DocumentResult,
    };
    use crate::render_settings::{OutputSettings, RenderSettings, TailSettings};
    use crate::session::Session;
    use mooloop_engine::{ExportProgress, RenderSummary, RenderedFile};
    use std::path::PathBuf;

    /// **The master mix goes to the folder and name typed on the card, with
    /// no chooser** (MOO-180): the settings build the job, and the job
    /// writes exactly that file.
    #[test]
    fn an_export_writes_the_typed_name_in_the_typed_folder() {
        let folder = tempfile::tempdir().unwrap();
        let session = Session::default();
        let settings = RenderSettings {
            tail: TailSettings { max_seconds: 0 },
            output: OutputSettings {
                folder: Some(folder.path().to_path_buf()),
                name: "first mix".into(),
                ..OutputSettings::default()
            },
            ..RenderSettings::default()
        };
        let request = session.export_request(120, 0, &settings).unwrap();
        let result = run_export(request, 48_000, &ExportProgress::new(), Default::default());
        let DocumentResult::Exported { files, cancelled } = result else {
            panic!("expected Exported");
        };
        assert!(!cancelled);
        assert_eq!(files.len(), 1);
        let target = folder.path().join("first mix.wav");
        assert_eq!(files[0].path, target);
        assert!(target.is_file());
        let names: Vec<_> = std::fs::read_dir(folder.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["first mix.wav"], "nothing but the file is left behind");

        // Again, to the same name: numbered, and the first file is untouched
        // (MOO-188).
        let before = std::fs::read(&target).unwrap();
        let request = session.export_request(120, 0, &settings).unwrap();
        let DocumentResult::Exported { files, .. } =
            run_export(request, 48_000, &ExportProgress::new(), Default::default())
        else {
            panic!("expected Exported");
        };
        assert_eq!(files[0].path, folder.path().join("first mix-001.wav"));
        assert!(files[0].path.is_file());
        assert_eq!(std::fs::read(&target).unwrap(), before);
    }

    /// **The loop selection renders exactly its points** (MOO-181), with
    /// looping switched off, and a range the song cannot hold is said on
    /// the card before any render.
    #[test]
    fn a_loop_selection_renders_exactly_its_points() {
        use crate::render_settings::RenderRange;
        use mooloop_core::{LoopRange, PatternPlacement, TICKS_PER_BAR};
        use mooloop_engine::RenderScope;
        let folder = tempfile::tempdir().unwrap();
        let session = Session {
            playlist: (0..4)
                .map(|bar| PatternPlacement::new(0, bar * TICKS_PER_BAR))
                .collect(),
            pattern_lengths: vec![16],
            loop_range: LoopRange {
                start_tick: TICKS_PER_BAR + 96,
                end_tick: 3 * TICKS_PER_BAR,
                enabled: false,
            },
            ..Session::default()
        };
        let mut settings = RenderSettings {
            range: RenderRange::Loop,
            tail: TailSettings { max_seconds: 0 },
            output: OutputSettings {
                folder: Some(folder.path().to_path_buf()),
                name: "loop".into(),
                ..OutputSettings::default()
            },
            ..RenderSettings::default()
        };
        let request = session.export_request(120, 0, &settings).unwrap();
        assert_eq!(
            request.job.passes[0].scope,
            RenderScope::Range {
                start_tick: TICKS_PER_BAR + 96,
                end_tick: 3 * TICKS_PER_BAR,
            }
        );
        let DocumentResult::Exported { files, .. } =
            run_export(request, 48_000, &ExportProgress::new(), Default::default())
        else {
            panic!("expected Exported");
        };
        // 672 ticks at 120 BPM and 48 kHz, 250 frames a tick.
        assert_eq!(files[0].summary.base_frames, 672 * 250);

        settings.range = RenderRange::Custom {
            start_tick: 0,
            end_tick: 5 * TICKS_PER_BAR,
        };
        let problem = session.export_request(120, 0, &settings).err().unwrap();
        assert_eq!(problem.0, "The song ends at 5.1.");
    }

    /// A folder that isn't there is said on the card, before any render.
    #[test]
    fn an_export_to_a_missing_folder_is_refused_before_it_renders() {
        let folder = tempfile::tempdir().unwrap();
        let settings = RenderSettings {
            output: OutputSettings {
                folder: Some(folder.path().join("gone")),
                name: String::new(),
                ..OutputSettings::default()
            },
            ..RenderSettings::default()
        };
        let problem = Session::default()
            .export_request(120, 0, &settings)
            .err()
            .expect("a missing folder is a problem");
        assert!(problem.0.contains("doesn't exist"), "{problem}");
    }

    #[test]
    fn a_job_of_several_files_is_counted_and_each_is_named() {
        let file = |name: &str| RenderedFile {
            path: PathBuf::from("/renders").join(name),
            summary: summary(),
        };
        let (title, detail) = export_job_result(&[file("mix.wav")], false);
        assert_eq!(title, "mix.wav");
        assert_eq!(detail, export_result_detail(&summary()));

        let (title, detail) = export_job_result(&[file("mix.wav"), file("mix.mp3")], false);
        assert_eq!(title, "2 files");
        assert!(detail.starts_with("mix.wav: 2.5 s"), "{detail}");
        assert!(detail.contains("\n\nmix.mp3: 2.5 s"), "{detail}");

        let (title, _) = export_job_result(&[file("mix.wav")], true);
        assert_eq!(title, "Cancelled; kept mix.wav");
    }

    /// **A worker that panics still reports** (MOO-103). The UI clears
    /// `document-busy` on whatever result arrives, so a panic that sent
    /// nothing left the File menu disabled until restart.
    #[test]
    fn a_document_worker_that_panics_reports_failed() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_document_worker(tx, "open this song", || panic!("a decoder fell over"));
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("a panicking worker still sends a result");
        match result {
            DocumentResult::Failed { action, problem } => {
                assert_eq!(action, "open this song");
                assert!(problem.report.contains("a decoder fell over"), "{}", problem.report);
            }
            _ => panic!("expected Failed"),
        }
        assert!(rx.recv().is_err(), "exactly one result, then the sender is gone");
    }

    #[test]
    fn a_document_worker_that_finishes_reports_its_result() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_document_worker(tx, "open this song", || DocumentResult::Cancelled);
        assert!(matches!(rx.recv().unwrap(), DocumentResult::Cancelled));
        assert!(rx.recv().is_err());
    }

    fn summary() -> RenderSummary {
        RenderSummary {
            sample_rate: 48_000,
            file_sample_rate: 48_000,
            base_frames: 96_000,
            tail_frames: 24_000,
            total_frames: 120_000,
            refused_events: 0,
            overs: 0,
            clipped_samples: 0,
            non_finite_samples: 0,
        }
    }

    #[test]
    fn a_clean_export_says_so() {
        assert_eq!(
            export_result_detail(&summary()),
            "2.5 s at 48 kHz, 0.5 s of it tail.\nNothing was over, clipped or lost."
        );
    }

    #[test]
    fn every_count_the_render_found_is_shown() {
        let detail = export_result_detail(&RenderSummary {
            sample_rate: 96_000,
            file_sample_rate: 48_000,
            overs: 12,
            non_finite_samples: 3,
            refused_events: 4,
            clipped_samples: 5,
            ..summary()
        });
        for needle in ["written at 48 kHz", "12 samples", "3 samples", "4 automation", "5 samples"] {
            assert!(detail.contains(needle), "{needle:?} missing from {detail:?}");
        }
        assert!(!detail.contains("Nothing was"));
    }
}
