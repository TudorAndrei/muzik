//! Durable watchlist jobs. Callers supply source and audio operations.

use super::source::availability;
use super::{
    AudioIndex, Holds, ItemAction, ItemId, Playlist, ReconcileOptions, Repository, Snapshot, Stage,
    WatchItem, Watchlist, cancel_orphans, now, view,
};
use crate::Result;
use crate::jobs::{self, Ending, Kind, NewJob, Status};
use muzik_core::{JobEvent, Severity, Task};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("watchlist job cancelled")]
    Cancelled,
    #[error("{0}")]
    Operation(String),
    #[error("waiting for a choice in the {stage} stage")]
    Waiting { stage: Stage, question: Value },
    #[error("{message}")]
    Failed { stage: Stage, message: String },
}

impl From<String> for JobError {
    fn from(value: String) -> Self {
        Self::Operation(value)
    }
}

impl From<crate::Error> for JobError {
    fn from(error: crate::Error) -> Self {
        Self::Operation(error.to_string())
    }
}

pub struct LoadedSource {
    pub title: Option<String>,
    pub items: Vec<WatchItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingItem {
    pub id: ItemId,
    pub title: String,
}

pub struct Synced {
    pub checked: usize,
    pub errors: usize,
    pub pending: Vec<PendingItem>,
    pub document: Watchlist,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemOutcome {
    Completed { stage: Stage },
    Waiting { stage: Stage, question: Value },
}

/// Implement source lookup and item processing with Rust adapters.
/// `process` records each stage on `item` when the stage ends.
pub trait Operations {
    /// # Errors
    /// Returns an error if the source cannot be read, or `JobError::Cancelled` after a cancel.
    fn load(&mut self, playlist: &Playlist) -> Result<LoadedSource, JobError>;
    /// # Errors
    /// Returns an error if a stage fails, waits for a choice, or is cancelled.
    fn process(
        &mut self,
        playlist: &Playlist,
        item: &mut WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<(), JobError>;
}

/// The running queue job that runs an item. Its status changes with the item result.
#[derive(Clone, Copy)]
pub struct RunningJob<'a> {
    pub id: i64,
    pub params: &'a Value,
}

#[derive(Clone, Copy)]
pub struct JobOptions<'a> {
    pub reconcile: ReconcileOptions<'a>,
    pub output: &'a Path,
    pub cache: &'a Path,
    pub dry_run: bool,
    pub playlist_id: Option<&'a str>,
    pub job: Option<RunningJob<'a>>,
}

/// # Errors
/// Returns an error if the watchlist has no matching source, cannot be saved, or the job is cancelled.
pub fn sync(
    repository: &Repository,
    options: JobOptions<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<Synced, JobError> {
    let mut draft = repository.snapshot()?;
    let mut ids: Vec<String> = draft
        .document
        .playlists
        .iter()
        .map(|playlist| playlist.playlist_id.clone())
        .collect();
    if ids.is_empty() {
        return Err(JobError::Operation(
            "Add a playlist before you refresh.".into(),
        ));
    }
    if let Some(only) = options.playlist_id {
        ids.retain(|id| id == only);
        if ids.is_empty() {
            return Err(JobError::Operation(format!(
                "The source {only} is not in the watchlist."
            )));
        }
    }
    on_event(JobEvent::ProgressStarted {
        task: Task::WatchlistRefresh,
        description: "Checking watchlist playlists.".into(),
        total: u64::try_from(ids.len()).ok(),
    });
    let mut errors: usize = 0;
    let mut loaded_ids = Vec::new();
    for id in &ids {
        check_cancelled(cancelled)?;
        let Some(playlist) = draft.document.playlist(id) else {
            continue;
        };
        match operations.load(playlist) {
            Err(JobError::Cancelled) => return Err(JobError::Cancelled),
            Err(error) => {
                let message = error.to_string();
                write(repository, options.dry_run, &mut draft, |document, _| {
                    if let Some(saved) = document.playlist_mut(id) {
                        saved.last_checked_at = Some(now());
                        saved.last_error = Some(message.clone());
                    }
                    Ok(())
                })?;
                on_event(JobEvent::WatchlistSaved);
                errors = errors.saturating_add(1);
                on_event(JobEvent::Message {
                    message,
                    severity: Severity::Error,
                });
            }
            Ok(loaded) => {
                check_cancelled(cancelled)?;
                write(
                    repository,
                    options.dry_run,
                    &mut draft,
                    |document, holds| {
                        if let Some(saved) = document.playlist_mut(id) {
                            if let Some(title) =
                                loaded.title.filter(|title| !title.trim().is_empty())
                            {
                                saved.title = Some(title);
                            }
                            saved.merge_items(loaded.items);
                            saved.last_checked_at = Some(now());
                            saved.last_error = None;
                        }
                        document.reconcile(options.reconcile, holds)
                    },
                )?;
                on_event(JobEvent::WatchlistSaved);
                loaded_ids.push(id.clone());
            }
        }
        on_event(JobEvent::ProgressAdvanced {
            task: Task::WatchlistRefresh,
            completed: None,
            total: None,
        });
    }
    let mut pending = Vec::new();
    for id in loaded_ids {
        let items = pending_of(&draft, &id);
        on_event(JobEvent::message(format!(
            "Playlist {id} has {} pending item(s).",
            items.len()
        )));
        pending.extend(items);
    }
    Ok(Synced {
        checked: ids.len(),
        errors,
        pending,
        document: draft.document,
    })
}

fn pending_of(snapshot: &Snapshot, playlist_id: &str) -> Vec<PendingItem> {
    snapshot
        .document
        .playlist(playlist_id)
        .map(|playlist| {
            playlist
                .pending(&snapshot.holds)
                .into_iter()
                .map(|(id, item)| PendingItem {
                    id,
                    title: item.title.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// # Errors
/// Returns an error if the sync fails, the watchlist cannot be read, or the job is cancelled.
pub fn refresh(
    repository: &Repository,
    options: JobOptions<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<Value, JobError> {
    let synced = sync(repository, options, operations, cancelled, on_event)?;
    let mut completed: usize = 0;
    let mut failed: usize = 0;
    let mut waiting: usize = 0;
    if !options.dry_run {
        let options = JobOptions {
            job: None,
            ..options
        };
        for item in &synced.pending {
            check_cancelled(cancelled)?;
            match run_item(
                repository,
                options,
                &item.id,
                ItemAction::Run,
                operations,
                cancelled,
            ) {
                Ok(ItemOutcome::Completed { .. }) => completed = completed.saturating_add(1),
                Ok(ItemOutcome::Waiting { .. }) => waiting = waiting.saturating_add(1),
                Err(JobError::Cancelled) => return Err(JobError::Cancelled),
                Err(_) => failed = failed.saturating_add(1),
            }
            on_event(JobEvent::WatchlistSaved);
        }
    }
    check_cancelled(cancelled)?;
    on_event(JobEvent::ProgressFinished {
        task: Task::WatchlistRefresh,
        success: failed == 0 && synced.errors == 0,
    });
    let snapshot = if options.dry_run {
        Snapshot::new(synced.document, repository.snapshot()?.holds)
    } else {
        repository.snapshot()?
    };
    let summary = json!({"playlists_checked":synced.checked, "pending_videos":synced.pending.len(), "completed_videos":completed, "failed_videos":failed, "waiting_videos":waiting, "playlist_errors":synced.errors});
    Ok(json!({"summary":summary, "watchlist":view(&snapshot, options.output, options.cache)}))
}

/// # Errors
/// Returns an error if the item is missing or unavailable, its stage fails, or the job is cancelled.
pub fn action(
    repository: &Repository,
    options: JobOptions<'_>,
    id: &ItemId,
    action: ItemAction,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
) -> Result<Value, JobError> {
    if options.dry_run {
        check_cancelled(cancelled)?;
        let snapshot = repository.snapshot()?;
        let item = find_item(&snapshot.document, id)?;
        check_available(item, action, options.output)?;
        return Ok(
            json!({"action":{"action":action,"planned_stage":action.stage(),"dry_run":true},
            "watchlist":view(&snapshot, options.output, options.cache)}),
        );
    }
    let outcome = run_item(repository, options, id, action, operations, cancelled)?;
    let summary = match outcome {
        ItemOutcome::Completed { stage } => json!({"action":action,"completed_stage":stage}),
        ItemOutcome::Waiting { stage, .. } => json!({"action":action,"waiting_stage":stage}),
    };
    Ok(
        json!({"action":summary, "watchlist":view(&repository.snapshot()?, options.output, options.cache)}),
    )
}

/// Runs `action` on one item. The item result and the status of `options.job` are saved
/// in one transaction.
/// # Errors
/// Returns an error if the item is missing or unavailable, its stage fails, or the job is cancelled.
pub fn run_item(
    repository: &Repository,
    options: JobOptions<'_>,
    id: &ItemId,
    action: ItemAction,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
) -> Result<ItemOutcome, JobError> {
    let stage = action.stage();
    let key = id.to_string();
    let job_id = options.job.map(|job| job.id);
    let (playlist, mut run) = repository.update_with(|document, connection| {
        if cancelled.load(Ordering::SeqCst) {
            end_job(connection, options, Ending::Cancelled)?;
            return Ok(Err(JobError::Cancelled));
        }
        jobs::cancel_waiting_on(connection, &key, job_id)?;
        let (playlist, index) = match find_item_mut(document, id) {
            Ok(found) => found,
            Err(error) => {
                end_job(connection, options, Ending::Failed(&error.to_string()))?;
                return Ok(Err(error.into()));
            }
        };
        let source = playlist.clone();
        let saved = playlist.items.get_mut(index).ok_or(MISSING_VIDEO)?;
        if let Err(error) = check_available(saved, action, options.output) {
            let message = error.to_string();
            saved.fail_before_start(stage, action, &message);
            end_job(connection, options, Ending::Failed(&message))?;
            return Ok(Err(error.into()));
        }
        Ok(Ok((source, saved.clone())))
    })??;
    let result = operations.process(&playlist, &mut run, action, cancelled);
    let result = match result {
        Ok(()) if cancelled.load(Ordering::SeqCst) => Err(JobError::Cancelled),
        other => other,
    };
    let saved = repository.update_with(|document, connection| {
        let Ok((playlist, index)) = find_item_mut(document, id) else {
            end_job(connection, options, Ending::Failed(GONE_VIDEO))?;
            return Ok(false);
        };
        match &result {
            Ok(()) => {
                run.record_action(action);
                let holds = Holds::read(connection)?;
                let ids = playlist.item_ids();
                let key = run.key().map(str::to_owned);
                for (slot, (other_id, other)) in
                    ids.iter().zip(playlist.items.iter_mut()).enumerate()
                {
                    let copy = key.is_some()
                        && other.key() == key.as_deref()
                        && holds.get(other_id).is_none();
                    if slot == index || copy {
                        other.record_run(&run);
                    }
                }
                if let Some(key) = key {
                    playlist.mark_processed(&key, run.is_done());
                }
                end_job(connection, options, Ending::Done)?;
                return Ok(true);
            }
            Err(JobError::Waiting {
                stage: waiting,
                question,
            }) => {
                run.record_action(action);
                let title = &playlist.items.get(index).ok_or(MISSING_VIDEO)?.title;
                park(connection, options, id, title, *waiting, question)?;
                end_job(connection, options, Ending::Done)?;
            }
            Err(JobError::Cancelled) => {
                end_job(connection, options, Ending::Cancelled)?;
            }
            Err(error) => {
                let failed = match error {
                    JobError::Failed { stage, .. } => *stage,
                    _ => stage,
                };
                let message = error.to_string();
                run.fail(failed, action, &message);
                end_job(connection, options, Ending::Failed(&message))?;
            }
        }
        playlist
            .items
            .get_mut(index)
            .ok_or(MISSING_VIDEO)?
            .record_run(&run);
        Ok(true)
    })?;
    match result {
        Err(JobError::Cancelled) => Err(JobError::Cancelled),
        _ if !saved => Err(JobError::Operation(GONE_VIDEO.into())),
        Ok(()) => Ok(ItemOutcome::Completed { stage }),
        Err(JobError::Waiting { stage, question }) => Ok(ItemOutcome::Waiting { stage, question }),
        Err(error) => Err(error),
    }
}

fn end_job(connection: &Connection, options: JobOptions<'_>, ending: Ending<'_>) -> Result<()> {
    if let Some(job) = options.job {
        jobs::end_on(connection, job.id, ending)?;
    }
    Ok(())
}

fn park(
    connection: &Connection,
    options: JobOptions<'_>,
    id: &ItemId,
    title: &str,
    stage: Stage,
    question: &Value,
) -> Result<()> {
    let mut params = options.job.map_or(Value::Null, |job| job.params.clone());
    id.write(&mut params)?;
    if let Some(fields) = params.as_object_mut() {
        fields.insert("action".into(), json!(stage.resume_action()));
    }
    jobs::park_on(
        connection,
        &NewJob {
            kind: Kind::Item,
            item_key: &id.to_string(),
            title,
            params: &params,
        },
        question,
    )?;
    Ok(())
}

const MISSING_PLAYLIST: &str = "The selected playlist is no longer available.";
const MISSING_VIDEO: &str = "The selected video is no longer available.";
const GONE_VIDEO: &str =
    "The video left its playlist while it was processed. Muzik could not save the result.";

/// Fails a running item job that ends before `run_item` starts, and saves the failure on
/// the stage of `action` in the same transaction unless that stage is done.
/// # Errors
/// Returns an error if the watchlist or the job cannot be saved.
pub fn abandon(
    repository: &Repository,
    job_id: i64,
    id: &ItemId,
    action: ItemAction,
    message: &str,
) -> Result<()> {
    repository.update_with(|document, connection| {
        if jobs::end_on(connection, job_id, Ending::Failed(message))?
            && let Ok((playlist, index)) = find_item_mut(document, id)
            && let Some(item) = playlist.items.get_mut(index)
        {
            item.fail_before_start(action.stage(), action, message);
        }
        Ok(())
    })
}

/// Brings the jobs back in line with the items after a stop.
///
/// Running jobs return to the queue, each item keeps one open job, jobs of missing items
/// end, and a waiting job whose input is gone ends so that its item can run again.
/// `output_of` gives the download folder for the parameters of a job.
/// # Errors
/// Returns an error if the watchlist or the jobs cannot be read or saved.
pub fn repair(
    repository: &Repository,
    output_of: impl Fn(&Value) -> Option<PathBuf>,
) -> Result<usize> {
    repository.update_with(|document, connection| {
        let mut changed = jobs::recover_on(connection)?;
        changed = changed.saturating_add(jobs::cancel_duplicates_on(connection)?);
        changed = changed.saturating_add(cancel_orphans(connection, document)?);
        for job in jobs::open_items_on(connection)? {
            if job.status != Status::Waiting {
                continue;
            }
            let Ok(id) = job.item_key.parse::<ItemId>() else {
                continue;
            };
            let Some(output) = output_of(&job.params) else {
                continue;
            };
            let action = job
                .params
                .get("action")
                .and_then(Value::as_str)
                .and_then(|action| action.parse::<ItemAction>().ok())
                .unwrap_or(ItemAction::Run);
            let answerable = find_item(document, &id)
                .is_ok_and(|item| check_available(item, action, &output).is_ok());
            if !answerable && jobs::cancel_open_on(connection, job.id)? {
                changed = changed.saturating_add(1);
            }
        }
        Ok(changed)
    })
}

fn find_item<'a>(document: &'a Watchlist, id: &ItemId) -> Result<&'a WatchItem> {
    let playlist = document
        .playlists
        .iter()
        .find(|playlist| playlist.playlist_id == id.playlist_id)
        .ok_or(MISSING_PLAYLIST)?;
    let item = playlist
        .find(id)
        .and_then(|index| playlist.items.get(index))
        .ok_or(MISSING_VIDEO)?;
    Ok(item)
}

fn find_item_mut<'a>(
    document: &'a mut Watchlist,
    id: &ItemId,
) -> Result<(&'a mut Playlist, usize)> {
    let playlist = document
        .playlists
        .iter_mut()
        .find(|playlist| playlist.playlist_id == id.playlist_id)
        .ok_or(MISSING_PLAYLIST)?;
    let item = playlist.find(id).ok_or(MISSING_VIDEO)?;
    Ok((playlist, item))
}

fn check_available(item: &WatchItem, action: ItemAction, output: &Path) -> Result<()> {
    let audio = item.downloaded_audio(&AudioIndex::scan(output));
    let (enabled, reason) = availability(item, action, audio.as_deref());
    if enabled {
        Ok(())
    } else {
        Err(reason.unwrap_or("This command is not available.").into())
    }
}

fn write<T>(
    repository: &Repository,
    dry_run: bool,
    draft: &mut Snapshot,
    change: impl FnOnce(&mut Watchlist, &Holds) -> Result<T>,
) -> Result<T, JobError> {
    if dry_run {
        return Ok(change(&mut draft.document, &draft.holds)?);
    }
    let (result, snapshot) = repository.update_with(|document, connection| {
        let holds = Holds::read(connection)?;
        let result = change(document, &holds)?;
        Ok((result, Snapshot::new(document.clone(), holds)))
    })?;
    *draft = snapshot;
    Ok(result)
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), JobError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}
