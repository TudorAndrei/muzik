//! Versioned watchlist data shared with the existing application.

use crate::{db, paths};
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantArray};

pub mod jobs;
mod library_lookup;
mod reconcile;
mod view;

pub use reconcile::{reconcile, ReconcileOptions};
pub use view::{view, Summary};

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
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Stage {
    Download,
    Quality,
    Parse,
    Split,
    Organize,
}

impl Stage {
    pub const ALL: &'static [Self] = <Self as strum::VariantArray>::VARIANTS;

    pub fn resume_action(self) -> ItemAction {
        match self {
            Self::Organize => ItemAction::OrganizeAgain,
            Self::Parse => ItemAction::ParseAgain,
            Self::Quality => ItemAction::CheckQualityAgain,
            Self::Download | Self::Split => ItemAction::Run,
        }
    }
}

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
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum StageStatus {
    NotStarted,
    Running,
    Waiting,
    Complete,
    Failed,
    Skipped,
    Stale,
}

impl StageStatus {
    pub fn is_done(self) -> bool {
        matches!(self, Self::Complete | Self::Skipped)
    }
}

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
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ItemAction {
    Run,
    Retry,
    DownloadAgain,
    CheckQualityAgain,
    ParseAgain,
    SplitAgain,
    OrganizeAgain,
    RunAllAgain,
}

impl ItemAction {
    pub const ALL: &'static [Self] = <Self as strum::VariantArray>::VARIANTS;

    pub fn stage(self) -> Stage {
        match self {
            Self::CheckQualityAgain => Stage::Quality,
            Self::ParseAgain => Stage::Parse,
            Self::SplitAgain => Stage::Split,
            Self::OrganizeAgain => Stage::Organize,
            Self::Run | Self::Retry | Self::DownloadAgain | Self::RunAllAgain => Stage::Download,
        }
    }

