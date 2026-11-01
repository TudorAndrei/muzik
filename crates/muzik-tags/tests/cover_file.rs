extern crate bolero as _;
use std::fs;
use std::path::Path;

use muzik_tags::{
    MimeType, embed_cover, find_folder_cover, first_front_cover, image_extension, write_cover_file,
};

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0];

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn image_data_gets_the_extension_of_its_format() {
    assert_eq!(image_extension(JPEG), Some("jpg"));
    assert_eq!(
        image_extension(&fs::read(fixture("cover.png")).unwrap()),
        Some("png")
    );
    assert_eq!(image_extension(b"GIF89a...."), Some("gif"));
    assert_eq!(image_extension(b"RIFF\0\0\0\0WEBPVP8 "), Some("webp"));
    assert_eq!(image_extension(b"RIFF\0\0\0\0WAVE"), None);
    assert_eq!(image_extension(b"<html>"), None);
}

#[test]
fn a_cover_file_is_written_once_and_never_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_cover_file(dir.path(), "cover", JPEG).unwrap();
    assert_eq!(path, dir.path().join("cover.jpg"));
    assert_eq!(fs::read(&path).unwrap(), JPEG);
    assert!(write_cover_file(dir.path(), "cover", JPEG).is_err());
    assert_eq!(fs::read(&path).unwrap(), JPEG);
    assert!(write_cover_file(dir.path(), "cover", b"not an image").is_err());
    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["cover.jpg"]);
}

#[test]
fn folder_covers_include_the_configured_name_in_any_image_format() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(find_folder_cover(dir.path(), "albumart"), None);
    fs::write(dir.path().join("albumart.webp"), b"art").unwrap();
    assert_eq!(
        find_folder_cover(dir.path(), "albumart"),
        Some(dir.path().join("albumart.webp"))
    );
    fs::write(dir.path().join("folder.jpg"), b"art").unwrap();
    assert_eq!(
        find_folder_cover(dir.path(), "albumart"),
        Some(dir.path().join("folder.jpg"))
    );
}

#[test]
fn the_first_readable_front_cover_comes_from_the_tracks() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("01.flac");
    let tagged = dir.path().join("02.flac");
    fs::copy(fixture("blank.flac"), &bare).unwrap();
    fs::copy(fixture("blank.flac"), &tagged).unwrap();
    embed_cover(&tagged, JPEG, &MimeType::Jpeg).unwrap();
    let broken = dir.path().join("00.flac");
    fs::write(&broken, b"not audio").unwrap();
    assert_eq!(
        first_front_cover([&broken, &bare, &tagged]),
        Some(JPEG.to_vec())
    );
    assert_eq!(first_front_cover([&broken, &bare]), None);
}
