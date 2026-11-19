//! Playlist files that list the library songs of each watchlist source.

use super::library_lookup::parse_title;
use super::{Playlist, SourceKind, WatchItem, Watchlist};
use crate::Result;
use muzik_core::{BeetsConfig, PlaylistFormat};
use muzik_library::{Item, Library, SqlValue, path_from_sql};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const FOLDER: &str = "Playlists";
const MARKER: &str = "#MUZIK-SOURCE:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub playlist_id: String,
    pub path: PathBuf,
    pub tracks: usize,
    pub missing: usize,
    pub changed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub written: Vec<Written>,
    pub removed: Vec<PathBuf>,
}

/// # Errors
/// Returns an error if the music library cannot be read or a playlist file cannot be written.
pub fn write(
    watchlist: &Watchlist,
    config: &Path,
    format: Option<PlaylistFormat>,
) -> Result<Report> {
    let values = BeetsConfig::load(config, json!({})).map_err(message)?;
    let directory = values
        .configured_path(config, "directory")
        .map_err(message)?;
    let folder = directory.join(FOLDER);
    let Some(format) = format else {
        return Ok(Report {
            written: Vec::new(),
            removed: remove_old(&folder, &HashSet::new())?,
        });
    };
    let database = values.configured_path(config, "library").map_err(message)?;
    let library = if database.is_file() {
        Some(Library::open_read_only(&database).map_err(message)?)
    } else {
        None
    };
    let tracks = match &library {
        Some(library) => Tracks::read(library, &directory)?,
        None => Tracks::default(),
    };
    write_folder(watchlist, &tracks, &folder, format)
}

fn write_folder(
    watchlist: &Watchlist,
    tracks: &Tracks,
    folder: &Path,
    format: PlaylistFormat,
) -> Result<Report> {
    let mut report = Report::default();
    let mut names = HashSet::new();
    let mut kept = HashSet::new();
    for playlist in watchlist
        .playlists
        .iter()
        .filter(|playlist| playlist.playlist_file)
    {
        let path = folder.join(unique_name(playlist, &mut names, format));
        let (text, count, missing) = render(playlist, tracks, folder);
        let changed = fs::read_to_string(&path).ok().as_deref() != Some(text.as_str());
        if changed {
            fs::create_dir_all(folder)?;
            replace(&path, &text)?;
        }
        kept.insert(path.clone());
        report.written.push(Written {
            playlist_id: playlist.playlist_id.clone(),
            path,
            tracks: count,
            missing,
            changed,
        });
    }
    report.removed = remove_old(folder, &kept)?;
    Ok(report)
}

fn unique_name(playlist: &Playlist, names: &mut HashSet<String>, format: PlaylistFormat) -> String {
    let title = playlist
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(&playlist.playlist_id);
    let base = file_name(title);
    let mut name = base.clone();
    let mut number = 1_u32;
    while !names.insert(name.to_lowercase()) {
        number = number.saturating_add(1);
        name = format!("{base} ({number})");
    }
    format!("{name}.{}", format.extension())
}

fn file_name(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|character| {
            if character.is_control() || "/\\:*?\"<>|".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let name = name.trim().trim_matches('.').trim();
    if name.is_empty() {
        "_".to_owned()
    } else {
        name.to_owned()
    }
}

fn render(playlist: &Playlist, tracks: &Tracks, folder: &Path) -> (String, usize, usize) {
    let mut text = format!("#EXTM3U\n{MARKER}{}\n", playlist.playlist_id);
    let mut listed = HashSet::new();
    let mut seen_items = HashSet::new();
    let mut count = 0_usize;
    let mut missing = 0_usize;
    for (id, item) in playlist.item_ids().into_iter().zip(&playlist.items) {
        if item.is_gone() || (id.key.is_some() && !seen_items.insert(id)) {
            continue;
        }
        let found = tracks.find(item);
        if found.is_empty() {
            missing = missing.saturating_add(1);
        }
        for track in found {
            if !listed.insert(track.path.clone()) {
                continue;
            }
            let seconds = track.seconds.map_or(-1, i64::from);
            let _ = writeln!(text, "#EXTINF:{seconds},{}", track.label);
            let _ = writeln!(text, "{}", entry(folder, &track.path));
            count = count.saturating_add(1);
        }
    }
    (text, count, missing)
}

fn entry(folder: &Path, track: &Path) -> String {
    let relative = folder
        .parent()
        .and_then(|root| track.strip_prefix(root).ok())
        .filter(|relative| {
            relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        });
    relative.map_or_else(
        || track.to_string_lossy().into_owned(),
        |relative| {
            let parts: Vec<String> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            format!("../{}", parts.join("/"))
        },
    )
}

fn replace(path: &Path, text: &str) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".muzik-part");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path)
}

