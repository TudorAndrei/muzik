use crate::support::StepResult;
use crate::world::WatchlistWorld;
use cucumber::{given, then, when};
use muzik_store::watchlist::{CheckedWrite, SourceKind, Stage, WatchItem, Watchlist};
use std::sync::Arc;

fn read(world: &mut WatchlistWorld) -> StepResult {
    let repository = world.repository();
    let (document, revision) = repository.load_revision()?;
    let current = repository.revision()?;
    anyhow::ensure!(
        revision == current,
        "the check read revision {revision} but the watchlist is at {current}"
    );
    world.before = Some((revision, document.clone()));
    world.read = Some((document, revision));
    Ok(())
}

fn what_was_read(world: &WatchlistWorld) -> anyhow::Result<(Watchlist, i64)> {
    world
        .read
        .clone()
        .ok_or_else(|| anyhow::anyhow!("the check did not read the watchlist"))
}

fn save_at(world: &mut WatchlistWorld, revision: i64, document: &Watchlist) {
    world.write = Some(world.repository().save_at(revision, document));
}

fn read_revision(world: &WatchlistWorld) -> anyhow::Result<i64> {
    Ok(what_was_read(world)?.1)
}

#[given("a check has read the watchlist")]
fn check_has_read(world: &mut WatchlistWorld) -> StepResult {
    read(world)
}

#[when("the check reads the watchlist again")]
fn check_reads_again(world: &mut WatchlistWorld) -> StepResult {
    read(world)
}

#[when(expr = "another editor renames the source {string} to {string}")]
fn another_editor_renames(world: &mut WatchlistWorld, id: String, title: String) -> StepResult {
    world.repository().rename(&id, &title)?;
    Ok(())
}

#[when(expr = "the check saves the title {string} for item {int} of {string}")]
fn check_saves_title(
    world: &mut WatchlistWorld,
    title: String,
    ordinal: usize,
    id: String,
) -> StepResult {
    let (mut document, revision) = what_was_read(world)?;
    let item = document
        .playlist_mut(&id)
        .and_then(|playlist| playlist.items.get_mut(ordinal.checked_sub(1)?))
        .ok_or_else(|| anyhow::anyhow!("the source {id} has no item {ordinal}"))?;
    item.title = title;
    save_at(world, revision, &document);
    Ok(())
}

#[when(expr = "a runner completes the download of item {int} of {string}")]
fn runner_completes_download(world: &mut WatchlistWorld, ordinal: usize, id: String) -> StepResult {
    world.repository().update(|document| {
        document
            .playlist_mut(&id)
            .and_then(|playlist| playlist.items.get_mut(ordinal.checked_sub(1)?))
            .ok_or("the item is not watched")?
            .complete(Stage::Download, None);
        Ok(())
    })?;
    Ok(())
}

#[when("the check saves what it read")]
fn check_saves_what_it_read(world: &mut WatchlistWorld) -> StepResult {
    let (document, revision) = what_was_read(world)?;
    save_at(world, revision, &document);
    Ok(())
}

#[given("the database refuses new watchlist items")]
fn database_refuses_items(world: &mut WatchlistWorld) -> StepResult {
    rusqlite::Connection::open(world.database())?.execute_batch(
        "CREATE TRIGGER refuse BEFORE INSERT ON watchlist_items
         BEGIN SELECT RAISE(ABORT, 'refused'); END;",
    )?;
    Ok(())
}

#[when(expr = "the check renames the source {string} to {string} and adds the item {string}")]
fn check_renames_and_adds(
    world: &mut WatchlistWorld,
    id: String,
    title: String,
    item: String,
) -> StepResult {
    let (mut document, revision) = what_was_read(world)?;
    let playlist = document
        .playlist_mut(&id)
        .ok_or_else(|| anyhow::anyhow!("the source {id} is not watched"))?;
    playlist.title = Some(title);
    playlist
        .items
        .push(WatchItem::new(2, &item, SourceKind::Youtube));
    save_at(world, revision, &document);
    Ok(())
}

#[then("the checked write is refused as a conflict")]
fn write_is_conflict(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.write, Some(Ok(CheckedWrite::Conflict))),
        "the checked write ended with {:?}",
        world.write
    );
    Ok(())
}

#[then("the checked write is saved")]
fn write_is_saved(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.write, Some(Ok(CheckedWrite::Written))),
        "the checked write ended with {:?}",
        world.write
    );
    Ok(())
}

#[then("the checked write fails")]
fn write_fails(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.write, Some(Err(_))),
        "the checked write ended with {:?}",
        world.write
    );
    Ok(())
}

#[then(expr = "the saved revision is {int} more than the revision that the check read")]
fn revision_is_more(world: &mut WatchlistWorld, more: i64) -> StepResult {
    let expected = read_revision(world)?
        .checked_add(more)
        .ok_or_else(|| anyhow::anyhow!("the revision is too large"))?;
    let actual = world.repository().revision()?;
    anyhow::ensure!(actual == expected, "the saved revision is {actual}");
    Ok(())
}

#[then("the saved revision is the revision that the check read")]
fn revision_is_read(world: &mut WatchlistWorld) -> StepResult {
    let expected = read_revision(world)?;
    let actual = world.repository().revision()?;
    anyhow::ensure!(actual == expected, "the saved revision is {actual}");
    Ok(())
}

#[when(
    expr = "{int} workers each mark a different video of {string} as processed at the same time"
)]
fn parallel_workers(world: &mut WatchlistWorld, count: usize, id: String) -> StepResult {
    let repository = Arc::new(world.repository());
    let workers: Vec<_> = (0..count)
        .map(|index| {
            let repository = Arc::clone(&repository);
            let id = id.clone();
            std::thread::spawn(move || {
                repository.update(|document| {
                    document
                        .playlist_mut(&id)
                        .ok_or("the source is not watched")?
                        .mark_processed(&format!("video_{index}"), true);
                    Ok(())
                })
            })
        })
        .collect();
    for worker in workers {
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("a worker stopped"))??;
    }
    Ok(())
}
