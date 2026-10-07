//! The rules that differ between YouTube, Spotify, and Bandcamp items.

use super::{BANDCAMP_PLAYLIST_ID, ItemAction, SourceKind, Stage, WatchItem};
use muzik_core::chapters;
use std::path::Path;

pub type Availability = (bool, Option<&'static str>);

impl SourceKind {
    pub fn of_playlist_id(id: &str) -> Self {
        if id.starts_with("spotify:") {
            Self::Spotify
        } else if id == BANDCAMP_PLAYLIST_ID {
            Self::Bandcamp
        } else {
            Self::Youtube
        }
    }

    pub fn single_file(self) -> bool {
        !self.is_youtube()
    }

    pub fn keeps_current_tags(self) -> bool {
        self == Self::Spotify
    }
}

pub fn availability(item: &WatchItem, action: ItemAction, audio: Option<&Path>) -> Availability {
    if item.is_gone() {
        return (false, Some("This video is private or was removed."));
    }
    if item.video_id.as_deref().is_none_or(str::is_empty)
        || item.video_url.as_deref().is_none_or(str::is_empty)
    {
        return (false, Some("This playlist item is unavailable."));
    }
    match item.kind {
        SourceKind::Youtube => youtube(item, action, audio),
        SourceKind::Spotify => spotify(item, action, audio),
        SourceKind::Bandcamp => bandcamp(item, action),
    }
}

fn youtube(item: &WatchItem, action: ItemAction, audio: Option<&Path>) -> Availability {
    if action.stage() == Stage::Download {
        return (true, None);
    }
    if action == ItemAction::OrganizeAgain {
        return if split_exists(item) || audio.is_some() {
            (true, None)
        } else {
            (
                false,
                Some("No downloaded audio or split directory is available."),
            )
        };
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

fn spotify(item: &WatchItem, action: ItemAction, audio: Option<&Path>) -> Availability {
    if item.track.as_ref().is_none_or(|track| {
        track.is_null() || track.as_object().is_some_and(serde_json::Map::is_empty)
    }) {
        return (false, Some("This track has no saved Spotify metadata."));
    }
    if action.stage() == Stage::Download {
        return (true, None);
    }
    if action == ItemAction::OrganizeAgain {
        return if split_exists(item) || audio.is_some() {
            (true, None)
        } else {
            (false, Some("No acquired audio is available."))
        };
    }
    (
        false,
        Some(
            "A Spotify track is one file: it has no quality check, no chapters to parse, and nothing to split.",
        ),
    )
}

fn bandcamp(item: &WatchItem, action: ItemAction) -> Availability {
    if action.stage() == Stage::Download {
        return (true, None);
    }
    if action != ItemAction::OrganizeAgain {
        return (
            false,
            Some(
                "A Bandcamp purchase has no quality check, no chapters to parse, and nothing to split.",
            ),
        );
    }
    if item.path(Stage::Download).is_some_and(Path::is_dir) {
        (true, None)
    } else {
        (
            false,
            Some("Download this purchase before you organize it again."),
        )
    }
}

fn split_exists(item: &WatchItem) -> bool {
    item.path(Stage::Split).is_some_and(Path::exists)
}

#[cfg(test)]
mod tests {
    use super::availability;
    use crate::watchlist::{ItemAction, SourceKind, Stage, WatchItem};
    use serde_json::json;

    fn item(kind: SourceKind) -> WatchItem {
        let mut item = WatchItem::new(1, "Song", kind);
        item.video_id = Some("abcdefghijk".into());
        item.video_url = Some("https://example.test/item".into());
        item
    }

    #[test]
    fn each_kind_offers_only_its_own_stages() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let audio = directory.path().join("Song.flac");
        std::fs::write(&audio, b"")?;
        let youtube = item(SourceKind::Youtube);
        assert!(availability(&youtube, ItemAction::CheckQualityAgain, Some(&audio)).0);
        assert!(!availability(&youtube, ItemAction::ParseAgain, None).0);
        let mut spotify = item(SourceKind::Spotify);
        assert!(!availability(&spotify, ItemAction::Run, None).0);
        spotify.track = Some(json!({"title":"Song"}));
        assert!(availability(&spotify, ItemAction::Run, None).0);
        assert!(availability(&spotify, ItemAction::OrganizeAgain, Some(&audio)).0);
        assert!(!availability(&spotify, ItemAction::SplitAgain, Some(&audio)).0);
        let mut bandcamp = item(SourceKind::Bandcamp);
        assert!(!availability(&bandcamp, ItemAction::OrganizeAgain, None).0);
        bandcamp.set_path(Stage::Download, Some(directory.path().to_path_buf()));
        assert!(availability(&bandcamp, ItemAction::OrganizeAgain, None).0);
        assert!(!availability(&bandcamp, ItemAction::ParseAgain, None).0);
        Ok(())
    }

    #[test]
    fn the_playlist_id_names_the_source_kind() {
        assert_eq!(
            SourceKind::of_playlist_id("spotify:liked"),
            SourceKind::Spotify
        );
        assert_eq!(
            SourceKind::of_playlist_id("bandcamp:collection"),
            SourceKind::Bandcamp
        );
        assert_eq!(SourceKind::of_playlist_id("PL123"), SourceKind::Youtube);
        assert!(SourceKind::Spotify.keeps_current_tags());
        assert!(!SourceKind::Bandcamp.keeps_current_tags());
    }
}
