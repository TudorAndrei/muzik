use super::tables::{check_fields, check_tags};
use crate::support::StepResult;
use crate::world::{Import, ImportWorld};
use cucumber::gherkin::Step;
use cucumber::{then, when};
use muzik_import::apply::{
    self, AlbumDecision, ApplyError, ApplyOptions, DuplicateDecision, MatchDecision,
};
use std::fs;
use std::path::Path;

#[derive(Clone, Copy)]
enum Cancel {
    Never,
    BeforeStart,
    AfterFirstFile,
}

fn decision(choice: &str, duplicate: Option<&str>) -> anyhow::Result<AlbumDecision> {
    let choice = match choice {
        "as it is" => MatchDecision::AsIs,
        "with the best match" => MatchDecision::Candidate(0),
        other => anyhow::bail!("unknown choice {other:?}"),
    };
    let duplicate = match duplicate {
        None => None,
        Some("keeps") => Some(DuplicateDecision::Keep),
        Some("skips") => Some(DuplicateDecision::Skip),
        Some("replaces") => Some(DuplicateDecision::Replace),
        Some(other) => anyhow::bail!("unknown duplicate decision {other:?}"),
    };
    Ok(AlbumDecision { choice, duplicate })
}

fn files_under(path: &Path) -> anyhow::Result<usize> {
    fs::read_dir(path)?.try_fold(0_usize, |count, entry| {
        let entry = entry?;
        let files = if entry.file_type()?.is_dir() {
            files_under(&entry.path())?
        } else {
            1
        };
        count
            .checked_add(files)
            .ok_or_else(|| anyhow::anyhow!("too many files"))
    })
}

fn import(world: &mut ImportWorld, decisions: &[AlbumDecision], cancel: Cancel) -> StepResult {
    let mut options = ApplyOptions::from_beets(&world.config, world.root.clone())?;
    if let Some(placement) = world.placement {
        options.placement = placement;
    }
    options.dry_run = world.dry_run;
    if let Some(path) = &world.singleton_path {
        options.paths.singleton.clone_from(path);
    }
    if let Some(path) = &world.default_path {
        options.paths.default.clone_from(path);
    }
    world.open_library()?;
    let Some(library) = world.library.as_mut() else {
        anyhow::bail!("the library is not open");
    };
    let plan = match &world.plan {
        Some(Ok(plan)) => plan,
        Some(Err(error)) => anyhow::bail!("planning failed: {error}"),
        None => anyhow::bail!("muzik did not plan the import"),
    };
    let items_before = library.items()?.len();
    let albums_before = library.albums()?.len();
    let root = &world.root;
    let outcome = match cancel {
        Cancel::Never => apply::apply(library, plan, decisions, &options),
        Cancel::BeforeStart => {
            apply::apply_with_cancel(library, plan, decisions, &options, &|| true)
        }
        Cancel::AfterFirstFile => {
            apply::apply_with_cancel(library, plan, decisions, &options, &|| {
                fs::read_dir(root).is_ok_and(|mut entries| entries.next().is_some())
            })
        }
    };
    let bytes = outcome
        .as_ref()
        .ok()
        .filter(|_| !world.dry_run)
        .and_then(|result| result.destinations.first())
        .and_then(|destination| fs::read(destination).ok());
    world.imports.push(Import {
        outcome,
        items_before,
        albums_before,
        bytes,
    });
    Ok(())
}

#[when(regex = r"^muzik imports the album (as it is|with the best match)$")]
fn import_album(world: &mut ImportWorld, choice: String) -> StepResult {
    import(world, &[decision(&choice, None)?], Cancel::Never)
}

#[when(
    regex = r"^muzik imports the album (as it is|with the best match) and (keeps|skips|replaces) duplicates$"
)]
fn import_album_with_duplicates(
    world: &mut ImportWorld,
    choice: String,
    duplicate: String,
) -> StepResult {
    import(
        world,
        &[decision(&choice, Some(&duplicate))?],
        Cancel::Never,
    )
}

#[when("muzik skips the album")]
fn skip_album(world: &mut ImportWorld) -> StepResult {
    let skip = AlbumDecision {
        choice: MatchDecision::Skip,
        duplicate: None,
    };
    import(world, &[skip], Cancel::Never)
}

#[when("muzik runs the plan")]
fn run_plan(world: &mut ImportWorld) -> StepResult {
    import(world, &[], Cancel::Never)
}

