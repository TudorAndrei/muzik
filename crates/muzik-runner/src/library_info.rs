use muzik_library::{Album, Item, Library, SqlValue};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub disc: i64,
    pub number: Option<i64>,
    pub title: String,
    pub artist: String,
    pub length: f64,
    pub format: String,
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumRow {
    pub id: i64,
    pub artist: String,
    pub title: String,
    pub year: Option<i64>,
    pub genre: String,
    pub label: String,
    pub added: f64,
    pub path: String,
    pub tracks: Vec<Track>,
}

impl AlbumRow {
    #[must_use]
    pub fn length(&self) -> f64 {
        self.tracks.iter().map(|track| track.length).sum()
    }

    #[must_use]
    pub fn format(&self) -> String {
        self.tracks
            .iter()
            .map(|track| track.format.as_str())
            .filter(|format| !format.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[must_use]
    pub fn matches(&self, needle: &str) -> bool {
        [&self.artist, &self.title, &self.genre, &self.label]
            .iter()
            .any(|text| text.to_lowercase().contains(needle))
            || self.year.is_some_and(|year| year.to_string() == needle)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryInfo {
    pub database: PathBuf,
    pub directory: PathBuf,
    pub albums: Vec<Arc<AlbumRow>>,
    pub singletons: usize,
    pub tracks: usize,
    pub artists: usize,
    pub length: f64,
    pub formats: Vec<(String, usize)>,
}

fn text(value: Option<&SqlValue>) -> String {
    value
        .and_then(muzik_library::scalar_text)
        .unwrap_or_default()
}

fn integer(value: Option<&SqlValue>) -> i64 {
    match value {
        Some(SqlValue::Integer(number)) => *number,
        Some(SqlValue::Text(text)) => text.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn positive(value: Option<&SqlValue>) -> Option<i64> {
    Some(integer(value)).filter(|number| *number > 0)
}

fn real(value: Option<&SqlValue>) -> f64 {
    match value {
        Some(SqlValue::Real(number)) => *number,
        Some(SqlValue::Integer(number)) => i32::try_from(*number).map_or(0., f64::from),
        Some(SqlValue::Text(text)) => text.trim().parse().unwrap_or(0.),
        _ => 0.,
    }
}

fn track(item: &Item) -> Track {
    Track {
        disc: integer(item.field("disc")),
        number: positive(item.field("track")),
        title: text(item.field("title")),
        artist: text(item.field("artist")),
        length: real(item.field("length")),
        format: text(item.field("format")).to_uppercase(),
        path: item.field("path").and_then(muzik_library::path_from_sql),
    }
}

fn album_row(album: &Album, tracks: Vec<Track>) -> AlbumRow {
    AlbumRow {
        id: album.id,
        artist: text(album.field("albumartist")),
        title: text(album.field("album")),
        year: positive(album.field("year")),
        genre: text(album.field("genre")),
        label: text(album.field("label")),
        added: real(album.field("added")),
        path: tracks
            .iter()
            .find_map(|track| track.path.as_deref()?.parent())
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        tracks,
    }
}

/// # Errors
/// Returns an error when the library database cannot be opened or read.
pub fn read(database: &Path, directory: &Path) -> crate::Result<LibraryInfo> {
    let library = Library::open_read_only(database)?;
    let items = library.items()?;
    let mut by_album: BTreeMap<i64, Vec<Track>> = BTreeMap::new();
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut artists = BTreeSet::new();
    let mut singletons = 0_usize;
    let mut length = 0.;
    for item in &items {
        let track = track(item);
        length += track.length;
        if !track.format.is_empty() {
            let count = formats.entry(track.format.clone()).or_default();
            *count = count.saturating_add(1);
        }
        if let Some(id) = item.album_id() {
            by_album.entry(id).or_default().push(track);
        } else {
            if !track.artist.is_empty() {
                artists.insert(track.artist.to_lowercase());
            }
            singletons = singletons.saturating_add(1);
        }
    }
    let mut albums = Vec::new();
    for album in library.albums()? {
        let mut tracks = by_album.remove(&album.id).unwrap_or_default();
        tracks.sort_by_key(|track| (track.disc, track.number));
        let row = album_row(&album, tracks);
        if !row.artist.is_empty() {
            artists.insert(row.artist.to_lowercase());
        }
        albums.push(Arc::new(row));
    }
    albums.sort_by(|left, right| right.added.total_cmp(&left.added));
    let mut formats: Vec<_> = formats.into_iter().collect();
    formats.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    Ok(LibraryInfo {
        database: database.to_path_buf(),
        directory: directory.to_path_buf(),
        albums,
        singletons,
        tracks: items.len(),
        artists: artists.len(),
        length,
        formats,
    })
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "std has no TryFrom<f64> for u64; the value is finite and positive"
)]
fn seconds(length: f64) -> u64 {
    if length.is_finite() && length > 0. {
        length.round() as u64
    } else {
        0
    }
}

#[must_use]
pub fn duration(length: f64) -> String {
    let total = seconds(length);
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[must_use]
pub fn playtime(length: f64) -> String {
    let total = seconds(length);
    let (days, hours) = (total / 86_400, total / 3600 % 24);
    if days > 0 {
        format!("{days} d {hours} h")
    } else {
        format!("{hours} h {} min", total / 60 % 60)
    }
}

#[must_use]
pub fn date(added: f64) -> String {
    i64::try_from(seconds(added))
        .ok()
        .filter(|stamp| *stamp > 0)
        .and_then(|stamp| jiff::Timestamp::from_second(stamp).ok())
        .map(|at| {
            at.to_zoned_datetime(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{duration, playtime, read};
    use muzik_library::{Fields, Library, SqlValue};

    #[test]
    fn read_groups_tracks_under_albums_and_counts_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("library.db");
        let fields = |pairs: &[(&str, SqlValue)]| -> Fields {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect()
        };
        Library::open_or_create(&database)
            .unwrap()
            .transaction(|library| {
                let album_id = library.insert_album(
                    &fields(&[
                        ("albumartist", SqlValue::Text("Phoniks".into())),
                        ("album", SqlValue::Text("Down To Earth".into())),
                        ("year", SqlValue::Integer(2015)),
                        ("added", SqlValue::Real(1_700_000_000.)),
                    ]),
                    &Fields::new(),
                )?;
                for (number, title, length) in [(2, "Second", 125.4), (1, "First", 60.)] {
                    library.insert_item(
                        &fields(&[
                            ("album_id", SqlValue::Integer(album_id)),
                            ("track", SqlValue::Integer(number)),
                            ("title", SqlValue::Text(title.into())),
                            ("artist", SqlValue::Text("Phoniks".into())),
                            ("length", SqlValue::Real(length)),
                            ("format", SqlValue::Text("FLAC".into())),
                            (
                                "path",
                                SqlValue::Text(format!("/Music/Phoniks/{title}.flac")),
                            ),
                        ]),
                        &Fields::new(),
                    )?;
                }
                library.insert_item(
                    &fields(&[
                        ("title", SqlValue::Text("Loose".into())),
                        ("artist", SqlValue::Text("Awon".into())),
                        ("length", SqlValue::Real(30.)),
                        ("format", SqlValue::Text("mp3".into())),
                    ]),
                    &Fields::new(),
                )?;
                Ok(())
            })
            .unwrap();
        let info = read(&database, dir.path()).unwrap();
        assert_eq!(info.tracks, 3);
        assert_eq!(info.singletons, 1);
        assert_eq!(info.artists, 2);
        assert_eq!(info.albums.len(), 1);
        let row = &info.albums[0];
        assert_eq!(
            row.tracks
                .iter()
                .map(|track| track.title.as_str())
                .collect::<Vec<_>>(),
            ["First", "Second"]
        );
        assert_eq!(duration(row.length()), "3:05");
        assert_eq!(row.format(), "FLAC");
        assert_eq!(row.path, "/Music/Phoniks");
        assert_eq!(row.year, Some(2015));
        assert_eq!(row.tracks[0].number, Some(1));
        assert_eq!(
            info.formats,
            [("FLAC".to_string(), 2), ("MP3".to_string(), 1)]
        );
    }

    #[test]
    fn durations_read_as_clock_time_and_playtime() {
        assert_eq!(duration(59.6), "1:00");
        assert_eq!(duration(3725.), "1:02:05");
        assert_eq!(playtime(5400.), "1 h 30 min");
        assert_eq!(playtime(90_000.), "1 d 1 h");
    }
}
