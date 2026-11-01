use super::tables::{cell, check_fields, records};
use crate::support::{self, StepResult};
use crate::world::ImportWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use muzik_import::mbid_fill::{FillSettings, albums_without_mbid, find_match, write_mbids};
use muzik_library::{Fields, Item, Library, SqlValue, path_to_sql};
use std::fs;
use std::path::Path;

fn text(value: &str) -> SqlValue {
    SqlValue::Text(value.into())
}

fn library_song(world: &mut ImportWorld, number: usize) -> anyhow::Result<Item> {
    let index = number
        .checked_sub(1)
        .ok_or_else(|| anyhow::anyhow!("songs are counted from 1"))?;
    world
        .library()?
        .items()?
        .into_iter()
        .nth(index)
        .ok_or_else(|| anyhow::anyhow!("the library has no song {number}"))
}

fn song_file(world: &ImportWorld, number: usize) -> anyhow::Result<&Path> {
    let index = number
        .checked_sub(1)
        .ok_or_else(|| anyhow::anyhow!("songs are counted from 1"))?;
    world
        .album_files
        .get(index)
        .map(|(path, _)| path.as_path())
        .ok_or_else(|| anyhow::anyhow!("the library has no song {number}"))
}

fn tag(path: &Path, name: &str) -> anyhow::Result<Option<String>> {
    Ok(muzik_tags::read(path, &[])?.fields.get(name).cloned())
}

#[given(expr = "a new beets library with the album {string} by {string} and these songs:")]
fn new_library(world: &mut ImportWorld, album: String, artist: String, step: &Step) -> StepResult {
    let folder = world.root.join(&artist).join(&album);
    fs::create_dir_all(&folder)?;
    let database = world.dir.path().join("new-library.db");
    let mut library = Library::open_or_create(&database)?;
    let album_id = library.insert_album(
        &Fields::from([
            ("album".into(), text(&album)),
            ("albumartist".into(), text(&artist)),
        ]),
        &Fields::new(),
    )?;
    for record in records(step)? {
        let track: i64 = cell(&record, "track")?.parse()?;
        let title = cell(&record, "title")?;
        let path = folder.join(format!("{track:02} {title}.flac"));
        fs::copy(
            support::fixture("muzik-tags/tests/fixtures/blank.flac"),
            &path,
        )?;
        let mut tags = muzik_tags::TagData::default();
        tags.fields.insert("title".into(), title.into());
        tags.fields.insert("artist".into(), artist.clone());
        tags.fields.insert("album".into(), album.clone());
        muzik_tags::write(&path, &tags)?;
        library.insert_item(
            &Fields::from([
                ("album_id".into(), SqlValue::Integer(album_id)),
                ("title".into(), text(title)),
                ("artist".into(), text(&artist)),
                ("album".into(), text(&album)),
                ("albumartist".into(), text(&artist)),
                ("track".into(), SqlValue::Integer(track)),
                ("disc".into(), SqlValue::Integer(1)),
                ("length".into(), SqlValue::Real(200.0)),
                ("path".into(), path_to_sql(&path)),
            ]),
            &Fields::new(),
        )?;
        world.album_files.push((path, title.to_owned()));
    }
    world.database = database;
    world.library = None;
    Ok(())
}

#[then(expr = "the library has {int} album(s) without a MusicBrainz id")]
fn albums_without_id(world: &mut ImportWorld, count: usize) -> StepResult {
    world.missing = albums_without_mbid(world.library()?)?;
    let missing = world.missing.len();
    anyhow::ensure!(
        missing == count,
        "the library has {missing} albums without an id"
    );
    Ok(())
}

#[when("muzik looks for the release of that album")]
fn look_for_release(world: &mut ImportWorld) -> StepResult {
    let settings = FillSettings::from_beets(&world.config)?;
    let album = world
        .missing
        .first()
        .ok_or_else(|| anyhow::anyhow!("no album is missing an id"))?;
    world.found = Some(find_match(&world.catalog, &settings, album, &|| false)?);
    Ok(())
}