fn remove_old(folder: &Path, kept: &HashSet<PathBuf>) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut removed = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if kept.contains(&path) || !is_ours(&path) {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => removed.push(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    removed.sort();
    Ok(removed)
}

fn is_ours(path: &Path) -> bool {
    let playlist = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            PlaylistFormat::CHOICES
                .iter()
                .any(|choice| extension.eq_ignore_ascii_case(choice))
        });
    playlist
        && fs::read_to_string(path).is_ok_and(|text| {
            text.lines()
                .take(2)
                .any(|line| line.trim_start().starts_with(MARKER))
        })
}

#[derive(Clone, Debug)]
struct Track {
    path: PathBuf,
    label: String,
    seconds: Option<u32>,
    disc: i64,
    number: i64,
}

#[derive(Default)]
struct Tracks {
    all: Vec<Track>,
    by_source: HashMap<String, Vec<usize>>,
    by_spotify: HashMap<String, usize>,
    by_isrc: HashMap<String, usize>,
    by_song: HashMap<(String, String), Vec<usize>>,
    by_title_album: HashMap<(String, String), Vec<usize>>,
    albums: HashMap<(String, String), i64>,
    by_album: BTreeMap<i64, Vec<usize>>,
}

impl Tracks {
    fn read(library: &Library, directory: &Path) -> Result<Self> {
        let mut tracks = Self::default();
        for item in library.items().map_err(message)? {
            tracks.add(&item, directory);
        }
        for indexes in tracks.by_album.values_mut() {
            indexes.sort_by_key(|index| {
                tracks
                    .all
                    .get(*index)
                    .map(|track| (track.disc, track.number))
            });
        }
        for indexes in tracks.by_source.values_mut() {
            indexes.sort_by_key(|index| {
                tracks
                    .all
                    .get(*index)
                    .map(|track| (track.disc, track.number))
            });
        }
        Ok(tracks)
    }

