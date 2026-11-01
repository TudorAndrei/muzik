use crate::support::{StepResult, fixture};
use crate::world::{SyncWorld, rows};
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use muzik_library::{Fields, Library, SqlValue};
use std::fs;
use std::path::{Path, PathBuf};

fn bytes(path: &Path) -> SqlValue {
    SqlValue::Blob(path.as_os_str().as_encoded_bytes().to_vec())
}

fn database(world: &SyncWorld) -> PathBuf {
    world.root().join("library.db")
}

#[given(expr = "a library with the album {string} that has the track {string} and a cover file")]
fn library_with_album(world: &mut SyncWorld, album: String, track: String) -> StepResult {
    fs::copy(
        fixture("muzik-library/tests/fixtures/library.db"),
        database(world),
    )?;
    fs::write(&world.cover, b"art")?;
    let mut library = Library::open_read_write(&database(world))?;
    let mut album_fields = Fields::new();
    album_fields.insert("album".into(), SqlValue::Text(album.clone()));
    album_fields.insert("artpath".into(), bytes(&world.cover));
    let album_id = library.insert_album(&album_fields, &Fields::new())?;
    let mut item_fields = Fields::new();
    item_fields.insert("album_id".into(), SqlValue::Integer(album_id));
    item_fields.insert("album".into(), SqlValue::Text(album));
    item_fields.insert("title".into(), SqlValue::Text("Song".into()));
    item_fields.insert("path".into(), bytes(Path::new(&track)));
    library.insert_item(&item_fields, &Fields::new())?;
    Ok(())
}

#[when(regex = r#"^muzik selects "([^"]*)" (with|without) covers$"#)]
fn selects(world: &mut SyncWorld, query: String, covers: String) -> StepResult {
    let library = Library::open_read_only(&database(world))?;
    let selection = muzik_sync::select(&library, &world.library(), &query, covers == "with")?;
    world.selected_tracks = selection.tracks;
    world.selected_covers = selection.covers;
    Ok(())
}

#[then("the selected tracks are:")]
fn selected_tracks(world: &mut SyncWorld, step: &Step) -> StepResult {
    let library = world.library();
    let expected: Vec<PathBuf> = rows(step)?.iter().map(|name| library.join(name)).collect();
    anyhow::ensure!(
        world.selected_tracks == expected,
        "the selected tracks are {:?}",
        world.selected_tracks
    );
    Ok(())
}

#[then("the album cover is selected")]
fn album_cover_selected(world: &mut SyncWorld) -> StepResult {
    anyhow::ensure!(
        world.selected_covers == std::slice::from_ref(&world.cover),
        "the selected covers are {:?}",
        world.selected_covers
    );
    Ok(())
}

#[then("no covers are selected")]
fn no_covers_selected(world: &mut SyncWorld) -> StepResult {
    anyhow::ensure!(
        world.selected_covers.is_empty(),
        "the selected covers are {:?}",
        world.selected_covers
    );
    Ok(())
}
