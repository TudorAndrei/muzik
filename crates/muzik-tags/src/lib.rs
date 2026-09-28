//! Read and write the tag fields used by beets and mediafile.

pub mod cover;
pub mod probe;
pub use cover::{embed_cover, find_cover, has_front_cover};
pub use probe::{probe, AudioProperties};

use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::id3::v2::Id3v2Tag;
use lofty::mp4::Mp4File;
use lofty::mp4::{Atom, AtomData, AtomIdent, Ilst};
use lofty::mpeg::MpegFile;
use lofty::ogg::tag::VorbisComments;
use lofty::ogg::OggPictureStorage;
use lofty::ogg::OpusFile;
use lofty::ogg::VorbisFile;
use lofty::tag::{ItemKey, ItemValue, Tag, TagExt, TagItem, TagType};
use thiserror::Error;
use tracing::debug;

/// A mediafile field name and its lofty key.
#[derive(Debug, Clone, Copy)]
pub struct Field {
    pub name: &'static str,
    pub key: ItemKey,
}

/// Scalar fields used by muzik. Numbers and booleans use mediafile text values.
pub const FIELDS: &[Field] = &[
    Field {
        name: "title",
        key: ItemKey::TrackTitle,
    },
    Field {
        name: "artist",
        key: ItemKey::TrackArtist,
    },
    Field {
        name: "album",
        key: ItemKey::AlbumTitle,
    },
    Field {
        name: "albumartist",
        key: ItemKey::AlbumArtist,
    },
    Field {
        name: "track",
        key: ItemKey::TrackNumber,
    },
    Field {
        name: "tracktotal",
        key: ItemKey::TrackTotal,
    },
    Field {
        name: "disc",
        key: ItemKey::DiscNumber,
    },
    Field {
        name: "disctotal",
        key: ItemKey::DiscTotal,
    },
    Field {
        name: "date",
        key: ItemKey::RecordingDate,
    },
    Field {
        name: "original_date",
        key: ItemKey::OriginalReleaseDate,
    },
    Field {
        name: "mb_trackid",
        key: ItemKey::MusicBrainzRecordingId,
    },
    Field {
        name: "mb_releasetrackid",
        key: ItemKey::MusicBrainzTrackId,
    },
    Field {
        name: "mb_workid",
        key: ItemKey::MusicBrainzWorkId,
    },
    Field {
        name: "mb_albumid",
        key: ItemKey::MusicBrainzReleaseId,
    },
    Field {
        name: "mb_releasegroupid",
        key: ItemKey::MusicBrainzReleaseGroupId,
    },
    Field {
        name: "mb_artistid",
        key: ItemKey::MusicBrainzArtistId,
    },
    Field {
        name: "mb_albumartistid",
        key: ItemKey::MusicBrainzReleaseArtistId,
    },
    Field {
        name: "label",
        key: ItemKey::Label,
    },
    Field {
        name: "catalognum",
        key: ItemKey::CatalogNumber,
    },
    Field {
        name: "country",
        key: ItemKey::ReleaseCountry,
    },
    Field {
        name: "media",
        key: ItemKey::OriginalMediaType,
    },
    Field {
        name: "comp",
        key: ItemKey::FlagCompilation,
    },
    Field {
        name: "rg_track_gain",
        key: ItemKey::ReplayGainTrackGain,
    },
    Field {
        name: "rg_track_peak",
        key: ItemKey::ReplayGainTrackPeak,
    },
    Field {
        name: "rg_album_gain",
        key: ItemKey::ReplayGainAlbumGain,
    },
    Field {
        name: "rg_album_peak",
        key: ItemKey::ReplayGainAlbumPeak,
    },
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagData {
    /// Field names follow mediafile (for example, `mb_albumid`).
    pub fields: BTreeMap<String, String>,
    /// Multi-value artist fields.
    pub lists: BTreeMap<String, Vec<String>>,
    /// User text fields use TXXX, MP4 freeform atoms, or Vorbis comments.
    pub custom: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub enum TagsError {
    #[error("cannot parse cover art: {0}")]
    Picture(String),
    #[error("cannot open audio file: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot read tags: {0}")]
    Read(#[from] lofty::error::FileParseError),
    #[error("cannot write tags: {0}")]
    Write(#[from] lofty::error::FileEncodingError),
    #[error("unsupported audio format: {0:?}")]
    Unsupported(FileType),
    #[error("unsupported tag field: {0}")]
    UnknownField(String),
}

/// Read known mediafile fields and the named custom keys.
pub fn read(path: impl AsRef<Path>, custom_keys: &[&str]) -> Result<TagData, TagsError> {
    let path = path.as_ref();
    let file = lofty::read_from_path(path)?;
    let tag = file.primary_tag();
    let mut data = TagData::default();
    if let Some(tag) = tag {
        for field in FIELDS {
            if let Some(value) = tag.get_string(field.key) {
                data.fields.insert(field.name.into(), value.into());
            }
        }
        if !data.fields.contains_key("label") {
            if let Some(value) = tag.get_string(ItemKey::Publisher) {
                data.fields.insert("label".into(), value.into());
            }
        }
        for (name, key) in [
            ("artists", ItemKey::TrackArtists),
            ("albumartists", ItemKey::AlbumArtists),
        ] {
            let values: Vec<String> = tag.get_strings(key).map(str::to_owned).collect();
            if !values.is_empty() {
                data.lists.insert(name.into(), values);
            }
        }
        let disambig_key = album_disambig_key(file.file_type());
        let mut keys = custom_keys.to_vec();
        if let Some(key) = disambig_key {
            keys.push(key);
        }
        data.custom = read_customs(path, file.file_type(), &keys)?;
        if let Some(key) = disambig_key {
            if let Some(value) = data.custom.remove(key) {
                data.fields.insert("albumdisambig".into(), value);
            }
        }
    }
    debug!(path = %path.display(), count = data.fields.len(), "read audio tags");
    Ok(data)
}

/// Write the supplied fields while keeping tags that are not supplied.
pub fn write(path: impl AsRef<Path>, data: &TagData) -> Result<(), TagsError> {
    let path = path.as_ref();
    let file = lofty::read_from_path(path)?;
    let kind = file.file_type();
    let tag_type = match kind {
        FileType::Mpeg => TagType::Id3v2,
        FileType::Mp4 => TagType::Mp4Ilst,
        FileType::Flac | FileType::Opus | FileType::Vorbis => TagType::VorbisComments,
        _ => return Err(TagsError::Unsupported(kind)),
    };
    let mut tag = file
        .tag(tag_type)
        .cloned()
        .unwrap_or_else(|| Tag::new(tag_type));
    let mut custom = data.custom.clone();
    for (name, value) in &data.fields {
        if name == "albumdisambig" {
            if let Some(key) = album_disambig_key(kind) {
                custom.insert(key.into(), value.clone());
                continue;
            }
        }
        let field = FIELDS
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| TagsError::UnknownField(name.clone()))?;
        tag.insert_text(field.key, value.clone());
    }
    for (name, values) in &data.lists {
        let key = match name.as_str() {
            "artists" => ItemKey::TrackArtists,
            "albumartists" => ItemKey::AlbumArtists,
            _ => return Err(TagsError::UnknownField(name.clone())),
        };
        tag.remove_key(key);
        for value in values {
            tag.push(TagItem::new(key, ItemValue::Text(value.clone())));
        }
    }
    match tag_type {
        TagType::Id3v2 => {
            let mut native = Id3v2Tag::from(tag);
            for (key, value) in &custom {
                native.insert_user_text(key.clone(), value.clone());
            }
            native.save_to_path(path, WriteOptions::default())?;
        }
        TagType::Mp4Ilst => {
            let mut native = Ilst::from(tag);
            for (key, value) in &custom {
                let ident = AtomIdent::Freeform {
                    mean: "com.apple.iTunes".into(),
                    name: key.clone().into(),
                };
                native.insert(Atom::new(ident, AtomData::UTF8(value.clone())));
            }
            native.save_to_path(path, WriteOptions::default())?;
        }
        TagType::VorbisComments => {
            let mut native = VorbisComments::from(tag);
            merge_vorbis_existing(path, kind, &mut native)?;
            for (key, value) in &custom {
                native.insert(key.clone(), value.clone());
            }
            native.save_to_path(path, WriteOptions::default())?;
        }
        _ => unreachable!(),
    }
    debug!(path = %path.display(), count = data.fields.len(), "wrote audio tags");
    Ok(())
}

fn read_customs(
    path: &Path,
    kind: FileType,
    keys: &[&str],
) -> Result<BTreeMap<String, String>, TagsError> {
    let mut result = BTreeMap::new();
    if keys.is_empty() {
        return Ok(result);
    }
    let mut reader = File::open(path)?;
    let options = ParseOptions::default();
    match kind {
        FileType::Mpeg => {
            let file = MpegFile::read_from(&mut reader, options)?;
            if let Some(tag) = file.id3v2() {
                for &key in keys {
                    if let Some(value) = tag.get_user_text(key) {
                        result.insert(key.into(), value.into());
                    }
                }
            }
        }
        FileType::Mp4 => {
            let file = Mp4File::read_from(&mut reader, options)?;
            if let Some(tag) = file.ilst() {
                for &key in keys {
                    let ident = AtomIdent::Freeform {
                        mean: "com.apple.iTunes".into(),
                        name: key.into(),
                    };
                    if let Some(value) = tag.get(&ident).and_then(|atom| {
                        atom.data().find_map(|datum| match datum {
                            AtomData::UTF8(value) | AtomData::UTF16(value) => Some(value.clone()),
                            _ => None,
                        })
                    }) {
                        result.insert(key.into(), value);
                    }
                }
            }
        }
        FileType::Flac => {
            let file = FlacFile::read_from(&mut reader, options)?;
            if let Some(tag) = file.vorbis_comments() {
                for &key in keys {
                    if let Some(value) = tag.get(key) {
                        result.insert(key.into(), value.into());
                    }
                }
            }
        }
        FileType::Opus => {
            let file = OpusFile::read_from(&mut reader, options)?;
            let tag = file.vorbis_comments();
            for &key in keys {
                if let Some(value) = tag.get(key) {
                    result.insert(key.into(), value.into());
                }
            }
        }
        FileType::Vorbis => {
            let file = VorbisFile::read_from(&mut reader, options)?;
            let tag = file.vorbis_comments();
            for &key in keys {
                if let Some(value) = tag.get(key) {
                    result.insert(key.into(), value.into());
                }
            }
        }
        _ => {}
    }
    Ok(result)
}

fn album_disambig_key(kind: FileType) -> Option<&'static str> {
    match kind {
        FileType::Mpeg | FileType::Mp4 => Some("MusicBrainz Album Comment"),
        FileType::Flac | FileType::Opus | FileType::Vorbis => Some("MUSICBRAINZ_ALBUMCOMMENT"),
        _ => None,
    }
}

fn merge_vorbis_existing(
    path: &Path,
    kind: FileType,
    native: &mut VorbisComments,
) -> Result<(), TagsError> {
    let mut reader = File::open(path)?;
    let options = ParseOptions::default();
    let existing = match kind {
        FileType::Flac => FlacFile::read_from(&mut reader, options)?
            .vorbis_comments()
            .cloned(),
        FileType::Opus => Some(
            OpusFile::read_from(&mut reader, options)?
                .vorbis_comments()
                .clone(),
        ),
        FileType::Vorbis => Some(
            VorbisFile::read_from(&mut reader, options)?
                .vorbis_comments()
                .clone(),
        ),
        _ => None,
    };
    if let Some(existing) = existing {
        if native.vendor().is_empty() {
            native.set_vendor(existing.vendor().to_owned());
        }
        for (key, value) in existing.items() {
            if native.get(key).is_none() {
                native.push(key.to_owned(), value.to_owned());
            }
        }
        if native.pictures().is_empty() {
            for (picture, info) in existing.pictures() {
                native
                    .insert_picture(picture.clone(), Some(*info))
                    .map_err(|error| TagsError::Picture(error.to_string()))?;
            }
        }
    }
    Ok(())
}