    pub fn replaces_files(self) -> bool {
        matches!(
            self,
            Self::DownloadAgain
                | Self::ParseAgain
                | Self::SplitAgain
                | Self::OrganizeAgain
                | Self::RunAllAgain
        )
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum SourceKind {
    #[default]
    Youtube,
    Spotify,
    Bandcamp,
}

impl SourceKind {
    pub fn of(value: &Value) -> Self {
        value["kind"]
            .as_str()
            .and_then(|kind| kind.parse().ok())
            .unwrap_or_default()
    }

    pub fn is_youtube(self) -> bool {
        self == Self::Youtube
    }
}

pub const BANDCAMP_PLAYLIST_ID: &str = "bandcamp:collection";

pub fn bandcamp_source(user: &str) -> Value {
    playlist(
        BANDCAMP_PLAYLIST_ID,
        &format!("https://bandcamp.com/{user}"),
        SourceKind::Bandcamp,
        Some("Bandcamp collection"),
    )
}

pub fn is_unavailable(item: &Value) -> bool {
    is_gone(item) || item["video_id"].as_str().is_none_or(str::is_empty)
}

fn is_gone(item: &Value) -> bool {
    item["unavailable"] == true
        || stage_status(item, Stage::Download) == Some(StageStatus::Failed)
            && item["stages"][Stage::Download.as_ref()]["error"]
                .as_str()
                .is_some_and(|error| {
                    error.lines().any(|line| {
                        line.contains("ERROR:")
                            && [": Video unavailable", ": Private video"]
                                .iter()
                                .any(|end| line.trim_end().ends_with(end))
                    })
                })
}

pub fn stage_status(item: &Value, stage: Stage) -> Option<StageStatus> {
    item["stages"][stage.as_ref()]["status"]
        .as_str()?
        .parse()
        .ok()
}

pub fn set_stage_status(item: &mut Value, stage: Stage, status: StageStatus) {
    item["stages"][stage.as_ref()]["status"] = json!(status);
}

pub fn stage_statuses(item: &Value) -> Vec<StageStatus> {
    Stage::ALL
        .iter()
        .filter_map(|stage| stage_status(item, *stage))
        .collect()
}

pub struct Repository {
    path: PathBuf,
    legacy: Option<PathBuf>,
}

static WRITER: Mutex<()> = Mutex::new(());

impl Default for Repository {
    fn default() -> Self {
        Self::new(db::default_path()).with_legacy(paths::config_dir().join("watchlist.json"))
    }
}

impl Repository {
    pub fn new(path: PathBuf) -> Self {
        Self { path, legacy: None }
    }

    pub fn with_legacy(mut self, legacy: PathBuf) -> Self {
        self.legacy = Some(legacy);
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn locked<T>(&self, work: impl FnOnce() -> T) -> T {
        let _writer = WRITER.lock().unwrap_or_else(PoisonError::into_inner);
        work()
    }

    pub fn update<T>(
        &self,
        change: impl FnOnce(&mut Value) -> Result<T, String>,
    ) -> Result<T, String> {
        self.locked(|| {
            let mut connection = self.connect()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db::text)?;
            let before = read_document(&transaction)?;
            let mut document = before.clone();
            let result = change(&mut document)?;
            write_changes(&transaction, &before, &normalize(document)?)?;
            transaction.commit().map_err(db::text)?;
            Ok(result)
        })
    }

    pub fn load(&self) -> Result<Value, String> {
        read_document(&self.connect()?)
    }

    pub fn save(&self, value: Value) -> Result<(), String> {
        let value = normalize(value)?;
        let mut connection = self.connect()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db::text)?;
        let before = read_document(&transaction)?;
        write_changes(&transaction, &before, &value)?;
        transaction.commit().map_err(db::text)
    }

    pub fn revision(&self) -> Result<i64, String> {
        self.connect()?
            .query_row(
                "SELECT revision FROM watchlist_revision WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(db::text)
    }

    pub fn ensure(&self, source: &Value) -> Result<bool, String> {
        if self.load()?["playlists"]
            .as_array()
            .is_some_and(|playlists| {
                playlists
                    .iter()
                    .any(|item| item.get("playlist_id") == source.get("playlist_id"))
            })
        {
            return Ok(false);
        }
        self.update(|document| {
            let playlists = playlists_mut(document)?;
            if playlists
                .iter()
                .any(|item| item.get("playlist_id") == source.get("playlist_id"))
            {
                return Ok(false);
            }
            playlists.push(source.clone());
            Ok(true)
        })
    }

    pub fn add(&self, input: &str) -> Result<Value, String> {
        let source = parse_source(input)?;
        self.update(|document| {
            let playlists = playlists_mut(document)?;
            if playlists
                .iter()
                .any(|item| item.get("playlist_id") == source.get("playlist_id"))
            {
                return Err("playlist is already in the watchlist".into());
            }
            playlists.push(source.clone());
            Ok(source)
        })
    }

    pub fn rename(&self, playlist_id: &str, title: &str) -> Result<bool, String> {
        self.update(|document| {
            let Some(playlist) = playlists_mut(document)?
                .iter_mut()
                .find(|item| item.get("playlist_id").and_then(Value::as_str) == Some(playlist_id))
            else {
                return Ok(false);
            };
            let fields = playlist
                .as_object_mut()
                .ok_or("playlist is not an object")?;
            let title = title.trim();
            fields.insert(
                "title".into(),
                if title.is_empty() {
                    Value::Null
                } else {
                    json!(title)
                },
            );
            Ok(true)
        })
    }

    pub fn remove(&self, playlist_id: &str) -> Result<bool, String> {
        self.update(|document| {
            let playlists = playlists_mut(document)?;
            let before = playlists.len();
            playlists.retain(|item| {
                item.get("playlist_id").and_then(Value::as_str) != Some(playlist_id)
            });
            Ok(playlists.len() != before)
        })
    }

    fn connect(&self) -> Result<Connection, String> {
        let mut connection = db::open(&self.path)?;
        if let Some(legacy) = &self.legacy {
            import_legacy(&mut connection, legacy)?;
        }
        Ok(connection)
    }
}

fn playlists_mut(document: &mut Value) -> Result<&mut Vec<Value>, String> {
    document
        .get_mut("playlists")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "watchlist playlists are missing".into())
}

fn import_legacy(connection: &mut Connection, legacy: &Path) -> Result<(), String> {
    if !legacy.is_file() {
        return Ok(());
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db::text)?;
    let stored: i64 = transaction
        .query_row("SELECT COUNT(*) FROM watchlist_playlists", [], |row| {
            row.get(0)
        })
        .map_err(db::text)?;
    if stored > 0 {
        return Ok(());
    }
    let source = fs::read_to_string(legacy)
        .map_err(|error| format!("cannot read {}: {error}", legacy.display()))?;
    let value: Value = serde_json::from_str(&source)
        .map_err(|error| format!("invalid watchlist {}: {error}", legacy.display()))?;
    let value = normalize(value)?;
    write_changes(&transaction, &json!({"playlists": []}), &value)?;
    transaction.commit().map_err(db::text)?;
    let mut backup = legacy.as_os_str().to_owned();
    backup.push(".migrated");
    fs::rename(legacy, backup).map_err(|error| error.to_string())
}

fn read_document(connection: &Connection) -> Result<Value, String> {
    let mut playlists = Vec::new();
    let mut positions = std::collections::HashMap::new();
    let mut statement = connection
        .prepare("SELECT playlist_id, data FROM watchlist_playlists ORDER BY ordinal")
        .map_err(db::text)?;
    let mut rows = statement.query([]).map_err(db::text)?;
    while let Some(row) = rows.next().map_err(db::text)? {
        let id: String = row.get(0).map_err(db::text)?;
        let data: String = row.get(1).map_err(db::text)?;
        let mut playlist: Value = serde_json::from_str(&data)
            .map_err(|error| format!("invalid playlist {id}: {error}"))?;
        playlist["items"] = json!([]);
        positions.insert(id, playlists.len());
        playlists.push(playlist);
    }
    let mut statement = connection
        .prepare("SELECT playlist_id, data FROM watchlist_items ORDER BY playlist_id, ordinal")
        .map_err(db::text)?;
    let mut rows = statement.query([]).map_err(db::text)?;
    while let Some(row) = rows.next().map_err(db::text)? {
        let id: String = row.get(0).map_err(db::text)?;
        let data: String = row.get(1).map_err(db::text)?;
        let item: Value = serde_json::from_str(&data)
            .map_err(|error| format!("invalid item in playlist {id}: {error}"))?;
        let index = *positions
            .get(&id)
            .ok_or_else(|| format!("item belongs to missing playlist {id}"))?;
        if let Some(items) = playlists[index]["items"].as_array_mut() {
            items.push(item);
        }
    }
    normalize(json!({"version": 3, "playlists": playlists}))
}

fn write_changes(connection: &Connection, before: &Value, after: &Value) -> Result<(), String> {
    let empty = Vec::new();
    let old_playlists = before["playlists"].as_array().unwrap_or(&empty);
    let new_playlists = after["playlists"].as_array().unwrap_or(&empty);
    let old: std::collections::HashMap<&str, (usize, &Value)> = old_playlists
        .iter()
        .enumerate()
        .filter_map(|(index, playlist)| {
            Some((playlist["playlist_id"].as_str()?, (index, playlist)))
        })
        .collect();
    let kept: std::collections::HashSet<&str> = new_playlists
        .iter()
        .filter_map(|playlist| playlist["playlist_id"].as_str())
        .collect();
    let mut changed = false;
    for id in old.keys().filter(|id| !kept.contains(*id)) {
        connection
            .execute(
                "DELETE FROM watchlist_playlists WHERE playlist_id = ?1",
                [id],
            )
            .map_err(db::text)?;
        changed = true;
    }
    for (ordinal, playlist) in new_playlists.iter().enumerate() {
        let id = playlist["playlist_id"]
            .as_str()
            .ok_or("playlist_id must be a non-empty string")?;
        let previous = old.get(id);
        let header = without_items(playlist);
        if previous.is_none_or(|(index, value)| *index != ordinal || without_items(value) != header)
        {
            connection
                .execute(
                    "INSERT INTO watchlist_playlists (playlist_id, ordinal, data)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT (playlist_id) DO UPDATE
                     SET ordinal = excluded.ordinal, data = excluded.data",
                    rusqlite::params![id, db::integer(ordinal)?, header.to_string()],
                )
                .map_err(db::text)?;
            changed = true;
        }
        let old_items = previous
            .and_then(|(_, value)| value["items"].as_array())
            .unwrap_or(&empty);
        let new_items = playlist["items"].as_array().unwrap_or(&empty);
        for (ordinal, item) in new_items.iter().enumerate() {
            if old_items.get(ordinal) != Some(item) {
                connection
                    .execute(
                        "INSERT INTO watchlist_items (playlist_id, ordinal, data)
                         VALUES (?1, ?2, ?3)
                         ON CONFLICT (playlist_id, ordinal) DO UPDATE SET data = excluded.data",
                        rusqlite::params![id, db::integer(ordinal)?, item.to_string()],
                    )
                    .map_err(db::text)?;
                changed = true;
            }
        }
        if old_items.len() > new_items.len() {
            connection
                .execute(
                    "DELETE FROM watchlist_items WHERE playlist_id = ?1 AND ordinal >= ?2",
                    rusqlite::params![id, db::integer(new_items.len())?],
                )
                .map_err(db::text)?;
            changed = true;
        }
    }
    if changed {
        connection
            .execute(
                "UPDATE watchlist_revision SET revision = revision + 1 WHERE id = 1",
                [],
            )
            .map_err(db::text)?;
    }
    Ok(())
}

fn without_items(playlist: &Value) -> Value {
    let mut header = playlist.clone();
    if let Some(fields) = header.as_object_mut() {
        fields.remove("items");
    }
    header
}

fn normalize(mut value: Value) -> Result<Value, String> {
    let root = value.as_object_mut().ok_or("watchlist must be an object")?;
    let version = root.get("version").and_then(Value::as_u64);
    if !matches!(version, Some(1..=3)) {
        return Err(format!("unsupported watchlist version: {version:?}"));
    }
    let playlists = root
        .get_mut("playlists")
        .and_then(Value::as_array_mut)
        .ok_or("watchlist playlists must be an array")?;
    for (index, playlist) in playlists.iter_mut().enumerate() {
        normalize_playlist(playlist).map_err(|error| format!("playlists[{index}]: {error}"))?;
    }
    root.insert("version".into(), json!(3));
    Ok(value)
}

fn normalize_playlist(value: &mut Value) -> Result<(), String> {
    let playlist = value.as_object_mut().ok_or("playlist must be an object")?;
    required_string(playlist, "playlist_id")?;
    required_string(playlist, "url")?;
    optional_string(playlist, "title")?;
    optional_string(playlist, "last_checked_at")?;
    optional_string(playlist, "last_error")?;
    kind(playlist)?;
    let items = playlist.entry("items").or_insert_with(|| json!([]));
    let items = items.as_array_mut().ok_or("items must be an array")?;
    for (index, item) in items.iter_mut().enumerate() {
        normalize_item(item).map_err(|error| format!("items[{index}]: {error}"))?;
    }
    let processed = playlist
        .entry("processed_video_ids")
        .or_insert_with(|| json!([]));
    let processed = processed
        .as_array_mut()
        .ok_or("processed_video_ids must be an array")?;
    for id in processed.iter() {
        id.as_str()
            .ok_or("processed_video_ids must contain strings")?;
    }
    let mut deduplicated = std::collections::HashSet::new();
    processed.retain(|value| {
        value
            .as_str()
            .is_some_and(|id| deduplicated.insert(id.to_owned()))
    });
    Ok(())
}

fn normalize_item(value: &mut Value) -> Result<(), String> {
    let item = value.as_object_mut().ok_or("item must be an object")?;
    if item
        .get("position")
        .and_then(Value::as_u64)
        .is_none_or(|position| position == 0)
    {
        return Err("position must be a positive integer".into());
    }
    required_string(item, "title")?;
    for key in [
        "video_id",
        "video_url",
        "thumbnail_url",
        "entry_id",
        "last_action",
        "last_error",
    ] {
        optional_string(item, key)?;
    }
    kind(item)?;
    if item
        .get("unavailable")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("unavailable must be a boolean".into());
    }
    let track = item.entry("track").or_insert(Value::Null);
    if !track.is_null() && !track.is_object() {
        return Err("track must be an object or null".into());
    }
    let stages = item.entry("stages").or_insert_with(|| json!({}));
    let stages = stages.as_object_mut().ok_or("stages must be an object")?;
    for stage in Stage::ALL {
        let name = stage.as_ref();
        let record = stages.entry(name).or_insert_with(|| json!({}));
        normalize_stage(record).map_err(|error| format!("stages.{name}: {error}"))?;
    }
    Ok(())
}