#[when("muzik imports the album as it is, but the import is cancelled before it starts")]
fn import_cancelled_before_start(world: &mut ImportWorld) -> StepResult {
    import(world, &[decision("as it is", None)?], Cancel::BeforeStart)
}

#[when(
    "muzik imports the album as it is and keeps duplicates, but the import is cancelled after the first file is placed"
)]
fn import_cancelled_after_first_file(world: &mut ImportWorld) -> StepResult {
    import(
        world,
        &[decision("as it is", Some("keeps"))?],
        Cancel::AfterFirstFile,
    )
}

#[then("the import is cancelled")]
fn import_is_cancelled(world: &mut ImportWorld) -> StepResult {
    let outcome = &world.last_import()?.outcome;
    anyhow::ensure!(
        matches!(outcome, Err(ApplyError::Cancelled)),
        "the import ended with {outcome:?}"
    );
    Ok(())
}

#[then("the import fails")]
fn import_fails(world: &mut ImportWorld) -> StepResult {
    let outcome = &world.last_import()?.outcome;
    anyhow::ensure!(outcome.is_err(), "the import ended with {outcome:?}");
    Ok(())
}

#[then("the import asks for a duplicate decision")]
fn import_asks_for_duplicate_decision(world: &mut ImportWorld) -> StepResult {
    let outcome = &world.last_import()?.outcome;
    anyhow::ensure!(
        matches!(outcome, Err(ApplyError::DuplicateDecision)),
        "the import ended with {outcome:?}"
    );
    Ok(())
}

#[then("the import fails because the album is already in the library")]
fn import_already_in_library(world: &mut ImportWorld) -> StepResult {
    let outcome = &world.last_import()?.outcome;
    anyhow::ensure!(
        matches!(outcome, Err(ApplyError::AlreadyInLibrary(_))),
        "the import ended with {outcome:?}"
    );
    Ok(())
}

#[then(expr = "the import added {int} song(s)")]
fn import_added_songs(world: &mut ImportWorld, count: usize) -> StepResult {
    let songs = world.result()?.item_ids.len();
    anyhow::ensure!(songs == count, "the import added {songs} songs");
    Ok(())
}

#[then(expr = "the import added {int} album(s)")]
fn import_added_albums(world: &mut ImportWorld, count: usize) -> StepResult {
    let albums = world.result()?.album_ids.len();
    anyhow::ensure!(albums == count, "the import added {albums} albums");
    Ok(())
}

#[then("the import added no albums")]
fn import_added_no_albums(world: &mut ImportWorld) -> StepResult {
    import_added_albums(world, 0)
}

#[then(expr = "the import has {int} destination(s)")]
fn import_destinations(world: &mut ImportWorld, count: usize) -> StepResult {
    let destinations = &world.result()?.destinations;
    anyhow::ensure!(
        destinations.len() == count,
        "the import has the destinations {destinations:?}"
    );
    Ok(())
}

#[then("the import has no destinations")]
fn import_no_destinations(world: &mut ImportWorld) -> StepResult {
    import_destinations(world, 0)
}

#[then(expr = "the import skipped {int} album(s)")]
fn import_skipped_albums(world: &mut ImportWorld, count: usize) -> StepResult {
    let skipped = world.result()?.skipped_albums;
    anyhow::ensure!(skipped == count, "the import skipped {skipped} albums");
    Ok(())
}

#[then(expr = "the import skipped {int} folder(s) imported before")]
fn import_skipped_folders(world: &mut ImportWorld, count: usize) -> StepResult {
    let skipped = world.result()?.skipped_incremental;
    anyhow::ensure!(skipped == count, "the import skipped {skipped} folders");
    Ok(())
}

#[then(expr = "the import found {int} album(s) already in the library")]
fn import_found_in_library(world: &mut ImportWorld, count: usize) -> StepResult {
    let found = world.result()?.already_in_library;
    anyhow::ensure!(found == count, "the import found {found} albums");
    Ok(())
}

#[then("no incoming song failed to clean up")]
fn no_source_cleanup_failed(world: &mut ImportWorld) -> StepResult {
    let failed = &world.result()?.source_cleanup_failed;
    anyhow::ensure!(failed.is_empty(), "these songs stayed: {failed:?}");
    Ok(())
}

#[then("no old file failed to clean up")]
fn no_cleanup_failed(world: &mut ImportWorld) -> StepResult {
    let failed = &world.result()?.cleanup_failed;
    anyhow::ensure!(failed.is_empty(), "these old files stayed: {failed:?}");
    Ok(())
}

