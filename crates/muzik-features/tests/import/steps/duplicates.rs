use crate::support::StepResult;
use crate::world::ImportWorld;
use cucumber::{given, then, when};
use muzik_import::plan::{AlbumPlan, Duplicate, DuplicateReason};
use muzik_library::{Fields, SqlValue};
use std::fs;
use std::path::Path;

fn path_value(path: &Path) -> SqlValue {
    SqlValue::Blob(path.as_os_str().as_encoded_bytes().to_vec())
}

fn old_album(world: &ImportWorld) -> anyhow::Result<i64> {
    world
        .old_album
        .ok_or_else(|| anyhow::anyhow!("the library has no old album"))
}

fn planned_album(world: &mut ImportWorld) -> anyhow::Result<&mut AlbumPlan> {
    world
        .current_plan_mut()?
        .albums
        .first_mut()
        .ok_or_else(|| anyhow::anyhow!("the plan has no albums"))
}

fn lists_duplicate(world: &ImportWorld, album_id: i64) -> StepResult {
    let duplicates = &world
        .current_plan()?
        .albums
        .first()
        .ok_or_else(|| anyhow::anyhow!("the plan has no albums"))?
        .duplicates;
    anyhow::ensure!(
        duplicates
            .iter()
            .any(|duplicate| duplicate.album_id == album_id),
        "the duplicates are {duplicates:?}"
    );
    Ok(())
}

fn first_album(world: &ImportWorld) -> anyhow::Result<i64> {
    world
        .first_result()?
        .album_ids
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("the first import added no albums"))
}

fn new_album(world: &ImportWorld) -> anyhow::Result<i64> {
    world
        .result()?
        .album_ids
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("the import added no albums"))
}

fn first_destination(world: &ImportWorld) -> anyhow::Result<&Path> {
    world
        .first_result()?
        .destinations
        .first()
        .map(std::path::PathBuf::as_path)
        .ok_or_else(|| anyhow::anyhow!("the first import has no destination"))
}

#[given(expr = "the library has the album {string} by {string} with the MusicBrainz id {string}")]
fn library_album(world: &mut ImportWorld, album: String, artist: String, id: String) -> StepResult {
    let fields = Fields::from([
        ("album".into(), SqlValue::Text(album)),
        ("albumartist".into(), SqlValue::Text(artist)),
        ("mb_albumid".into(), SqlValue::Text(id)),
    ]);
    world.old_album = Some(world.library()?.insert_album(&fields, &Fields::new())?);
    Ok(())
}

#[given("that album has a song whose file is missing")]
fn album_song_missing(world: &mut ImportWorld) -> StepResult {
    let fields = Fields::from([
        ("album_id".into(), SqlValue::Integer(old_album(world)?)),
        (
            "path".into(),
            path_value(&world.root.join("old-missing.flac")),
        ),
    ]);
    world.old_item = Some(world.library()?.insert_item(&fields, &Fields::new())?);
    Ok(())
}

#[given("the missing song points at the planned destination")]
fn missing_song_points_at_destination(world: &mut ImportWorld) -> StepResult {
    let id = world
        .old_item
        .ok_or_else(|| anyhow::anyhow!("the library has no missing song"))?;
    let fields = Fields::from([("path".into(), path_value(world.destination()?))]);
    world.library()?.update_item(id, &fields, &Fields::new())?;
    Ok(())
}

#[given(expr = "a copy of the song in the folder {string}")]
fn copy_of_song(world: &mut ImportWorld, folder: String) -> StepResult {
    let source = world.source()?.to_path_buf();
    let folder = world.dir.path().join(folder);
    fs::create_dir(&folder)?;
    for file in [source.clone(), source.with_extension("muzik.json")] {
        let name = file
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("{} has no file name", file.display()))?;
        fs::copy(&file, folder.join(name))?;
    }
    let name = source
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("the song has no file name"))?;
    world.copy = Some(folder.join(name));
    Ok(())
}

