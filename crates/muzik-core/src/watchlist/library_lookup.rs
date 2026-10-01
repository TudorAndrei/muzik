use crate::BeetsConfig;
use muzik_library::{Library, SqlValue};
use regex::Regex;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub(super) struct MusicLibrary {
    source_paths: HashMap<String, PathBuf>,
    album_paths: HashMap<(String, String), PathBuf>,
}

impl MusicLibrary {
    /// The music library is optional during reconciliation.
    pub(super) fn open(config: Option<&Path>) -> Option<Self> {
        let path = config?.to_path_buf();
        let values = BeetsConfig::load(&path, json!({})).ok()?;
        let database = values.get(&["library"])?.as_str()?;
        let directory = values.get(&["directory"])?.as_str()?;
        let database = expand_path(database, &path);
        let directory = expand_path(directory, &path);
        let library = Library::open_read_only(&database).ok()?;
        let items = library.items().ok()?;
        let albums = library.albums().ok()?;
        let mut source_paths = HashMap::new();
        let mut first_item_by_album = HashMap::new();
        for item in items {
            let Some(item_path) = item
                .field("path")
                .and_then(sql_text)
                .map(|path| resolve(&directory, path))
            else {
                continue;
            };
            if let Some(source_id) = item
                .attribute("muzik_source_id")
                .or_else(|| item.field("muzik_source_id"))
                .and_then(sql_text)
            {
                source_paths
                    .entry(source_id.to_owned())
                    .or_insert_with(|| item_path.clone());
            }
            if let Some(album_id) = item.album_id() {
                first_item_by_album.entry(album_id).or_insert(item_path);
            }
        }
        let mut album_paths = HashMap::new();
        for album in albums {
            let Some(item_path) = first_item_by_album.get(&album.id) else {
                continue;
            };
            let artist = album
                .field("albumartist")
                .and_then(sql_text)
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            let name = album
                .field("album")
                .and_then(sql_text)
                .unwrap_or_default()
                .trim()
                .to_lowercase();
            album_paths
                .entry((artist, name))
                .or_insert_with(|| item_path.clone());
        }
        Some(Self {
            source_paths,
            album_paths,
        })
    }

    pub(super) fn find(&self, video_id: &str, title: &str) -> Option<PathBuf> {
        if let Some(path) = self.source_paths.get(video_id) {
            return Some(path.clone());
        }
        let (artist, album) = parse_title(title)?;
        self.album_paths
            .get(&(artist.to_lowercase(), album.to_lowercase()))
            .cloned()
    }
}

fn resolve(directory: &Path, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        directory.join(path)
    }
}

fn sql_text(value: &SqlValue) -> Option<&str> {
    match value {
        SqlValue::Text(text) => Some(text),
        SqlValue::Blob(bytes) => std::str::from_utf8(bytes).ok(),
        _ => None,
    }
}

fn expand_path(raw: &str, config: &Path) -> PathBuf {
    let path = if let Some(rest) = raw.strip_prefix("~/") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(rest)
    } else {
        PathBuf::from(raw)
    };
    if path.is_absolute() {
        path
    } else {
        config.parent().unwrap_or(Path::new("")).join(path)
    }
}

fn parse_title(title: &str) -> Option<(String, String)> {
    static YEAR: OnceLock<Option<Regex>> = OnceLock::new();
    static NOISE: OnceLock<Option<Regex>> = OnceLock::new();
    let year = YEAR
        .get_or_init(|| Regex::new(r"\s*[\(\[](?:19|20)\d{2}[\)\]]").ok())
        .as_ref()?;
    let noise = NOISE.get_or_init(|| Regex::new(r"(?i)\s*[\(\[]\s*(?:full\s+album|complete\s+album|full\s+lp|official\s+album|remaster(?:ed)?|deluxe(?:\s+edition)?|bonus\s+tracks?)\s*[\)\]]").ok()).as_ref()?;
    let title = if let Some(found) = year.find(title) {
        title[..found.start()].trim_end()
    } else {
        title
    };
    let (artist, album) = title.split_once(" - ")?;
    let artist = artist.trim();
    let album = noise.replace_all(album, "");
    let album = year.replace_all(&album, "");
    let album = album.trim();
    if artist.is_empty() || album.is_empty() {
        return None;
    }
    Some((artist.to_owned(), album.to_owned()))
}
