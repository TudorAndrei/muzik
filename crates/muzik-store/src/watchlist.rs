//! Versioned watchlist data shared with the existing application.

use crate::{Result, db};
use muzik_core::paths::Paths;
use parking_lot::Mutex;
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantArray};

mod check;
mod item;
pub mod jobs;
mod legacy;
mod library_lookup;
mod reconcile;
pub mod source;
mod view;

pub use check::{LocalCheck, LocalCheckOptions, LocalChecks};
pub use item::{AudioIndex, ItemId, Playlist, StageRecord, Stages, WatchItem, Watchlist, now};
pub use legacy::import_cache;
pub use reconcile::{ReconcileOptions, reconcile};
pub use view::{Availability, ItemView, PlaylistView, PrimaryAction, Summary, WatchlistView, view};

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

    #[must_use]
    pub const fn of_decision(kind: muzik_core::DecisionKind) -> Self {
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

    #[must_use]
    pub const fn resume_action(self) -> ItemAction {
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
    #[must_use]
    pub const fn is_done(self) -> bool {
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

    #[must_use]
    pub const fn stage(self) -> Stage {
        match self {
            Self::CheckQualityAgain => Stage::Quality,
            Self::ParseAgain => Stage::Parse,
            Self::SplitAgain => Stage::Split,
            Self::OrganizeAgain => Stage::Organize,
            Self::Run | Self::Retry | Self::DownloadAgain | Self::RunAllAgain => Stage::Download,
        }
    }

    #[must_use]
    pub const fn replaces_files(self) -> bool {
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
    #[must_use]
    pub fn is_youtube(self) -> bool {
        self == Self::Youtube
    }
}

pub const BANDCAMP_PLAYLIST_ID: &str = "bandcamp:collection";

#[must_use]
pub fn bandcamp_source(user: &str) -> Playlist {
    Playlist::new(
        BANDCAMP_PLAYLIST_ID,
        &format!("https://bandcamp.com/{user}"),
        SourceKind::Bandcamp,
        Some("Bandcamp collection"),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedWrite {
    Written,
    Conflict,
}

pub struct Repository {
    path: PathBuf,
    legacy: Option<PathBuf>,
}

static WRITER: Mutex<()> = Mutex::new(());

impl Repository {
    #[must_use]
    pub fn open(paths: &Paths) -> Self {
        Self::new(paths.database()).with_legacy(paths.config.join("watchlist.json"))
    }

    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self { path, legacy: None }
    }

    #[must_use]
    pub fn with_legacy(mut self, legacy: PathBuf) -> Self {
        self.legacy = Some(legacy);
        self
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn locked<T>(work: impl FnOnce() -> T) -> T {
        let _writer = WRITER.lock();
        work()
    }

    /// # Errors
    /// Returns an error if the database cannot be opened or written, or if `change` fails.
    pub fn update<T>(&self, change: impl FnOnce(&mut Watchlist) -> Result<T>) -> Result<T> {
        self.update_with(|document, _| change(document))
    }

    pub(crate) fn update_with<T>(
        &self,
        change: impl FnOnce(&mut Watchlist, &Connection) -> Result<T>,
    ) -> Result<T> {
        Self::locked(|| {
            let mut connection = self.connect()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let before = read_document(&transaction)?;
            let mut document = before.clone();
            let result = change(&mut document, &transaction)?;
            write_changes(&transaction, &before, &document.normalized()?)?;
            transaction.commit()?;
            Ok(result)
        })
    }

    /// # Errors
    /// Returns an error if the database cannot be opened or holds an invalid watchlist.
    pub fn load(&self) -> Result<Watchlist> {
        read_document(&self.connect()?)
    }

    /// # Errors
    /// Returns an error if the watchlist is invalid or the database cannot be written.
    pub fn save(&self, value: &Watchlist) -> Result<()> {
        let value = value.clone().normalized()?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = read_document(&transaction)?;
        write_changes(&transaction, &before, &value)?;
        Ok(transaction.commit()?)
    }

    /// # Errors
    /// Returns an error if the database cannot be opened or read.
    pub fn revision(&self) -> Result<i64> {
        read_revision(&self.connect()?)
    }

    /// # Errors
    /// Returns an error if the database cannot be opened or holds an invalid watchlist.
    pub fn load_revision(&self) -> Result<(Watchlist, i64)> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let revision = read_revision(&transaction)?;
        let document = read_document(&transaction)?;
        transaction.commit()?;
        Ok((document, revision))
    }

    /// # Errors
    /// Returns an error if the watchlist is invalid or the database cannot be written.
    pub fn save_at(&self, revision: i64, value: &Watchlist) -> Result<CheckedWrite> {
        let value = value.clone().normalized()?;
        Self::locked(|| {
            let mut connection = self.connect()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if read_revision(&transaction)? != revision {
                return Ok(CheckedWrite::Conflict);
            }
            let before = read_document(&transaction)?;
            write_changes(&transaction, &before, &value)?;
            transaction.commit()?;
            Ok(CheckedWrite::Written)
        })
    }

    /// # Errors
    /// Returns an error if the watchlist cannot be read or written.
    pub fn ensure(&self, source: &Playlist) -> Result<bool> {
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

    /// # Errors
    /// Returns an error if `input` is not a known source, the source is already present, or the write fails.
    pub fn add(&self, input: &str) -> Result<Playlist> {
        let source = parse_source(input)?;
        self.update(|document| {
            if document.playlist(&source.playlist_id).is_some() {
                return Err("playlist is already in the watchlist".into());
            }
            document.playlists.push(source.clone());
            Ok(source)
        })
    }

    /// # Errors
    /// Returns an error if the watchlist cannot be read or written.
    pub fn rename(&self, playlist_id: &str, title: &str) -> Result<bool> {
        self.update(|document| {
            let Some(playlist) = document.playlist_mut(playlist_id) else {
                return Ok(false);
            };
            let title = title.trim();
            playlist.title = (!title.is_empty()).then(|| title.to_owned());
            Ok(true)
        })
    }

    /// # Errors
    /// Returns an error if the watchlist cannot be read or written.
    pub fn remove(&self, playlist_id: &str) -> Result<bool> {
        self.update(|document| {
            let before = document.playlists.len();
            document
                .playlists
                .retain(|playlist| playlist.playlist_id != playlist_id);
            Ok(document.playlists.len() != before)
        })
    }

    fn connect(&self) -> Result<Connection> {
        let mut connection = db::open(&self.path)?;
        if let Some(legacy) = &self.legacy {
            import_legacy(&mut connection, legacy)?;
        }
        Ok(connection)
    }
}

fn import_legacy(connection: &mut Connection, legacy: &Path) -> Result<()> {
    if !legacy.is_file() {
        return Ok(());
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let stored: i64 =
        transaction.query_row("SELECT COUNT(*) FROM watchlist_playlists", [], |row| {
            row.get(0)
        })?;
    if stored > 0 {
        return Ok(());
    }
    let source = fs::read_to_string(legacy)
        .map_err(|error| format!("cannot read {}: {error}", legacy.display()))?;
    let value: Value = serde_json::from_str(&source)
        .map_err(|error| format!("invalid watchlist {}: {error}", legacy.display()))?;
    let value = Watchlist::from_value(value)?;
    write_changes(&transaction, &Watchlist::default(), &value)?;
    transaction.commit()?;
    let mut backup = legacy.as_os_str().to_owned();
    backup.push(".migrated");
    Ok(fs::rename(legacy, backup)?)
}

fn read_revision(connection: &Connection) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT revision FROM watchlist_revision WHERE id = 1",
        [],
        |row| row.get(0),
    )?)
}

fn read_document(connection: &Connection) -> Result<Watchlist> {
    let mut playlists = Vec::new();
    let mut positions = HashMap::new();
    let mut statement =
        connection.prepare("SELECT playlist_id, data FROM watchlist_playlists ORDER BY ordinal")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let data: String = row.get(1)?;
        let mut playlist: Playlist = serde_json::from_str(&data)
            .map_err(|error| format!("invalid playlist {id}: {error}"))?;
        playlist.items.clear();
        positions.insert(id, playlists.len());
        playlists.push(playlist);
    }
    let mut statement = connection
        .prepare("SELECT playlist_id, data FROM watchlist_items ORDER BY playlist_id, ordinal")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let data: String = row.get(1)?;
        let item: WatchItem = serde_json::from_str(&data)
            .map_err(|error| format!("invalid item in playlist {id}: {error}"))?;
        let index = *positions
            .get(&id)
            .ok_or_else(|| format!("item belongs to missing playlist {id}"))?;
        playlists
            .get_mut(index)
            .ok_or_else(|| format!("item belongs to missing playlist {id}"))?
            .items
            .push(item);
    }
    Watchlist {
        playlists,
        ..Watchlist::default()
    }
    .normalized()
}

#[derive(Serialize)]
struct PlaylistHeader<'a> {
    playlist_id: &'a str,
    url: &'a str,
    kind: SourceKind,
    title: Option<&'a str>,
    processed_video_ids: &'a [String],
    last_checked_at: Option<&'a str>,
    last_error: Option<&'a str>,
    #[serde(flatten)]
    extra: &'a serde_json::Map<String, Value>,
}

