use super::{Holds, ItemAction, SourceKind, Stage, StageEvent, StageRecord, StageStatus, Stages};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watchlist<T = WatchItem> {
    #[serde(default = "current_version")]
    pub version: u64,
    pub playlists: Vec<Playlist<T>>,
}

impl<T> Default for Watchlist<T> {
    fn default() -> Self {
        Self {
            version: current_version(),
            playlists: Vec::new(),
        }
    }
}

const fn current_version() -> u64 {
    3
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct Playlist<T = WatchItem> {
    pub playlist_id: String,
    pub url: String,
    #[serde(default)]
    pub kind: SourceKind,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub items: Vec<T>,
    #[serde(default)]
    pub processed_video_ids: Vec<String>,
    #[serde(default)]
    pub last_checked_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchItem {
    pub position: u64,
    pub title: String,
    #[serde(default)]
    pub kind: SourceKind,
    #[serde(default)]
    pub video_id: Option<String>,
    #[serde(default)]
    pub video_url: Option<String>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<bool>,
    #[serde(default)]
    pub last_action: Option<LastAction>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub track: Option<Value>,
    #[serde(default)]
    pub stages: Stages,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum LastAction {
    Action(ItemAction),
    Refresh,
    Other(String),
}

impl From<ItemAction> for LastAction {
    fn from(action: ItemAction) -> Self {
        Self::Action(action)
    }
}

impl From<String> for LastAction {
    fn from(text: String) -> Self {
        match text.parse() {
            Ok(action) => Self::Action(action),
            Err(_) if text == "refresh" => Self::Refresh,
            Err(_) => Self::Other(text),
        }
    }
}

impl From<LastAction> for String {
    fn from(action: LastAction) -> Self {
        match action {
            LastAction::Action(action) => action.to_string(),
            LastAction::Refresh => "refresh".to_owned(),
            LastAction::Other(text) => text,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    from = "(String, Option<String>, u64)",
    into = "(String, Option<String>, u64)"
)]
pub struct ItemId {
    pub playlist_id: String,
    pub key: Option<String>,
    pub occurrence: u64,
}

impl ItemId {
    pub fn new(playlist_id: &str, key: Option<&str>, occurrence: u64) -> Self {
        Self {
            playlist_id: playlist_id.to_owned(),
            key: key.filter(|key| !key.is_empty()).map(str::to_owned),
            occurrence,
        }
    }

    /// # Errors
    /// Returns an error if `playlist_id` is missing or blank, or the item fields have the wrong type.
    pub fn from_params(params: &Value) -> Result<Self> {
        let params = ItemParams::read(params)?;
        if params.playlist_id.trim().is_empty() {
            return Err("playlist_id must be a non-empty string.".into());
        }
        Ok(Self::new(
            &params.playlist_id,
            params.key.as_deref(),
            params.occurrence,
        ))
    }

    /// # Errors
    /// Returns an error if `params` is not an object or holds item fields of the wrong type.
    pub fn write(&self, params: &mut Value) -> Result<()> {
        let mut fields = if params.is_null() {
            ItemParams::default()
        } else {
            ItemParams::read(params)?
        };
        self.playlist_id.clone_into(&mut fields.playlist_id);
        fields.key.clone_from(&self.key);
        fields.occurrence = self.occurrence;
        *params = serde_json::to_value(fields)?;
        Ok(())
    }
}

impl From<(String, Option<String>, u64)> for ItemId {
    fn from((playlist_id, key, occurrence): (String, Option<String>, u64)) -> Self {
        Self::new(&playlist_id, key.as_deref(), occurrence)
    }
}

impl From<ItemId> for (String, Option<String>, u64) {
    fn from(id: ItemId) -> Self {
        (id.playlist_id, id.key, id.occurrence)
    }
}

impl std::str::FromStr for ItemId {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        serde_json::from_str(text).map_err(|_| format!("invalid item id: {text}").into())
    }
}

impl std::fmt::Display for ItemId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        formatter.write_str(&text)
    }
}

#[derive(Default, Deserialize, Serialize)]
struct ItemParams {
    #[serde(default)]
    playlist_id: String,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    occurrence: u64,
    #[serde(flatten)]
    rest: Map<String, Value>,
}

impl ItemParams {
    fn read(params: &Value) -> Result<Self> {
        Self::deserialize(params).map_err(|error| format!("invalid item params: {error}").into())
    }
}

fn item_ids<T: AsRef<WatchItem>>(playlist_id: &str, items: &[T]) -> Vec<ItemId> {
    let mut counts = HashMap::<Option<&str>, u64>::new();
    items
        .iter()
        .map(|item| {
            let key = item.as_ref().key();
            let occurrence = counts.entry(key).or_default();
            let id = ItemId::new(playlist_id, key, *occurrence);
            *occurrence = occurrence.saturating_add(1);
            id
        })
        .collect()
}

#[must_use]
pub fn now() -> String {
    jiff::ZonedDateTime::now()
        .strftime("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

impl Watchlist {
    /// # Errors
    /// Returns an error if `value` is not a valid watchlist document.
    pub fn from_value(value: Value) -> Result<Self> {
        let watchlist: Self =
            serde_json::from_value(value).map_err(|error| format!("invalid watchlist: {error}"))?;
        watchlist.normalized()
    }

    /// # Errors
    /// Returns an error if the watchlist cannot be serialized.
    pub fn to_value(&self) -> Result<Value> {
        Ok(serde_json::to_value(self)?)
    }

    pub(super) fn normalized(mut self) -> Result<Self> {
        if !(1..=3).contains(&self.version) {
            return Err(format!("unsupported watchlist version: {}", self.version).into());
        }
        self.version = current_version();
        for (index, playlist) in self.playlists.iter_mut().enumerate() {
            playlist
                .normalize()
                .map_err(|error| format!("playlists[{index}]: {error}"))?;
        }
        Ok(self)
    }

    #[must_use]
    pub fn playlist(&self, id: &str) -> Option<&Playlist> {
        self.playlists
            .iter()
            .find(|playlist| playlist.playlist_id == id)
    }

    pub fn playlist_mut(&mut self, id: &str) -> Option<&mut Playlist> {
        self.playlists
            .iter_mut()
            .find(|playlist| playlist.playlist_id == id)
    }

    pub fn items(&self) -> impl Iterator<Item = &WatchItem> {
        self.playlists
            .iter()
            .flat_map(|playlist| playlist.items.iter())
    }
}

impl Playlist {
    pub fn new(id: &str, url: &str, kind: SourceKind, title: Option<&str>) -> Self {
        Self {
            playlist_id: id.to_owned(),
            url: url.to_owned(),
            kind,
            title: title.map(str::to_owned),
            items: Vec::new(),
            processed_video_ids: Vec::new(),
            last_checked_at: None,
            last_error: None,
            extra: Map::new(),
        }
    }

    fn normalize(&mut self) -> Result<()> {
        if self.playlist_id.trim().is_empty() {
            return Err("playlist_id must be a non-empty string".into());
        }
        if self.url.trim().is_empty() {
            return Err("url must be a non-empty string".into());
        }
        for (index, item) in self.items.iter().enumerate() {
            item.validate()
                .map_err(|error| format!("items[{index}]: {error}"))?;
        }
        let mut seen = HashSet::new();
        self.processed_video_ids
            .retain(|id| seen.insert(id.clone()));
        Ok(())
    }

    #[must_use]
    pub fn is_processed(&self, key: &str) -> bool {
        self.processed_video_ids.iter().any(|id| id == key)
    }

    pub fn mark_processed(&mut self, key: &str, processed: bool) {
        if processed {
            if !self.is_processed(key) {
                self.processed_video_ids.push(key.to_owned());
            }
        } else {
            self.processed_video_ids.retain(|id| id != key);
        }
    }

    #[must_use]
    pub fn find(&self, id: &ItemId) -> Option<usize> {
        if id.playlist_id != self.playlist_id {
            return None;
        }
        self.item_ids().iter().position(|item| item == id)
    }

    #[must_use]
    pub fn pending(&self, holds: &Holds) -> Vec<(ItemId, &WatchItem)> {
        let mut seen = HashSet::new();
        self.item_ids()
            .into_iter()
            .zip(&self.items)
            .filter(|(id, item)| !holds.is_waiting(id) && !item.is_unavailable())
            .filter(|(_, item)| {
                item.key()
                    .is_some_and(|key| !self.is_processed(key) && seen.insert(key.to_owned()))
            })
            .collect()
    }

    pub fn merge_items(&mut self, discovered: Vec<WatchItem>) {
        let mut old: HashMap<ItemId, WatchItem> = self
            .item_ids()
            .into_iter()
            .zip(self.items.drain(..))
            .collect();
        self.items = item_ids(&self.playlist_id, &discovered)
            .into_iter()
            .zip(discovered)
            .map(|(id, mut item)| {
                if let Some(previous) = old.remove(&id) {
                    item.stages = previous.stages;
                    item.last_action = previous.last_action;
                    item.last_error = previous.last_error;
                }
                item
            })
            .collect();
    }
}

impl<T: AsRef<WatchItem>> Playlist<T> {
    #[must_use]
    pub fn item_ids(&self) -> Vec<ItemId> {
        item_ids(&self.playlist_id, &self.items)
    }
}

impl AsRef<Self> for WatchItem {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl WatchItem {
    #[must_use]
    pub fn new(position: u64, title: &str, kind: SourceKind) -> Self {
        Self {
            position,
            title: title.to_owned(),
            kind,
            video_id: None,
            video_url: None,
            thumbnail_url: None,
            entry_id: None,
            unavailable: None,
            last_action: None,
            last_error: None,
            track: None,
            stages: Stages::default(),
            extra: Map::new(),
        }
    }

    fn validate(&self) -> Result<()> {
        if self.position == 0 {
            return Err("position must be a positive integer".into());
        }
        if self.title.trim().is_empty() {
            return Err("title must be a non-empty string".into());
        }
        Ok(())
    }

    #[must_use]
    pub fn key(&self) -> Option<&str> {
        let key = if self.kind.is_youtube() {
            &self.video_id
        } else {
            &self.entry_id
        };
        key.as_deref().filter(|key| !key.is_empty())
    }

    #[must_use]
    pub const fn stage(&self, stage: Stage) -> &StageRecord {
        match stage {
            Stage::Download => &self.stages.download,
            Stage::Quality => &self.stages.quality,
            Stage::Parse => &self.stages.parse,
            Stage::Split => &self.stages.split,
            Stage::Organize => &self.stages.organize,
        }
    }

    pub(super) const fn stage_mut(&mut self, stage: Stage) -> &mut StageRecord {
        match stage {
            Stage::Download => &mut self.stages.download,
            Stage::Quality => &mut self.stages.quality,
            Stage::Parse => &mut self.stages.parse,
            Stage::Split => &mut self.stages.split,
            Stage::Organize => &mut self.stages.organize,
        }
    }

    #[must_use]
    pub const fn status(&self, stage: Stage) -> StageStatus {
        self.stage(stage).status()
    }

    pub fn statuses(&self) -> impl Iterator<Item = StageStatus> + '_ {
        Stage::ALL.iter().map(|stage| self.status(*stage))
    }

    #[must_use]
    pub fn path(&self, stage: Stage) -> Option<&Path> {
        self.stage(stage).path()
    }

    #[must_use]
    pub fn kept_path(&self, stage: Stage) -> Option<PathBuf> {
        self.path(stage).map(Path::to_path_buf)
    }

    pub fn apply(&mut self, stage: Stage, event: StageEvent) {
        self.stage_mut(stage).apply(event);
    }

    pub fn invalidate(&mut self, stages: &[Stage]) {
        for stage in stages {
            self.apply(*stage, StageEvent::Invalidate);
        }
    }

    pub fn record_action(&mut self, action: ItemAction) {
        self.last_action = Some(action.into());
        self.last_error = None;
    }

    pub fn fail(&mut self, stage: Stage, action: ItemAction, message: &str) {
        self.last_action = Some(action.into());
        self.last_error = Some(message.to_owned());
        self.apply(stage, StageEvent::Fail(message.to_owned()));
    }

    /// Fails `stage` for a job that ended before it started, unless the stage is done.
    pub fn fail_before_start(&mut self, stage: Stage, action: ItemAction, message: &str) {
        if !self.status(stage).is_done() {
            self.fail(stage, action, message);
        }
    }

    pub(super) fn record_run(&mut self, run: &Self) {
        self.stages.clone_from(&run.stages);
        self.last_action.clone_from(&run.last_action);
        self.last_error.clone_from(&run.last_error);
    }

    pub fn is_done(&self) -> bool {
        self.statuses().all(StageStatus::is_done)
    }

    #[must_use]
    pub fn is_gone(&self) -> bool {
        self.unavailable == Some(true)
            || self.status(Stage::Download) == StageStatus::Failed
                && self.stage(Stage::Download).error().is_some_and(|error| {
                    error.lines().any(|line| {
                        line.contains("ERROR:")
                            && [": Video unavailable", ": Private video"]
                                .iter()
                                .any(|end| line.trim_end().ends_with(end))
                    })
                })
    }

    pub fn is_unavailable(&self) -> bool {
        self.is_gone() || self.video_id.as_deref().is_none_or(str::is_empty)
    }

    pub fn downloaded_audio(&self, index: &AudioIndex) -> Option<PathBuf> {
        self.path(Stage::Download)
            .filter(|path| path.is_file())
            .map(Path::to_path_buf)
            .or_else(|| {
                self.download_folder(&index.output)
                    .and_then(|folder| first_audio(&folder))
            })
            .or_else(|| index.find(self.video_id.as_deref()?))
    }
}

pub struct AudioIndex {
    output: PathBuf,
    by_id: HashMap<String, PathBuf>,
}

impl AudioIndex {
    #[must_use]
    pub fn scan(output: &Path) -> Self {
        let mut by_id = HashMap::<String, PathBuf>::new();
        walk_audio(output, |path| {
            if let Some(id) = bracketed_id(&path) {
                by_id
                    .entry(id)
                    .and_modify(|current| {
                        if path < *current {
                            current.clone_from(&path);
                        }
                    })
                    .or_insert(path);
            }
        });
        Self {
            output: output.to_path_buf(),
            by_id,
        }
    }

    #[must_use]
    pub fn find(&self, id: &str) -> Option<PathBuf> {
        self.by_id.get(id).cloned()
    }
}

fn first_audio(folder: &Path) -> Option<PathBuf> {
    let mut first: Option<PathBuf> = None;
    walk_audio(folder, |path| {
        if first.as_ref().is_none_or(|current| path < *current) {
            first = Some(path);
        }
    });
    first
}

fn walk_audio(root: &Path, mut found: impl FnMut(PathBuf)) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            if path.is_dir() {
                pending.push(path);
            } else if muzik_core::audio::is_audio(&path) {
                found(path);
            }
        }
    }
}

