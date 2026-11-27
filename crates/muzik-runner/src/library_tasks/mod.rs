//! Library maintenance tasks that run as queue jobs and after imports.

mod covers;
mod mbids;
mod replaygain;
mod singletons;

use crate::gates::{self, Gate};
use muzik_core::{DecisionKind, JobEvent, Severity, Task};
use muzik_import::beets::BeetsPaths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr};

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    Serialize,
    Deserialize,
)]
pub enum LibraryTask {
    #[strum(serialize = "replaygain")]
    #[serde(rename = "replaygain")]
    ReplayGain,
    #[strum(serialize = "covers")]
    #[serde(rename = "covers")]
    Covers,
    #[strum(serialize = "mbids")]
    #[serde(rename = "mbids")]
    Mbids,
    #[strum(serialize = "singletons")]
    #[serde(rename = "singletons")]
    Singletons,
}

impl LibraryTask {
    pub const ALL: [Self; 4] = [
        Self::ReplayGain,
        Self::Covers,
        Self::Mbids,
        Self::Singletons,
    ];
    pub const AFTER_IMPORT: [Self; 2] = [Self::ReplayGain, Self::Covers];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReplayGain => "replaygain",
            Self::Covers => "covers",
            Self::Mbids => "mbids",
            Self::Singletons => "singletons",
        }
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        text.trim().parse().ok()
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::ReplayGain => "Calculate ReplayGain",
            Self::Covers => "Add cover files",
            Self::Mbids => "Fill MusicBrainz IDs",
            Self::Singletons => "Make one-song albums singletons",
        }
    }

    #[must_use]
    pub const fn gate(self) -> Gate {
        match self {
            Self::ReplayGain => Gate::Process,
            Self::Covers => Gate::Download,
            Self::Mbids | Self::Singletons => Gate::Import,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryRequest {
    pub task: LibraryTask,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<PathBuf>,
}

impl LibraryRequest {
    #[must_use]
    pub const fn new(task: LibraryTask) -> Self {
        Self {
            task,
            dry_run: false,
            config: None,
        }
    }

    #[must_use]
    pub fn item_key(&self) -> String {
        let task = self.task.as_str();
        if self.dry_run {
            format!("library:{task}:dry-run")
        } else {
            format!("library:{task}")
        }
    }

    #[must_use]
    pub fn title(&self) -> String {
        let title = self.task.title();
        if self.dry_run {
            format!("Preview: {title}")
        } else {
            title.to_owned()
        }
    }

    /// # Errors
    /// Returns an error when the params have no valid task or a field has the wrong type.
    pub fn from_params(params: &Value) -> crate::Result<Self> {
        Ok(serde_json::from_value(params.clone())?)
    }

    /// # Errors
    /// Returns an error when the request cannot be written as JSON.
    pub fn to_params(&self) -> crate::Result<Value> {
        Ok(serde_json::to_value(self)?)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AfterImport {
    pub replaygain: bool,
    pub covers: bool,
}

impl AfterImport {
    #[must_use]
    pub const fn enabled(self, task: LibraryTask) -> bool {
        match task {
            LibraryTask::ReplayGain => self.replaygain,
            LibraryTask::Covers => self.covers,
            LibraryTask::Mbids | LibraryTask::Singletons => false,
        }
    }

    #[must_use]
    pub const fn any(self) -> bool {
        self.replaygain || self.covers
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskReport {
    pub processed: usize,
    pub changed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub messages: Vec<String>,
}

impl TaskReport {
    pub fn merge(&mut self, other: Self) {
        self.processed = self.processed.saturating_add(other.processed);
        self.changed = self.changed.saturating_add(other.changed);
        self.skipped = self.skipped.saturating_add(other.skipped);
        self.failed = self.failed.saturating_add(other.failed);
        self.messages.extend(other.messages);
    }

    #[must_use]
    pub fn summary(&self, task: LibraryTask) -> String {
        format!(
            "{}: {} checked, {} changed, {} skipped, {} failed.",
            task.title(),
            self.processed,
            self.changed,
            self.skipped,
            self.failed
        )
    }
}

pub type Decide<'a> = dyn FnMut(DecisionKind, Value) -> Result<Value, String> + 'a;
pub type Events<'a> = dyn FnMut(JobEvent) + 'a;

pub struct TaskContext<'a> {
    pub paths: &'a BeetsPaths,
    pub dry_run: bool,
    pub cancelled: &'a AtomicBool,
    events: RefCell<&'a mut Events<'a>>,
    decide: RefCell<&'a mut Decide<'a>>,
}

impl<'a> TaskContext<'a> {
    pub fn new(
        paths: &'a BeetsPaths,
        dry_run: bool,
        cancelled: &'a AtomicBool,
        events: &'a mut Events<'a>,
        decide: &'a mut Decide<'a>,
    ) -> Self {
        Self {
            paths,
            dry_run,
            cancelled,
            events: RefCell::new(events),
            decide: RefCell::new(decide),
        }
    }

    #[must_use]
    pub fn library(&self) -> &Path {
        &self.paths.library
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.paths.directory
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// # Errors
    /// Returns [`crate::Error::Cancelled`] when the job was cancelled.
    pub fn check_cancelled(&self) -> crate::Result<()> {
        if self.is_cancelled() {
            Err(crate::Error::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn event(&self, event: JobEvent) {
        if let Ok(mut events) = self.events.try_borrow_mut() {
            (*events)(event);
        }
    }

    pub fn message(&self, message: impl Into<String>) {
        self.event(JobEvent::message(message));
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.event(JobEvent::Message {
            message: message.into(),
            severity: Severity::Warning,
        });
    }

    pub fn progress_started(&self, description: impl Into<String>, total: usize) {
        self.event(JobEvent::ProgressStarted {
            task: Task::LibraryMaintenance,
            description: description.into(),
            total: u64::try_from(total).ok(),
        });
    }

    pub fn progress(&self, completed: usize, total: usize) {
        self.event(JobEvent::ProgressAdvanced {
            task: Task::LibraryMaintenance,
            completed: u64::try_from(completed).ok(),
            total: u64::try_from(total).ok(),
        });
    }

    pub fn progress_finished(&self, success: bool) {
        self.event(JobEvent::ProgressFinished {
            task: Task::LibraryMaintenance,
            success,
        });
    }

    /// # Errors
    /// Returns the reason when nobody can answer the question or the job stops while it waits.
    pub fn decide(&self, kind: DecisionKind, payload: Value) -> Result<Value, String> {
        let mut decide = self
            .decide
            .try_borrow_mut()
            .map_err(|_| "Another question is open.".to_owned())?;
        (*decide)(kind, payload)
    }
}

/// # Errors
/// Returns an error when the task fails or is cancelled.
pub fn run(task: LibraryTask, context: &TaskContext<'_>) -> crate::Result<TaskReport> {
    match task {
        LibraryTask::ReplayGain => replaygain::run(context),
        LibraryTask::Covers => covers::run(context),
        LibraryTask::Mbids => mbids::run(context),
        LibraryTask::Singletons => singletons::run(context),
    }
}

#[must_use]
pub fn album_dirs(destinations: &[PathBuf]) -> Vec<PathBuf> {
    destinations
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn singleton_paths(paths: &BeetsPaths) -> BTreeSet<PathBuf> {
    let Ok(library) = muzik_library::Library::open_read_only(&paths.library) else {
        return BTreeSet::new();
    };
    library
        .items()
        .unwrap_or_default()
        .iter()
        .filter(|item| item.album_id().is_none())
        .filter_map(|item| item.field("path").and_then(muzik_library::path_from_sql))
        .map(|path| paths.directory.join(path))
        .collect()
}

pub fn after_import(
    enabled: AfterImport,
    paths: &BeetsPaths,
    destinations: &[PathBuf],
    cancelled: &AtomicBool,
    on_event: &mut Events<'_>,
) -> TaskReport {
    let mut report = TaskReport::default();
    if !enabled.any() || destinations.is_empty() {
        return report;
    }
    let singletons = singleton_paths(paths);
    let (songs, in_albums): (Vec<PathBuf>, Vec<PathBuf>) = destinations
        .iter()
        .cloned()
        .partition(|path| singletons.contains(path));
    let dirs = album_dirs(&in_albums);
    let mut refuse = |_: DecisionKind, _: Value| {
        Err("A task after an import does not ask for choices.".to_owned())
    };
    let context = TaskContext::new(paths, false, cancelled, on_event, &mut refuse);
    for task in LibraryTask::AFTER_IMPORT {
        if !enabled.enabled(task) || context.is_cancelled() {
            continue;
        }
        let songs = if task == LibraryTask::ReplayGain {
            songs.as_slice()
        } else {
            &[]
        };
        if dirs.is_empty() && songs.is_empty() {
            continue;
        }
        context.message(format!(
            "{} for {} album folder(s) and {} song(s) without an album.",
            task.title(),
            dirs.len(),
            songs.len()
        ));
        let outcome = gates::enter(task.gate(), cancelled).and_then(|_permit| match task {
            LibraryTask::ReplayGain => replaygain::after_import(&context, &dirs, songs),
            LibraryTask::Covers => covers::after_import(&context, &dirs),
            LibraryTask::Mbids | LibraryTask::Singletons => Ok(TaskReport::default()),
        });
        match outcome {
            Ok(done) => report.merge(done),
            Err(error) => {
                context.warn(format!("{} did not finish: {error}", task.title()));
                report.failed = report.failed.saturating_add(1);
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::{AfterImport, LibraryRequest, LibraryTask, TaskReport, after_import, album_dirs};
    use muzik_core::JobEvent;
    use muzik_import::beets::BeetsPaths;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    fn paths(root: &Path) -> BeetsPaths {
        BeetsPaths {
            config: root.join("config.yaml"),
            library: root.join("library.db"),
            directory: root.join("Music"),
            statefile: root.join("state.pickle"),
        }
    }

    #[test]
    fn every_task_reads_back_from_its_name_and_its_job_params() {
        for task in LibraryTask::ALL {
            assert_eq!(LibraryTask::parse(task.as_str()), Some(task));
            assert_eq!(task.as_ref(), task.as_str());
            let request = LibraryRequest {
                task,
                dry_run: true,
                config: None,
            };
            let params = request.to_params().unwrap();
            assert_eq!(params, json!({"task":task.as_str(),"dry_run":true}));
            assert_eq!(LibraryRequest::from_params(&params).unwrap(), request);
        }
        assert_eq!(
            LibraryTask::parse(" replaygain "),
            Some(LibraryTask::ReplayGain)
        );
        assert_eq!(LibraryTask::parse("replay_gain"), None);
        assert_eq!(
            LibraryRequest::from_params(&json!({"task":"covers"})).unwrap(),
            LibraryRequest::new(LibraryTask::Covers)
        );
        assert!(LibraryRequest::from_params(&json!({"task":"lyrics"})).is_err());
        assert!(LibraryRequest::from_params(&json!({})).is_err());
    }

    #[test]
    fn album_folders_come_once_from_the_imported_files() {
        let dirs = album_dirs(&[
            PathBuf::from("/m/B/Two/02.flac"),
            PathBuf::from("/m/A/One/01.flac"),
            PathBuf::from("/m/A/One/02.flac"),
        ]);
        assert_eq!(dirs, [PathBuf::from("/m/A/One"), PathBuf::from("/m/B/Two")]);
    }

    #[test]
    fn the_hook_runs_only_the_enabled_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let imported = [dir.path().join("Music/A/One/01.flac")];
        let cancelled = AtomicBool::new(false);
        let run = |enabled: AfterImport| {
            let mut events = Vec::new();
            let report = after_import(enabled, &paths, &imported, &cancelled, &mut |event| {
                events.push(event);
            });
            (report, events)
        };
        let (report, events) = run(AfterImport::default());
        assert_eq!(report, TaskReport::default());
        assert_eq!(events, []);
        let (_, events) = run(AfterImport {
            replaygain: false,
            covers: true,
        });
        let messages: Vec<String> = events
            .into_iter()
            .filter_map(|event| match event {
                JobEvent::Message { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].starts_with(LibraryTask::Covers.title()));
        let mut events = Vec::new();
        after_import(
            AfterImport {
                replaygain: true,
                covers: true,
            },
            &paths,
            &[],
            &cancelled,
            &mut |event| events.push(event),
        );
        assert_eq!(events, []);
    }

    #[test]
    fn the_hook_adds_no_cover_file_next_to_a_song_without_an_album() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        let song = paths.directory.join("Non-Album/A/Song.flac");
        std::fs::create_dir_all(song.parent().unwrap()).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../muzik-tags/tests/fixtures/blank.flac"),
            &song,
        )
        .unwrap();
        let png = std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../muzik-tags/tests/fixtures/cover.png"),
        )
        .unwrap();
        muzik_tags::embed_cover(&song, &png, &muzik_tags::MimeType::Png).unwrap();
        muzik_library::Library::open_or_create(&paths.library)
            .unwrap()
            .insert_item(
                &muzik_library::Fields::from([(
                    "path".to_owned(),
                    muzik_library::path_to_sql(&song),
                )]),
                &muzik_library::Fields::new(),
            )
            .unwrap();
        let cancelled = AtomicBool::new(false);
        let report = after_import(
            AfterImport {
                replaygain: false,
                covers: true,
            },
            &paths,
            std::slice::from_ref(&song),
            &cancelled,
            &mut |_| {},
        );
        assert_eq!(report, TaskReport::default());
        assert_eq!(muzik_tags::find_cover(song.parent().unwrap()), None);
    }
}
