//! A one-time import of the cache files that the Python version wrote.

use super::{ReconcileOptions, Repository, Stage, StageStatus, WatchItem, Watchlist, now};
use crate::Result;
use muzik_core::audio::is_audio;
use rusqlite::OptionalExtension;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use strum_macros::{AsRefStr, EnumString};

const IMPORTED: &str = "legacy_cache_imported";

#[derive(Clone, Copy, PartialEq, Eq, AsRefStr, EnumString)]
#[strum(serialize_all = "snake_case")]
enum CacheStatus {
    Downloaded,
    Split,
    Organized,
}

/// # Errors
/// Returns an error if the watchlist or the import marker cannot be read or written.
pub fn import_cache(repository: &Repository, options: ReconcileOptions<'_>) -> Result<bool> {
    repository.update_with(|document, connection| {
        let done: Option<String> = connection
            .query_row("SELECT value FROM meta WHERE key = ?1", [IMPORTED], |row| {
                row.get(0)
            })
            .optional()?;
        if done.is_some() {
            return Ok(false);
        }
        apply(document, options);
        connection.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            [IMPORTED, &now()],
        )?;
        Ok(true)
    })
}

fn apply(document: &mut Watchlist, options: ReconcileOptions<'_>) {
    for playlist in &mut document.playlists {
        let state = read_state(
            options.cache,
            &state_id(&playlist.playlist_id, playlist.kind.single_file()),
        );
        let videos = state.get("videos").and_then(Value::as_object);
        let mut processed = playlist.processed_video_ids.clone();
        for item in &mut playlist.items {
            if item.statuses().any(|status| status == StageStatus::Stale) {
                continue;
            }
            let waiting: Vec<_> = Stage::ALL
                .iter()
                .filter(|stage| item.status(**stage) == StageStatus::Waiting)
                .map(|stage| (*stage, item.stage(*stage).clone()))
                .collect();
            apply_item(item, videos, &mut processed, options);
            for (stage, record) in waiting {
                *item.stage_mut(stage) = record;
                for key in [item.video_id.as_deref(), item.entry_id.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    processed.retain(|processed| processed != key);
                }
            }
        }
        playlist.processed_video_ids = processed;
    }
}

fn apply_item(
    item: &mut WatchItem,
    videos: Option<&serde_json::Map<String, Value>>,
    processed: &mut Vec<String>,
    options: ReconcileOptions<'_>,
) {
    let entry_of = |id: &str| {
        videos
            .and_then(|values| values.get(id))
            .cloned()
            .unwrap_or(Value::Null)
    };
    if item.kind.single_file() {
        let entry_id = item.entry_id.clone().unwrap_or_default();
        let entry = entry_of(&entry_id);
        let status = cache_status(&entry);
        if matches!(
            status,
            Some(CacheStatus::Downloaded | CacheStatus::Organized)
        ) {
            item.complete(Stage::Download, first_file(&entry));
            if status == Some(CacheStatus::Organized) {
                item.set(Stage::Organize, StageStatus::Complete);
                if !entry_id.is_empty() && !processed.contains(&entry_id) {
                    processed.push(entry_id);
                }
            } else if options.no_organize {
                item.set(Stage::Organize, StageStatus::Skipped);
            }
        }
        return;
    }
    let Some(video_id) = item.video_id.clone().filter(|id| !id.is_empty()) else {
        return;
    };
    let mut entry = entry_of(&video_id);
    if !options.no_organize
        && let Some(target) = remaining_organize_target(&entry, options.splits)
    {
        processed.retain(|id| id != &video_id);
        mark_organize_failed(item, &entry, &target);
        return;
    }
    if processed.contains(&video_id) {
        return;
    }
    if entry.is_null() {
        entry = legacy_entry(options.cache, options.splits, &video_id);
    }
    let Some(status) = cache_status(&entry) else {
        return;
    };
    let path = entry
        .get("audio_file")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| first_file(&entry));
    item.complete(Stage::Download, path);
    if matches!(status, CacheStatus::Split | CacheStatus::Organized) {
        item.set(Stage::Parse, StageStatus::Complete);
        let split = entry
            .get("split_dir")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        item.set(
            Stage::Split,
            if split.is_some() {
                StageStatus::Complete
            } else {
                StageStatus::Skipped
            },
        );
        item.set_path(Stage::Split, split);
    }
    if status == CacheStatus::Organized {
        item.set(Stage::Organize, StageStatus::Complete);
        processed.push(video_id);
    }
}

