use super::library_lookup::MusicLibrary;
use super::{is_audio, now, AudioIndex, Stage, StageStatus, WatchItem, Watchlist};
use crate::QualityPolicy;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use strum_macros::{AsRefStr, EnumString};

/// Refresh saved stage state from the existing local workflow cache and files.
/// Save the document with `Repository::save` after this call succeeds.
#[derive(Clone, Copy)]
pub struct ReconcileOptions<'a> {
    pub output: &'a Path,
    pub splits: &'a Path,
    pub cache: &'a Path,
    pub config: Option<&'a Path>,
    pub no_organize: bool,
    pub no_split: bool,
    pub quality_policy: QualityPolicy,
}

#[derive(Clone, Copy, PartialEq, Eq, AsRefStr, EnumString)]
#[strum(serialize_all = "snake_case")]
enum CacheStatus {
    Downloaded,
    Split,
    Organized,
}

fn cache_status(entry: &Value) -> Option<CacheStatus> {
    entry["status"].as_str()?.parse().ok()
}

pub fn reconcile(document: &mut Watchlist, options: ReconcileOptions<'_>) -> Result<(), String> {
    let ReconcileOptions {
        output,
        splits,
        cache,
        config,
        no_organize,
        no_split,
        quality_policy,
    } = options;
    let music_library = MusicLibrary::open(config);
    let audio = AudioIndex::scan(output);
    for playlist in &mut document.playlists {
        let id = playlist.playlist_id.clone();
        let by_entry = !playlist.kind.is_youtube();
        let state_id = if by_entry {
            let short = id.rsplit(':').next().unwrap_or(&id);
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
        } else {
            id.clone()
        };
        let state = read_state(cache, &state_id);
        let videos = state.get("videos").and_then(Value::as_object);
        let mut processed_ids = playlist.processed_video_ids.clone();
        let waiting: Vec<(usize, Stage, super::StageRecord)> = playlist
            .items
            .iter()
            .enumerate()
            .flat_map(|(index, item)| {
                Stage::ALL
                    .iter()
                    .filter(|stage| item.status(**stage) == StageStatus::Waiting)
                    .map(move |stage| (index, *stage, item.stage(*stage).clone()))
            })
            .collect();
        for item in &mut playlist.items {
            for stage in Stage::ALL {
                if item.status(*stage) == StageStatus::Running {
                    item.set(*stage, StageStatus::NotStarted);
                }
            }
            if item.statuses().any(|status| status == StageStatus::Stale) {
                if let Some(key) = item.key() {
                    processed_ids.retain(|processed| processed != key);
                }
                continue;
            }
            if by_entry {
                for stage in [Stage::Quality, Stage::Parse, Stage::Split] {
                    item.set(stage, StageStatus::Skipped);
                }
                let entry_id = item.entry_id.clone().unwrap_or_default();
                let entry = videos
                    .and_then(|values| values.get(&entry_id))
                    .cloned()
                    .unwrap_or(Value::Null);
                let status = cache_status(&entry);
                if matches!(
                    status,
                    Some(CacheStatus::Downloaded | CacheStatus::Organized)
                ) {
                    let path = entry["files"]
                        .as_array()
                        .and_then(|files| files.first())
                        .and_then(Value::as_str)
                        .map(PathBuf::from);
                    item.complete(Stage::Download, path);
                    if status == Some(CacheStatus::Organized) {
                        item.set(Stage::Organize, StageStatus::Complete);
                        if !entry_id.is_empty() && !processed_ids.contains(&entry_id) {
                            processed_ids.push(entry_id);
                        }
                    } else if no_organize {
                        item.set(Stage::Organize, StageStatus::Skipped);
                    }
                }
                continue;
            }
            let Some(video_id) = item.video_id.clone().filter(|id| !id.is_empty()) else {
                continue;
            };
            let mut entry = videos
                .and_then(|values| values.get(&video_id))
                .cloned()
                .unwrap_or(Value::Null);
            if !no_organize {
                if let Some(target) = remaining_organize_target(&entry, splits) {
                    processed_ids.retain(|id| id != &video_id);
                    mark_organize_failed(item, &entry, &target);
                    continue;
                }
            }
            if processed_ids.contains(&video_id) {
                mark_completed(item, no_organize, no_split, quality_policy);
                continue;
            }
            if entry.is_null() {
                entry = legacy_entry(cache, splits, &video_id);
            }
            let status = cache_status(&entry);
            if status.is_some() {
                let path = entry["audio_file"]
                    .as_str()
                    .or_else(|| {
                        entry["files"]
                            .as_array()
                            .and_then(|files| files.first())
                            .and_then(Value::as_str)
                    })
                    .map(PathBuf::from);
                item.complete(Stage::Download, path);
            } else if let Some(path) = audio.find(&video_id) {
                item.complete(Stage::Download, Some(path));
            } else if let Some(path) = music_library
                .as_ref()
                .and_then(|library| library.find(&video_id, &item.title))
            {
                item.complete(Stage::Download, Some(path));
                item.set(Stage::Parse, StageStatus::Complete);
                item.set(Stage::Split, StageStatus::Skipped);
                item.set(Stage::Organize, StageStatus::Complete);
                processed_ids.push(video_id.clone());
            }
            if matches!(status, Some(CacheStatus::Split | CacheStatus::Organized)) {
                item.set(Stage::Parse, StageStatus::Complete);
                let split = entry["split_dir"].as_str().map(PathBuf::from);
                let status = if split.is_some() {
                    StageStatus::Complete
                } else {
                    StageStatus::Skipped
                };
                item.set(Stage::Split, status);
                item.set_path(Stage::Split, split);
            }
            if status == Some(CacheStatus::Organized) {
                item.set(Stage::Organize, StageStatus::Complete);
                processed_ids.push(video_id);
            }
        }
        for (index, stage, record) in waiting {
            let item = &mut playlist.items[index];
            *item.stage_mut(stage) = record;
            for key in [item.video_id.clone(), item.entry_id.clone()]
                .into_iter()
                .flatten()
            {
                processed_ids.retain(|processed| processed != &key);
            }
        }
        playlist.processed_video_ids = processed_ids;
    }
    *document = std::mem::take(document).normalized()?;
    Ok(())
}

fn read_state(cache: &Path, id: &str) -> Value {
    if !id
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return Value::Null;
    }
    fs::read(cache.join(format!("playlist_{id}.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null)
}

fn legacy_entry(cache: &Path, splits: &Path, id: &str) -> Value {
    if !id
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
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

fn mark_completed(
    item: &mut WatchItem,
    no_organize: bool,
    no_split: bool,
    quality_policy: QualityPolicy,
) {
    let done = |skipped: bool| {
        if skipped {
            StageStatus::Skipped
        } else {
            StageStatus::Complete
        }
    };
    item.last_action = Some("refresh".into());
    item.last_error = None;
    item.set(Stage::Download, StageStatus::Complete);
    item.set(Stage::Quality, done(quality_policy == QualityPolicy::Off));
    item.set(Stage::Parse, done(no_split));
    if no_split || item.status(Stage::Split) != StageStatus::Complete {
        item.set(Stage::Split, StageStatus::Skipped);
    } else {
        item.set(Stage::Split, StageStatus::Complete);
    }
    item.set(Stage::Organize, done(no_organize));
}
