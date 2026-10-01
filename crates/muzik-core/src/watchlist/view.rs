use super::{
    is_gone, is_unavailable, normalize, stage_statuses, ItemAction, SourceKind, Stage, StageStatus,
};
use crate::chapters;
use crate::thumbnails;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantArray};

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantArray,
)]
pub enum Summary {
    Pending,
    Processing,
    Waiting,
    Failed,
    Processed,
    Unavailable,
}

impl Summary {
    pub const ALL: &'static [Self] = <Self as strum::VariantArray>::VARIANTS;
}

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
    let available = !is_unavailable(item);
    item["stages"]
        .as_object()
        .ok_or("item stages are missing")?;
    let statuses = stage_statuses(item);
    let summary = if !available {
        Summary::Unavailable
    } else if statuses.contains(&StageStatus::Running) {
        Summary::Processing
    } else if statuses.contains(&StageStatus::Waiting) {
        Summary::Waiting
    } else if statuses.contains(&StageStatus::Failed) {
        Summary::Failed
    } else if statuses.iter().all(|status| status.is_done()) {
        Summary::Processed
    } else {
        Summary::Pending
    };
    let primary = if !available || matches!(summary, Summary::Processed | Summary::Waiting) {
        Value::Null
    } else if statuses.contains(&StageStatus::Failed) {
        json!({"action": ItemAction::Retry, "label": "Retry"})
    } else if statuses.contains(&StageStatus::Stale) {
        json!({"action": ItemAction::Run, "label": "Resume"})
    } else {
        json!({"action": ItemAction::Run, "label": "Run"})
    };
    let thumbnail = item
        .get("video_id")
        .and_then(Value::as_str)
        .and_then(|id| thumbnails::cached_path(id, cache))
        .map(|path| path.to_string_lossy().into_owned());
    let mut actions = serde_json::Map::new();
    for action in ItemAction::ALL {
        let (enabled, reason) = availability(item, *action, output);
        actions.insert(
            action.to_string(),
            json!({"enabled": enabled, "reason": reason}),
        );
    }
    let fields = item.as_object_mut().ok_or("item is not an object")?;
    fields.insert("thumbnail_path".into(), json!(thumbnail));
    fields.insert("summary".into(), json!(summary));
    fields.insert("primary_action".into(), primary);
    fields.insert("actions".into(), Value::Object(actions));
    Ok(())
}

pub(super) fn availability(
    item: &Value,
    action: ItemAction,
    output: &Path,
) -> (bool, Option<&'static str>) {
    if is_gone(item) {
        return (false, Some("This video is private or was removed."));
    }
    let video_id = item.get("video_id").and_then(Value::as_str);
    let video_url = item.get("video_url").and_then(Value::as_str);
    if video_id.is_none_or(str::is_empty) || video_url.is_none_or(str::is_empty) {
        return (false, Some("This playlist item is unavailable."));
    }
    let spotify = SourceKind::of(item) == SourceKind::Spotify;
    if spotify
        && item.get("track").is_none_or(|track| {
            track.is_null() || track.as_object().is_some_and(serde_json::Map::is_empty)
        })
    {
        return (false, Some("This track has no saved Spotify metadata."));
    }
    if action.stage() == Stage::Download {
        return (true, None);
    }
    if SourceKind::of(item) == SourceKind::Bandcamp {
        if action != ItemAction::OrganizeAgain {
            return (false, Some("A Bandcamp purchase has no quality check, no chapters to parse, and nothing to split."));
        }
        let saved = item["stages"][Stage::Download.as_ref()]["path"]
            .as_str()
            .map(Path::new)
            .is_some_and(Path::is_dir);
        return if saved {
            (true, None)
        } else {
            (
                false,
                Some("Download this purchase before you organize it again."),
            )
        };
    }
    let audio = audio_path(item, output);
    if action == ItemAction::OrganizeAgain {
        let split = item["stages"][Stage::Split.as_ref()]["path"]
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
    if action == ItemAction::SplitAgain
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
    item["stages"][Stage::Download.as_ref()]["path"]
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