#[then("the library has the same songs and albums as before")]
fn library_same_songs_and_albums(world: &mut ImportWorld) -> StepResult {
    let items_before = world.last_import()?.items_before;
    let items = world.library()?.items()?.len();
    anyhow::ensure!(
        items == items_before,
        "the library has {items} songs, not {items_before}"
    );
    library_same_albums(world)
}

#[then("the library has the same albums as before")]
fn library_same_albums(world: &mut ImportWorld) -> StepResult {
    let albums_before = world.last_import()?.albums_before;
    let albums = world.library()?.albums()?.len();
    anyhow::ensure!(
        albums == albums_before,
        "the library has {albums} albums, not {albums_before}"
    );
    Ok(())
}

#[then("the library folder has no files")]
fn library_folder_empty(world: &mut ImportWorld) -> StepResult {
    let files = files_under(&world.root)?;
    anyhow::ensure!(files == 0, "the library folder has {files} files");
    Ok(())
}

#[then(expr = "the imported file is named {string}")]
fn imported_file_named(world: &mut ImportWorld, name: String) -> StepResult {
    let destination = world.destination()?;
    anyhow::ensure!(
        destination.file_name() == Some(name.as_ref()),
        "the imported file is {}",
        destination.display()
    );
    Ok(())
}

#[then(expr = "the imported file is at {string} in the library folder")]
fn imported_file_at(world: &mut ImportWorld, path: String) -> StepResult {
    let destination = world.destination()?;
    anyhow::ensure!(
        destination == world.root.join(path),
        "the imported file is {}",
        destination.display()
    );
    Ok(())
}

#[then(expr = "every imported file is in {string} in the library folder")]
fn every_imported_file_in(world: &mut ImportWorld, folder: String) -> StepResult {
    let folder = world.root.join(folder);
    let destinations = &world.result()?.destinations;
    anyhow::ensure!(
        destinations.iter().all(|path| path.starts_with(&folder)),
        "the imported files are {destinations:?}"
    );
    Ok(())
}

#[then("the imported file exists")]
fn imported_file_exists(world: &mut ImportWorld) -> StepResult {
    let destination = world.destination()?;
    anyhow::ensure!(destination.exists(), "{} is missing", destination.display());
    Ok(())
}

#[then("the imported file does not exist")]
fn imported_file_does_not_exist(world: &mut ImportWorld) -> StepResult {
    let destination = world.destination()?;
    anyhow::ensure!(!destination.exists(), "{} exists", destination.display());
    Ok(())
}

#[then("the imported file has these tags:")]
fn imported_file_tags(world: &mut ImportWorld, step: &Step) -> StepResult {
    check_tags(world.destination()?, step)
}

#[then("the imported file has a front cover")]
fn imported_file_cover(world: &mut ImportWorld) -> StepResult {
    anyhow::ensure!(
        muzik_tags::has_front_cover(world.destination()?)?,
        "the imported file has no front cover"
    );
    Ok(())
}

#[then(expr = "the imported song has the source {string}")]
fn imported_song_source(world: &mut ImportWorld, source_id: String) -> StepResult {
    let item = world.imported_item()?;
    let source = item.attribute("muzik_source_id");
    anyhow::ensure!(
        source == Some(&muzik_library::SqlValue::Text(source_id)),
        "the imported song has the source {source:?}"
    );
    Ok(())
}

#[then("the imported song has these fields:")]
fn imported_song_fields(world: &mut ImportWorld, step: &Step) -> StepResult {
    let item = world.imported_item()?;
    check_fields("the imported song", &item.fields, step)
}

#[then("the imported song belongs to no album")]
fn imported_song_no_album(world: &mut ImportWorld) -> StepResult {
    let album = world.imported_item()?.album_id();
    anyhow::ensure!(
        album.is_none(),
        "the imported song belongs to album {album:?}"
    );
    Ok(())
}

#[then("the imported song is in the library")]
fn imported_song_in_library(world: &mut ImportWorld) -> StepResult {
    world.imported_item()?;
    Ok(())
}

#[then("the imported album has these fields:")]
fn imported_album_fields(world: &mut ImportWorld, step: &Step) -> StepResult {
    let id = *world
        .result()?
        .album_ids
        .first()
        .ok_or_else(|| anyhow::anyhow!("the import added no albums"))?;
    let album = world
        .library()?
        .album(id)?
        .ok_or_else(|| anyhow::anyhow!("album {id} is not in the library"))?;
    check_fields("the imported album", &album.fields, step)
}
