use crate::support::StepResult;
use crate::world::LibraryWorld;
use cucumber::{given, then, when};
use muzik_library::{Error, Fields, Item, Library, SqlValue};
use std::fs;

fn text(name: &str, value: &str) -> (String, SqlValue) {
    (name.to_owned(), SqlValue::Text(value.to_owned()))
}

fn track(world: &LibraryWorld) -> anyhow::Result<Item> {
    world
        .library()?
        .items_for_album(world.album_id()?)?
        .pop()
        .ok_or_else(|| anyhow::anyhow!("the album has no track"))
}

#[given("there is no library file yet")]
fn no_library_file(world: &mut LibraryWorld) -> StepResult {
    world.database = world.root().join("new").join("library.db");
    anyhow::ensure!(
        !world.database.exists(),
        "{} exists",
        world.database.display()
    );
    Ok(())
}

#[then("a library in memory has no items")]
fn memory_library_is_empty(_world: &mut LibraryWorld) -> StepResult {
    let items = Library::empty()?.items()?;
    anyhow::ensure!(items.is_empty(), "items {items:?}");
    Ok(())
}

#[when(expr = "I add the album {string} with the track {string}")]
fn add_album_with_track(world: &mut LibraryWorld, album: String, title: String) -> StepResult {
    let mut library = Library::open_or_create(&world.database)?;
    let album_id = library.insert_album(&Fields::from([text("album", &album)]), &Fields::new())?;
    library.insert_item(
        &Fields::from([
            ("album_id".to_owned(), SqlValue::Integer(album_id)),
            text("title", &title),
        ]),
        &Fields::new(),
    )?;
    world.album_id = Some(album_id);
    world.library = Some(library);
    Ok(())
}

#[then("the library file exists")]
fn library_file_exists(world: &mut LibraryWorld) -> StepResult {
    anyhow::ensure!(
        world.database.is_file(),
        "{} is missing",
        world.database.display()
    );
    Ok(())
}

#[then(expr = "a reader of the library file sees {int} item(s) in the album")]
fn reader_sees_items(world: &mut LibraryWorld, count: usize) -> StepResult {
    let reader = Library::open_read_only(&world.database)?;
    let items = reader.items_for_album(world.album_id()?)?;
    anyhow::ensure!(items.len() == count, "items {items:?}");
    Ok(())
}

#[when(
    expr = "one group of writes adds the album {string} with the review {string}, adds its track {string} from the source {string}, and renames the track to {string}"
)]
fn write_group(
    world: &mut LibraryWorld,
    album: String,
    review: String,
    title: String,
    source: String,
    renamed: String,
) -> StepResult {
    let album_id = world.library_mut()?.transaction(|writer| {
        let album_id = writer.insert_album(
            &Fields::from([text("album", &album)]),
            &Fields::from([text("review", &review)]),
        )?;
        let item_id = writer.insert_item(
            &Fields::from([
                ("album_id".to_owned(), SqlValue::Integer(album_id)),
                text("title", &title),
                (
                    "path".to_owned(),
                    SqlValue::Blob(b"/fixture/new.mp3".to_vec()),
                ),
            ]),
            &Fields::from([text("muzik_source_id", &source)]),
        )?;
        writer.update_item(
            item_id,
            &Fields::from([text("title", &renamed)]),
            &Fields::new(),
        )?;
        Ok(album_id)
    })?;
    world.album_id = Some(album_id);
    Ok(())
}

#[then(expr = "the track in the album has the field {string} set to {string}")]
fn track_has_field(world: &mut LibraryWorld, name: String, value: String) -> StepResult {
    let item = track(world)?;
    let field = item.field(&name);
    anyhow::ensure!(field == Some(&SqlValue::Text(value)), "{name} is {field:?}");
    Ok(())
}

#[then(expr = "the track in the album has the flexible field {string} set to {string}")]
fn track_has_flexible_field(world: &mut LibraryWorld, name: String, value: String) -> StepResult {
    let item = track(world)?;
    let attribute = item.attribute(&name);
    anyhow::ensure!(
        attribute == Some(&SqlValue::Text(value)),
        "{name} is {attribute:?}"
    );
    Ok(())
}

