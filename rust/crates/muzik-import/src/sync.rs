//! Update selected library albums from their MusicBrainz release IDs.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ftclean;
use crate::plan::ReleaseProvider;
use muzik_core::ReleaseCandidate;
use muzik_library::{Fields, Item, Library, SqlValue};

pub use crate::Error as SyncError;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncResult {
    pub albums_updated: usize,
    pub items_updated: usize,
    pub albums_without_release_id: usize,
    pub items_without_match: usize,
}

/// Synchronize albums selected by a beets query. Albums without a release ID are skipped.
pub fn sync<P: ReleaseProvider>(
    library: &mut Library,
    provider: &P,
    query: &str,
    write_tags: bool,
) -> Result<SyncResult, SyncError> {
    let albums = library.query_albums(query)?;
    let mut result = SyncResult::default();
    for album in albums {
        let Some(release_id) = text(&album.fields, "mb_albumid") else {
            result.albums_without_release_id += 1;
            continue;
        };
        let release = provider.lookup_release(release_id)?;
        let items = library.items_for_album(album.id)?;
        let mut updates = Vec::new();
        for item in &items {
            let Some(track) = match_track(item, &release) else {
                result.items_without_match += 1;
                continue;
            };
            let (title, artist) = ftclean::clean(&track.title, &track.artist);
            let mut fields = Fields::new();
            fields.insert("title".into(), SqlValue::Text(title));
            fields.insert("artist".into(), SqlValue::Text(artist));
            fields.insert("album".into(), SqlValue::Text(release.title.clone()));
            fields.insert("albumartist".into(), SqlValue::Text(release.artist.clone()));
            fields.insert(
                "track".into(),
                SqlValue::Integer(i64::from(track.medium_index)),
            );
            fields.insert("disc".into(), SqlValue::Integer(i64::from(track.medium)));
            fields.insert("mb_albumid".into(), SqlValue::Text(release.id.0.clone()));
            if let Some(id) = &track.recording_id {
                fields.insert("mb_trackid".into(), SqlValue::Text(id.0.clone()));
            }
            if let Some(year) = release.year {
                fields.insert("year".into(), SqlValue::Integer(i64::from(year)));
            }
            if let Some(id) = &release.release_group_id {
                fields.insert("mb_releasegroupid".into(), SqlValue::Text(id.clone()));
            }
            updates.push((item.id, path(item)?, fields));
        }
        let mut album_fields = Fields::new();
        album_fields.insert("album".into(), SqlValue::Text(release.title));
        album_fields.insert("albumartist".into(), SqlValue::Text(release.artist));
        if let Some(year) = release.year {
            album_fields.insert("year".into(), SqlValue::Integer(i64::from(year)));
        }
        if let Some(id) = release.release_group_id {
            album_fields.insert("mb_releasegroupid".into(), SqlValue::Text(id));
        }
        let backup = if write_tags {
            let temporary = tempfile::tempdir()?;
            for (index, (_, path, _)) in updates.iter().enumerate() {
                fs::copy(path, temporary.path().join(index.to_string()))?;
            }
            Some(temporary)
        } else {
            None
        };
        let update = (|| -> Result<(), SyncError> {
            if write_tags {
                for (_, path, fields) in &updates {
                    let mut tags = muzik_tags::read(path, &[])?;
                    for (name, value) in fields {
                        let tag_name = if name == "year" { "date" } else { name };
                        if let SqlValue::Text(value) = value {
                            tags.fields.insert(tag_name.into(), value.clone());
                        } else if name == "year" {
                            if let SqlValue::Integer(value) = value {
                                tags.fields.insert("date".into(), value.to_string());
                            }
                        } else if ["track", "disc"].contains(&name.as_str())
                            && let SqlValue::Integer(value) = value
                        {
                            tags.fields.insert(name.clone(), value.to_string());
                        }
                    }
                    muzik_tags::write(path, &tags)?;
                }
            }
            library.transaction(|writer| {
                writer.update_album(album.id, &album_fields, &Fields::new())?;
                for (id, _, fields) in &updates {
                    writer.update_item(*id, fields, &Fields::new())?;
                }
                Ok(())
            })?;
            Ok(())
        })();
        if let Err(error) = update {
            if let Some(backup) = &backup {
                for (index, (_, path, _)) in updates.iter().enumerate() {
                    if let Err(restore) = fs::copy(backup.path().join(index.to_string()), path) {
                        tracing::warn!(path = %path.display(), %restore, "cannot restore sync tag backup");
                    }
                }
            }
            return Err(error);
        }
        result.albums_updated += 1;
        result.items_updated += updates.len();
    }
    Ok(result)
}

fn text<'a>(fields: &'a Fields, key: &str) -> Option<&'a str> {
    match fields.get(key) {
        Some(SqlValue::Text(value)) if !value.is_empty() => Some(value),
        _ => None,
    }
}

fn number(fields: &Fields, key: &str) -> Option<i64> {
    match fields.get(key) {
        Some(SqlValue::Integer(value)) => Some(*value),
        _ => None,
    }
}

fn match_track<'a>(
    item: &Item,
    release: &'a ReleaseCandidate,
) -> Option<&'a muzik_core::TrackCandidate> {
    if let Some(id) = text(&item.fields, "mb_trackid")
        && let Some(track) = release.tracks.iter().find(|track| {
            track
                .recording_id
                .as_ref()
                .is_some_and(|recording| recording.0 == id)
        })
    {
        return Some(track);
    }
    let disc = number(&item.fields, "disc").unwrap_or(1);
    let track = number(&item.fields, "track")?;
    release.tracks.iter().find(|candidate| {
        i64::from(candidate.medium) == disc && i64::from(candidate.medium_index) == track
    })
}

fn path(item: &Item) -> Result<PathBuf, SyncError> {
    match item.fields.get("path") {
        Some(SqlValue::Blob(bytes)) => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                Ok(std::ffi::OsString::from_vec(bytes.clone()).into())
            }
            #[cfg(not(unix))]
            {
                Ok(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
            }
        }
        Some(SqlValue::Text(value)) => Ok(Path::new(value).to_owned()),
        _ => Err(SyncError::MissingPath(item.id)),
    }
}
