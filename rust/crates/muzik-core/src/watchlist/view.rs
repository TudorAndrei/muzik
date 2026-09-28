use super::normalize;
use crate::chapters;
use crate::thumbnails;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const ACTIONS: [&str; 8] = [
    "run",
    "retry",
    "download_again",
    "check_quality_again",
    "parse_again",
    "split_again",
    "organize_again",
    "run_all_again",
];

/// Add the fields used by the saved watchlist cards without changing the file.
pub fn view(document: Value, output: &Path, cache: &Path) -> Result<Value, String> {
    let mut document = normalize(document)?;
    let playlists = document["playlists"]
        .as_array_mut()
        .ok_or("watchlist playlists are missing")?;
    for playlist in playlists {
        let items = playlist["items"]
            .as_array_mut()
            .ok_or("playlist items are missing")?;
        for item in items {
            enrich(item, output, cache)?;
        }
    }
    Ok(document)
}

fn enrich(item: &mut Value, output: &Path, cache: &Path) -> Result<(), String> {
    let available = item
        .get("video_id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty());
    let statuses: Vec<&str> = item["stages"]
        .as_object()
        .ok_or("item stages are missing")?
        .values()
        .filter_map(|stage| stage.get("status").and_then(Value::as_str))
        .collect();
    let summary = if !available {
        "Unavailable"
    } else if statuses.contains(&"running") {
        "Processing"
    } else if statuses.contains(&"failed") {
        "Failed"
    } else if statuses
        .iter()
        .all(|status| matches!(*status, "complete" | "skipped"))
    {
        "Processed"
    } else {
        "Pending"
    };
    let primary = if !available || summary == "Processed" {
        Value::Null
    } else if statuses.contains(&"failed") {
        json!({"action": "retry", "label": "Retry"})
    } else if statuses.contains(&"stale") {
        json!({"action": "run", "label": "Resume"})
    } else {
        json!({"action": "run", "label": "Run"})
    };
    let thumbnail = item
        .get("video_id")
        .and_then(Value::as_str)
        .and_then(|id| thumbnails::cached_path(id, cache))
        .map(|path| path.to_string_lossy().into_owned());
    let mut actions = serde_json::Map::new();
    for name in ACTIONS {
        let (enabled, reason) = availability(item, name, output);
        actions.insert(name.into(), json!({"enabled": enabled, "reason": reason}));
    }
    let fields = item.as_object_mut().ok_or("item is not an object")?;
    fields.insert("thumbnail_path".into(), json!(thumbnail));
    fields.insert("summary".into(), json!(summary));
    fields.insert("primary_action".into(), primary);
    fields.insert("actions".into(), Value::Object(actions));
    Ok(())
}

fn availability(item: &Value, action: &str, output: &Path) -> (bool, Option<&'static str>) {
    let video_id = item.get("video_id").and_then(Value::as_str);
    let video_url = item.get("video_url").and_then(Value::as_str);
    if video_id.is_none_or(str::is_empty) || video_url.is_none_or(str::is_empty) {
        return (false, Some("This playlist item is unavailable."));
    }
    let spotify = item.get("kind").and_then(Value::as_str) == Some("spotify");
    if spotify
        && item.get("track").is_none_or(|track| {
            track.is_null() || track.as_object().is_some_and(serde_json::Map::is_empty)
        })
    {
        return (false, Some("This track has no saved Spotify metadata."));
    }
    if matches!(action, "run" | "retry" | "download_again" | "run_all_again") {
        return (true, None);
    }
    let audio = audio_path(item, output);
    if action == "organize_again" {
        let split = item["stages"]["split"]["path"]
            .as_str()
            .map(PathBuf::from)
            .filter(|path| path.exists());
        if split.is_some() || audio.is_some() {
            return (true, None);
        }
        return (
            false,
            Some(if spotify {
                "No acquired audio is available."
            } else {
                "No downloaded audio or split directory is available."
            }),
        );
    }
    if spotify {
        return (false, Some("A Spotify track is one file: it has no quality check, no chapters to parse, and nothing to split."));
    }
    let Some(audio) = audio else {
        return (
            false,
            Some("Download this video before you run this command."),
        );
    };
    if action == "split_again"
        && !chapters::find_chapters(&audio).is_ok_and(|chapters| !chapters.is_empty())
    {
        return (
            false,
            Some("Parse and accept chapters before you split this video."),
        );
    }
    (true, None)
}

fn audio_path(item: &Value, output: &Path) -> Option<PathBuf> {
    item["stages"]["download"]["path"]
        .as_str()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            item.get("video_id")
                .and_then(Value::as_str)
                .and_then(|id| find_audio(output, id))
        })
}

pub(super) fn find_audio(output: &Path, id: &str) -> Option<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(output)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && is_audio(path)
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().contains(&format!("[{id}]")))
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .next()
        .and_then(|path| path.canonicalize().ok())
}

pub(super) fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "flac" | "mp3" | "m4a" | "opus" | "wav" | "aac"
            )
        })
}
