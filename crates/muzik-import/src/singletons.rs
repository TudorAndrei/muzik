//! Songs without an album: the album tags they lose, and one-song albums that become such songs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use muzik_core::BeetsConfig;
use muzik_library::{
    Album, Fields, Item, Library, SqlValue, path_from_sql, path_to_sql, scalar_text,
};
use muzik_tags::{ListField, TagData};

use crate::apply::{ApplyOptions, item_destination, path_fields};
use crate::fields::tags_from_item;
use crate::files::{self, Placement};
use crate::paths::{PathKind, TemplateContext};
use crate::{Error, Result};

pub const ORIGINAL_ALBUM: &str = "muzik_original_album";
pub const ORIGINAL_RELEASE: &str = "muzik_original_mb_albumid";

const ALBUM_TAGS: [&str; 18] = [
    "album",
    "albumartist",
    "track",
    "tracktotal",
    "disc",
    "disctotal",
    "mb_albumid",
    "mb_releasegroupid",
    "mb_albumartistid",
    "mb_releasetrackid",
    "comp",
    "label",
    "catalognum",
    "country",
    "media",
    "albumdisambig",
    "rg_album_gain",
    "rg_album_peak",
];

const TEXT_COLUMNS: [&str; 25] = [
    "album",
    "albumartist",
    "albumartist_credit",
    "albumartist_sort",
    "albumartists",
    "albumartists_credit",
    "albumartists_sort",
    "albumdisambig",
    "albumstatus",
    "albumtype",
    "albumtypes",
    "asin",
    "barcode",
    "catalognum",
    "country",
    "disctitle",
    "label",
    "mb_albumartistid",
    "mb_albumartistids",
    "mb_albumid",
    "mb_releasegroupid",
    "mb_releasetrackid",
    "media",
    "release_group_title",
    "releasegroupdisambig",
];

const NUMBER_COLUMNS: [&str; 5] = ["track", "tracktotal", "disc", "disctotal", "comp"];

const NULL_COLUMNS: [&str; 4] = [
    "album_id",
    "rg_album_gain",
    "rg_album_peak",
    "r128_album_gain",
];

/// Remove the album tags of a song and return the album name and release ID it had.
pub(crate) fn strip_album_tags(tags: &mut TagData) -> Fields {
    let mut original = Fields::new();
    for (tag, attribute) in [("album", ORIGINAL_ALBUM), ("mb_albumid", ORIGINAL_RELEASE)] {
        if let Some(value) = tags
            .fields
            .get(tag)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            original.insert(attribute.to_owned(), SqlValue::Text(value.to_owned()));
        }
    }
    for name in ALBUM_TAGS {
        tags.fields.remove(name);
        tags.removed.insert(name.to_owned());
    }
    tags.lists.remove(&ListField::AlbumArtists);
    tags.removed
        .insert(ListField::AlbumArtists.name().to_owned());
    original
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OneSongAlbum {
    pub album_id: i64,
    pub item_id: i64,
    pub name: String,
    pub path: PathBuf,
    /// The cover file of the album, when no other album uses it.
    pub cover: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Converted {
    pub destination: PathBuf,
    /// Old album files that remain after the conversion.
    pub cleanup_failed: Vec<PathBuf>,
}

/// Find the albums that have one song and are not a single release.
///
/// # Errors
/// Returns an error when the library cannot be read or a song has no path.
pub fn one_song_albums(library: &Library, root: &Path) -> Result<Vec<OneSongAlbum>> {
    let items = library.items()?;
    let mut by_album = BTreeMap::<i64, Vec<&Item>>::new();
    for item in &items {
        if let Some(id) = item.album_id() {
            by_album.entry(id).or_default().push(item);
        }
    }
    let albums = library.albums()?;
    let mut covers = BTreeMap::<PathBuf, usize>::new();
    for album in &albums {
        if let Some(path) = album.field("artpath").and_then(path_from_sql) {
            let count = covers.entry(root.join(path)).or_default();
            *count = count.saturating_add(1);
        }
    }
    let mut found = Vec::new();
    for album in &albums {
        let Some([item]) = by_album.get(&album.id).map(Vec::as_slice) else {
            continue;
        };
        if is_single_release(album, item) {
            continue;
        }
        let path = item
            .field("path")
            .and_then(path_from_sql)
            .ok_or(Error::MissingPath(item.id))?;
        let cover = album
            .field("artpath")
            .and_then(path_from_sql)
            .map(|path| root.join(path))
            .filter(|path| covers.get(path) == Some(&1));
        let text = |name: &str| {
            album
                .field(name)
                .and_then(scalar_text)
                .or_else(|| item.field(name).and_then(scalar_text))
                .unwrap_or_default()
        };
        found.push(OneSongAlbum {
            album_id: album.id,
            item_id: item.id,
            name: format!("{} - {}", text("albumartist"), text("album")),
            path: root.join(path),
            cover,
        });
    }
    Ok(found)
}

fn is_single_release(album: &Album, item: &Item) -> bool {
    matches!(item.field("tracktotal"), Some(SqlValue::Integer(1)))
        || album
            .field("albumtype")
            .and_then(scalar_text)
            .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("single"))
}

