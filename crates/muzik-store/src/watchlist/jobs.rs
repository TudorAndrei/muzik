//! Durable watchlist jobs. Callers supply source and audio operations.

use super::source::availability;
use super::{
    AudioIndex, ItemAction, ItemId, Playlist, ReconcileOptions, Repository, Stage, StageStatus,
    WatchItem, Watchlist, now, view,
};
use crate::Result;
use muzik_core::{JobEvent, Severity, Task};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::Path;
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
/// `park` runs in the transaction that saves a waiting stage.
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
    /// # Errors
    /// Returns an error if the waiting job cannot be saved.
    fn park(
        &mut self,
        _connection: &Connection,
        _id: &ItemId,
        _title: &str,
        _stage: Stage,
        _question: &Value,
    ) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct JobOptions<'a> {
    pub reconcile: ReconcileOptions<'a>,
    pub output: &'a Path,
    pub cache: &'a Path,
    pub dry_run: bool,
    pub playlist_id: Option<&'a str>,
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
    let mut draft = repository.load()?;
    let mut ids: Vec<String> = draft
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
        let Some(playlist) = draft.playlist(id) else {
            continue;
        };
        match operations.load(playlist) {
            Err(JobError::Cancelled) => return Err(JobError::Cancelled),
            Err(error) => {
                let message = error.to_string();
                write(repository, options.dry_run, &mut draft, |document| {
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
                write(repository, options.dry_run, &mut draft, |document| {
                    if let Some(saved) = document.playlist_mut(id) {
                        if let Some(title) = loaded.title.filter(|title| !title.trim().is_empty()) {
                            saved.title = Some(title);
                        }
                        saved.merge_items(loaded.items);
                        saved.last_checked_at = Some(now());
                        saved.last_error = None;
                    }
                    document.reconcile_keeping_running(options.reconcile)
                })?;
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
        let items: Vec<PendingItem> = draft
            .playlist(&id)
            .map(|playlist| {
                playlist
                    .pending()
                    .into_iter()
                    .map(|(id, item)| PendingItem {
                        id,
                        title: item.title.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
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
        document: draft,
    })
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
    let document = if options.dry_run {
        synced.document
    } else {
        repository.load()?
    };
    let summary = json!({"playlists_checked":synced.checked, "pending_videos":synced.pending.len(), "completed_videos":completed, "failed_videos":failed, "waiting_videos":waiting, "playlist_errors":synced.errors});
    Ok(json!({"summary":summary, "watchlist":view(&document, options.output, options.cache)}))
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
        let document = repository.load()?;
        let item = find_item(&document, id)?;
        check_available(item, action, options.output)?;
        return Ok(
            json!({"action":{"action":action,"planned_stage":action.stage(),"dry_run":true},
            "watchlist":view(&document, options.output, options.cache)}),
        );
    }
    let outcome = run_item(repository, options, id, action, operations, cancelled)?;
    let summary = match outcome {
        ItemOutcome::Completed { stage } => json!({"action":action,"completed_stage":stage}),
        ItemOutcome::Waiting { stage, .. } => json!({"action":action,"waiting_stage":stage}),
    };
    Ok(
        json!({"action":summary, "watchlist":view(&repository.load()?, options.output, options.cache)}),
    )
}

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
    check_cancelled(cancelled)?;
    let stage = action.stage();
    let (playlist, mut run) = repository.update(|document| {
        let (playlist, index) = find_item_mut(document, id)?;
        let source = playlist.clone();
        let saved = playlist.items.get_mut(index).ok_or(MISSING_VIDEO)?;
        if let Err(error) = check_available(saved, action, options.output) {
            saved.fail_held(&error.to_string())?;
            return Ok(Err(error));
        }
        let item = saved.clone();
        saved.start(action)?;
        Ok(Ok((source, item)))
    })??;
    let result = operations.process(&playlist, &mut run, action, cancelled);
    let result = match result {
        Ok(()) if cancelled.load(Ordering::SeqCst) => Err(JobError::Cancelled),
        other => other,
    };
    let saved = repository.update_with(|document, connection| {
        let Ok((playlist, index)) = find_item_mut(document, id) else {
            return Ok(false);
        };
        match &result {
            Ok(()) => {
                run.release_held()?;
                run.finish(stage, action)?;
                let key = run.key().map(str::to_owned);
                for (slot, other) in playlist.items.iter_mut().enumerate() {
                    let copy = key.is_some()
                        && other.key() == key.as_deref()
                        && !other.statuses().any(StageStatus::is_held);
                    if slot == index || copy {
                        other.record_run(&run);
                    }
                }
                if let Some(key) = key {
                    playlist.mark_processed(&key, run.is_done());
                }
                return Ok(true);
            }
            Err(JobError::Waiting {
                stage: waiting,
                question,
            }) => {
                run.release_held()?;
                run.wait(*waiting, action, question.clone())?;
                let title = &playlist.items.get(index).ok_or(MISSING_VIDEO)?.title;
                operations.park(connection, id, title, *waiting, question)?;
            }
            Err(JobError::Cancelled) => {}
            Err(error) => {
                let failed = match error {
                    JobError::Failed { stage, .. } => *stage,
                    _ => stage,
                };
                run.release_held()?;
                run.fail(failed, action, &error.to_string())?;
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

const MISSING_PLAYLIST: &str = "The selected playlist is no longer available.";
const MISSING_VIDEO: &str = "The selected video is no longer available.";
const GONE_VIDEO: &str =
    "The video left its playlist while it was processed. Muzik could not save the result.";

/// # Errors
/// Returns an error if the watchlist cannot be saved.
pub fn abandon(repository: &Repository, id: &ItemId, message: &str) -> Result<()> {
    repository.update(|document| {
        if let Ok((playlist, index)) = find_item_mut(document, id)
            && let Some(item) = playlist.items.get_mut(index)
        {
            item.fail_held(message)?;
        }
        Ok(())
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
    draft: &mut Watchlist,
    change: impl FnOnce(&mut Watchlist) -> Result<T>,
) -> Result<T, JobError> {
    if dry_run {
        return Ok(change(draft)?);
    }
    let (result, document) = repository.update(|document| {
        let result = change(document)?;
        Ok((result, document.clone()))
    })?;
    *draft = document;
    Ok(result)
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), JobError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}
