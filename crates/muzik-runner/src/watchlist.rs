//! Watchlist jobs on the queue. Each source kind is one module in `sources`.

use crate::queue::item_request;
use crate::settings::Settings;
use crate::sources;
use muzik_core::{DecisionKind, JobEvent, Task};
use muzik_store::watchlist::jobs::{
    self, JobError, JobOptions, LoadedSource, Operations, PendingItem, RunningJob,
};
use muzik_store::watchlist::{
    AudioIndex, ItemAction, Playlist, Repository, Stage, WatchItem, import_cache, playlist_files,
};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) fn sync(
    settings: &Settings,
    playlist_id: Option<&str>,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<Vec<PendingItem>, JobError> {
    let prepared = Prepared::new(settings);
    sources::ensure(&prepared.repository, &settings.paths)?;
    import_cache(&prepared.repository, settings.reconcile())?;
    let events = RefCell::new(on_event);
    let parked = RefCell::new(None);
    let mut adapter = Adapter {
        prepared: &prepared,
        events: &events,
        on_import_event: &mut |_| {},
        decide: &mut |_, _| Err("A playlist check does not ask for choices.".into()),
        parked: &parked,
        cancelled,
    };
    let mut options = prepared.job_options(None);
    options.playlist_id = playlist_id.filter(|id| !id.is_empty());
    let synced = jobs::sync(
        &prepared.repository,
        options,
        &mut adapter,
        cancelled,
        &mut |record| {
            (events.borrow_mut())(record);
        },
    )?;
    (events.borrow_mut())(JobEvent::ProgressFinished {
        task: Task::WatchlistRefresh,
        success: synced.errors == 0,
    });
    Ok(synced.pending)
}

/// # Errors
/// Returns an error when the watchlist, the music library, or a playlist file cannot be read or written.
pub fn write_playlists(settings: &Settings) -> crate::Result<playlist_files::Report> {
    let watchlist = Repository::open(&settings.paths).load()?;
    let config = settings
        .options
        .config
        .as_deref()
        .unwrap_or(&settings.paths.beets);
    Ok(playlist_files::write(
        &watchlist,
        config,
        settings.playlists,
    )?)
}

pub(crate) fn update_playlists(settings: &Settings, on_event: &mut dyn FnMut(JobEvent)) {
    if settings.playlists.is_none() || settings.options.dry_run {
        return;
    }
    match write_playlists(settings) {
        Ok(report) => {
            let changed = report
                .written
                .iter()
                .filter(|written| written.changed)
                .count();
            if changed > 0 || !report.removed.is_empty() {
                on_event(JobEvent::message(format!(
                    "Updated {changed} playlist file(s) and removed {}.",
                    report.removed.len()
                )));
            }
        }
        Err(error) => on_event(JobEvent::Message {
            message: format!("The playlist files were not updated: {error}"),
            severity: muzik_core::Severity::Warning,
        }),
    }
}

/// # Errors
/// Returns an error when the watchlist sources cannot be read or saved.
pub fn ensure_sources(paths: &muzik_core::paths::Paths) -> crate::Result<bool> {
    sources::ensure(&Repository::open(paths), paths)
}

pub(crate) struct ItemJob<'a> {
    pub(crate) id: i64,
    pub(crate) params: &'a Value,
}

pub(crate) fn action(
    settings: &Settings,
    job: &ItemJob<'_>,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
    on_import_event: &mut dyn FnMut(JobEvent),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    parked: &RefCell<Option<Parked>>,
) -> Result<Value, JobError> {
    let prepared = Prepared::new(settings);
    let (id, action) = item_request(job.params)?;
    let events = RefCell::new(on_event);
    let mut adapter = Adapter {
        prepared: &prepared,
        events: &events,
        on_import_event,
        decide,
        parked,
        cancelled,
    };
    jobs::action(
        &prepared.repository,
        prepared.job_options(Some(RunningJob {
            id: job.id,
            params: job.params,
        })),
        &id,
        action,
        &mut adapter,
        cancelled,
    )
}

pub(crate) fn abandon(
    paths: &muzik_core::paths::Paths,
    job: &ItemJob<'_>,
    message: &str,
) -> Result<(), JobError> {
    let (id, action) = item_request(job.params)?;
    Ok(jobs::abandon(
        &Repository::open(paths),
        job.id,
        &id,
        action,
        message,
    )?)
}

pub(crate) struct Prepared<'a> {
    pub(crate) settings: &'a Settings,
    repository: Repository,
}

pub struct Parked {
    pub kind: DecisionKind,
    pub payload: Value,
}

impl Parked {
    fn question(&self) -> Value {
        json!({"kind":self.kind,"payload":self.payload})
    }
}

impl<'a> Prepared<'a> {
    pub(crate) fn new(settings: &'a Settings) -> Self {
        Self {
            settings,
            repository: Repository::open(&settings.paths),
        }
    }

    fn job_options<'b>(&'b self, job: Option<RunningJob<'b>>) -> JobOptions<'b> {
        JobOptions {
            reconcile: self.settings.reconcile(),
            output: &self.settings.request.output,
            cache: &self.settings.paths.cache,
            dry_run: self.settings.options.dry_run,
            playlist_id: None,
            job,
        }
    }

    pub(crate) fn audio(&self, item: &WatchItem) -> Option<PathBuf> {
        item.downloaded_audio(&AudioIndex::scan(&self.settings.request.output))
    }
}

pub(crate) struct Adapter<'a, 'b> {
    pub(crate) prepared: &'a Prepared<'a>,
    pub(crate) events: &'a RefCell<&'b mut dyn FnMut(JobEvent)>,
    pub(crate) on_import_event: &'a mut dyn FnMut(JobEvent),
    pub(crate) decide: &'a mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    pub(crate) parked: &'a RefCell<Option<Parked>>,
    pub(crate) cancelled: &'a AtomicBool,
}

impl Operations for Adapter<'_, '_> {
    fn load(&mut self, playlist: &Playlist) -> Result<LoadedSource, JobError> {
        check_cancelled(self.cancelled)?;
        sources::of(playlist.kind).load(self, playlist)
    }

    fn process(
        &mut self,
        _playlist: &Playlist,
        item: &mut WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<(), JobError> {
        check_cancelled(cancelled)?;
        self.parked.replace(None);
        let result = sources::of(item.kind).process(self, item, action, cancelled);
        result.map_err(|error| {
            let Some(parked) = self.parked.replace(None) else {
                return error;
            };
            let stage = Stage::of_decision(parked.kind);
            let question = parked.question();
            (self.events.borrow_mut())(JobEvent::ItemWaiting {
                title: item.title.clone(),
                question: question.clone(),
            });
            JobError::Waiting { stage, question }
        })
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), JobError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}
