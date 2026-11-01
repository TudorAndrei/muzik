use crate::support::StepResult;
use crate::world::{LibraryWorld, ids};
use cucumber::{given, then, when};
use muzik_library::{Library, SqlValue};

#[given("the sample beets library")]
fn sample_library(world: &mut LibraryWorld) -> StepResult {
    world.library = Some(Library::open_read_only(&LibraryWorld::sample())?);
    Ok(())
}

#[given("a copy of the sample beets library")]
fn copy_of_sample_library(world: &mut LibraryWorld) -> StepResult {
    std::fs::copy(LibraryWorld::sample(), &world.database)?;
    world.library = Some(Library::open_read_write(&world.database)?);
    Ok(())
}

#[when("I list the items")]
fn list_items(world: &mut LibraryWorld) -> StepResult {
    world.items = world.library()?.items()?;
    Ok(())
}

#[when("I list the albums")]
fn list_albums(world: &mut LibraryWorld) -> StepResult {
    world.albums = world.library()?.albums()?;
    Ok(())
}

#[then(expr = "the items are {string}")]
fn items_are(world: &mut LibraryWorld, expected: String) -> StepResult {
    let found: Vec<i64> = world.items.iter().map(|item| item.id).collect();
    anyhow::ensure!(found == ids(&expected)?, "items {found:?}");
    Ok(())
}

#[then(expr = "the albums are {string}")]
fn albums_are(world: &mut LibraryWorld, expected: String) -> StepResult {
    let found: Vec<i64> = world.albums.iter().map(|album| album.id).collect();
    anyhow::ensure!(found == ids(&expected)?, "albums {found:?}");
    Ok(())
}

#[then(expr = "the library has {int} item(s)")]
fn library_has_items(world: &mut LibraryWorld, count: usize) -> StepResult {
    let items = world.library()?.items()?;
    anyhow::ensure!(items.len() == count, "items {items:?}");
    Ok(())
}

#[then(expr = "the library has {int} album(s)")]
fn library_has_albums(world: &mut LibraryWorld, count: usize) -> StepResult {
    let albums = world.library()?.albums()?;
    anyhow::ensure!(albums.len() == count, "albums {albums:?}");
    Ok(())
}

#[then(expr = "item {int} has the field {string} set to {string}")]
fn item_has_text_field(
    world: &mut LibraryWorld,
    id: i64,
    name: String,
    value: String,
) -> StepResult {
    let field = world.item(id)?.field(&name);
    anyhow::ensure!(field == Some(&SqlValue::Text(value)), "{name} is {field:?}");
    Ok(())
}

#[then(expr = "item {int} has the field {string} set to {float}")]
fn item_has_real_field(world: &mut LibraryWorld, id: i64, name: String, value: f64) -> StepResult {
    let field = world.item(id)?.field(&name);
    anyhow::ensure!(field == Some(&SqlValue::Real(value)), "{name} is {field:?}");
    Ok(())
}

#[then(expr = "item {int} has its path stored as bytes")]
fn item_path_is_bytes(world: &mut LibraryWorld, id: i64) -> StepResult {
    let path = world.item(id)?.field("path");
    anyhow::ensure!(matches!(path, Some(SqlValue::Blob(_))), "path is {path:?}");
    Ok(())
}

#[then(expr = "item {int} is in album {int}")]
fn item_is_in_album(world: &mut LibraryWorld, id: i64, album: i64) -> StepResult {
    let album_id = world.item(id)?.album_id();
    anyhow::ensure!(album_id == Some(album), "album id is {album_id:?}");
    Ok(())
}

#[then(expr = "item {int} is not in an album")]
fn item_is_not_in_album(world: &mut LibraryWorld, id: i64) -> StepResult {
    let album_id = world.item(id)?.album_id();
    anyhow::ensure!(album_id.is_none(), "album id is {album_id:?}");
    Ok(())
}

#[then(expr = "item {int} has the flexible field {string} set to {string}")]
fn item_has_flexible_field(
    world: &mut LibraryWorld,
    id: i64,
    name: String,
    value: String,
) -> StepResult {
    let attribute = world.item(id)?.attribute(&name);
    anyhow::ensure!(
        attribute == Some(&SqlValue::Text(value)),
        "{name} is {attribute:?}"
    );
    Ok(())
}

#[then(expr = "item {int} reads the same alone as in the list")]
fn item_reads_alone(world: &mut LibraryWorld, id: i64) -> StepResult {
    let listed = world.item(id)?;
    let alone = world.library()?.item(id)?;
    anyhow::ensure!(alone.as_ref() == Some(listed), "alone {alone:?}");
    Ok(())
}

#[then(expr = "album {int} has the field {string} set to {string}")]
fn album_has_field(world: &mut LibraryWorld, id: i64, name: String, value: String) -> StepResult {
    let field = world.album(id)?.field(&name);
    anyhow::ensure!(field == Some(&SqlValue::Text(value)), "{name} is {field:?}");
    Ok(())
}

#[then(expr = "album {int} has the flexible field {string} set to {string}")]
fn album_has_flexible_field(
    world: &mut LibraryWorld,
    id: i64,
    name: String,
    value: String,
) -> StepResult {
    let attribute = world.album(id)?.attribute(&name);
    anyhow::ensure!(
        attribute == Some(&SqlValue::Text(value)),
        "{name} is {attribute:?}"
    );
    Ok(())
}

#[then(expr = "album {int} reads the same alone as in the list")]
fn album_reads_alone(world: &mut LibraryWorld, id: i64) -> StepResult {
    let listed = world.album(id)?;
    let alone = world.library()?.album(id)?;
    anyhow::ensure!(alone.as_ref() == Some(listed), "alone {alone:?}");
    Ok(())
}

#[then(expr = "album {int} holds the items {string}")]
fn album_holds_items(world: &mut LibraryWorld, id: i64, expected: String) -> StepResult {
    let found: Vec<i64> = world
        .library()?
        .items_for_album(id)?
        .iter()
        .map(|item| item.id)
        .collect();
    anyhow::ensure!(found == ids(&expected)?, "items {found:?}");
    Ok(())
}
