use super::source::availability;
use super::{AudioIndex, ItemAction, Playlist, StageStatus, WatchItem, Watchlist};
use muzik_core::thumbnails;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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

    #[must_use]
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

pub type WatchlistView = Watchlist<ItemView>;
pub type PlaylistView = Playlist<ItemView>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemView {
    #[serde(flatten)]
    pub item: WatchItem,
    pub thumbnail_path: Option<String>,
    pub summary: Summary,
    pub primary_action: Option<PrimaryAction>,
    pub actions: HashMap<ItemAction, Availability>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimaryAction {
    pub action: ItemAction,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
    pub enabled: bool,
    pub reason: Option<String>,
}

/// Builds the display data without changing the saved watchlist.
#[must_use]
pub fn view(document: &Watchlist, output: &Path, cache: &Path) -> WatchlistView {
    let index = AudioIndex::scan(output);
    Watchlist {
        version: document.version,
        playlists: document
            .playlists
            .iter()
            .map(|playlist| Playlist {
                playlist_id: playlist.playlist_id.clone(),
                url: playlist.url.clone(),
                kind: playlist.kind,
                title: playlist.title.clone(),
                items: playlist
                    .items
                    .iter()
                    .map(|item| enrich(item, &index, cache))
                    .collect(),
                processed_video_ids: playlist.processed_video_ids.clone(),
                last_checked_at: playlist.last_checked_at.clone(),
                last_error: playlist.last_error.clone(),
                extra: playlist.extra.clone(),
            })
            .collect(),
    }
}

fn enrich(item: &WatchItem, index: &AudioIndex, cache: &Path) -> ItemView {
    let summary = Summary::of(item);
    let primary_action = if matches!(
        summary,
        Summary::Unavailable | Summary::Processed | Summary::Waiting
    ) {
        None
    } else {
        let (action, label) = if item.statuses().any(|status| status == StageStatus::Failed) {
            (ItemAction::Retry, "Retry")
        } else if item.statuses().any(|status| status == StageStatus::Stale) {
            (ItemAction::Run, "Resume")
        } else {
            (ItemAction::Run, "Run")
        };
        Some(PrimaryAction {
            action,
            label: label.to_owned(),
        })
    };
    let thumbnail_path = item
        .video_id
        .as_deref()
        .and_then(|id| thumbnails::cached_path(id, cache))
        .map(|path| path.to_string_lossy().into_owned());
    let audio = item.downloaded_audio(index);
    let actions = ItemAction::ALL
        .iter()
        .map(|action| {
            let (enabled, reason) = availability(item, *action, audio.as_deref());
            (
                *action,
                Availability {
                    enabled,
                    reason: reason.map(str::to_owned),
                },
            )
        })
        .collect();
    ItemView {
        item: item.clone(),
        thumbnail_path,
        summary,
        primary_action,
        actions,
    }
}