#[then("the match is strong")]
fn match_is_strong(world: &mut ImportWorld) -> StepResult {
    let found = world
        .found
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("muzik did not look for a match"))?;
    anyhow::ensure!(found.strong, "the match is weak: {found:?}");
    Ok(())
}

#[when("muzik writes the MusicBrainz ids of the best match")]
fn write_ids(world: &mut ImportWorld) -> StepResult {
    world.open_library()?;
    let Some(library) = world.library.as_mut() else {
        anyhow::bail!("the library is not open");
    };
    let album = world
        .missing
        .first()
        .ok_or_else(|| anyhow::anyhow!("no album is missing an id"))?;
    let candidate = world
        .found
        .as_ref()
        .and_then(|found| found.candidates.first())
        .ok_or_else(|| anyhow::anyhow!("there is no match"))?;
    write_mbids(library, &world.catalog, album, candidate)?;
    Ok(())
}

#[then("the library album has these fields:")]
fn library_album_fields(world: &mut ImportWorld, step: &Step) -> StepResult {
    let album = world
        .library()?
        .albums()?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("the library has no albums"))?;
    check_fields("the library album", &album.fields, step)
}

#[then(expr = "library song {int} has these fields:")]
fn library_song_fields(world: &mut ImportWorld, number: usize, step: &Step) -> StepResult {
    let item = library_song(world, number)?;
    check_fields(&format!("library song {number}"), &item.fields, step)
}

#[then("every library song keeps its file and title and has no year")]
fn every_song_keeps_fields(world: &mut ImportWorld) -> StepResult {
    let files = world.album_files.clone();
    let items = world.library()?.items()?;
    anyhow::ensure!(
        items.len() == files.len(),
        "the library has {} songs",
        items.len()
    );
    for (item, (path, title)) in items.iter().zip(&files) {
        anyhow::ensure!(
            item.field("path") == Some(&path_to_sql(path)),
            "song {} moved",
            item.id
        );
        anyhow::ensure!(
            item.field("title") == Some(&text(title)),
            "song {} has the title {:?}",
            item.id,
            item.field("title")
        );
        anyhow::ensure!(
            item.field("year") == Some(&SqlValue::Null),
            "song {} has the year {:?}",
            item.id,
            item.field("year")
        );
    }
    Ok(())
}

#[then(expr = "every song file keeps the artist {string}, the album {string}, and its title")]
fn every_file_keeps_tags(world: &mut ImportWorld, artist: String, album: String) -> StepResult {
    for (path, title) in &world.album_files {
        for (name, value) in [("title", title), ("artist", &artist), ("album", &album)] {
            let actual = tag(path, name)?;
            anyhow::ensure!(
                actual.as_ref() == Some(value),
                "{} has the tag {name} = {actual:?}",
                path.display()
            );
        }
    }
    Ok(())
}

#[then(expr = "every song file has the tag {string} set to {string}")]
fn every_file_has_tag(world: &mut ImportWorld, name: String, value: String) -> StepResult {
    for (path, _) in &world.album_files {
        let actual = tag(path, &name)?;
        anyhow::ensure!(
            actual.as_ref() == Some(&value),
            "{} has the tag {name} = {actual:?}",
            path.display()
        );
    }
    Ok(())
}

#[then(expr = "no song file has the tag {string}")]
fn no_file_has_tag(world: &mut ImportWorld, name: String) -> StepResult {
    for (path, _) in &world.album_files {
        let actual = tag(path, &name)?;
        anyhow::ensure!(
            actual.is_none(),
            "{} has the tag {name} = {actual:?}",
            path.display()
        );
    }
    Ok(())
}

#[then(expr = "the file of library song {int} has the tag {string} set to {string}")]
fn song_file_has_tag(
    world: &mut ImportWorld,
    number: usize,
    name: String,
    value: String,
) -> StepResult {
    let path = song_file(world, number)?;
    let actual = tag(path, &name)?;
    anyhow::ensure!(
        actual.as_ref() == Some(&value),
        "{} has the tag {name} = {actual:?}",
        path.display()
    );
    Ok(())
}
