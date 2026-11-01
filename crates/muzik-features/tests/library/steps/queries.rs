use crate::support::StepResult;
use crate::world::LibraryWorld;
use cucumber::{given, when};
use muzik_library::query::Query;
use muzik_library::{Album, Fields, Item, SqlValue};

#[when(regex = r"^I search the items for (.+)$")]
fn search_items(world: &mut LibraryWorld, query: String) -> StepResult {
    world.items = world.library()?.query_items(&query)?;
    Ok(())
}

#[when(regex = r"^I search the albums for (.+)$")]
fn search_albums(world: &mut LibraryWorld, query: String) -> StepResult {
    world.albums = world.library()?.query_albums(&query)?;
    Ok(())
}

#[given(expr = "item {int} by {string} with the artist sort name {string}")]
fn item_with_sort_name(world: &mut LibraryWorld, id: i64, artist: String, sort: String) {
    world.items.push(Item {
        id,
        fields: Fields::from([
            ("artist".into(), SqlValue::Text(artist)),
            ("artist_sort".into(), SqlValue::Text(sort)),
        ]),
        attributes: Fields::new(),
    });
}

#[given(expr = "album {int} by {string} with the album artist sort name {string}")]
fn album_with_sort_name(world: &mut LibraryWorld, id: i64, artist: String, sort: String) {
    world.albums.push(Album {
        id,
        fields: Fields::from([
            ("albumartist".into(), SqlValue::Text(artist)),
            ("albumartist_sort".into(), SqlValue::Text(sort)),
        ]),
        attributes: Fields::new(),
    });
}

#[when("I sort the items with an empty query")]
fn sort_items(world: &mut LibraryWorld) -> StepResult {
    Query::parse("")?.sort_items(&mut world.items);
    Ok(())
}

#[when("I sort the albums with an empty query")]
fn sort_albums(world: &mut LibraryWorld) -> StepResult {
    Query::parse("")?.sort_albums(&mut world.albums);
    Ok(())
}
