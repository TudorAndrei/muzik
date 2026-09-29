//! Durable watchlist jobs. Callers supply source and audio operations.

use super::{reconcile, view, ReconcileOptions, Repository};
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
    Waiting { stage: String, question: Value },
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
    pub action: &'a str,
}

/// Implement source lookup and item processing with Rust adapters.
/// `process` returns the item with its final stage state.
pub trait Operations {
    fn load(&mut self, playlist: &Value) -> Result<LoadedSource, JobError>;
    fn process(
        &mut self,
        playlist: &Value,
        item: &Value,
        action: &str,
        cancelled: &AtomicBool,
    ) -> Result<Value, JobError>;
}

#[derive(Clone, Copy)]
pub struct JobOptions<'a> {
    pub reconcile: ReconcileOptions<'a>,
    pub output: &'a Path,
    pub cache: &'a Path,
    pub dry_run: bool,
}

pub fn refresh(
    repository: &Repository,
    options: JobOptions<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
) -> Result<Value, JobError> {
    let mut document = repository.load()?;
    let count = document["playlists"].as_array().map_or(0, Vec::len);
    if count == 0 {
        return Err(JobError::Operation(
            "Add a playlist before you refresh.".into(),
        ));
    }
    emit(
        on_event,
        "progress_started",
        json!({"task_id":"watchlist-refresh", "description":"Checking watchlist playlists.", "total":count}),
    );
    let mut pending = 0;
    let mut completed = 0;
    let mut failed = 0;
    let mut waiting = 0;
    let mut errors = 0;
    let mut loaded_playlists = Vec::new();
    for index in 0..count {
        check_cancelled(cancelled)?;
        let playlist = document["playlists"][index].clone();
        let id = playlist["playlist_id"].as_str().unwrap_or("").to_owned();
        let loaded = match operations.load(&playlist) {
            Ok(loaded) => loaded,
            Err(JobError::Cancelled) => return Err(JobError::Cancelled),
            Err(error) => {
                let saved = &mut document["playlists"][index];
                saved["last_checked_at"] = json!(now());
                saved["last_error"] = json!(error.to_string());
                save(repository, &document, options.dry_run)?;
                emit(on_event, "watchlist_saved", json!({"playlist_id":id}));
                errors += 1;
                emit(
                    on_event,
                    "message",
                    json!({"message":error.to_string(), "severity":"error"}),
                );
                emit(
                    on_event,
                    "progress_advanced",
                    json!({"task_id":"watchlist-refresh"}),
                );
                continue;
            }
        };
        check_cancelled(cancelled)?;
        let saved = &mut document["playlists"][index];
        if let Some(title) = loaded.title.filter(|title| !title.trim().is_empty()) {
            saved["title"] = json!(title);
        }
        saved["items"] = json!(merge_items(&playlist["items"], loaded.items));
        saved["last_checked_at"] = json!(now());
        saved["last_error"] = Value::Null;
        reconcile(&mut document, options.reconcile)?;
        save(repository, &document, options.dry_run)?;
        emit(on_event, "watchlist_saved", json!({"playlist_id":id}));
        loaded_playlists.push(index);
    }
    for index in loaded_playlists {
        let id = document["playlists"][index]["playlist_id"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        let ids = pending_ids(&document["playlists"][index]);
        pending += ids.len();
        emit(
            on_event,
            "message",
            json!({"message":format!("Playlist {id} has {} pending item(s).", ids.len())}),
        );
        for key in ids {
            if options.dry_run {
                continue;
            }
            check_cancelled(cancelled)?;
            let playlist = document["playlists"][index].clone();
            let Some(item_index) = playlist["items"].as_array().and_then(|items| {
                items
                    .iter()
                    .position(|item| item_key(item) == Some(key.as_str()))
            }) else {
                continue;
            };
            let item = playlist["items"][item_index].clone();
            match operations.process(&playlist, &item, "run", cancelled) {
                Ok(mut updated) => {
                    check_cancelled(cancelled)?;
                    updated["last_action"] = json!("refresh");
                    updated["last_error"] = Value::Null;
                    let finished = all_done(&updated);
                    if let Some(items) = document["playlists"][index]["items"].as_array_mut() {
                        for card in items
                            .iter_mut()
                            .filter(|card| item_key(card) == Some(key.as_str()))
                        {
                            let mut merged = updated.clone();
                            merged["position"] = card["position"].clone();
                            *card = merged;
                        }
                    }
                    if finished {
                        add_processed(&mut document["playlists"][index], &key);
                    }
                    completed += 1;
                }
                Err(JobError::Cancelled) => return Err(JobError::Cancelled),
                Err(JobError::Waiting { stage, question }) => {
                    if let Some(items) = document["playlists"][index]["items"].as_array_mut() {
                        for card in items
                            .iter_mut()
                            .filter(|card| item_key(card) == Some(key.as_str()))
                        {
                            mark_waiting(card, &stage, "refresh", &question);
                        }
                    }
                    waiting += 1;
                }
                Err(error) => {
                    if let Some(items) = document["playlists"][index]["items"].as_array_mut() {
                        for card in items
                            .iter_mut()
                            .filter(|card| item_key(card) == Some(key.as_str()))
                        {
                            mark_failed(card, "download", "refresh", &error.to_string());
                        }
                    }
                    failed += 1;
                }
            }
            save(repository, &document, options.dry_run)?;
            emit(on_event, "watchlist_saved", json!({"playlist_id":id}));
        }
        emit(
            on_event,
            "progress_advanced",
            json!({"task_id":"watchlist-refresh"}),
        );
    }
    check_cancelled(cancelled)?;
    emit(
        on_event,
        "progress_finished",
        json!({"task_id":"watchlist-refresh", "success":failed == 0 && errors == 0}),
    );
    let summary = json!({"playlists_checked":count, "pending_videos":pending, "completed_videos":completed, "failed_videos":failed, "waiting_videos":waiting, "playlist_errors":errors});
    Ok(json!({"summary":summary, "watchlist":view(document, options.output, options.cache)?}))
}

pub fn action(
    repository: &Repository,
    options: JobOptions<'_>,
    selection: ItemSelection<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
) -> Result<Value, JobError> {
    check_cancelled(cancelled)?;
    let ItemSelection {
        playlist_id,
        position,
        video_id,
        action,
    } = selection;
    if !matches!(
        action,
        "run"
            | "retry"
            | "download_again"
            | "check_quality_again"
            | "parse_again"
            | "split_again"
            | "organize_again"
            | "run_all_again"
    ) {
        return Err(JobError::Operation(format!(
            "Unknown item action: {action}"
        )));
    }
    let mut document = repository.load()?;
    let playlist_index = document["playlists"]
        .as_array()
        .and_then(|playlists| {
            playlists
                .iter()
                .position(|playlist| playlist["playlist_id"] == playlist_id)
        })
        .ok_or_else(|| {
            JobError::Operation("The selected playlist is no longer available.".into())
        })?;
    let playlist = document["playlists"][playlist_index].clone();
    let item_index = playlist["items"]
        .as_array()
        .and_then(|items| {
            items.iter().position(|item| {
                item["position"] == position && item["video_id"].as_str() == video_id
            })
        })
        .ok_or_else(|| JobError::Operation("The selected video is no longer available.".into()))?;
    let item = playlist["items"][item_index].clone();
    let displayed = view(
        json!({"version":3,"playlists":[playlist.clone()]}),
        options.output,
        options.cache,
    )?;
    let available = &displayed["playlists"][0]["items"][item_index]["actions"][action];
    if available["enabled"] != true {
        return Err(JobError::Operation(
            available["reason"]
                .as_str()
                .unwrap_or("This command is not available.")
                .to_owned(),
        ));
    }
    let stage = match action {
        "check_quality_again" => "quality",
        "parse_again" => "parse",
        "split_again" => "split",
        "organize_again" => "organize",
        _ => "download",
    };
    if options.dry_run {
        return Ok(
            json!({"action":{"action":action,"planned_stage":stage,"dry_run":true},
            "watchlist":view(document, options.output, options.cache)?}),
        );
    }
    let target = &mut document["playlists"][playlist_index]["items"][item_index];
    target["last_action"] = json!(action);
    target["last_error"] = Value::Null;
    target["stages"][stage] =
        json!({"status":"running", "updated_at":now(), "path":null, "error":null});
    save(repository, &document, options.dry_run)?;
    match operations.process(&playlist, &item, action, cancelled) {
        Ok(updated) => {
            if cancelled.load(Ordering::SeqCst) {
                document["playlists"][playlist_index]["items"][item_index] = item;
                repository.save(document)?;
                return Err(JobError::Cancelled);
            }
            let key = item_key(&item).map(str::to_owned);
            document["playlists"][playlist_index]["items"][item_index] = updated;
            let target = &mut document["playlists"][playlist_index]["items"][item_index];
            target["last_action"] = json!(action);
            target["last_error"] = Value::Null;
            if target["stages"][stage]["status"] == "running" {
                target["stages"][stage] =
                    json!({"status":"complete", "updated_at":now(), "path":null, "error":null});
            }
            if let Some(key) = key {
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
                    add_processed(playlist, &key);
                } else {
                    remove_processed(playlist, &key);
                }
            }
            save(repository, &document, options.dry_run)?;
            Ok(
                json!({"action":{"action":action,"completed_stage":stage}, "watchlist":view(document, options.output, options.cache)?}),
            )
        }
        Err(JobError::Waiting {
            stage: waiting_stage,
            question,
        }) => {
            let target = &mut document["playlists"][playlist_index]["items"][item_index];
            *target = item;
            mark_waiting(target, &waiting_stage, action, &question);
            save(repository, &document, options.dry_run)?;
            Ok(
                json!({"action":{"action":action,"waiting_stage":waiting_stage}, "watchlist":view(document, options.output, options.cache)?}),
            )
        }
        Err(error) => {
            let target = &mut document["playlists"][playlist_index]["items"][item_index];
            *target = item;
            if matches!(error, JobError::Cancelled) {
                repository.save(document)?;
            } else {
                mark_failed(target, stage, action, &error.to_string());
                repository.save(document)?;
            }
            Err(error)
        }
    }
}

