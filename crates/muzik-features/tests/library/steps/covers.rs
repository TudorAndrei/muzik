use crate::support::{StepResult, fixture};
use crate::world::LibraryWorld;
use cucumber::{given, then, when};
use muzik_tags::{
    MimeType, embed_cover, find_folder_cover, first_front_cover, image_extension, write_cover_file,
};
use std::fs;
use std::path::PathBuf;

const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0];

fn tracks(world: &LibraryWorld, names: &str) -> Vec<PathBuf> {
    names
        .split(',')
        .map(|name| world.album_dir().join(name.trim()))
        .collect()
}

fn blank_track(world: &LibraryWorld, name: &str) -> anyhow::Result<PathBuf> {
    let track = world.album_dir().join(name);
    fs::copy(fixture("muzik-tags/tests/fixtures/blank.flac"), &track)?;
    Ok(track)
}

#[given(expr = "{word} image data")]
fn image_data(world: &mut LibraryWorld, format: String) -> StepResult {
    world.image = match format.as_str() {
        "JPEG" => JPEG.to_vec(),
        "PNG" => fs::read(fixture("muzik-tags/tests/fixtures/cover.png"))?,
        "GIF" => b"GIF89a....".to_vec(),
        "WebP" => b"RIFF\0\0\0\0WEBPVP8 ".to_vec(),
        "WAV" => b"RIFF\0\0\0\0WAVE".to_vec(),
        "HTML" => b"<html>".to_vec(),
        other => anyhow::bail!("unknown image format {other}"),
    };
    Ok(())
}

#[given(expr = "the data {string}")]
fn plain_data(world: &mut LibraryWorld, data: String) {
    world.image = data.into_bytes();
}

#[then(expr = "the image file extension is {string}")]
fn image_extension_is(world: &mut LibraryWorld, expected: String) -> StepResult {
    let extension = image_extension(&world.image);
    let expected = (expected != "none").then_some(expected.as_str());
    anyhow::ensure!(extension == expected, "extension is {extension:?}");
    Ok(())
}

#[when(expr = "I write the cover file {string}")]
fn write_cover(world: &mut LibraryWorld, name: String) {
    world.cover = Some(write_cover_file(world.album_dir(), &name, &world.image));
}

#[then(expr = "the cover file is {string} with the image data")]
fn cover_file_is(world: &mut LibraryWorld, name: String) -> StepResult {
    let expected = world.album_dir().join(name);
    match &world.cover {
        Some(Ok(path)) => anyhow::ensure!(*path == expected, "cover file is {}", path.display()),
        other => anyhow::bail!("writing the cover file gave {other:?}"),
    }
    anyhow::ensure!(
        fs::read(&expected)? == world.image,
        "the cover data differs"
    );
    Ok(())
}

#[then("writing the cover file fails")]
fn writing_cover_fails(world: &mut LibraryWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.cover, Some(Err(_))),
        "writing the cover file gave {:?}",
        world.cover
    );
    Ok(())
}

#[then(expr = "the file {string} still has the image data")]
fn file_still_has_image(world: &mut LibraryWorld, name: String) -> StepResult {
    let data = fs::read(world.album_dir().join(name))?;
    anyhow::ensure!(data == world.image, "the file data changed");
    Ok(())
}

#[then(expr = "the album folder holds only {string}")]
fn album_folder_holds_only(world: &mut LibraryWorld, name: String) -> StepResult {
    let names = fs::read_dir(world.album_dir())?
        .map(|entry| Ok(entry?.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    anyhow::ensure!(names == [name.as_str()], "the album folder holds {names:?}");
    Ok(())
}

#[given("an empty album folder")]
fn empty_album_folder(world: &mut LibraryWorld) -> StepResult {
    let count = fs::read_dir(world.album_dir())?.count();
    anyhow::ensure!(count == 0, "the album folder holds {count} files");
    Ok(())
}

#[given(expr = "the album folder holds {string}")]
fn album_folder_holds(world: &mut LibraryWorld, name: String) -> StepResult {
    fs::write(world.album_dir().join(name), b"art")?;
    Ok(())
}

#[then(expr = "the album folder has no cover for the name {string}")]
fn no_folder_cover(world: &mut LibraryWorld, name: String) -> StepResult {
    let cover = find_folder_cover(world.album_dir(), &name);
    anyhow::ensure!(cover.is_none(), "found {cover:?}");
    Ok(())
}

#[then(expr = "the album folder cover for the name {string} is {string}")]
fn folder_cover_is(world: &mut LibraryWorld, name: String, file: String) -> StepResult {
    let cover = find_folder_cover(world.album_dir(), &name);
    anyhow::ensure!(
        cover == Some(world.album_dir().join(file)),
        "found {cover:?}"
    );
    Ok(())
}

#[given(expr = "the track {string} is not audio")]
fn track_is_not_audio(world: &mut LibraryWorld, name: String) -> StepResult {
    fs::write(world.album_dir().join(name), b"not audio")?;
    Ok(())
}

#[given(expr = "the track {string} has no cover")]
fn track_has_no_cover(world: &mut LibraryWorld, name: String) -> StepResult {
    blank_track(world, &name)?;
    Ok(())
}

#[given(expr = "the track {string} has a JPEG front cover")]
fn track_has_jpeg_cover(world: &mut LibraryWorld, name: String) -> StepResult {
    let track = blank_track(world, &name)?;
    embed_cover(&track, JPEG, &MimeType::Jpeg)?;
    Ok(())
}

#[then(expr = "the front cover of the tracks {string} is the JPEG image")]
fn front_cover_is_jpeg(world: &mut LibraryWorld, names: String) -> StepResult {
    let cover = first_front_cover(tracks(world, &names));
    anyhow::ensure!(cover.as_deref() == Some(JPEG), "front cover {cover:?}");
    Ok(())
}

#[then(expr = "the tracks {string} have no front cover")]
fn tracks_have_no_cover(world: &mut LibraryWorld, names: String) -> StepResult {
    let cover = first_front_cover(tracks(world, &names));
    anyhow::ensure!(cover.is_none(), "front cover {cover:?}");
    Ok(())
}
