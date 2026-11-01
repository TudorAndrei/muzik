use crate::support::StepResult;
use crate::world::WatchlistWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use serde_json::{Value, json};
use std::fs;

fn write_old_file(world: &mut WatchlistWorld, content: Vec<u8>) -> StepResult {
    fs::write(world.legacy_file(), &content)?;
    world.legacy = true;
    world.legacy_content = Some(content);
    Ok(())
}

fn old_content(world: &WatchlistWorld) -> anyhow::Result<&[u8]> {
    world
        .legacy_content
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("there is no old watchlist file"))
}

#[given("an old watchlist file from version 1")]
fn old_file_from_version_1(world: &mut WatchlistWorld) -> StepResult {
    let old = json!({
        "version": 1,
        "playlists": [{
            "playlist_id": "PL1",
            "url": "https://www.youtube.com/playlist?list=PL1",
            "items": [{
                "position": 1,
                "title": "Song",
                "video_id": "abcdefghijk",
                "stages": {
                    "download": {"status": "complete", "path": "/music/song.flac"},
                    "parse": {"status": "complete"},
                    "split": {"status": "skipped"},
                    "organize": {"status": "complete"}
                }
            }],
            "processed_video_ids": ["abcdefghijk", "abcdefghijk"]
        }]
    });
    write_old_file(world, serde_json::to_vec(&old)?)
}

#[given("an old watchlist file with the content:")]
fn old_file_with_content(world: &mut WatchlistWorld, step: &Step) -> StepResult {
    let content = step
        .docstring
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("the step has no content"))?;
    write_old_file(world, content.trim().as_bytes().to_vec())
}

#[when("the watchlist is loaded")]
fn watchlist_is_loaded(world: &mut WatchlistWorld) -> StepResult {
    world.loaded = Some(world.repository().load()?);
    Ok(())
}

#[when("the loaded watchlist is saved again")]
fn loaded_is_saved_again(world: &mut WatchlistWorld) -> StepResult {
    let loaded = world
        .loaded
        .clone()
        .ok_or_else(|| anyhow::anyhow!("the watchlist was not loaded"))?;
    let repository = world.repository();
    world.before = Some((repository.revision()?, loaded.clone()));
    repository.save(&loaded)?;
    Ok(())
}

#[then("the old watchlist file is replaced by its backup")]
fn old_file_is_replaced(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        !world.legacy_file().exists(),
        "the old watchlist file is still there"
    );
    let backup = world.root().join("watchlist.json.migrated");
    let expected: Value = serde_json::from_slice(old_content(world)?)?;
    let actual: Value = serde_json::from_slice(&fs::read(backup)?)?;
    anyhow::ensure!(actual == expected, "the backup holds {actual}");
    Ok(())
}

#[then("the old watchlist file still has its content")]
fn old_file_unchanged(world: &mut WatchlistWorld) -> StepResult {
    let actual = fs::read(world.legacy_file())?;
    anyhow::ensure!(
        actual == old_content(world)?,
        "the old watchlist file changed"
    );
    Ok(())
}
