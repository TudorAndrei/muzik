//! Find album art near an audio file and embed a front cover.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::flac::FlacFile;
use lofty::id3::v2::Id3v2Tag;
use lofty::mp4::{Ilst, Mp4File};
use lofty::mpeg::MpegFile;
use lofty::ogg::{OggPictureStorage, OpusFile, VorbisFile};
use lofty::picture::{Picture, PictureType};
use lofty::tag::TagExt;

use crate::TagsError;

const NAMES: &[&str] = &["cover", "front", "art", "album", "folder"];
const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];

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

/// Replace the front cover while keeping all non-picture tag values.
pub fn embed_cover(
    path: impl AsRef<Path>,
    image_bytes: &[u8],
    mime_type: &str,
) -> Result<(), TagsError> {
    let path = path.as_ref();
    let mut picture = Picture::from_reader(&mut Cursor::new(image_bytes))
        .map_err(|error| TagsError::Picture(error.to_string()))?;
    if picture
        .mime_type()
        .is_none_or(|actual| actual.as_str() != mime_type)
    {
        return Err(TagsError::Picture(format!(
            "cover image is not {mime_type}"
        )));
    }
    picture.set_pic_type(PictureType::CoverFront);
    let kind = lofty::read_from_path(path)?.file_type();
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
                _ => unreachable!(),
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
pub fn front_cover(path: impl AsRef<Path>) -> Result<Option<(Vec<u8>, String)>, TagsError> {
    let audio = lofty::read_from_path(path)?;
    let pictures: Vec<&Picture> = audio.tags().iter().flat_map(|tag| tag.pictures()).collect();
    let picture = pictures
        .iter()
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures.first());
    Ok(picture.and_then(|picture| {
        let mime = picture.mime_type()?.as_str().to_owned();
        Some((picture.data().to_vec(), mime))
    }))
}

/// Return whether a file has a front cover.
pub fn has_front_cover(path: impl AsRef<Path>) -> Result<bool, TagsError> {
    let audio = lofty::read_from_path(path)?;
    let mp4 = audio.file_type() == FileType::Mp4;
    Ok(audio.tags().iter().any(|tag| {
        tag.pictures()
            .iter()
            .any(|picture| mp4 || picture.pic_type() == PictureType::CoverFront)
    }))
}
