use super::source::availability;
use super::{AudioIndex, ItemAction, StageStatus, WatchItem, Watchlist};
use crate::Result;
use muzik_core::thumbnails;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
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

pub fn view(document: &Watchlist, output: &Path, cache: &Path) -> Result<Value> {
    let index = AudioIndex::scan(output);
    let mut value = document.to_value();
    let playlists = value
        .get_mut("playlists")
        .and_then(Value::as_array_mut)
        .ok_or("watchlist playlists are missing")?;
    for (playlist, saved) in playlists.iter_mut().zip(&document.playlists) {
        let items = playlist
            .get_mut("items")
            .and_then(Value::as_array_mut)
            .ok_or("playlist items are missing")?;
        for (card, item) in items.iter_mut().zip(&saved.items) {
            enrich(card, item, &index, cache)?;
        }
    }
    Ok(value)
}

fn enrich(card: &mut Value, item: &WatchItem, index: &AudioIndex, cache: &Path) -> Result<()> {
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
