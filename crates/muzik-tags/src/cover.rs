//! Find album art near an audio file and embed a front cover.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::id3::v2::Id3v2Tag;
use lofty::mp4::{Ilst, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::ogg::{OggPictureStorage, OpusFile, VorbisFile};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::tag::{Tag, TagExt};

use crate::TagsError;

const NAMES: &[&str] = &["cover", "front", "art", "album", "folder"];
const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];
const FOLDER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp"];

/// Find the first named image in a directory tree, in beets cover-name order.
pub fn find_cover(directory: impl AsRef<Path>) -> Option<PathBuf> {
    let mut directories = vec![directory.as_ref().to_path_buf()];
    let mut candidates = Vec::new();
    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                directories.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if !EXTENSIONS.contains(&extension.as_str()) {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if let Some(priority) = NAMES
                .iter()
                .position(|name| stem.split(['_', '-', ' ', '.']).any(|part| part == *name))
            {
                candidates.push((priority, path));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    candidates.into_iter().next().map(|(_, path)| path)
}

/// Return the MIME type for a cover found by [`find_cover`].
#[must_use]
pub fn cover_mime_type(path: impl AsRef<Path>) -> MimeType {
    if path
        .as_ref()
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        MimeType::Png
    } else {
        MimeType::Jpeg
    }
}

/// Replace the front cover while keeping all non-picture tag values.
///
/// # Errors
///
/// Returns an error when the image is not of `mime_type`, the format is not supported, or the file cannot be read or written.
pub fn embed_cover(
    path: impl AsRef<Path>,
    image_bytes: &[u8],
    mime_type: &MimeType,
) -> Result<(), TagsError> {
    let path = path.as_ref();
    let mut picture = Picture::from_reader(&mut Cursor::new(image_bytes))
        .map_err(|error| TagsError::Picture(error.to_string()))?;
    if picture.mime_type() != Some(mime_type) {
        return Err(TagsError::Picture(format!(
            "cover image is not {}",
            mime_type.as_str()
        )));
    }
    picture.set_pic_type(PictureType::CoverFront);
    let kind = crate::open(path)?.file_type();
    let mut reader = fs::File::open(path)?;
    let options = ParseOptions::default();
    match kind {
        FileType::Mpeg => {
            let file = MpegFile::read_from(&mut reader, options)?;
            let mut tag = file.id3v2().cloned().unwrap_or_else(Id3v2Tag::new);
            tag.remove_picture_type(PictureType::CoverFront);
            tag.insert_picture(picture);
            tag.save_to_path(path, WriteOptions::default())?;
        }
        FileType::Mp4 => {
            let file = Mp4File::read_from(&mut reader, options)?;
            let mut tag = file.ilst().cloned().unwrap_or_else(Ilst::new);
            tag.remove_pictures();
            tag.insert_picture(picture);
            tag.save_to_path(path, WriteOptions::default())?;
        }
        FileType::Flac | FileType::Opus | FileType::Vorbis => {
            let mut tag = match kind {
                FileType::Flac => FlacFile::read_from(&mut reader, options)?
                    .vorbis_comments()
                    .cloned()
                    .unwrap_or_default(),
                FileType::Opus => OpusFile::read_from(&mut reader, options)?
                    .vorbis_comments()
                    .clone(),
                FileType::Vorbis => VorbisFile::read_from(&mut reader, options)?
                    .vorbis_comments()
                    .clone(),
                _ => return Err(TagsError::Unsupported(kind)),
            };
            tag.remove_picture_type(PictureType::CoverFront);
            tag.insert_picture(picture, None)
                .map_err(|error| TagsError::Picture(error.to_string()))?;
            tag.save_to_path(path, WriteOptions::default())?;
        }
        _ => return Err(TagsError::Unsupported(kind)),
    }
    Ok(())
}

/// Return the front cover image and its MIME type, or the first picture.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
pub fn front_cover(path: impl AsRef<Path>) -> Result<Option<(Vec<u8>, MimeType)>, TagsError> {
    let audio = crate::open(path.as_ref())?;
    let pictures: Vec<&Picture> = audio.tags().iter().flat_map(Tag::pictures).collect();
    let picture = pictures
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first());
    Ok(picture.and_then(|picture| {
        let mime = picture.mime_type()?.clone();
        Some((picture.data().to_vec(), mime))
    }))
}

/// Return the file extension for JPEG, PNG, GIF, or WebP image data.
#[must_use]
pub fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']) {
        Some("png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        Some("webp")
    } else {
        None
    }
}

/// Find a cover image in an album folder, including `<name>.<ext>` files that [`find_cover`] does not match.
pub fn find_folder_cover(directory: impl AsRef<Path>, name: &str) -> Option<PathBuf> {
    let directory = directory.as_ref();
    find_cover(directory).or_else(|| {
        FOLDER_EXTENSIONS
            .iter()
            .map(|extension| directory.join(format!("{name}.{extension}")))
            .find(|path| path.symlink_metadata().is_ok())
    })
}

/// Return the first front cover found in the files, and skip files that cannot be read.
pub fn first_front_cover<P: AsRef<Path>>(paths: impl IntoIterator<Item = P>) -> Option<Vec<u8>> {
    paths.into_iter().find_map(|path| match front_cover(path) {
        Ok(Some((bytes, _))) if !bytes.is_empty() => Some(bytes),
        _ => None,
    })
}

/// Write image data as `<name>.<ext>` in a folder. The file appears in one step and an existing file is never replaced.
///
/// # Errors
///
/// Returns an error when the data is not a known image, the file already exists, or the file cannot be written.
pub fn write_cover_file(
    directory: impl AsRef<Path>,
    name: &str,
    image_bytes: &[u8],
) -> Result<PathBuf, TagsError> {
    let directory = directory.as_ref();
    let path = cover_file_path(directory, name, image_bytes)?;
    let mut file = tempfile::Builder::new()
        .prefix(".muzik-cover-")
        .tempfile_in(directory)?;
    file.write_all(image_bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(&path)
        .map_err(|error| TagsError::Io(error.error))?;
    Ok(path)
}

/// Return the path that [`write_cover_file`] uses for the image data.
///
/// # Errors
///
/// Returns an error when the data is not a known image.
pub fn cover_file_path(
    directory: impl AsRef<Path>,
    name: &str,
    image_bytes: &[u8],
) -> Result<PathBuf, TagsError> {
    let extension = image_extension(image_bytes).ok_or_else(|| {
        TagsError::Picture("cover data is not a JPEG, PNG, GIF, or WebP image".into())
    })?;
    Ok(directory.as_ref().join(format!("{name}.{extension}")))
}

/// Return whether a file has a front cover.
///
/// # Errors
///
/// Returns an error when the file cannot be read.
pub fn has_front_cover(path: impl AsRef<Path>) -> Result<bool, TagsError> {
    let audio = crate::open(path.as_ref())?;
    let mp4 = audio.file_type() == FileType::Mp4;
    Ok(audio.tags().iter().any(|tag| {
        tag.pictures()
            .iter()
            .any(|picture| mp4 || picture.pic_type() == PictureType::CoverFront)
    }))
}