fn header(playlist: &Playlist) -> Result<String> {
    Ok(serde_json::to_string(&PlaylistHeader {
        playlist_id: &playlist.playlist_id,
        url: &playlist.url,
        kind: playlist.kind,
        title: playlist.title.as_deref(),
        processed_video_ids: &playlist.processed_video_ids,
        last_checked_at: playlist.last_checked_at.as_deref(),
        last_error: playlist.last_error.as_deref(),
        extra: &playlist.extra,
    })?)
}

fn write_changes(connection: &Connection, before: &Watchlist, after: &Watchlist) -> Result<()> {
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
        connection.execute(
            "DELETE FROM watchlist_playlists WHERE playlist_id = ?1",
            [id],
        )?;
        changed = true;
    }
    for (ordinal, playlist) in after.playlists.iter().enumerate() {
        let id = playlist.playlist_id.as_str();
        let previous = old.get(id);
        let data = header(playlist)?;
        if previous.is_none_or(|(index, value)| {
            *index != ordinal || header(value).ok().as_deref() != Some(data.as_str())
        }) {
            connection.execute(
                "INSERT INTO watchlist_playlists (playlist_id, ordinal, data)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT (playlist_id) DO UPDATE
                     SET ordinal = excluded.ordinal, data = excluded.data",
                rusqlite::params![id, db::integer(ordinal)?, data],
            )?;
            changed = true;
        }
        let old_items = previous.map_or(&[][..], |(_, value)| value.items.as_slice());
        for (ordinal, item) in playlist.items.iter().enumerate() {
            if old_items.get(ordinal) != Some(item) {
                let data = serde_json::to_string(item)?;
                connection.execute(
                    "INSERT INTO watchlist_items (playlist_id, ordinal, data)
                         VALUES (?1, ?2, ?3)
                         ON CONFLICT (playlist_id, ordinal) DO UPDATE SET data = excluded.data",
                    rusqlite::params![id, db::integer(ordinal)?, data],
                )?;
                changed = true;
            }
        }
        if old_items.len() > playlist.items.len() {
            connection.execute(
                "DELETE FROM watchlist_items WHERE playlist_id = ?1 AND ordinal >= ?2",
                rusqlite::params![id, db::integer(playlist.items.len())?],
            )?;
            changed = true;
        }
    }
    if changed {
        connection.execute(
            "UPDATE watchlist_revision SET revision = revision + 1 WHERE id = 1",
            [],
        )?;
    }
    Ok(())
}

