//! Durable watchlist jobs. Callers supply source and audio operations.

use super::view::availability;
use super::{
    now, reconcile, view, AudioIndex, ItemAction, Playlist, ReconcileOptions, Repository, Stage,
    StageStatus, WatchItem, Watchlist,
};
use serde_json::{json, Value};
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

pub struct LoadedSource {
    pub title: Option<String>,
    pub items: Vec<WatchItem>,
}

pub struct ItemSelection<'a> {
    pub playlist_id: &'a str,
    pub position: u64,
    pub video_id: Option<&'a str>,
    pub action: ItemAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingItem {
    pub playlist_id: String,
    pub position: u64,
    pub video_id: Option<String>,
    pub title: String,
}

impl PendingItem {
    pub fn selection(&self, action: ItemAction) -> ItemSelection<'_> {
        ItemSelection {
            playlist_id: &self.playlist_id,
            position: self.position,
            video_id: self.video_id.as_deref(),
            action,
        }
    }
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
    Waiting { stage: Stage },
}

/// Implement source lookup and item processing with Rust adapters.
/// `process` returns the item with its final stage state.
pub trait Operations {
    fn load(&mut self, playlist: &Playlist) -> Result<LoadedSource, JobError>;
    fn process(
        &mut self,
        playlist: &Playlist,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError>;
}

#[derive(Clone, Copy)]
pub struct JobOptions<'a> {
    pub reconcile: ReconcileOptions<'a>,
    pub output: &'a Path,
    pub cache: &'a Path,
    pub dry_run: bool,
    pub playlist_id: Option<&'a str>,
}