pub struct Converter {
    options: ApplyOptions,
}

impl Converter {
    /// # Errors
    /// Returns an error when a `replace` rule in the config is not a valid regex.
    pub fn from_beets(config: &BeetsConfig, library_root: PathBuf) -> Result<Self> {
        Ok(Self {
            options: ApplyOptions::from_beets(config, library_root)?,
        })
    }

    /// The path the song gets as a song without an album.
    ///
    /// # Errors
    /// Returns an error when the song is not in the library or the path format fails.
    pub fn destination(&self, library: &Library, album: &OneSongAlbum) -> Result<PathBuf> {
        let (tags, _) = song_tags(library, album)?;
        self.destination_for(&tags, &album.path)
    }

    fn destination_for(&self, tags: &TagData, source: &Path) -> Result<PathBuf> {
        let options = &self.options;
        let context = TemplateContext {
            fields: path_fields(tags),
            album_id: None,
            albums: &[],
            aunique_keys: &options.aunique_keys,
            aunique_disambiguators: &options.aunique_disambiguators,
            aunique_bracket: &options.aunique_bracket,
            path_sep_replace: &options.path_sep_replace,
            drive_sep_replace: &options.drive_sep_replace,
        };
        item_destination(options, PathKind::Singleton, &context, source)
    }

    /// Remove the album tags from the file, embed the album cover, move the file to the singleton
    /// path, and remove the album from the library.
    ///
    /// # Errors
    /// Returns an error when the destination is taken or a file, tag, or library write fails.
    /// The file and the library stay unchanged after an error.
    pub fn convert(&self, library: &mut Library, album: &OneSongAlbum) -> Result<Converted> {
        let (tags, original) = song_tags(library, album)?;
        let destination = self.destination_for(&tags, &album.path)?;
        let moves = destination != album.path;
        if moves && destination.symlink_metadata().is_ok() {
            return Err(Error::DestinationExists(destination));
        }
        let cover = album.cover.as_ref().filter(|path| path.is_file());
        let database = library.path().to_path_buf();
        let mut fields = cleared_fields();
        fields.insert("path".into(), path_to_sql(&destination));
        crate::backup::with_backups(&database, [album.path.clone()], || {
            if self.options.write_tags {
                muzik_tags::write(&album.path, &tags)?;
            }
            if self.options.embed_art
                && let Some(cover) = cover
                && !muzik_tags::has_front_cover(&album.path)?
            {
                let mime = muzik_tags::cover_mime_type(cover);
                muzik_tags::embed_cover(&album.path, &fs::read(cover)?, &mime)?;
            }
            if moves {
                files::place(&album.path, &destination, Placement::Move)?;
            }
            let written = library.transaction(|writer| {
                writer.update_item(album.item_id, &fields, &original)?;
                writer.remove_album(album.album_id)
            });
            if let Err(error) = written {
                if moves && let Err(back) = files::place(&destination, &album.path, Placement::Move)
                {
                    tracing::error!(path = %destination.display(), %back, "cannot move the song back after a failed library write");
                }
                return Err(error.into());
            }
            Ok(())
        })?;
        let mut cleanup_failed = Vec::new();
        if let Some(cover) = cover
            && let Err(error) = files::move_to_trash(cover)
        {
            tracing::warn!(path = %cover.display(), %error, "old album cover remains");
            cleanup_failed.push(cover.clone());
        }
        if moves
            && let Some(folder) = album.path.parent()
            && folder.is_dir()
            && let Err(error) = files::prune_empty_parents(folder, &self.options.library_root)
        {
            tracing::warn!(path = %folder.display(), %error, "old album folder remains");
        }
        Ok(Converted {
            destination,
            cleanup_failed,
        })
    }
}

fn song_tags(library: &Library, album: &OneSongAlbum) -> Result<(TagData, Fields)> {
    let item = library
        .item(album.item_id)?
        .ok_or(Error::MissingPath(album.item_id))?;
    let mut tags = tags_from_item(&item);
    let original = strip_album_tags(&mut tags);
    Ok((tags, original))
}

fn cleared_fields() -> Fields {
    TEXT_COLUMNS
        .into_iter()
        .map(|name| (name.to_owned(), SqlValue::Text(String::new())))
        .chain(
            NUMBER_COLUMNS
                .into_iter()
                .map(|name| (name.to_owned(), SqlValue::Integer(0))),
        )
        .chain(
            NULL_COLUMNS
                .into_iter()
                .map(|name| (name.to_owned(), SqlValue::Null)),
        )
        .collect()
}
