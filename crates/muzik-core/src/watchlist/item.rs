use super::{ItemAction, SourceKind, Stage, StageStatus};
use chrono::{Local, SecondsFormat};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Watchlist {
    #[serde(default = "current_version")]
    pub version: u64,
    pub playlists: Vec<Playlist>,
}

impl Default for Watchlist {
    fn default() -> Self {
        Self {
            version: current_version(),
            playlists: Vec::new(),
        }
    }
}

fn current_version() -> u64 {
    3
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub playlist_id: String,
    pub url: String,
    #[serde(default)]
    pub kind: SourceKind,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub items: Vec<WatchItem>,
    #[serde(default)]
    pub processed_video_ids: Vec<String>,
    #[serde(default)]
    pub last_checked_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    pub last_action: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub track: Option<Value>,
    #[serde(default)]
    pub stages: Stages,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stages {
    #[serde(default)]
    pub download: StageRecord,
    #[serde(default)]
    pub quality: StageRecord,
    #[serde(default)]
    pub parse: StageRecord,
    #[serde(default)]
    pub split: StageRecord,
    #[serde(default)]
    pub organize: StageRecord,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StageRecord {
    #[serde(default)]
    pub status: StageStatus,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<Value>,
}

pub fn now() -> String {
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

impl Watchlist {
    pub fn from_value(value: Value) -> Result<Self, String> {
        let watchlist: Self =
            serde_json::from_value(value).map_err(|error| format!("invalid watchlist: {error}"))?;
        watchlist.normalized()
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn normalized(mut self) -> Result<Self, String> {
        if !(1..=3).contains(&self.version) {
            return Err(format!("unsupported watchlist version: {}", self.version));
        }
        self.version = current_version();
        for (index, playlist) in self.playlists.iter_mut().enumerate() {
            playlist
                .normalize()
                .map_err(|error| format!("playlists[{index}]: {error}"))?;
        }
        Ok(self)
    }

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

    fn normalize(&mut self) -> Result<(), String> {
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

    pub fn find(&self, position: u64, video_id: Option<&str>) -> Option<usize> {
        self.items
            .iter()
            .position(|item| item.position == position && item.video_id.as_deref() == video_id)
    }

    pub fn pending(&self) -> Vec<&WatchItem> {
        let mut seen = HashSet::new();
        self.items
            .iter()
            .filter(|item| !item.is_waiting() && !item.is_unavailable())
            .filter(|item| {
                item.key()
                    .is_some_and(|key| !self.is_processed(key) && seen.insert(key.to_owned()))
            })
            .collect()
    }

    pub fn merge_items(&mut self, discovered: Vec<WatchItem>) {
        let mut old = HashMap::<(String, usize), WatchItem>::new();
        let mut counts = HashMap::<String, usize>::new();
        for item in self.items.drain(..) {
            let key = item.key().unwrap_or("").to_owned();
            let occurrence = counts.entry(key.clone()).or_default();
            old.insert((key, *occurrence), item);
            *occurrence += 1;
        }
        counts.clear();
        self.items = discovered
            .into_iter()
            .map(|mut item| {
                let key = item.key().unwrap_or("").to_owned();
                let occurrence = counts.entry(key.clone()).or_default();
                if let Some(previous) = old.remove(&(key, *occurrence)) {
                    item.stages = previous.stages;
                    item.last_action = previous.last_action;
                    item.last_error = previous.last_error;
                }
                *occurrence += 1;
                item
            })
            .collect();
    }
}

impl WatchItem {
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

    fn validate(&self) -> Result<(), String> {
        if self.position == 0 {
            return Err("position must be a positive integer".into());
        }
        if self.title.trim().is_empty() {
            return Err("title must be a non-empty string".into());
        }
        Ok(())
    }

    pub fn key(&self) -> Option<&str> {
        let key = if self.kind.is_youtube() {
            &self.video_id
        } else {
            &self.entry_id
        };
        key.as_deref().filter(|key| !key.is_empty())
    }

    pub fn stage(&self, stage: Stage) -> &StageRecord {
        match stage {
            Stage::Download => &self.stages.download,
            Stage::Quality => &self.stages.quality,
            Stage::Parse => &self.stages.parse,
            Stage::Split => &self.stages.split,
            Stage::Organize => &self.stages.organize,
        }
    }

    pub fn stage_mut(&mut self, stage: Stage) -> &mut StageRecord {
        match stage {
            Stage::Download => &mut self.stages.download,
            Stage::Quality => &mut self.stages.quality,
            Stage::Parse => &mut self.stages.parse,
            Stage::Split => &mut self.stages.split,
            Stage::Organize => &mut self.stages.organize,
        }
    }

    pub fn status(&self, stage: Stage) -> StageStatus {
        self.stage(stage).status
    }

    pub fn statuses(&self) -> impl Iterator<Item = StageStatus> + '_ {
        Stage::ALL.iter().map(|stage| self.status(*stage))
    }

    pub fn path(&self, stage: Stage) -> Option<&Path> {
        self.stage(stage).path.as_deref()
    }

    pub fn set(&mut self, stage: Stage, status: StageStatus) {
        let record = self.stage_mut(stage);
        record.status = status;
        record.updated_at = Some(now());
    }

    pub fn set_path(&mut self, stage: Stage, path: Option<PathBuf>) {
        self.stage_mut(stage).path = path;
    }

    pub fn complete(&mut self, stage: Stage, path: Option<PathBuf>) {
        self.set(stage, StageStatus::Complete);
        self.set_path(stage, path);
    }

    pub fn invalidate(&mut self, stages: &[Stage]) {
        for stage in stages {
            self.set(*stage, StageStatus::Stale);
        }
    }

    pub fn start(&mut self, action: ItemAction) {
        self.last_action = Some(action.to_string());
        self.last_error = None;
        *self.stage_mut(action.stage()) = StageRecord {
            status: StageStatus::Running,
            updated_at: Some(now()),
            ..StageRecord::default()
        };
    }

    pub fn finish(&mut self, stage: Stage, action: ItemAction) {
        self.last_action = Some(action.to_string());
        self.last_error = None;
        if self.status(stage) == StageStatus::Running {
            *self.stage_mut(stage) = StageRecord {
                status: StageStatus::Complete,
                updated_at: Some(now()),
                ..StageRecord::default()
            };
        }
    }

    pub fn wait(&mut self, stage: Stage, action: ItemAction, question: Value) {
        self.last_action = Some(action.to_string());
        self.last_error = None;
        let record = self.stage_mut(stage);
        record.status = StageStatus::Waiting;
        record.updated_at = Some(now());
        record.question = Some(question);
    }

    pub fn fail(&mut self, stage: Stage, action: ItemAction, message: &str) {
        self.last_action = Some(action.to_string());
        self.last_error = Some(message.to_owned());
        *self.stage_mut(stage) = StageRecord {
            status: StageStatus::Failed,
            updated_at: Some(now()),
            error: Some(message.to_owned()),
            ..StageRecord::default()
        };
    }

    pub fn is_done(&self) -> bool {
        self.statuses().all(StageStatus::is_done)
    }

    pub fn is_waiting(&self) -> bool {
        self.statuses().any(|status| status == StageStatus::Waiting)
    }

    pub fn is_gone(&self) -> bool {
        self.unavailable == Some(true)
            || self.status(Stage::Download) == StageStatus::Failed
                && self
                    .stage(Stage::Download)
                    .error
                    .as_deref()
                    .is_some_and(|error| {
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
            .or_else(|| index.find(self.video_id.as_deref()?))
    }
}

#[derive(Default)]
pub struct AudioIndex {
    by_id: HashMap<String, PathBuf>,
}

impl AudioIndex {
    pub fn scan(output: &Path) -> Self {
        let mut index = Self::default();
        let mut pending = vec![output.to_path_buf()];
        while let Some(directory) = pending.pop() {
            let Ok(entries) = fs::read_dir(&directory) else {
                continue;
            };
            for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
                if path.is_dir() {
                    pending.push(path);
                } else if is_audio(&path) {
                    if let Some(id) = bracketed_id(&path) {
                        index
                            .by_id
                            .entry(id)
                            .and_modify(|current| {
                                if path < *current {
                                    current.clone_from(&path);
                                }
                            })
                            .or_insert(path);
                    }
                }
            }
        }
        index
    }

    pub fn find(&self, id: &str) -> Option<PathBuf> {
        self.by_id.get(id).cloned()
    }
}

fn bracketed_id(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let start = stem.rfind('[')?;
    let id = stem[start + 1..].strip_suffix(']')?;
    (!id.is_empty()).then(|| id.to_owned())
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "flac" | "mp3" | "m4a" | "opus" | "wav" | "aac"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::{AudioIndex, WatchItem, Watchlist};
    use crate::watchlist::{ItemAction, SourceKind, Stage, StageStatus};
    use serde_json::json;
    use std::fs;

    #[test]
    fn a_stored_document_reads_and_writes_the_same_fields() -> Result<(), String> {
        let stored = json!({"version":2,"playlists":[{
            "playlist_id":"PL1","url":"https://www.youtube.com/playlist?list=PL1","kind":"youtube",
            "title":null,"last_checked_at":null,"last_error":null,"processed_video_ids":["a","a"],
            "custom":"kept",
            "items":[{"position":1,"title":"Song","video_id":"abcdefghijk","video_url":"u",
                "thumbnail_url":null,"entry_id":null,"kind":"youtube","last_action":null,
                "last_error":null,"track":null,"note":1,
                "stages":{"download":{"status":"waiting","updated_at":null,"path":"/a.flac",
                    "error":null,"question":{"kind":"import_match"}}}}]}]});
        let watchlist = Watchlist::from_value(stored)?;
        let value = watchlist.to_value();
        assert_eq!(value["version"], 3);
        assert_eq!(value["playlists"][0]["custom"], "kept");
        assert_eq!(value["playlists"][0]["processed_video_ids"], json!(["a"]));
        let item = &value["playlists"][0]["items"][0];
        assert_eq!(item["note"], 1);
        assert_eq!(
            item["stages"]["download"]["question"]["kind"],
            "import_match"
        );
        assert_eq!(item["stages"]["quality"]["status"], "not_started");
        assert!(item.get("unavailable").is_none());
        assert_eq!(Watchlist::from_value(value)?, watchlist);
        Ok(())
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
        item.complete(Stage::Download, Some("/a.flac".into()));
        item.start(ItemAction::ParseAgain);
        assert_eq!(item.status(Stage::Parse), StageStatus::Running);
        item.wait(
            Stage::Parse,
            ItemAction::ParseAgain,
            json!({"kind":"chapter_review"}),
        );
        assert!(item.is_waiting());
        item.fail(Stage::Split, ItemAction::SplitAgain, "split failed");
        assert_eq!(item.last_error.as_deref(), Some("split failed"));
        assert_eq!(
            item.stage(Stage::Split).error.as_deref(),
            Some("split failed")
        );
        assert_eq!(
            item.path(Stage::Download),
            Some(std::path::Path::new("/a.flac"))
        );
        item.invalidate(&[Stage::Organize]);
        assert_eq!(item.status(Stage::Organize), StageStatus::Stale);
        assert!(item.stage(Stage::Organize).updated_at.is_some());
    }

    #[test]
    fn audio_is_found_in_nested_folders_by_its_id() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let nested = directory.path().join("album");
        fs::create_dir(&nested)?;
        fs::write(nested.join("Song [abcdefghijk].opus"), b"")?;
        fs::write(directory.path().join("Song [abcdefghijk].info.json"), b"")?;
        let index = AudioIndex::scan(directory.path());
        let mut item = WatchItem::new(1, "Song", SourceKind::Youtube);
        item.video_id = Some("abcdefghijk".into());
        assert_eq!(
            item.downloaded_audio(&index),
            Some(nested.join("Song [abcdefghijk].opus"))
        );
        item.video_id = Some("missing".into());
        assert_eq!(item.downloaded_audio(&index), None);
        Ok(())
    }
}
