//! Find `MusicBrainz` releases for library albums without IDs and write only their IDs.

use std::fs;
use std::path::{Path, PathBuf};

use muzik_core::BeetsConfig;
use muzik_library::{Album, Fields, Item, Library, SqlValue, path_from_sql};
use muzik_match::{MatchConfig, MatchItem, Recommendation};
use muzik_metadata::MetadataClient;
use muzik_tags::TagData;

use crate::fields::tags_from_item;
use crate::plan::{ImportPlanner, PlannedCandidate, ReleaseProvider, match_item};
use crate::{Error, Result};

pub struct FillSettings {
    pub match_config: MatchConfig,
    pub search_limit: u8,
}

impl FillSettings {
    /// # Errors
    /// Returns an error when the beets match settings are invalid.
    pub fn from_beets(config: &BeetsConfig) -> Result<Self> {
        Ok(Self {
            match_config: MatchConfig::from_beets(config)?,
            search_limit: config
                .get(&["musicbrainz", "searchlimit"])
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u8::try_from(value).ok())
                .unwrap_or(5),
        })
    }
}

#[must_use]
pub fn metadata_client() -> MetadataClient {
    MetadataClient::new(crate::beets::USER_AGENT)
}

#[derive(Clone, Debug)]
pub struct MissingAlbum {
    pub album_id: i64,
    pub artist: String,
    pub album: String,
    pub year: Option<i64>,
    pub items: Vec<MissingItem>,
}

#[derive(Clone, Debug)]
pub struct MissingItem {
    pub id: i64,
    pub path: PathBuf,
    pub match_item: MatchItem,
}

#[derive(Clone, Debug)]
pub struct AlbumMatch {
    pub candidates: Vec<PlannedCandidate>,
    pub strong: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FillResult {
    pub album: Fields,
    pub items: Vec<(i64, Fields)>,
}

/// # Errors
/// Returns an error when the library cannot be read or an item has no path.
pub fn albums_without_mbid(library: &Library) -> Result<Vec<MissingAlbum>> {
    let mut albums = Vec::new();
    for album in library.albums()? {
        if text(&album.fields, "mb_albumid").is_some() {
            continue;
        }
        let items = library
            .items_for_album(album.id)?
            .iter()
            .map(missing_item)
            .collect::<Result<Vec<_>>>()?;
        if items.is_empty() {
            continue;
        }
        albums.push(missing_album(&album, items));
    }
    Ok(albums)
}

fn missing_album(album: &Album, items: Vec<MissingItem>) -> MissingAlbum {
    let first = items.first().map(|item| &item.match_item);
    let artist = text(&album.fields, "albumartist")
        .map(str::to_owned)
        .or_else(|| first.map(|item| item.artist.clone()))
        .unwrap_or_default();
    let title = text(&album.fields, "album")
        .map(str::to_owned)
        .or_else(|| first.map(|item| item.album.clone()))
        .unwrap_or_default();
    MissingAlbum {
        album_id: album.id,
        artist,
        album: title,
        year: match album.fields.get("year") {
            Some(SqlValue::Integer(year)) if *year > 0 => Some(*year),
            _ => None,
        },
        items,
    }
}

fn missing_item(item: &Item) -> Result<MissingItem> {
    let path = item
        .field("path")
        .and_then(path_from_sql)
        .ok_or(Error::MissingPath(item.id))?;
    let mut match_item = match_item(&tags_from_item(item));
    if let Some(SqlValue::Real(length)) = item.field("length") {
        match_item.length = *length;
    }
    Ok(MissingItem {
        id: item.id,
        path,
        match_item,
    })
}

/// # Errors
/// Returns an error when the `MusicBrainz` search fails, the job is cancelled, or scoring fails.
pub fn find_match<P: ReleaseProvider>(
    provider: &P,
    settings: &FillSettings,
    album: &MissingAlbum,
    cancelled: &dyn Fn() -> bool,
) -> Result<AlbumMatch> {
    let library = Library::empty()?;
    let planner = ImportPlanner {
        provider,
        library: &library,
        match_config: &settings.match_config,
        search_limit: settings.search_limit,
    };
    let current: Vec<_> = album
        .items
        .iter()
        .map(|item| item.match_item.clone())
        .collect();
    let folder = album
        .items
        .first()
        .and_then(|item| item.path.parent())
        .unwrap_or_else(|| Path::new(""));
    let releases = planner.find_releases(&current, folder, cancelled, true)?;
    let (candidates, recommendation) = planner.rank(&current, &releases)?;
    Ok(AlbumMatch {
        candidates,
        strong: recommendation == Recommendation::Strong,
    })
}

#[must_use]
pub fn mbid_fields(
    album: &MissingAlbum,
    candidate: &PlannedCandidate,
    artist_ids: &muzik_metadata::ReleaseArtistIds,
) -> FillResult {
    let release = &candidate.release;
    let mut album_fields = Fields::new();
    let mut put = |name: &str, value: Option<&str>| {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            album_fields.insert(name.to_owned(), SqlValue::Text(value.to_owned()));
        }
    };
    put("mb_albumid", Some(&release.id.0));
    put("mb_releasegroupid", release.release_group_id.as_deref());
    put("mb_albumartistid", artist_ids.album_artist.as_deref());
    let items = album
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let mut fields = album_fields.clone();
            let track = candidate
                .assignment
                .pairs
                .iter()
                .find(|(source, _)| *source == index)
                .and_then(|(_, track)| release.tracks.get(*track));
            if let Some(track) = track {
                if let Some(id) = &track.recording_id {
                    fields.insert("mb_trackid".into(), SqlValue::Text(id.0.clone()));
                }
                if let Some(id) = &track.release_track_id {
                    fields.insert("mb_releasetrackid".into(), SqlValue::Text(id.clone()));
                    if let Some(artist) = artist_ids.tracks.get(id) {
                        fields.insert("mb_artistid".into(), SqlValue::Text(artist.clone()));
                    }
                }
            }
            (item.id, fields)
        })
        .collect();
    FillResult {
        album: album_fields,
        items,
    }
}

