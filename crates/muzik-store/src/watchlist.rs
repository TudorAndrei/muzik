//! Versioned watchlist data shared with the existing application.

use crate::db;
use muzik_core::paths::Paths;
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantArray};

mod item;
pub mod jobs;
mod legacy;
mod library_lookup;
mod reconcile;
pub mod source;
mod view;

pub use item::{now, AudioIndex, ItemId, Playlist, StageRecord, Stages, WatchItem, Watchlist};
pub use legacy::import_cache;
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

    pub fn of_decision(kind: muzik_core::DecisionKind) -> Self {
        match kind {
            muzik_core::DecisionKind::ImportMatch | muzik_core::DecisionKind::ImportDuplicate => {
                Self::Organize
            }
            muzik_core::DecisionKind::ChapterReview | muzik_core::DecisionKind::ChapterEdit => {
                Self::Parse
            }
            muzik_core::DecisionKind::QualityReplacement => Self::Quality,
            muzik_core::DecisionKind::SoulseekCandidate => Self::Download,
        }
    }

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
pub enum StageStatus {
    #[default]
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

pub fn bandcamp_source(user: &str) -> Playlist {
    Playlist::new(
        BANDCAMP_PLAYLIST_ID,
        &format!("https://bandcamp.com/{user}"),
        SourceKind::Bandcamp,
        Some("Bandcamp collection"),
    )
}

pub fn stage_status(item: &Value, stage: Stage) -> Option<StageStatus> {
    item["stages"][stage.as_ref()]["status"]
        .as_str()?
        .parse()
        .ok()
}

pub struct Repository {
    path: PathBuf,
    legacy: Option<PathBuf>,
}

static WRITER: Mutex<()> = Mutex::new(());

impl Repository {
    pub fn open(paths: &Paths) -> Self {
        Self::new(paths.database()).with_legacy(paths.config.join("watchlist.json"))
    }

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
        change: impl FnOnce(&mut Watchlist) -> Result<T, String>,
    ) -> Result<T, String> {
        self.update_with(|document, _| change(document))
    }

