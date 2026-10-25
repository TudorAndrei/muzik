//! A one-time import of the cache files that the Python version wrote.

use super::{
    LastAction, ReconcileOptions, Repository, Stage, StageStatus, WatchItem, Watchlist, now,
};
use crate::Result;
use muzik_core::audio::is_audio;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use strum_macros::EnumString;

const IMPORTED: &str = "legacy_cache_imported";

#[derive(Clone, Copy, PartialEq, Eq, EnumString)]
#[strum(serialize_all = "snake_case")]
enum CacheStatus {
    Downloaded,
    Split,
    Organized,
}

#[derive(Default, Deserialize)]
struct LegacyState {
    #[serde(default)]
    videos: HashMap<String, Value>,
}

#[derive(Clone, Deserialize)]
struct LegacyEntry {
    #[serde(default, deserialize_with = "known_status")]
    status: Option<CacheStatus>,
    #[serde(default)]
    audio_file: Option<PathBuf>,
    #[serde(default)]
    split_dir: Option<PathBuf>,
    #[serde(default)]
    files: Option<Vec<Value>>,
}

fn known_status<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<CacheStatus>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.and_then(|status| status.parse().ok()))
}

impl LegacyEntry {
    fn first_file(&self) -> Option<PathBuf> {
        self.files.as_ref()?.first()?.as_str().map(PathBuf::from)
    }

    fn remaining_organize_target(&self, splits: &Path) -> Option<PathBuf> {
        if self.status != Some(CacheStatus::Organized) {
            return None;
        }
        let split = self.split_dir.clone().or_else(|| {
            self.audio_file
                .as_ref()
                .map(|path| splits.join(path.file_stem().unwrap_or_default()))
                .filter(|path| path.is_dir())
        });
        if let Some(split) = split.filter(|path| path.is_dir()) {
            return contains_audio(&split).then_some(split);
        }
        if let Some(audio) = self.audio_file.clone().filter(|path| path.is_file()) {
            return Some(audio);
        }
        self.files
            .iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .find(|path| path.is_file())
    }
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
        document.apply_legacy_cache(options);
        connection.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            [IMPORTED, &now()],
        )?;
        Ok(true)
    })
}

impl Watchlist {
    fn apply_legacy_cache(&mut self, options: ReconcileOptions<'_>) {
        for playlist in &mut self.playlists {
            let videos = read_state(
                options.cache,
                &state_id(&playlist.playlist_id, playlist.kind.single_file()),
            );
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
                apply_item(item, &videos, &mut processed, options);
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
}

fn apply_item(
    item: &mut WatchItem,
    videos: &HashMap<String, LegacyEntry>,
    processed: &mut Vec<String>,
    options: ReconcileOptions<'_>,
) {
    let Some(key) = item.key().map(str::to_owned) else {
        return;
    };
    let entry = videos.get(&key);
    if item.kind.single_file() {
        let Some(entry) = entry else {
            return;
        };
        if matches!(
            entry.status,
            Some(CacheStatus::Downloaded | CacheStatus::Organized)
        ) {
            item.complete(Stage::Download, entry.first_file());
            if entry.status == Some(CacheStatus::Organized) {
                item.set(Stage::Organize, StageStatus::Complete);
                if !processed.contains(&key) {
                    processed.push(key);
                }
            } else if options.no_organize {
                item.set(Stage::Organize, StageStatus::Skipped);
            }
        }
        return;
    }
    if !options.no_organize
        && let Some(entry) = entry
        && let Some(target) = entry.remaining_organize_target(options.splits)
    {
        processed.retain(|id| id != &key);
        mark_organize_failed(item, entry.audio_file.clone(), &target);
        return;
    }
    if processed.contains(&key) {
        return;
    }
    let Some(entry) = entry
        .cloned()
        .or_else(|| legacy_entry(options.cache, options.splits, &key))
    else {
        return;
    };
    let Some(status) = entry.status else {
        return;
    };
    item.complete(
        Stage::Download,
        entry.audio_file.clone().or_else(|| entry.first_file()),
    );
    if matches!(status, CacheStatus::Split | CacheStatus::Organized) {
        item.set(Stage::Parse, StageStatus::Complete);
        item.set(
            Stage::Split,
            if entry.split_dir.is_some() {
                StageStatus::Complete
            } else {
                StageStatus::Skipped
            },
        );
        item.set_path(Stage::Split, entry.split_dir);
    }
    if status == CacheStatus::Organized {
        item.set(Stage::Organize, StageStatus::Complete);
        processed.push(key);
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

fn safe(id: &str) -> bool {
    id.bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

fn read_state(cache: &Path, id: &str) -> HashMap<String, LegacyEntry> {
    if !safe(id) {
        return HashMap::new();
    }
    fs::read(cache.join(format!("playlist_{id}.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LegacyState>(&bytes).ok())
        .unwrap_or_default()
        .videos
        .into_iter()
        .filter_map(|(id, entry)| Some((id, serde_json::from_value(entry).ok()?)))
        .collect()
}

fn legacy_entry(cache: &Path, splits: &Path, id: &str) -> Option<LegacyEntry> {
    if !safe(id) {
        return None;
    }
    let value = fs::read_to_string(cache.join(format!("yt_{id}.txt"))).ok()?;
    let path = PathBuf::from(value.trim());
    if path.as_os_str().is_empty() {
        return None;
    }
    let split = splits.join(path.file_stem().unwrap_or_default());
    let (status, split_dir) = if path.is_file() {
        (CacheStatus::Downloaded, None)
    } else if split.exists() {
        (CacheStatus::Split, Some(split))
    } else {
        (CacheStatus::Organized, None)
    };
    Some(LegacyEntry {
        status: Some(status),
        audio_file: Some(PathBuf::from(value)),
        split_dir,
        files: None,
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

fn mark_organize_failed(item: &mut WatchItem, audio: Option<PathBuf>, target: &Path) {
    let message = "The music library did not import this item. Select Retry.";
    item.last_action = Some(LastAction::Refresh);
    item.last_error = Some(message.into());
    item.complete(Stage::Download, audio);
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
