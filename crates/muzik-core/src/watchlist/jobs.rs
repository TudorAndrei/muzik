//! Durable watchlist jobs. Callers supply source and audio operations.

use super::view::availability;
use super::{
    is_unavailable, reconcile, stage_status, stage_statuses, view, ItemAction, ReconcileOptions,
    Repository, SourceKind, Stage, StageStatus,
};
use chrono::{Local, SecondsFormat};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
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
    pub items: Vec<Value>,
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
    pub document: Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemOutcome {
    Completed { stage: Stage },
    Waiting { stage: Stage },
}

/// Implement source lookup and item processing with Rust adapters.
/// `process` returns the item with its final stage state.
pub trait Operations {
    fn load(&mut self, playlist: &Value) -> Result<LoadedSource, JobError>;
    fn process(
        &mut self,
        playlist: &Value,
        item: &Value,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<Value, JobError>;
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
    let mut playlists = draft["playlists"].as_array().cloned().unwrap_or_default();
    if playlists.is_empty() {
        return Err(JobError::Operation(
            "Add a playlist before you refresh.".into(),
        ));
    }
    if let Some(only) = options.playlist_id {
        playlists.retain(|playlist| playlist["playlist_id"] == only);
        if playlists.is_empty() {
            return Err(JobError::Operation(format!(
                "The source {only} is not in the watchlist."
            )));
        }
    }
    emit(
        on_event,
        "progress_started",
        json!({"task_id":"watchlist-refresh", "description":"Checking watchlist playlists.", "total":playlists.len()}),
    );
    let mut errors = 0;
    let mut loaded_ids = Vec::new();
    for playlist in &playlists {
        check_cancelled(cancelled)?;
        let id = playlist["playlist_id"].as_str().unwrap_or("").to_owned();
        match operations.load(playlist) {
            Err(JobError::Cancelled) => return Err(JobError::Cancelled),
            Err(error) => {
                let message = error.to_string();
                write(repository, options.dry_run, &mut draft, |document| {
                    if let Some(saved) = find_playlist(document, &id) {
                        saved["last_checked_at"] = json!(now());
                        saved["last_error"] = json!(message);
                    }
                    Ok(())
                })?;
                emit(on_event, "watchlist_saved", json!({"playlist_id":id}));
                errors += 1;
                emit(
                    on_event,
                    "message",
                    json!({"message":error.to_string(), "severity":"error"}),
                );
            }
            Ok(loaded) => {
                check_cancelled(cancelled)?;
                write(repository, options.dry_run, &mut draft, |document| {
                    if let Some(saved) = find_playlist(document, &id) {
                        if let Some(title) = loaded.title.filter(|title| !title.trim().is_empty()) {
                            saved["title"] = json!(title);
                        }
                        saved["items"] = json!(merge_items(&saved["items"], loaded.items));
                        saved["last_checked_at"] = json!(now());
                        saved["last_error"] = Value::Null;
                    }
                    reconcile_keeping_running(document, options.reconcile)
                })?;
                emit(on_event, "watchlist_saved", json!({"playlist_id":id}));
                loaded_ids.push(id);
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
        let items = pending_items(&draft, &id);
        emit(
            on_event,
            "message",
            json!({"message":format!("Playlist {id} has {} pending item(s).", items.len())}),
        );
        pending.extend(items);
    }
    Ok(Synced {
        checked: playlists.len(),
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
    Ok(json!({"summary":summary, "watchlist":view(document, options.output, options.cache)?}))
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
        let (_, _, item) = find_item(&document, &selection)?;
        check_available(&item, action, options.output)?;
        return Ok(
            json!({"action":{"action":action,"planned_stage":action.stage(),"dry_run":true},
            "watchlist":view(document, options.output, options.cache)?}),
        );
    }
    let outcome = run_item(repository, options, selection, operations, cancelled)?;
    let summary = match outcome {
        ItemOutcome::Completed { stage } => json!({"action":action,"completed_stage":stage}),
        ItemOutcome::Waiting { stage } => json!({"action":action,"waiting_stage":stage}),
    };
    Ok(
        json!({"action":summary, "watchlist":view(repository.load()?, options.output, options.cache)?}),
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
        let (playlist_index, item_index, item) = find_item(document, &selection)?;
        check_available(&item, action, options.output)?;
        let playlist = document["playlists"][playlist_index].clone();
        let target = &mut document["playlists"][playlist_index]["items"][item_index];
        target["last_action"] = json!(action);
        target["last_error"] = Value::Null;
        set_stage(target, stage, StageStatus::Running, None);
        Ok((playlist, item))
    })?;
    let result = operations.process(&playlist, &item, action, cancelled);
    let result = match result {
        Ok(_) if cancelled.load(Ordering::SeqCst) => Err(JobError::Cancelled),
        other => other,
    };
    let key = item_key(&item).map(str::to_owned);
    repository.update(|document| {
        let Ok((playlist_index, item_index, _)) = find_item(document, &selection) else {
            return Ok(());
        };
        let target = &mut document["playlists"][playlist_index]["items"][item_index];
        match &result {
            Ok(updated) => {
                *target = updated.clone();
                target["position"] = json!(selection.position);
                target["last_action"] = json!(action);
                target["last_error"] = Value::Null;
                if stage_status(target, stage) == Some(StageStatus::Running) {
                    set_stage(target, stage, StageStatus::Complete, None);
                }
                if let Some(key) = &key {
                    let finished = all_done(target);
                    let updated = target.clone();
                    let playlist = &mut document["playlists"][playlist_index];
                    if let Some(items) = playlist["items"].as_array_mut() {
                        for card in items
                            .iter_mut()
                            .filter(|card| item_key(card) == Some(key.as_str()))
                        {
                            let position = card["position"].clone();
                            *card = updated.clone();
                            card["position"] = position;
                        }
                    }
                    if finished {
                        add_processed(playlist, key);
                    } else {
                        remove_processed(playlist, key);
                    }
                }
            }
            Err(JobError::Waiting {
                stage: waiting,
                question,
            }) => {
                *target = item.clone();
                mark_waiting(target, *waiting, action, question);
            }
            Err(JobError::Cancelled) => *target = item.clone(),
            Err(error) => {
                *target = item.clone();
                let failed = match error {
                    JobError::Failed { stage, .. } => *stage,
                    _ => stage,
                };
                mark_failed(target, failed, action, &error.to_string());
            }
        }
        Ok(())
    })?;
    match result {
        Ok(_) => Ok(ItemOutcome::Completed { stage }),
        Err(JobError::Waiting { stage, .. }) => Ok(ItemOutcome::Waiting { stage }),
        Err(error) => Err(error),
    }
}

fn set_stage(item: &mut Value, stage: Stage, status: StageStatus, error: Option<&str>) {
    item["stages"][stage.as_ref()] =
        json!({"status":status, "updated_at":now(), "path":null, "error":error});
}

fn find_playlist<'a>(document: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    document["playlists"]
        .as_array_mut()?
        .iter_mut()
        .find(|playlist| playlist["playlist_id"] == id)
}

fn find_item(
    document: &Value,
    selection: &ItemSelection<'_>,
) -> Result<(usize, usize, Value), String> {
    let playlist_index = document["playlists"]
        .as_array()
        .and_then(|playlists| {
            playlists
                .iter()
                .position(|playlist| playlist["playlist_id"] == selection.playlist_id)
        })
        .ok_or("The selected playlist is no longer available.")?;
    let item_index = document["playlists"][playlist_index]["items"]
        .as_array()
        .and_then(|items| {
            items.iter().position(|item| {
                item["position"] == selection.position
                    && item["video_id"].as_str() == selection.video_id
            })
        })
        .ok_or("The selected video is no longer available.")?;
    let item = document["playlists"][playlist_index]["items"][item_index].clone();
    Ok((playlist_index, item_index, item))
}

fn check_available(item: &Value, action: ItemAction, output: &Path) -> Result<(), String> {
    let (enabled, reason) = availability(item, action, output);
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
    draft: &mut Value,
    change: impl FnOnce(&mut Value) -> Result<T, String>,
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
    document: &mut Value,
    options: ReconcileOptions<'_>,
) -> Result<(), String> {
    let mut running = Vec::new();
    for (playlist_index, playlist) in document["playlists"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        for (item_index, item) in playlist["items"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            for stage in Stage::ALL {
                if stage_status(item, *stage) == Some(StageStatus::Running) {
                    running.push((
                        playlist_index,
                        item_index,
                        *stage,
                        item["stages"][stage.as_ref()].clone(),
                    ));
                }
            }
        }
    }
    reconcile(document, options)?;
    for (playlist_index, item_index, stage, record) in running {
        document["playlists"][playlist_index]["items"][item_index]["stages"][stage.as_ref()] =
            record;
    }
    Ok(())
}

fn remove_processed(playlist: &mut Value, key: &str) {
    if let Some(processed) = playlist["processed_video_ids"].as_array_mut() {
        processed.retain(|value| value != key);
    }
}

fn merge_items(existing: &Value, discovered: Vec<Value>) -> Vec<Value> {
    let mut old = HashMap::<(String, usize), Value>::new();
    let mut counts = HashMap::<String, usize>::new();
    for item in existing.as_array().into_iter().flatten() {
        let key = item_key(item).unwrap_or("").to_owned();
        let occurrence = counts.entry(key.clone()).or_default();
        old.insert((key, *occurrence), item.clone());
        *occurrence += 1;
    }
    counts.clear();
    discovered
        .into_iter()
        .map(|mut item| {
            let key = item_key(&item).unwrap_or("").to_owned();
            let occurrence = counts.entry(key.clone()).or_default();
            if let Some(previous) = old.remove(&(key, *occurrence)) {
                for field in ["stages", "last_action", "last_error"] {
                    item[field] = previous[field].clone();
                }
            }
            *occurrence += 1;
            item
        })
        .collect()
}

fn pending_items(document: &Value, playlist_id: &str) -> Vec<PendingItem> {
    let Some(playlist) = document["playlists"].as_array().and_then(|playlists| {
        playlists
            .iter()
            .find(|playlist| playlist["playlist_id"] == playlist_id)
    }) else {
        return Vec::new();
    };
    pending_ids(playlist)
        .into_iter()
        .filter_map(|key| {
            let item = playlist["items"]
                .as_array()?
                .iter()
                .find(|item| item_key(item) == Some(key.as_str()))?;
            Some(PendingItem {
                playlist_id: playlist_id.to_owned(),
                position: item["position"].as_u64()?,
                video_id: item["video_id"].as_str().map(str::to_owned),
                title: item["title"].as_str().unwrap_or(&key).to_owned(),
            })
        })
        .collect()
}

fn pending_ids(playlist: &Value) -> Vec<String> {
    let processed: HashSet<_> = playlist["processed_video_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let mut seen = HashSet::new();
    playlist["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| !is_waiting(item) && !is_unavailable(item))
        .filter_map(item_key)
        .filter(|key| !processed.contains(*key) && seen.insert((*key).to_owned()))
        .map(str::to_owned)
        .collect()
}

fn item_key(item: &Value) -> Option<&str> {
    let field = if SourceKind::of(item).is_youtube() {
        "video_id"
    } else {
        "entry_id"
    };
    item[field].as_str().filter(|value| !value.is_empty())
}

fn add_processed(playlist: &mut Value, key: &str) {
    if let Some(processed) = playlist["processed_video_ids"].as_array_mut() {
        if !processed.iter().any(|value| value == key) {
            processed.push(json!(key));
        }
    }
}

fn all_done(item: &Value) -> bool {
    Stage::ALL
        .iter()
        .all(|stage| stage_status(item, *stage).is_some_and(StageStatus::is_done))
}

fn is_waiting(item: &Value) -> bool {
    stage_statuses(item).contains(&StageStatus::Waiting)
}

fn mark_waiting(item: &mut Value, stage: Stage, action: ItemAction, question: &Value) {
    item["last_action"] = json!(action);
    item["last_error"] = Value::Null;
    let path = item["stages"][stage.as_ref()]["path"].clone();
    item["stages"][stage.as_ref()] = json!({"status":StageStatus::Waiting, "updated_at":now(), "path":path, "question":question});
}

fn mark_failed(item: &mut Value, stage: Stage, action: ItemAction, message: &str) {
    item["last_action"] = json!(action);
    item["last_error"] = json!(message);
    set_stage(item, stage, StageStatus::Failed, Some(message));
}

fn now() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
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