    pub fn update_with<T>(
        &self,
        change: impl FnOnce(&mut Watchlist, &Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        self.locked(|| {
            let mut connection = self.connect()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(db::text)?;
            let before = read_document(&transaction)?;
            let mut document = before.clone();
            let result = change(&mut document, &transaction)?;
            write_changes(&transaction, &before, &document.normalized()?)?;
            transaction.commit().map_err(db::text)?;
            Ok(result)
        })
    }

    pub fn load(&self) -> Result<Watchlist, String> {
        read_document(&self.connect()?)
    }

    pub fn save(&self, value: &Watchlist) -> Result<(), String> {
        let value = value.clone().normalized()?;
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

    pub fn ensure(&self, source: &Playlist) -> Result<bool, String> {
        if self.load()?.playlist(&source.playlist_id).is_some() {
            return Ok(false);
        }
        self.update(|document| {
            if document.playlist(&source.playlist_id).is_some() {
                return Ok(false);
            }
            document.playlists.push(source.clone());
            Ok(true)
        })
    }

    pub fn add(&self, input: &str) -> Result<Playlist, String> {
        let source = parse_source(input)?;
        self.update(|document| {
            if document.playlist(&source.playlist_id).is_some() {
                return Err("playlist is already in the watchlist".into());
            }
            document.playlists.push(source.clone());
            Ok(source)
        })
    }

    pub fn rename(&self, playlist_id: &str, title: &str) -> Result<bool, String> {
        self.update(|document| {
            let Some(playlist) = document.playlist_mut(playlist_id) else {
                return Ok(false);
            };
            let title = title.trim();
            playlist.title = (!title.is_empty()).then(|| title.to_owned());
            Ok(true)
        })
    }

    pub fn remove(&self, playlist_id: &str) -> Result<bool, String> {
        self.update(|document| {
            let before = document.playlists.len();
            document
                .playlists
                .retain(|playlist| playlist.playlist_id != playlist_id);
            Ok(document.playlists.len() != before)
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
    let value = Watchlist::from_value(value)?;
    write_changes(&transaction, &Watchlist::default(), &value)?;
    transaction.commit().map_err(db::text)?;
    let mut backup = legacy.as_os_str().to_owned();
    backup.push(".migrated");
    fs::rename(legacy, backup).map_err(|error| error.to_string())
}

fn read_document(connection: &Connection) -> Result<Watchlist, String> {
    let mut playlists = Vec::new();
    let mut positions = HashMap::new();
    let mut statement = connection
        .prepare("SELECT playlist_id, data FROM watchlist_playlists ORDER BY ordinal")
        .map_err(db::text)?;
    let mut rows = statement.query([]).map_err(db::text)?;
    while let Some(row) = rows.next().map_err(db::text)? {
        let id: String = row.get(0).map_err(db::text)?;
        let data: String = row.get(1).map_err(db::text)?;
        let mut playlist: Playlist = serde_json::from_str(&data)
            .map_err(|error| format!("invalid playlist {id}: {error}"))?;
        playlist.items.clear();
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
        let item: WatchItem = serde_json::from_str(&data)
            .map_err(|error| format!("invalid item in playlist {id}: {error}"))?;
        let index = *positions
            .get(&id)
            .ok_or_else(|| format!("item belongs to missing playlist {id}"))?;
        playlists[index].items.push(item);
    }
    Watchlist {
        playlists,
        ..Watchlist::default()
    }
    .normalized()
}

fn header(playlist: &Playlist) -> Result<String, String> {
    let mut value = serde_json::to_value(Playlist {
        items: Vec::new(),
        ..playlist.clone()
    })
    .map_err(|error| error.to_string())?;
    if let Some(fields) = value.as_object_mut() {
        fields.remove("items");
    }
    Ok(value.to_string())
}

fn write_changes(
    connection: &Connection,
    before: &Watchlist,
    after: &Watchlist,
) -> Result<(), String> {
    let old: HashMap<&str, (usize, &Playlist)> = before
        .playlists
        .iter()
        .enumerate()
        .map(|(index, playlist)| (playlist.playlist_id.as_str(), (index, playlist)))
        .collect();
    let kept: HashSet<&str> = after
        .playlists
        .iter()
        .map(|playlist| playlist.playlist_id.as_str())
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
    for (ordinal, playlist) in after.playlists.iter().enumerate() {
        let id = playlist.playlist_id.as_str();
        let previous = old.get(id);
        let data = header(playlist)?;
        if previous.is_none_or(|(index, value)| {
            *index != ordinal || header(value).as_deref() != Ok(data.as_str())
        }) {
            connection
                .execute(
                    "INSERT INTO watchlist_playlists (playlist_id, ordinal, data)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT (playlist_id) DO UPDATE
                     SET ordinal = excluded.ordinal, data = excluded.data",
                    rusqlite::params![id, db::integer(ordinal)?, data],
                )
                .map_err(db::text)?;
            changed = true;
        }
        let old_items = previous.map_or(&[][..], |(_, value)| value.items.as_slice());
        for (ordinal, item) in playlist.items.iter().enumerate() {
            if old_items.get(ordinal) != Some(item) {
                let data = serde_json::to_string(item).map_err(|error| error.to_string())?;
                connection
                    .execute(
                        "INSERT INTO watchlist_items (playlist_id, ordinal, data)
                         VALUES (?1, ?2, ?3)
                         ON CONFLICT (playlist_id, ordinal) DO UPDATE SET data = excluded.data",
                        rusqlite::params![id, db::integer(ordinal)?, data],
                    )
                    .map_err(db::text)?;
                changed = true;
            }
        }
        if old_items.len() > playlist.items.len() {
            connection
                .execute(
                    "DELETE FROM watchlist_items WHERE playlist_id = ?1 AND ordinal >= ?2",
                    rusqlite::params![id, db::integer(playlist.items.len())?],
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

pub fn parse_source(input: &str) -> Result<Playlist, String> {
    let text = input.trim();
    let lower = text.to_ascii_lowercase();
    if matches!(
        lower.trim_end_matches('/'),
        "liked" | "liked songs" | "spotify:liked" | "https://open.spotify.com/collection/tracks"
    ) {
        return Ok(Playlist::new(
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
                return Ok(Playlist::new(
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
            return Ok(Playlist::new(
                &format!("spotify:{kind}:{id}"),
                &format!("https://open.spotify.com/{kind}/{id}"),
                SourceKind::Spotify,
                None,
            ));
        }
    }
    Err("enter a YouTube playlist URL, Spotify playlist or album link, or liked".into())
}