/// Write only the `MusicBrainz` ID fields of a chosen release to the audio files and library.
///
/// # Errors
/// Returns an error when the artist lookup, a tag write, or the library write fails. Written
/// files are restored when a later step fails.
pub fn write_mbids<P: ReleaseProvider>(
    library: &mut Library,
    provider: &P,
    album: &MissingAlbum,
    candidate: &PlannedCandidate,
) -> Result<FillResult> {
    let artist_ids = provider.lookup_release_artist_ids(&candidate.release.id.0)?;
    let result = mbid_fields(album, candidate, &artist_ids);
    let backup = tempfile::tempdir()?;
    for (index, item) in album.items.iter().enumerate() {
        fs::copy(&item.path, backup.path().join(index.to_string()))?;
    }
    let update = (|| -> Result<()> {
        for (item, (_, fields)) in album.items.iter().zip(&result.items) {
            muzik_tags::write(&item.path, &tag_data(fields))?;
        }
        library.transaction(|writer| {
            writer.update_album(album.album_id, &result.album, &Fields::new())?;
            for (id, fields) in &result.items {
                writer.update_item(*id, fields, &Fields::new())?;
            }
            Ok(())
        })?;
        Ok(())
    })();
    if let Err(error) = update {
        for (index, item) in album.items.iter().enumerate() {
            if let Err(restore) = fs::copy(backup.path().join(index.to_string()), &item.path) {
                tracing::warn!(path = %item.path.display(), %restore, "cannot restore MusicBrainz ID backup");
            }
        }
        return Err(error);
    }
    Ok(result)
}

fn tag_data(fields: &Fields) -> TagData {
    let mut tags = TagData::default();
    for (name, value) in fields {
        if let SqlValue::Text(value) = value {
            tags.fields.insert(name.clone(), value.clone());
        }
    }
    tags
}

fn text<'a>(fields: &'a Fields, key: &str) -> Option<&'a str> {
    match fields.get(key) {
        Some(SqlValue::Text(value)) if !value.is_empty() => Some(value),
        _ => None,
    }
}