fn bracketed_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let id = stem.rsplit_once('[')?.1.strip_suffix(']')?;
    (!id.is_empty()).then(|| id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{AudioIndex, LastAction, WatchItem, Watchlist};
    use crate::watchlist::{ItemAction, SourceKind, Stage, StageEvent, StageStatus};
    use serde_json::json;
    use std::fs;

    #[test]
    fn a_stored_document_reads_and_writes_the_same_fields() {
        let stored = json!({"version":2,"playlists":[{
            "playlist_id":"PL1","url":"https://www.youtube.com/playlist?list=PL1","kind":"youtube",
            "title":null,"last_checked_at":null,"last_error":null,"processed_video_ids":["a","a"],
            "custom":"kept",
            "items":[{"position":1,"title":"Song","video_id":"abcdefghijk","video_url":"u",
                "thumbnail_url":null,"entry_id":null,"kind":"youtube","last_action":null,
                "last_error":null,"track":null,"note":1,
                "stages":{"download":{"status":"complete","updated_at":null,"path":"/a.flac",
                    "error":null}}}]}]});
        let watchlist = Watchlist::from_value(stored).unwrap();
        let value = watchlist.to_value().unwrap();
        assert_eq!(value["version"], 3);
        assert_eq!(value["playlists"][0]["custom"], "kept");
        assert_eq!(value["playlists"][0]["processed_video_ids"], json!(["a"]));
        let item = &value["playlists"][0]["items"][0];
        assert_eq!(item["note"], 1);
        assert_eq!(item["stages"]["download"]["path"], "/a.flac");
        assert_eq!(item["stages"]["quality"]["status"], "not_started");
        assert!(item.get("unavailable").is_none());
        assert_eq!(Watchlist::from_value(value).unwrap(), watchlist);
    }

    #[test]
    fn a_last_action_keeps_its_stored_text() {
        let stored = json!(["retry", "refresh", "an_old_action"]);
        let actions: Vec<LastAction> = serde_json::from_value(stored.clone()).unwrap();
        assert_eq!(
            actions,
            [
                LastAction::Action(ItemAction::Retry),
                LastAction::Refresh,
                LastAction::Other("an_old_action".into()),
            ]
        );
        assert_eq!(serde_json::to_value(actions).unwrap(), stored);
    }

    #[test]
    fn an_item_id_reads_and_writes_job_params() {
        let id = super::ItemId::from_params(
            &json!({"playlist_id":"spotify:playlist:p","key":"spotify:track:t#0","occurrence":1,"action":"run"}),
        )
        .unwrap();
        assert_eq!(
            id.to_string(),
            r#"["spotify:playlist:p","spotify:track:t#0",1]"#
        );
        let mut params = json!({"output":"/music","occurrence":9});
        id.write(&mut params).unwrap();
        assert_eq!(
            params,
            json!({"playlist_id":"spotify:playlist:p","key":"spotify:track:t#0","occurrence":1,"output":"/music"})
        );
        assert_eq!(super::ItemId::from_params(&params).unwrap(), id);
        let bare = super::ItemId::from_params(&json!({"playlist_id":"PL1","key":""})).unwrap();
        assert_eq!(bare.to_string(), r#"["PL1",null,0]"#);
        let mut params = serde_json::Value::Null;
        bare.write(&mut params).unwrap();
        assert_eq!(
            params,
            json!({"playlist_id":"PL1","key":null,"occurrence":0})
        );
        assert_eq!(super::ItemId::from_params(&params).unwrap(), bare);
        assert!(super::ItemId::from_params(&json!({"playlist_id":" ","key":"a"})).is_err());
    }

    #[test]
    fn malformed_item_ids_are_refused() {
        assert!("PL1:1:video".parse::<super::ItemId>().is_err());
        assert!(r#"["PL1","video","x"]"#.parse::<super::ItemId>().is_err());
    }

    #[test]
    fn repeated_items_keep_their_identity_when_the_playlist_moves() {
        let item = |position, video: &str| {
            let mut item = WatchItem::new(position, video, SourceKind::Youtube);
            item.video_id = Some(video.into());
            item
        };
        let mut playlist = super::Playlist::new("PL1", "u", SourceKind::Youtube, None);
        playlist.merge_items(vec![
            item(1, "aaaaaaaaaaa"),
            item(2, "bbbbbbbbbbb"),
            item(3, "aaaaaaaaaaa"),
        ]);
        let ids = playlist.item_ids();
        assert_eq!(ids[0], super::ItemId::new("PL1", Some("aaaaaaaaaaa"), 0));
        assert_eq!(ids[2], super::ItemId::new("PL1", Some("aaaaaaaaaaa"), 1));
        playlist.items[2].apply(Stage::Download, StageEvent::Complete(None));
        playlist.merge_items(vec![
            item(1, "ccccccccccc"),
            item(2, "aaaaaaaaaaa"),
            item(3, "bbbbbbbbbbb"),
            item(4, "aaaaaaaaaaa"),
        ]);
        let index = playlist.find(&ids[2]).unwrap();
        assert_eq!(playlist.items[index].position, 4);
        assert_eq!(
            playlist.items[index].status(Stage::Download),
            StageStatus::Complete
        );
        assert_eq!(
            playlist
                .find(&ids[1])
                .map(|index| playlist.items[index].position),
            Some(3)
        );
    }

    #[test]
    fn invalid_documents_are_refused() {
        for value in [
            json!({"version":4,"playlists":[]}),
            json!({"version":3,"playlists":[{"playlist_id":" ","url":"u"}]}),
            json!({"version":3,"playlists":[{"playlist_id":"P","url":"u","items":[{"position":0,"title":"x"}]}]}),
            json!({"version":3,"playlists":[{"playlist_id":"P","url":"u","kind":"radio"}]}),
            json!({"version":3,"playlists":[{"playlist_id":"P","url":"u","items":[{"position":1,"title":"x","stages":{"download":{"status":"lost"}}}]}]}),
        ] {
            assert!(Watchlist::from_value(value).is_err());
        }
    }

    #[test]
    fn transitions_keep_one_record_rule() {
        let mut item = WatchItem::new(1, "Song", SourceKind::Youtube);
        item.apply(
            Stage::Download,
            StageEvent::Complete(Some("/a.flac".into())),
        );
        item.fail(Stage::Split, ItemAction::SplitAgain, "split failed");
        assert_eq!(item.last_error.as_deref(), Some("split failed"));
        assert_eq!(item.stage(Stage::Split).error(), Some("split failed"));
        assert_eq!(
            item.path(Stage::Download),
            Some(std::path::Path::new("/a.flac"))
        );
        item.record_action(ItemAction::ParseAgain);
        assert_eq!(item.last_error, None);
        assert_eq!(item.stage(Stage::Split).error(), Some("split failed"));
        item.invalidate(&[Stage::Organize]);
        assert_eq!(item.status(Stage::Organize), StageStatus::Stale);
        assert!(item.stage(Stage::Organize).updated_at().is_some());
    }

    #[test]
    fn audio_is_found_in_nested_folders_by_its_id() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("album");
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("Song [abcdefghijk].opus"), b"").unwrap();
        fs::write(directory.path().join("Song [abcdefghijk].info.json"), b"").unwrap();
        let index = AudioIndex::scan(directory.path());
        let mut item = WatchItem::new(1, "Song", SourceKind::Youtube);
        item.video_id = Some("abcdefghijk".into());
        assert_eq!(
            item.downloaded_audio(&index),
            Some(nested.join("Song [abcdefghijk].opus"))
        );
        item.video_id = Some("missing".into());
        assert_eq!(item.downloaded_audio(&index), None);
    }

    #[test]
    fn spotify_audio_is_found_in_the_download_folder_of_its_track() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory
            .path()
            .join("spotify-watchlist/spotify_track_1lb3H41lbvAN28N7yqRNL9_0");
        fs::create_dir_all(folder.join("youtube-x")).unwrap();
        let audio = folder.join("youtube-x/Gibran Alcocer - Idea 10 [5OIeIaAhQOg].opus");
        fs::write(&audio, b"").unwrap();
        fs::write(folder.join("cover.jpg"), b"").unwrap();
        let mut item = WatchItem::new(1, "Gibran Alcocer - Idea 10", SourceKind::Spotify);
        item.video_id = Some("1lb3H41lbvAN28N7yqRNL9".into());
        item.entry_id = Some("spotify:track:1lb3H41lbvAN28N7yqRNL9#0".into());
        let index = AudioIndex::scan(directory.path());
        assert_eq!(item.downloaded_audio(&index), Some(audio));
        item.entry_id = Some("spotify:track:1lb3H41lbvAN28N7yqRNL9#1".into());
        assert_eq!(item.downloaded_audio(&index), None);
    }
}