pub fn sync(
    repository: &Repository,
    options: JobOptions<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
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
    emit(
        on_event,
        "progress_started",
        json!({"task_id":"watchlist-refresh", "description":"Checking watchlist playlists.", "total":ids.len()}),
    );
    let mut errors = 0;
    let mut loaded_ids = Vec::new();
    for id in &ids {
        check_cancelled(cancelled)?;
        let Some(playlist) = draft.playlist(id).cloned() else {
            continue;
        };
        match operations.load(&playlist) {
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
                emit(on_event, "watchlist_saved", json!({"playlist_id":id}));
                errors += 1;
                emit(
                    on_event,
                    "message",
                    json!({"message":message, "severity":"error"}),
                );
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
                    reconcile_keeping_running(document, options.reconcile)
                })?;
                emit(on_event, "watchlist_saved", json!({"playlist_id":id}));
                loaded_ids.push(id.clone());
            }
        }
        emit(
            on_event,
            "progress_advanced",
            json!({"task_id":"watchlist-refresh"}),
        );
    }
    let mut pending = Vec::new();
    for id in loaded_ids {
        let items: Vec<PendingItem> = draft
            .playlist(&id)
            .map(|playlist| {
                playlist
                    .pending()
                    .into_iter()
                    .map(|item| PendingItem {
                        playlist_id: id.clone(),
                        position: item.position,
                        video_id: item.video_id.clone(),
                        title: item.title.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        emit(
            on_event,
            "message",
            json!({"message":format!("Playlist {id} has {} pending item(s).", items.len())}),
        );
        pending.extend(items);
    }
    Ok(Synced {
        checked: ids.len(),
        errors,
        pending,
        document: draft,
    })
}

pub fn refresh(
    repository: &Repository,
    options: JobOptions<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
) -> Result<Value, JobError> {
    let synced = sync(repository, options, operations, cancelled, on_event)?;
    let mut completed = 0;
    let mut failed = 0;
    let mut waiting = 0;
    if !options.dry_run {
        for item in &synced.pending {
            check_cancelled(cancelled)?;
            match run_item(
                repository,
                options,
                item.selection(ItemAction::Run),
                operations,
                cancelled,
            ) {
                Ok(ItemOutcome::Completed { .. }) => completed += 1,
                Ok(ItemOutcome::Waiting { .. }) => waiting += 1,
                Err(JobError::Cancelled) => return Err(JobError::Cancelled),
                Err(_) => failed += 1,
            }
            emit(
                on_event,
                "watchlist_saved",
                json!({"playlist_id":item.playlist_id}),
            );
        }
    }
    check_cancelled(cancelled)?;
    emit(
        on_event,
        "progress_finished",
        json!({"task_id":"watchlist-refresh", "success":failed == 0 && synced.errors == 0}),
    );
    let document = if options.dry_run {
        synced.document
    } else {
        repository.load()?
    };
    let summary = json!({"playlists_checked":synced.checked, "pending_videos":synced.pending.len(), "completed_videos":completed, "failed_videos":failed, "waiting_videos":waiting, "playlist_errors":synced.errors});
    Ok(json!({"summary":summary, "watchlist":view(&document, options.output, options.cache)?}))
}

pub fn action(
    repository: &Repository,
    options: JobOptions<'_>,
    selection: ItemSelection<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
) -> Result<Value, JobError> {
    let action = selection.action;
    if options.dry_run {
        check_cancelled(cancelled)?;
        let document = repository.load()?;
        let (playlist, item) = find_item(&document, &selection)?;
        check_available(
            &document.playlists[playlist].items[item],
            action,
            options.output,
        )?;
        return Ok(
            json!({"action":{"action":action,"planned_stage":action.stage(),"dry_run":true},
            "watchlist":view(&document, options.output, options.cache)?}),
        );
    }
    let outcome = run_item(repository, options, selection, operations, cancelled)?;
    let summary = match outcome {
        ItemOutcome::Completed { stage } => json!({"action":action,"completed_stage":stage}),
        ItemOutcome::Waiting { stage } => json!({"action":action,"waiting_stage":stage}),
    };
    Ok(
        json!({"action":summary, "watchlist":view(&repository.load()?, options.output, options.cache)?}),
    )
}

pub fn run_item(
    repository: &Repository,
    options: JobOptions<'_>,
    selection: ItemSelection<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
) -> Result<ItemOutcome, JobError> {
    check_cancelled(cancelled)?;
    let action = selection.action;
    let stage = action.stage();
    let (playlist, item) = repository.update(|document| {
        let (playlist, index) = find_item(document, &selection)?;
        let item = document.playlists[playlist].items[index].clone();
        check_available(&item, action, options.output)?;
        let source = document.playlists[playlist].clone();
        document.playlists[playlist].items[index].start(action);
        Ok((source, item))
    })?;
    let result = operations.process(&playlist, &item, action, cancelled);
    let result = match result {
        Ok(_) if cancelled.load(Ordering::SeqCst) => Err(JobError::Cancelled),
        other => other,
    };
    repository.update(|document| {
        let Ok((playlist, index)) = find_item(document, &selection) else {
            return Ok(());
        };
        let playlist = &mut document.playlists[playlist];
        let mut card = item.clone();
        match &result {
            Ok(updated) => {
                card = updated.clone();
                card.position = selection.position;
                card.finish(stage, action);
                if let Some(key) = item.key() {
                    for other in playlist
                        .items
                        .iter_mut()
                        .filter(|other| other.key() == Some(key))
                    {
                        let position = other.position;
                        *other = card.clone();
                        other.position = position;
                    }
                    playlist.mark_processed(key, card.is_done());
                }
            }
            Err(JobError::Waiting {
                stage: waiting,
                question,
            }) => card.wait(*waiting, action, question.clone()),
            Err(JobError::Cancelled) => {}
            Err(error) => {
                let failed = match error {
                    JobError::Failed { stage, .. } => *stage,
                    _ => stage,
                };
                card.fail(failed, action, &error.to_string());
            }
        }
        playlist.items[index] = card;
        Ok(())
    })?;
    match result {
        Ok(_) => Ok(ItemOutcome::Completed { stage }),
        Err(JobError::Waiting { stage, .. }) => Ok(ItemOutcome::Waiting { stage }),
        Err(error) => Err(error),
    }
}

fn find_item(
    document: &Watchlist,
    selection: &ItemSelection<'_>,
) -> Result<(usize, usize), String> {
    let playlist = document
        .playlists
        .iter()
        .position(|playlist| playlist.playlist_id == selection.playlist_id)
        .ok_or("The selected playlist is no longer available.")?;
    let item = document.playlists[playlist]
        .find(selection.position, selection.video_id)
        .ok_or("The selected video is no longer available.")?;
    Ok((playlist, item))
}

fn check_available(item: &WatchItem, action: ItemAction, output: &Path) -> Result<(), String> {
    let audio = item.downloaded_audio(&AudioIndex::scan(output));
    let (enabled, reason) = availability(item, action, audio.as_deref());
    if enabled {
        Ok(())
    } else {
        Err(reason
            .unwrap_or("This command is not available.")
            .to_owned())
    }
}

fn write<T>(
    repository: &Repository,
    dry_run: bool,
    draft: &mut Watchlist,
    change: impl FnOnce(&mut Watchlist) -> Result<T, String>,
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

fn reconcile_keeping_running(
    document: &mut Watchlist,
    options: ReconcileOptions<'_>,
) -> Result<(), String> {
    let mut running = Vec::new();
    for (playlist_index, playlist) in document.playlists.iter().enumerate() {
        for (item_index, item) in playlist.items.iter().enumerate() {
            for stage in Stage::ALL {
                if item.status(*stage) == StageStatus::Running {
                    running.push((
                        playlist_index,
                        item_index,
                        *stage,
                        item.stage(*stage).clone(),
                    ));
                }
            }
        }
    }
    reconcile(document, options)?;
    for (playlist_index, item_index, stage, record) in running {
        if let Some(item) = document
            .playlists
            .get_mut(playlist_index)
            .and_then(|playlist| playlist.items.get_mut(item_index))
        {
            *item.stage_mut(stage) = record;
        }
    }
    Ok(())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), JobError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}

fn emit(on_event: &mut dyn FnMut(Value), event: &str, data: Value) {
    on_event(json!({"event":event,"data":data}));
}