/// # Errors
/// Returns an error if `input` is not a `YouTube` playlist, Spotify playlist or album, or liked songs.
pub fn parse_source(input: &str) -> Result<Playlist> {
    use muzik_core::source_reference::{SpotifyReference, youtube_playlist};
    if let Ok(id) = youtube_playlist(input) {
        return Ok(Playlist::new(
            &id,
            &format!("https://www.youtube.com/playlist?list={id}"),
            SourceKind::Youtube,
            None,
        ));
    }
    let reference = SpotifyReference::parse(input).map_err(|error| error.to_string())?;
    let (id, url, title) = match reference {
        SpotifyReference::Liked => (
            "spotify:liked".to_owned(),
            "https://open.spotify.com/collection/tracks".to_owned(),
            Some("Liked Songs"),
        ),
        SpotifyReference::Playlist(id) => (
            format!("spotify:playlist:{id}"),
            format!("https://open.spotify.com/playlist/{id}"),
            None,
        ),
        SpotifyReference::Album(id) => (
            format!("spotify:album:{id}"),
            format!("https://open.spotify.com/album/{id}"),
            None,
        ),
    };
    Ok(Playlist::new(&id, &url, SourceKind::Spotify, title))
}

#[cfg(test)]
mod tests {
    use super::{Playlist, SourceKind, WatchItem, header};
    use serde_json::json;

    #[test]
    fn a_playlist_header_keeps_the_stored_field_order_without_items() {
        let mut playlist = Playlist::new("PL1", "u", SourceKind::Youtube, Some("Mix"));
        playlist.processed_video_ids = vec!["a".into()];
        playlist.extra.insert("custom".into(), json!("kept"));
        playlist
            .items
            .push(WatchItem::new(1, "Song", SourceKind::Youtube));
        assert_eq!(
            header(&playlist).unwrap(),
            r#"{"playlist_id":"PL1","url":"u","kind":"youtube","title":"Mix","processed_video_ids":["a"],"last_checked_at":null,"last_error":null,"custom":"kept"}"#
        );
    }
}