#[when("the plan keeps only that album as a duplicate")]
fn keep_only_that_duplicate(world: &mut ImportWorld) -> StepResult {
    let id = old_album(world)?;
    planned_album(world)?
        .duplicates
        .retain(|duplicate| duplicate.album_id == id);
    Ok(())
}

#[when(expr = "the plan marks album {int} in the library as a duplicate")]
fn mark_duplicate(world: &mut ImportWorld, album_id: i64) -> StepResult {
    planned_album(world)?.duplicates.push(Duplicate {
        album_id,
        reason: DuplicateReason::ArtistAndAlbum,
    });
    Ok(())
}

#[when("the plan lists no duplicates")]
fn clear_duplicates(world: &mut ImportWorld) -> StepResult {
    planned_album(world)?.duplicates.clear();
    Ok(())
}

#[then("the planned album lists that album as a duplicate")]
fn lists_that_album(world: &mut ImportWorld) -> StepResult {
    lists_duplicate(world, old_album(world)?)
}

#[then("the planned album lists the first imported album as a duplicate")]
fn lists_first_album(world: &mut ImportWorld) -> StepResult {
    lists_duplicate(world, first_album(world)?)
}

#[then("that album is still in the library")]
fn that_album_still_there(world: &mut ImportWorld) -> StepResult {
    let id = old_album(world)?;
    anyhow::ensure!(world.library()?.album(id)?.is_some(), "album {id} is gone");
    Ok(())
}

#[then("the first imported album is still in the library")]
fn first_album_still_there(world: &mut ImportWorld) -> StepResult {
    let id = first_album(world)?;
    anyhow::ensure!(world.library()?.album(id)?.is_some(), "album {id} is gone");
    Ok(())
}

#[then("the first imported album is no longer in the library")]
fn first_album_gone(world: &mut ImportWorld) -> StepResult {
    let id = first_album(world)?;
    anyhow::ensure!(
        world.library()?.album(id)?.is_none(),
        "album {id} is still there"
    );
    Ok(())
}

#[then("the new album is in the library")]
fn new_album_in_library(world: &mut ImportWorld) -> StepResult {
    let id = new_album(world)?;
    anyhow::ensure!(
        world.library()?.album(id)?.is_some(),
        "album {id} is missing"
    );
    Ok(())
}

#[then(expr = "the new album has {int} song(s)")]
fn new_album_songs(world: &mut ImportWorld, count: usize) -> StepResult {
    let id = new_album(world)?;
    let songs = world.library()?.items_for_album(id)?.len();
    anyhow::ensure!(songs == count, "the new album has {songs} songs");
    Ok(())
}

#[then("the song of the new album points at the imported file")]
fn new_album_song_path(world: &mut ImportWorld) -> StepResult {
    let id = new_album(world)?;
    let expected = path_value(world.destination()?);
    let items = world.library()?.items_for_album(id)?;
    let path = items.first().and_then(|item| item.field("path"));
    anyhow::ensure!(path == Some(&expected), "the song points at {path:?}");
    Ok(())
}

#[then("the copy is still in its folder")]
fn copy_still_there(world: &mut ImportWorld) -> StepResult {
    let copy = world
        .copy
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("there is no copy of the song"))?;
    anyhow::ensure!(copy.exists(), "{} is gone", copy.display());
    Ok(())
}

#[then("the imported file is where the first import put it")]
fn imported_where_first_put_it(world: &mut ImportWorld) -> StepResult {
    let first = first_destination(world)?.to_path_buf();
    let destinations = &world.result()?.destinations;
    anyhow::ensure!(
        destinations.as_slice() == std::slice::from_ref(&first),
        "the import put the files at {destinations:?}"
    );
    Ok(())
}

#[then("the file from the first import is unchanged")]
fn first_file_unchanged(world: &mut ImportWorld) -> StepResult {
    let before = world
        .imports
        .first()
        .and_then(|import| import.bytes.as_ref())
        .ok_or_else(|| anyhow::anyhow!("the first import wrote no file"))?;
    anyhow::ensure!(
        fs::read(first_destination(world)?)? == *before,
        "the file from the first import changed"
    );
    Ok(())
}