fn save(repository: &Repository, document: &Value, dry_run: bool) -> Result<(), JobError> {
    if !dry_run {
        repository.save(document.clone())?;
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
        .filter(|item| !is_waiting(item))
        .filter_map(item_key)
        .filter(|key| !processed.contains(*key) && seen.insert((*key).to_owned()))
        .map(str::to_owned)
        .collect()
}

fn item_key(item: &Value) -> Option<&str> {
    let field = if item["kind"] == "spotify" {
        "entry_id"
    } else {
        "video_id"
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
    item["stages"].as_object().is_some_and(|stages| {
        stages
            .values()
            .all(|stage| matches!(stage["status"].as_str(), Some("complete" | "skipped")))
    })
}

fn is_waiting(item: &Value) -> bool {
    item["stages"]
        .as_object()
        .is_some_and(|stages| stages.values().any(|stage| stage["status"] == "waiting"))
}

fn mark_waiting(item: &mut Value, stage: &str, action: &str, question: &Value) {
    item["last_action"] = json!(action);
    item["last_error"] = Value::Null;
    item["stages"][stage] = json!({"status":"waiting", "updated_at":now(), "path":item["stages"][stage]["path"].clone(), "question":question});
}

fn mark_failed(item: &mut Value, stage: &str, action: &str, message: &str) {
    item["last_action"] = json!(action);
    item["last_error"] = json!(message);
    item["stages"][stage] =
        json!({"status":"failed", "updated_at":now(), "path":null, "error":message});
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