    fn add(&mut self, item: &Item, directory: &Path) {
        let Some(path) = item.field("path").and_then(path_from_sql).map(|path| {
            if path.is_absolute() {
                path
            } else {
                directory.join(path)
            }
        }) else {
            return;
        };
        let text = |name: &str| {
            item.field(name)
                .and_then(sql_text)
                .map(str::trim)
                .unwrap_or_default()
                .to_owned()
        };
        let artist = text("artist");
        let albumartist = text("albumartist");
        let title = text("title");
        let album = text("album");
        let index = self.all.len();
        let label = match (artist.is_empty(), title.is_empty()) {
            (false, false) => format!("{artist} - {title}"),
            (true, false) => title.clone(),
            _ => path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        self.all.push(Track {
            path,
            label: label.replace(['\r', '\n'], " "),
            seconds: item.field("length").and_then(seconds),
            disc: number(item.field("disc")),
            number: number(item.field("track")),
        });
        if let Some(source) = item
            .attribute("muzik_source_id")
            .or_else(|| item.field("muzik_source_id"))
            .and_then(sql_text)
            .filter(|source| !source.trim().is_empty())
        {
            self.by_source
                .entry(source.trim().to_owned())
                .or_default()
                .push(index);
        }
        if let Some(id) = spotify_id_in(&album) {
            self.by_spotify.entry(id).or_insert(index);
        }
        let isrc = text("isrc").to_uppercase();
        if !isrc.is_empty() {
            self.by_isrc.entry(isrc).or_insert(index);
        }
        let song = key(&title);
        if !song.is_empty() {
            let mut artists: Vec<String> = [&artist, &albumartist]
                .into_iter()
                .flat_map(|name| [key(name), key(primary(name))])
                .filter(|name| !name.is_empty())
                .collect();
            artists.sort();
            artists.dedup();
            for name in artists {
                self.by_song
                    .entry((name, song.clone()))
                    .or_default()
                    .push(index);
            }
            if !album.is_empty() {
                self.by_title_album
                    .entry((song, key(&album)))
                    .or_default()
                    .push(index);
            }
        }
        if let Some(album_id) = item.album_id() {
            self.by_album.entry(album_id).or_default().push(index);
            if !album.is_empty() {
                for name in [&albumartist, &artist] {
                    let name = key(name);
                    if !name.is_empty() {
                        self.albums.entry((name, key(&album))).or_insert(album_id);
                    }
                }
            }
        }
    }

    fn tracks(&self, indexes: &[usize]) -> Vec<&Track> {
        indexes
            .iter()
            .filter_map(|index| self.all.get(*index))
            .collect()
    }

    fn one(&self, index: Option<usize>) -> Vec<&Track> {
        index
            .and_then(|index| self.all.get(index))
            .into_iter()
            .collect()
    }

    fn album(&self, artist: &str, album: &str) -> Vec<&Track> {
        self.albums
            .get(&(key(artist), key(album)))
            .and_then(|id| self.by_album.get(id))
            .map(|indexes| self.tracks(indexes))
            .unwrap_or_default()
    }

    fn song(&self, artists: &[&str], title: &str) -> Vec<&Track> {
        let title = key(title);
        if title.is_empty() {
            return Vec::new();
        }
        artists
            .iter()
            .flat_map(|artist| [key(artist), key(primary(artist))])
            .find_map(|artist| self.by_song.get(&(artist, title.clone())))
            .and_then(|indexes| indexes.first())
            .map(|index| self.one(Some(*index)))
            .unwrap_or_default()
    }

    fn find(&self, item: &WatchItem) -> Vec<&Track> {
        let sources = [
            item.video_id.as_deref(),
            item.entry_id.as_deref(),
            track_text(item, "source_id"),
        ];
        if let Some(indexes) = sources
            .into_iter()
            .flatten()
            .find_map(|source| self.by_source.get(source.trim()))
        {
            return self.tracks(indexes);
        }
        match item.kind {
            SourceKind::Spotify => self.spotify(item),
            SourceKind::Youtube => self.youtube(item),
            SourceKind::Bandcamp => self.bandcamp(item),
        }
    }

    fn spotify(&self, item: &WatchItem) -> Vec<&Track> {
        if let Some(index) = item
            .video_id
            .as_deref()
            .and_then(|id| self.by_spotify.get(id))
        {
            return self.one(Some(*index));
        }
        if let Some(index) = track_text(item, "isrc")
            .map(str::to_uppercase)
            .and_then(|isrc| self.by_isrc.get(&isrc))
        {
            return self.one(Some(*index));
        }
        let title = track_text(item, "title").unwrap_or_default();
        let artists: Vec<&str> = item
            .track
            .as_ref()
            .and_then(|track| track.get("artists"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let found = self.song(&artists, title);
        if !found.is_empty() {
            return found;
        }
        let album = track_text(item, "album").unwrap_or_default();
        self.by_title_album
            .get(&(key(title), key(album)))
            .filter(|indexes| indexes.len() == 1)
            .map(|indexes| self.tracks(indexes))
            .unwrap_or_default()
    }

    fn youtube(&self, item: &WatchItem) -> Vec<&Track> {
        if let Some((artist, album)) = parse_title(&item.title) {
            let found = self.album(&artist, &album);
            if !found.is_empty() {
                return found;
            }
            return self.song(&[artist.as_str()], &album);
        }
        Vec::new()
    }

    fn bandcamp(&self, item: &WatchItem) -> Vec<&Track> {
        let artist = track_text(item, "artist").unwrap_or_default();
        let title = track_text(item, "title").unwrap_or_default();
        let single = item
            .track
            .as_ref()
            .and_then(|track| track.get("single"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (first, second) = if single {
            (self.song(&[artist], title), self.album(artist, title))
        } else {
            (self.album(artist, title), self.song(&[artist], title))
        };
        if first.is_empty() { second } else { first }
    }
}

fn message(error: impl std::fmt::Display) -> crate::Error {
    crate::Error::Message(error.to_string())
}

fn track_text<'a>(item: &'a WatchItem, field: &str) -> Option<&'a str> {
    item.track
        .as_ref()?
        .get(field)?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn sql_text(value: &SqlValue) -> Option<&str> {
    match value {
        SqlValue::Text(text) => Some(text),
        SqlValue::Blob(bytes) => std::str::from_utf8(bytes).ok(),
        _ => None,
    }
}

fn seconds(value: &SqlValue) -> Option<u32> {
    match value {
        SqlValue::Real(length) if length.is_finite() && *length >= 0.0 => {
            format!("{length:.0}").parse().ok()
        }
        SqlValue::Integer(length) => u32::try_from(*length).ok(),
        _ => None,
    }
}

fn number(value: Option<&SqlValue>) -> i64 {
    match value {
        Some(SqlValue::Integer(number)) => *number,
        Some(SqlValue::Text(text)) => text.trim().parse().unwrap_or_default(),
        _ => 0,
    }
}

fn spotify_id_in(album: &str) -> Option<String> {
    let rest = album.split_once("spotify_track_")?.1;
    let id: String = rest
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    (!id.is_empty()).then_some(id)
}

fn primary(artist: &str) -> &str {
    let lower = artist.to_lowercase();
    [
        ",",
        " & ",
        " feat.",
        " feat ",
        " ft.",
        " ft ",
        " featuring ",
        " x ",
        " and ",
    ]
    .iter()
    .filter_map(|separator| lower.find(separator))
    .min()
    .and_then(|end| artist.get(..end))
    .unwrap_or(artist)
}

fn key(text: &str) -> String {
    let mut plain = String::new();
    let mut depth = 0_u32;
    for character in text.chars() {
        match character {
            '(' | '[' => depth = depth.saturating_add(1),
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => plain.push(character),
            _ => {}
        }
    }
    let plain = plain.split(" - ").next().unwrap_or_default();
    deunicode::deunicode(plain)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Tracks, key, primary, write_folder};
    use crate::watchlist::{Playlist, SourceKind, WatchItem, Watchlist};
    use muzik_core::PlaylistFormat;
    use muzik_library::{Fields, Library, SqlValue};
    use serde_json::json;
    use std::path::Path;

    fn add(library: &mut Library, directory: &Path, fields: &[(&str, SqlValue)]) {
        let mut values: Fields = fields
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect();
        let path = directory.join(format!("{}.flac", values.len()));
        values
            .entry("path".into())
            .or_insert_with(|| muzik_library::path_to_sql(&path));
        library.insert_item(&values, &Fields::new()).unwrap();
    }

    fn text(value: &str) -> SqlValue {
        SqlValue::Text(value.into())
    }

    fn spotify(position: u64, id: &str, artists: &[&str], title: &str) -> WatchItem {
        let mut item = WatchItem::new(position, title, SourceKind::Spotify);
        item.video_id = Some(id.into());
        item.entry_id = Some(format!("spotify:track:{id}#0"));
        item.track = Some(json!({"title": title, "artists": artists, "album": "Record"}));
        item
    }

    #[test]
    fn names_ignore_case_accents_and_bracketed_text() {
        assert_eq!(key("Tatlı Dile (Remastered 2011)"), key("tatli dile"));
        assert_eq!(key("Tamborito - Gramophiles Edit"), "tamborito");
        assert_eq!(
            primary("Marshall Jefferson x Solardo"),
            "Marshall Jefferson"
        );
        assert_eq!(primary("Nina Sky"), "Nina Sky");
    }

    #[test]
    fn a_playlist_lists_found_songs_in_source_order_with_relative_paths() {
        let root = tempfile::tempdir().unwrap();
        let music = root.path().join("Music");
        let mut library = Library::open_or_create(&root.path().join("library.db")).unwrap();
        add(
            &mut library,
            &music,
            &[
                (
                    "path",
                    muzik_library::path_to_sql(&music.join("A/One/01 Song.flac")),
                ),
                ("artist", text("Giorgos Mazonakis")),
                ("title", text("Nikotini")),
                ("length", SqlValue::Real(229.6)),
            ],
        );
        add(
            &mut library,
            &music,
            &[
                ("path", muzik_library::path_to_sql(&music.join("B/x.opus"))),
                ("artist", text("M.Hustler")),
                ("title", text("Other")),
                ("album", text("spotify_track_t2_0")),
            ],
        );
        let tracks = Tracks::read(&library, &music).unwrap();
        let mut playlist = Playlist::new(
            "spotify:liked",
            "u",
            SourceKind::Spotify,
            Some("Liked Songs"),
        );
        playlist.items = vec![
            spotify(1, "t2", &["Someone"], "Other"),
            spotify(2, "t1", &["Giorgos Mazonakis", "Foivos"], "Nikotini"),
            spotify(3, "t3", &["Nobody"], "Missing"),
        ];
        let watchlist = Watchlist {
            playlists: vec![playlist],
            ..Watchlist::default()
        };
        let folder = music.join("Playlists");
        let report = write_folder(&watchlist, &tracks, &folder, PlaylistFormat::M3u8).unwrap();
        let written = &report.written[0];
        assert_eq!(written.path, folder.join("Liked Songs.m3u8"));
        assert_eq!(
            (written.tracks, written.missing, written.changed),
            (2, 1, true)
        );
        assert_eq!(
            std::fs::read_to_string(&written.path).unwrap(),
            "#EXTM3U\n#MUZIK-SOURCE:spotify:liked\n#EXTINF:-1,M.Hustler - Other\n../B/x.opus\n#EXTINF:230,Giorgos Mazonakis - Nikotini\n../A/One/01 Song.flac\n"
        );
        let again = write_folder(&watchlist, &tracks, &folder, PlaylistFormat::M3u8).unwrap();
        assert!(!again.written[0].changed);
    }

    #[test]
    fn a_source_that_is_off_loses_only_its_own_file() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("Playlists");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Mine.m3u"), "#EXTM3U\n../song.flac\n").unwrap();
        let mut playlist = Playlist::new("PL1", "u", SourceKind::Youtube, Some("Mix"));
        let mut watchlist = Watchlist {
            playlists: vec![playlist.clone()],
            ..Watchlist::default()
        };
        let tracks = Tracks::default();
        write_folder(&watchlist, &tracks, &folder, PlaylistFormat::M3u).unwrap();
        assert!(folder.join("Mix.m3u").is_file());
        playlist.playlist_file = false;
        watchlist.playlists = vec![playlist];
        let report = write_folder(&watchlist, &tracks, &folder, PlaylistFormat::M3u).unwrap();
        assert_eq!(report.removed, [folder.join("Mix.m3u")]);
        assert!(folder.join("Mine.m3u").is_file());
    }

    #[test]
    fn a_youtube_album_lists_all_its_tracks_in_track_order() {
        let root = tempfile::tempdir().unwrap();
        let music = root.path().join("Music");
        let mut library = Library::open_or_create(&root.path().join("library.db")).unwrap();
        let album = library
            .insert_album(
                &Fields::from([
                    ("album".into(), text("Homecomer")),
                    ("albumartist".into(), text("Umbría")),
                ]),
                &Fields::new(),
            )
            .unwrap();
        for (number, name) in [(2, "Two"), (1, "One")] {
            add(
                &mut library,
                &music,
                &[
                    (
                        "path",
                        muzik_library::path_to_sql(&music.join(format!("U/H/{name}.opus"))),
                    ),
                    ("album_id", SqlValue::Integer(album)),
                    ("album", text("Homecomer")),
                    ("albumartist", text("Umbría")),
                    ("title", text(name)),
                    ("track", SqlValue::Integer(number)),
                ],
            );
        }
        let tracks = Tracks::read(&library, &music).unwrap();
        let item = WatchItem::new(
            1,
            "Umbría - Homecomer (2023) (Dungeon Synth, Folk Ambient)",
            SourceKind::Youtube,
        );
        let found: Vec<String> = tracks
            .find(&item)
            .into_iter()
            .map(|track| track.label.clone())
            .collect();
        assert_eq!(found, ["One", "Two"]);
    }
}