fn state_id(playlist_id: &str, single_file: bool) -> String {
    if !single_file {
        return playlist_id.to_owned();
    }
    let short = playlist_id.rsplit(':').next().unwrap_or(playlist_id);
    format!(
        "spotify_{}",
        short
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            })
            .collect::<String>()
    )
}

fn cache_status(entry: &Value) -> Option<CacheStatus> {
    entry["status"].as_str()?.parse().ok()
}

fn first_file(entry: &Value) -> Option<PathBuf> {
    entry["files"]
        .as_array()
        .and_then(|files| files.first())
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

fn safe(id: &str) -> bool {
    id.bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

fn read_state(cache: &Path, id: &str) -> Value {
    if !safe(id) {
        return Value::Null;
    }
    fs::read(cache.join(format!("playlist_{id}.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

fn legacy_entry(cache: &Path, splits: &Path, id: &str) -> Value {
    if !safe(id) {
        return Value::Null;
    }
    let Ok(value) = fs::read_to_string(cache.join(format!("yt_{id}.txt"))) else {
        return Value::Null;
    };
    if value.trim().is_empty() {
        return Value::Null;
    }
    let path = PathBuf::from(value.trim());
    if path.is_file() {
        return json!({"status": CacheStatus::Downloaded.as_ref(), "audio_file": value});
    }
    let split = splits.join(path.file_stem().unwrap_or_default());
    if split.exists() {
        return json!({"status": CacheStatus::Split.as_ref(), "audio_file": value, "split_dir": split});
    }
    json!({"status": CacheStatus::Organized.as_ref(), "audio_file": value})
}

fn remaining_organize_target(entry: &Value, splits: &Path) -> Option<PathBuf> {
    if cache_status(entry) != Some(CacheStatus::Organized) {
        return None;
    }
    let audio = entry["audio_file"].as_str().map(PathBuf::from);
    let split = entry["split_dir"].as_str().map(PathBuf::from).or_else(|| {
        audio
            .as_ref()
            .map(|path| splits.join(path.file_stem().unwrap_or_default()))
            .filter(|path| path.is_dir())
    });
    if let Some(split) = split.filter(|path| path.is_dir()) {
        return contains_audio(&split).then_some(split);
    }
    if let Some(audio) = audio.filter(|path| path.is_file()) {
        return Some(audio);
    }
    entry["files"].as_array().and_then(|files| {
        files
            .iter()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .find(|path| path.is_file())
    })
}

fn contains_audio(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .any(|child| {
            (child.is_file() && is_audio(&child)) || (child.is_dir() && contains_audio(&child))
        })
}

fn mark_organize_failed(item: &mut WatchItem, entry: &Value, target: &Path) {
    let message = "The music library did not import this item. Select Retry.";
    item.last_action = Some("refresh".into());
    item.last_error = Some(message.into());
    item.complete(
        Stage::Download,
        entry["audio_file"].as_str().map(PathBuf::from),
    );
    item.complete(Stage::Parse, None);
    if target.is_dir() {
        item.complete(Stage::Split, Some(target.to_path_buf()));
    } else {
        item.set(Stage::Split, StageStatus::Skipped);
        item.set_path(Stage::Split, None);
    }
    let organize = item.stage_mut(Stage::Organize);
    organize.status = StageStatus::Failed;
    organize.updated_at = Some(now());
    organize.path = None;
    organize.error = Some(message.into());
}
