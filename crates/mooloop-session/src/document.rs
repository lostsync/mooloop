//! Opening, saving, and failing to do either.
//!
//! A document that cannot be saved is still a document the user must not
//! lose, so the failure paths here carry as much as the success ones.

use crate::audio_file;
use mooloop_core::{log_error, log_warn, Project, SampleReference};
use mooloop_dsp::SampleData;
use mooloop_project::{AssetMode, AssetWarning, Issue, LoadReport, LoadedDocument, SaveReport};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ResolvedDocument {
    pub report: LoadReport,
    pub samples: Vec<Option<Arc<SampleData>>>,
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

pub enum DocumentResult {
    Cancelled,
    NewSong(Project),
    SavedSong {
        path: PathBuf,
        mode: AssetMode,
        revision: u64,
        report: SaveReport,
        sample_references: Vec<Option<SampleReference>>,
    },
    SavedOther {
        label: &'static str,
        report: SaveReport,
    },
    SavedPreset {
        label: &'static str,
        report: SaveReport,
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
    Exported {
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum LoadTarget {
    Song,
    Kit,
    Channel,
    Generator,
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
    Ok(ResolvedDocument { report, samples })
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
