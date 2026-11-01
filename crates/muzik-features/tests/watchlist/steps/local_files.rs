use crate::support::{StepResult, fixture};
use crate::world::WatchlistWorld;
use cucumber::{given, then, when};
use muzik_store::watchlist::import_cache;
use serde_json::{Value, json};
use std::fs;

fn cache_name(id: &str) -> String {
    id.strip_prefix("spotify:").map_or_else(
        || id.to_owned(),
        |rest| format!("spotify_{}", rest.rsplit(':').next().unwrap_or(rest)),
    )
}

fn write_cache(world: &WatchlistWorld, id: &str, key: &str, entry: Value) -> StepResult {
    fs::write(
        world
            .cache()
            .join(format!("playlist_{}.json", cache_name(id))),
        serde_json::to_vec(&json!({"videos": {key: entry}}))?,
    )?;
    Ok(())
}

fn import(world: &mut WatchlistWorld) -> anyhow::Result<bool> {
    let repository = world.repository();
    Ok(world.with_options(|options| import_cache(&repository, options))?)
}

fn check(world: &mut WatchlistWorld) -> StepResult {
    let mut document = world.repository().load()?;
    world.with_options(|options| document.reconcile(options))?;
    world.checked = Some(document.to_value()?);
    Ok(())
}

#[given(
    expr = "the cache records the video {string} of {string} as {string} from the file {string}"
)]
fn cache_records_video(
    world: &mut WatchlistWorld,
    video: String,
    id: String,
    status: String,
    file: String,
) -> StepResult {
    let entry = json!({"status": status, "audio_file": world.downloads().join(file)});
    write_cache(world, &id, &video, entry)
}

#[given(
    expr = "the cache records the video {string} of {string} as {string} from the file {string} into the split folder {string}"
)]
fn cache_records_split_video(
    world: &mut WatchlistWorld,
    video: String,
    id: String,
    status: String,
    file: String,
    folder: String,
) -> StepResult {
    let split = world.splits().join(folder);
    fs::create_dir_all(&split)?;
    let entry = json!({
        "status": status,
        "audio_file": world.downloads().join(file),
        "split_dir": split
    });
    write_cache(world, &id, &video, entry)
}

#[given(
    expr = "the cache records the track {string} of {string} as {string} with the file {string}"
)]
fn cache_records_track(
    world: &mut WatchlistWorld,
    track: String,
    id: String,
    status: String,
    file: String,
) -> StepResult {
    let entry = json!({"status": status, "files": [world.downloads().join(file)]});
    write_cache(world, &id, &track, entry)
}

#[given(expr = "the old cache links the video {string} to the file {string}")]
fn old_cache_link(world: &mut WatchlistWorld, video: String, file: String) -> StepResult {
    fs::write(
        world.cache().join(format!("yt_{video}.txt")),
        world.downloads().join(file).to_string_lossy().as_bytes(),
    )?;
    Ok(())
}

#[given(expr = "the downloaded file {string} exists")]
fn downloaded_file_exists(world: &mut WatchlistWorld, file: String) -> StepResult {
    fs::write(world.downloads().join(file), b"audio")?;
    Ok(())
}

#[when(expr = "the downloaded file {string} is deleted")]
fn downloaded_file_is_deleted(world: &mut WatchlistWorld, file: String) -> StepResult {
    fs::remove_file(world.downloads().join(file))?;
    Ok(())
}

#[given(expr = "the cache has the thumbnail {string}")]
fn cache_has_thumbnail(world: &mut WatchlistWorld, name: String) -> StepResult {
    fs::write(world.cache().join(name), b"image")?;
    Ok(())
}

#[given("the music library from the beets fixture")]
fn music_library(world: &mut WatchlistWorld) -> StepResult {
    fs::copy(
        fixture("muzik-library/tests/fixtures/library.db"),
        world.root().join("library.db"),
    )?;
    let config = world.root().join("config.yaml");
    fs::write(&config, "library: library.db\ndirectory: .\n")?;
    world.config = Some(config);
    Ok(())
}

#[given("the organize stage is turned off")]
const fn organize_is_off(world: &mut WatchlistWorld) {
    world.no_organize = true;
}

#[when("the cache is imported and the watchlist is checked")]
fn import_and_check(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(import(world)?, "the cache import did not run");
    check(world)
}

#[when("the cache is imported again")]
fn import_again(world: &mut WatchlistWorld) -> StepResult {
    world.imported = Some(import(world)?);
    Ok(())
}

#[then("the cache import does nothing")]
fn import_does_nothing(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        world.imported == Some(false),
        "the cache import ran: {:?}",
        world.imported
    );
    Ok(())
}

#[when("the watchlist is checked")]
fn watchlist_is_checked(world: &mut WatchlistWorld) -> StepResult {
    check(world)
}

#[when(expr = "the watchlist is checked and saved {int} time(s)")]
fn checked_and_saved(world: &mut WatchlistWorld, times: usize) -> StepResult {
    world.snapshot()?;
    let repository = world.repository();
    for _ in 0..times {
        let mut document = repository.load()?;
        world.with_options(|options| document.reconcile(options))?;
        repository.save(&document)?;
    }
    Ok(())
}
