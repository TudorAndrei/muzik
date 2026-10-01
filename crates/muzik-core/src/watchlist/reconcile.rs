use super::library_lookup::MusicLibrary;
use super::view::{find_audio, is_audio};
use super::{normalize, stage_status, SourceKind, Stage, StageStatus};
use crate::QualityPolicy;
use chrono::{Local, SecondsFormat};
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

pub fn reconcile(document: &mut Value, options: ReconcileOptions<'_>) -> Result<(), String> {
    let ReconcileOptions {
        output,
        splits,
        cache,
        config,
        no_organize,
        no_split,
        quality_policy,
    } = options;
    let mut normalized = normalize(document.clone())?;
    let music_library = MusicLibrary::open(config);
    let playlists = normalized["playlists"]
        .as_array_mut()
        .ok_or("watchlist playlists are missing")?;
    for playlist in playlists {
        let id = playlist["playlist_id"]
            .as_str()
            .ok_or("playlist ID is missing")?;
        let by_entry = !SourceKind::of(playlist).is_youtube();
        let state_id = if by_entry {
            let short = id.rsplit(':').next().unwrap_or(id);
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
            id.to_owned()
        };
        let state = read_state(cache, &state_id);
        let videos = state.get("videos").and_then(Value::as_object);
        let processed = playlist["processed_video_ids"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let mut processed_ids: Vec<String> = processed
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let items = playlist["items"]
            .as_array_mut()
            .ok_or("playlist items are missing")?;
        let waiting: Vec<(usize, Stage, Value)> = items
            .iter()
            .enumerate()
            .flat_map(|(index, item)| {
                Stage::ALL
                    .iter()
                    .filter(|stage| stage_status(item, **stage) == Some(StageStatus::Waiting))
                    .map(move |stage| (index, *stage, item["stages"][stage.as_ref()].clone()))
            })
            .collect();
        for item in items.iter_mut() {
            for stage in Stage::ALL {
                if stage_status(item, *stage) == Some(StageStatus::Running) {
                    set_status(item, *stage, StageStatus::NotStarted);
                }
            }
            // Explicit repeat actions invalidate later stages. Older cache records
            // must not turn these stages back into completed work.
            if Stage::ALL
                .iter()
                .any(|stage| stage_status(item, *stage) == Some(StageStatus::Stale))
            {
                let key = if by_entry { "entry_id" } else { "video_id" };
                if let Some(id) = item[key].as_str() {
                    processed_ids.retain(|processed| processed != id);
                }
                continue;
            }
            if by_entry {
                for stage in [Stage::Quality, Stage::Parse, Stage::Split] {
                    set_status(item, stage, StageStatus::Skipped);
                }
                let entry_id = item["entry_id"].as_str().unwrap_or("").to_owned();
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
                        .and_then(Value::as_str);
                    set_stage(item, Stage::Download, StageStatus::Complete, path);
                    if status == Some(CacheStatus::Organized) {
                        set_status(item, Stage::Organize, StageStatus::Complete);
                        if !entry_id.is_empty() && !processed_ids.contains(&entry_id) {
                            processed_ids.push(entry_id);
                        }
                    } else if no_organize {
                        set_status(item, Stage::Organize, StageStatus::Skipped);
                    }
                }
                continue;
            }
            let Some(video_id) = item["video_id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
            else {
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
                let path = entry["audio_file"].as_str().or_else(|| {
                    entry["files"]
                        .as_array()
                        .and_then(|files| files.first())
                        .and_then(Value::as_str)
                });
                set_stage(item, Stage::Download, StageStatus::Complete, path);
            } else if let Some(path) = find_audio(output, &video_id) {
                set_stage(item, Stage::Download, StageStatus::Complete, path.to_str());
            } else if let Some(path) = music_library
                .as_ref()
                .and_then(|library| library.find(&video_id, item["title"].as_str().unwrap_or("")))
            {
                set_stage(item, Stage::Download, StageStatus::Complete, path.to_str());
                set_status(item, Stage::Parse, StageStatus::Complete);
                set_status(item, Stage::Split, StageStatus::Skipped);
                set_status(item, Stage::Organize, StageStatus::Complete);
                processed_ids.push(video_id.clone());
            }
            if matches!(status, Some(CacheStatus::Split | CacheStatus::Organized)) {
                set_status(item, Stage::Parse, StageStatus::Complete);
                let split = entry["split_dir"].as_str();
                set_stage(
                    item,
                    Stage::Split,
                    if split.is_some() {
                        StageStatus::Complete
                    } else {
                        StageStatus::Skipped
                    },
                    split,
                );
            }
            if status == Some(CacheStatus::Organized) {
                set_status(item, Stage::Organize, StageStatus::Complete);
                processed_ids.push(video_id);
            }
        }
        for (index, stage, record) in waiting {
            let item = &mut items[index];
            item["stages"][stage.as_ref()] = record;
            for key in ["video_id", "entry_id"] {
                if let Some(id) = item[key].as_str() {
                    processed_ids.retain(|processed| processed != id);
                }
            }
        }
        playlist["processed_video_ids"] = json!(processed_ids);
    }
    *document = normalized;
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

fn set_status(item: &mut Value, stage: Stage, status: StageStatus) {
    super::set_stage_status(item, stage, status);
}

fn set_stage(item: &mut Value, stage: Stage, status: StageStatus, path: Option<&str>) {
    item["stages"][stage.as_ref()] =
        json!({"status": status, "updated_at": null, "path": path, "error": null});
}

fn mark_organize_failed(item: &mut Value, entry: &Value, target: &Path) {
    let message = "The music library did not import this item. Select Retry.";
    let updated_at = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);
    item["last_action"] = json!("refresh");
    item["last_error"] = json!(message);
    set_stage(
        item,
        Stage::Download,
        StageStatus::Complete,
        entry["audio_file"].as_str(),
    );
    set_stage(item, Stage::Parse, StageStatus::Complete, None);
    set_stage(
        item,
        Stage::Split,
        if target.is_dir() {
            StageStatus::Complete
        } else {
            StageStatus::Skipped
        },
        if target.is_dir() {
            target.to_str()
        } else {
            None
        },
    );
    for stage in [Stage::Download, Stage::Parse, Stage::Split] {
        item["stages"][stage.as_ref()]["updated_at"] = json!(updated_at);
    }
    item["stages"][Stage::Organize.as_ref()] = json!({"status": StageStatus::Failed, "updated_at": updated_at, "path": null, "error": message});
}

fn mark_completed(
    item: &mut Value,
    no_organize: bool,
    no_split: bool,
    quality_policy: QualityPolicy,
) {
    let updated_at = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);
    item["last_action"] = json!("refresh");
    item["last_error"] = Value::Null;
    set_status(item, Stage::Download, StageStatus::Complete);
    set_status(
        item,
        Stage::Quality,
        if quality_policy == QualityPolicy::Off {
            StageStatus::Skipped
        } else {
            StageStatus::Complete
        },
    );
    set_status(
        item,
        Stage::Parse,
        if no_split {
            StageStatus::Skipped
        } else {
            StageStatus::Complete
        },
    );
    if no_split || stage_status(item, Stage::Split) != Some(StageStatus::Complete) {
        set_status(item, Stage::Split, StageStatus::Skipped);
    }
    set_status(
        item,
        Stage::Organize,
        if no_organize {
            StageStatus::Skipped
        } else {
            StageStatus::Complete
        },
    );
    for stage in Stage::ALL {
        item["stages"][stage.as_ref()]["updated_at"] = json!(updated_at);
    }
}