fn normalize_stage(value: &mut Value) -> Result<(), String> {
    let record = value.as_object_mut().ok_or("stage must be an object")?;
    let status = record
        .entry("status")
        .or_insert_with(|| json!(StageStatus::NotStarted));
    if status
        .as_str()
        .is_none_or(|status| status.parse::<StageStatus>().is_err())
    {
        return Err("unknown stage status".into());
    }
    for key in ["updated_at", "path", "error"] {
        optional_string(record, key)?;
    }
    Ok(())
}

fn required_string(map: &Map<String, Value>, key: &str) -> Result<(), String> {
    if map
        .get(key)
        .and_then(Value::as_str)
        .is_none_or(|value| value.trim().is_empty())
    {
        Err(format!("{key} must be a non-empty string"))
    } else {
        Ok(())
    }
}

fn optional_string(map: &mut Map<String, Value>, key: &str) -> Result<(), String> {
    let value = map.entry(key).or_insert(Value::Null);
    if value.is_null() || value.is_string() {
        Ok(())
    } else {
        Err(format!("{key} must be a string or null"))
    }
}

fn kind(map: &mut Map<String, Value>) -> Result<(), String> {
    let value = map
        .entry("kind")
        .or_insert_with(|| json!(SourceKind::Youtube));
    if value
        .as_str()
        .is_some_and(|kind| kind.parse::<SourceKind>().is_ok())
    {
        Ok(())
    } else {
        Err("unknown source kind".into())
    }
}