#[then(expr = "the album has the flexible field {string} set to {string}")]
fn new_album_has_flexible_field(
    world: &mut LibraryWorld,
    name: String,
    value: String,
) -> StepResult {
    let album = world
        .library()?
        .album(world.album_id()?)?
        .ok_or_else(|| anyhow::anyhow!("the album is missing"))?;
    let attribute = album.attribute(&name);
    anyhow::ensure!(
        attribute == Some(&SqlValue::Text(value)),
        "{name} is {attribute:?}"
    );
    Ok(())
}

#[then("a backup of the library exists")]
fn backup_exists(world: &mut LibraryWorld) -> StepResult {
    let backup = world.root().join("library.db.native-backup");
    anyhow::ensure!(backup.exists(), "{} is missing", backup.display());
    Ok(())
}

#[when("I remove the track in the album")]
fn remove_track(world: &mut LibraryWorld) -> StepResult {
    let id = track(world)?.id;
    world.library_mut()?.remove_item(id)?;
    Ok(())
}

#[then("the album is removed too")]
fn album_is_removed(world: &mut LibraryWorld) -> StepResult {
    let album = world.library()?.album(world.album_id()?)?;
    anyhow::ensure!(album.is_none(), "album {album:?}");
    Ok(())
}

#[when(
    expr = "one group of writes adds the album {string} and then an item with the unknown field {string}"
)]
fn write_bad_group(world: &mut LibraryWorld, album: String, field: String) -> StepResult {
    let result = world.library_mut()?.transaction(|writer| {
        writer.insert_album(&Fields::from([text("album", &album)]), &Fields::new())?;
        writer.insert_item(&Fields::from([text(&field, "bad")]), &Fields::new())?;
        Ok(())
    });
    world.written = Some(result);
    Ok(())
}

#[then("the group of writes fails because of an invalid field")]
fn group_fails(world: &mut LibraryWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.written, Some(Err(Error::InvalidField { .. }))),
        "written {:?}",
        world.written
    );
    Ok(())
}

#[given("a music folder that holds the file of a new item")]
fn music_folder_with_item(world: &mut LibraryWorld) -> StepResult {
    let music = world.root().join("music");
    fs::create_dir(&music)?;
    let present = music.join("present.mp3");
    fs::write(&present, b"music")?;
    let id = world.library_mut()?.insert_item(
        &Fields::from([(
            "path".to_owned(),
            SqlValue::Blob(present.as_os_str().as_encoded_bytes().to_vec()),
        )]),
        &Fields::new(),
    )?;
    world.kept_item = Some((id, present));
    Ok(())
}

#[when(expr = "I prune the missing items with a safety fraction of {float}")]
fn prune(world: &mut LibraryWorld, fraction: f64) -> StepResult {
    let music = world.root().join("music");
    world.pruned = Some(world.library_mut()?.prune_missing_items(&music, fraction));
    Ok(())
}

#[then(expr = "pruning stops because {int} of {int} items are missing")]
fn pruning_stops(world: &mut LibraryWorld, missing: usize, total: usize) -> StepResult {
    anyhow::ensure!(
        matches!(
            world.pruned,
            Some(Err(Error::PruneAborted { missing: found, total: all }))
                if found == missing && all == total
        ),
        "pruned {:?}",
        world.pruned
    );
    Ok(())
}

#[then(expr = "{int} items are pruned")]
fn items_are_pruned(world: &mut LibraryWorld, count: usize) -> StepResult {
    anyhow::ensure!(
        matches!(world.pruned, Some(Ok(pruned)) if pruned == count),
        "pruned {:?}",
        world.pruned
    );
    Ok(())
}

#[then("the item that remains has the file in the music folder")]
fn remaining_item_has_file(world: &mut LibraryWorld) -> StepResult {
    let (id, present) = world
        .kept_item
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no item was added"))?;
    let item = world
        .library()?
        .item(id)?
        .ok_or_else(|| anyhow::anyhow!("item {id} is missing"))?;
    let path = item.field("path");
    anyhow::ensure!(
        path == Some(&SqlValue::Blob(
            present.as_os_str().as_encoded_bytes().to_vec()
        )),
        "path is {path:?}"
    );
    Ok(())
}
