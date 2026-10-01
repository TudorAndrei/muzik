use super::{AudioIndex, ItemAction, SourceKind, Stage, StageStatus, WatchItem, Watchlist};
use crate::chapters;
use crate::thumbnails;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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

    pub fn of(item: &WatchItem) -> Self {
        let statuses: Vec<StageStatus> = item.statuses().collect();
        if item.is_unavailable() {
            Self::Unavailable
        } else if statuses.contains(&StageStatus::Running) {
            Self::Processing
        } else if statuses.contains(&StageStatus::Waiting) {
            Self::Waiting
        } else if statuses.contains(&StageStatus::Failed) {
            Self::Failed
        } else if statuses.iter().all(|status| status.is_done()) {
            Self::Processed
        } else {
            Self::Pending
        }
    }
}

pub fn view(document: &Watchlist, output: &Path, cache: &Path) -> Result<Value, String> {
    let index = AudioIndex::scan(output);
    let mut value = document.to_value();
    let playlists = value["playlists"]
        .as_array_mut()
        .ok_or("watchlist playlists are missing")?;
    for (playlist, saved) in playlists.iter_mut().zip(&document.playlists) {
        let items = playlist["items"]
            .as_array_mut()
            .ok_or("playlist items are missing")?;
        for (card, item) in items.iter_mut().zip(&saved.items) {
            enrich(card, item, &index, cache)?;
        }
    }
    Ok(value)
}

fn enrich(
    card: &mut Value,
    item: &WatchItem,
    index: &AudioIndex,
    cache: &Path,
) -> Result<(), String> {
    let summary = Summary::of(item);
    let primary = if matches!(
        summary,
        Summary::Unavailable | Summary::Processed | Summary::Waiting
    ) {
        Value::Null
    } else if item.statuses().any(|status| status == StageStatus::Failed) {
        json!({"action": ItemAction::Retry, "label": "Retry"})
    } else if item.statuses().any(|status| status == StageStatus::Stale) {
        json!({"action": ItemAction::Run, "label": "Resume"})
    } else {
        json!({"action": ItemAction::Run, "label": "Run"})
    };
    let thumbnail = item
        .video_id
        .as_deref()
        .and_then(|id| thumbnails::cached_path(id, cache))
        .map(|path| path.to_string_lossy().into_owned());
    let audio = item.downloaded_audio(index);
    let mut actions = serde_json::Map::new();
    for action in ItemAction::ALL {
        let (enabled, reason) = availability(item, *action, audio.as_deref());
        actions.insert(
            action.to_string(),
            json!({"enabled": enabled, "reason": reason}),
        );
    }
    let fields = card.as_object_mut().ok_or("item is not an object")?;
    fields.insert("thumbnail_path".into(), json!(thumbnail));
    fields.insert("summary".into(), json!(summary));
    fields.insert("primary_action".into(), primary);
    fields.insert("actions".into(), Value::Object(actions));
    Ok(())
}

pub(super) fn availability(
    item: &WatchItem,
    action: ItemAction,
    audio: Option<&Path>,
) -> (bool, Option<&'static str>) {
    if item.is_gone() {
        return (false, Some("This video is private or was removed."));
    }
    if item.video_id.as_deref().is_none_or(str::is_empty)
        || item.video_url.as_deref().is_none_or(str::is_empty)
    {
        return (false, Some("This playlist item is unavailable."));
    }
    let spotify = item.kind == SourceKind::Spotify;
    if spotify
        && item.track.as_ref().is_none_or(|track| {
            track.is_null() || track.as_object().is_some_and(serde_json::Map::is_empty)
        })
    {
        return (false, Some("This track has no saved Spotify metadata."));
    }
    if action.stage() == Stage::Download {
        return (true, None);
    }
    if item.kind == SourceKind::Bandcamp {
        if action != ItemAction::OrganizeAgain {
            return (false, Some("A Bandcamp purchase has no quality check, no chapters to parse, and nothing to split."));
        }
        return if item.path(Stage::Download).is_some_and(Path::is_dir) {
            (true, None)
        } else {
            (
                false,
                Some("Download this purchase before you organize it again."),
            )
        };
    }
    if action == ItemAction::OrganizeAgain {
        let split = item
            .path(Stage::Split)
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
        && !chapters::find_chapters(audio).is_ok_and(|chapters| !chapters.is_empty())
    {
        return (
            false,
            Some("Parse and accept chapters before you split this video."),
        );
    }
    (true, None)
}