pub fn parse_source(input: &str) -> Result<Value, String> {
    let text = input.trim();
    let lower = text.to_ascii_lowercase();
    if matches!(
        lower.trim_end_matches('/'),
        "liked" | "liked songs" | "spotify:liked" | "https://open.spotify.com/collection/tracks"
    ) {
        return Ok(playlist(
            "spotify:liked",
            "https://open.spotify.com/collection/tracks",
            SourceKind::Spotify,
            Some("Liked Songs"),
        ));
    }
    if let Some((_, query)) = text.split_once('?') {
        if let Some(id) = query.split('&').find_map(|pair| pair.strip_prefix("list=")) {
            if !id.is_empty()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return Ok(playlist(
                    id,
                    &format!("https://www.youtube.com/playlist?list={id}"),
                    SourceKind::Youtube,
                    None,
                ));
            }
        }
    }
    let spotify = text
        .strip_prefix("spotify:playlist:")
        .map(|id| ("playlist", id))
        .or_else(|| text.strip_prefix("spotify:album:").map(|id| ("album", id)))
        .or_else(|| {
            let (_, rest) = text.split_once("open.spotify.com/")?;
            let rest = rest
                .strip_prefix("intl-")
                .and_then(|part| part.split_once('/').map(|(_, path)| path))
                .unwrap_or(rest);
            let (kind, id) = rest.split_once('/')?;
            Some((kind, id.split(['?', '/']).next()?))
        });
    if let Some((kind, id)) = spotify {
        if matches!(kind, "playlist" | "album")
            && id.len() >= 10
            && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Ok(playlist(
                &format!("spotify:{kind}:{id}"),
                &format!("https://open.spotify.com/{kind}/{id}"),
                SourceKind::Spotify,
                None,
            ));
        }
    }
    Err("enter a YouTube playlist URL, Spotify playlist or album link, or liked".into())
}

fn playlist(id: &str, url: &str, kind: SourceKind, title: Option<&str>) -> Value {
    json!({
        "playlist_id": id,
        "url": url,
        "kind": kind,
        "title": title,
        "items": [],
        "processed_video_ids": [],
        "last_checked_at": null,
        "last_error": null
    })
}
