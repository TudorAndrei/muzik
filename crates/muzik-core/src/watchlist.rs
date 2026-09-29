//! Versioned watchlist data shared with the existing application.

use crate::paths;
use serde_json::{json, Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

pub mod jobs;
mod library_lookup;
mod reconcile;
mod view;

pub use reconcile::{reconcile, ReconcileOptions};
pub use view::view;

const STAGES: [&str; 5] = ["download", "quality", "parse", "split", "organize"];

pub struct Repository {
    path: PathBuf,
}

static WRITER: Mutex<()> = Mutex::new(());

impl Repository {
    pub fn locked<T>(&self, work: impl FnOnce() -> T) -> T {
        let _writer = WRITER.lock().unwrap_or_else(PoisonError::into_inner);
        work()
    }

    pub fn update<T>(
        &self,
        change: impl FnOnce(&mut Value) -> Result<T, String>,
    ) -> Result<T, String> {
        self.locked(|| {
            let mut document = self.load()?;
            let result = change(&mut document)?;
            self.save(document)?;
            Ok(result)
        })
    }

    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn default_path() -> PathBuf {
        paths::config_dir().join("watchlist.json")
    }

    pub fn load(&self) -> Result<Value, String> {
        let source = match fs::read_to_string(&self.path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(json!({"version": 3, "playlists": []}));
            }
            Err(error) => return Err(format!("cannot read {}: {error}", self.path.display())),
        };
        let value: Value = serde_json::from_str(&source)
            .map_err(|error| format!("invalid watchlist {}: {error}", self.path.display()))?;
        normalize(value)
    }

    pub fn save(&self, value: Value) -> Result<(), String> {
        let value = normalize(value)?;
        let parent = self.path.parent().ok_or("watchlist path has no parent")?;
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".watchlist.json.")
            .suffix(".tmp")
            .tempfile_in(parent)
            .map_err(|error| error.to_string())?;
        serde_json::to_writer_pretty(&mut temporary, &value).map_err(|error| error.to_string())?;
        temporary
            .write_all(b"\n")
            .map_err(|error| error.to_string())?;
        temporary.flush().map_err(|error| error.to_string())?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| error.to_string())?;
        temporary
            .persist(&self.path)
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn add(&self, input: &str) -> Result<Value, String> {
        let source = parse_source(input)?;
        self.locked(|| self.add_source(source))
    }

    fn add_source(&self, source: Value) -> Result<Value, String> {
        let mut document = self.load()?;
        let playlists = document
            .get_mut("playlists")
            .and_then(Value::as_array_mut)
            .ok_or("watchlist playlists are missing")?;
        if playlists
            .iter()
            .any(|item| item.get("playlist_id") == source.get("playlist_id"))
        {
            return Err("playlist is already in the watchlist".into());
        }
        playlists.push(source.clone());
        self.save(document)?;
        Ok(source)
    }

    pub fn rename(&self, playlist_id: &str, title: &str) -> Result<bool, String> {
        self.locked(|| self.rename_playlist(playlist_id, title))
    }

    fn rename_playlist(&self, playlist_id: &str, title: &str) -> Result<bool, String> {
        let mut document = self.load()?;
        let playlists = document
            .get_mut("playlists")
            .and_then(Value::as_array_mut)
            .ok_or("watchlist playlists are missing")?;
        let Some(playlist) = playlists
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
        self.save(document)?;
        Ok(true)
    }

    pub fn remove(&self, playlist_id: &str) -> Result<bool, String> {
        self.locked(|| self.remove_playlist(playlist_id))
    }

    fn remove_playlist(&self, playlist_id: &str) -> Result<bool, String> {
        let mut document = self.load()?;
        let playlists = document
            .get_mut("playlists")
            .and_then(Value::as_array_mut)
            .ok_or("watchlist playlists are missing")?;
        let before = playlists.len();
        playlists
            .retain(|item| item.get("playlist_id").and_then(Value::as_str) != Some(playlist_id));
        if playlists.len() == before {
            return Ok(false);
        }
        self.save(document)?;
        Ok(true)
    }
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
    let track = item.entry("track").or_insert(Value::Null);
    if !track.is_null() && !track.is_object() {
        return Err("track must be an object or null".into());
    }
    let stages = item.entry("stages").or_insert_with(|| json!({}));
    let stages = stages.as_object_mut().ok_or("stages must be an object")?;
    for name in STAGES {
        let record = stages.entry(name).or_insert_with(|| json!({}));
        normalize_stage(record).map_err(|error| format!("stages.{name}: {error}"))?;
    }
    Ok(())
}

fn normalize_stage(value: &mut Value) -> Result<(), String> {
    let record = value.as_object_mut().ok_or("stage must be an object")?;
    let status = record
        .entry("status")
        .or_insert_with(|| json!("not_started"));
    if !matches!(
        status.as_str(),
        Some("not_started" | "running" | "complete" | "failed" | "skipped" | "stale" | "waiting")
    ) {
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
    let value = map.entry("kind").or_insert_with(|| json!("youtube"));
    if matches!(value.as_str(), Some("youtube" | "spotify")) {
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
            "spotify",
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
                    "youtube",
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
                "spotify",
                None,
            ));
        }
    }
    Err("enter a YouTube playlist URL, Spotify playlist or album link, or liked".into())
}

fn playlist(id: &str, url: &str, kind: &str, title: Option<&str>) -> Value {
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
