use super::tables::{check_fields, check_tags};
use crate::support::{self, StepResult};
use crate::world::ImportWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use muzik_import::sync::{self, SyncResult};
use muzik_library::{Fields, SqlValue};
use std::fs;
use std::path::PathBuf;

fn add_audio(world: &mut ImportWorld, name: &str) -> anyhow::Result<SqlValue> {
    let audio = world.dir.path().join(name);
    fs::copy(
        support::fixture("muzik-tags/tests/fixtures/blank.flac"),
        &audio,
    )?;
    let path = SqlValue::Blob(audio.as_os_str().as_encoded_bytes().to_vec());
    world.audio = Some(audio);
    Ok(path)
}

fn synced(world: &ImportWorld) -> anyhow::Result<&SyncResult> {
    world
        .synced
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("muzik did not sync"))
}

fn audio(world: &ImportWorld) -> anyhow::Result<&PathBuf> {
    world
        .audio
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the library song has no file"))
}

#[given(expr = "the library has the album {string} with the MusicBrainz id {string}")]
fn library_album(world: &mut ImportWorld, album: String, id: String) -> StepResult {
    let fields = Fields::from([
        ("album".into(), SqlValue::Text(album)),
        ("mb_albumid".into(), SqlValue::Text(id)),
    ]);
    world.old_album = Some(world.library()?.insert_album(&fields, &Fields::new())?);
    Ok(())
}

#[given(
    expr = "that album has the song {string} as track {int} of the recording {string} and the release track {string}"
)]
fn album_song(
    world: &mut ImportWorld,
    title: String,
    track: i64,
    recording: String,
    release_track: String,
) -> StepResult {
    let album = world
        .old_album
        .ok_or_else(|| anyhow::anyhow!("the library has no album"))?;
    let path = add_audio(world, "song.flac")?;
    let fields = Fields::from([
        ("album_id".into(), SqlValue::Integer(album)),
        ("title".into(), SqlValue::Text(title)),
        ("track".into(), SqlValue::Integer(track)),
        ("disc".into(), SqlValue::Integer(1)),
        ("mb_trackid".into(), SqlValue::Text(recording)),
        ("mb_releasetrackid".into(), SqlValue::Text(release_track)),
        ("path".into(), path),
    ]);
    world.song = Some(world.library()?.insert_item(&fields, &Fields::new())?);
    Ok(())
}

#[given(expr = "the library has the singleton {string} of the recording {string}")]
fn library_singleton(world: &mut ImportWorld, title: String, recording: String) -> StepResult {
    let path = add_audio(world, "single.flac")?;
    let fields = Fields::from([
        ("title".into(), SqlValue::Text(title)),
        ("mb_trackid".into(), SqlValue::Text(recording)),
        ("path".into(), path),
    ]);
    world.song = Some(world.library()?.insert_item(&fields, &Fields::new())?);
    Ok(())
}

#[when(expr = "muzik syncs {string} and writes tags")]
fn sync_query(world: &mut ImportWorld, query: String) -> StepResult {
    world.open_library()?;
    let Some(library) = world.library.as_mut() else {
        anyhow::bail!("the library is not open");
    };
    world.synced = Some(sync::sync(library, &world.catalog, &query, true)?);
    Ok(())
}

#[then(expr = "the sync updated {int} album(s) and {int} song(s)")]
fn sync_updated_albums(world: &mut ImportWorld, albums: usize, songs: usize) -> StepResult {
    let result = synced(world)?;
    anyhow::ensure!(
        result.albums_updated == albums && result.items_updated == songs,
        "the sync result is {result:?}"
    );
    Ok(())
}

#[then(expr = "the sync updated {int} singleton(s) and {int} song(s)")]
fn sync_updated_singletons(world: &mut ImportWorld, singletons: usize, songs: usize) -> StepResult {
    let result = synced(world)?;
    anyhow::ensure!(
        result.singletons_updated == singletons && result.items_updated == songs,
        "the sync result is {result:?}"
    );
    Ok(())
}

#[then("the synced song has these fields:")]
fn synced_song_fields(world: &mut ImportWorld, step: &Step) -> StepResult {
    let id = world
        .song
        .ok_or_else(|| anyhow::anyhow!("the library has no synced song"))?;
    let item = world
        .library()?
        .item(id)?
        .ok_or_else(|| anyhow::anyhow!("song {id} is not in the library"))?;
    check_fields("the synced song", &item.fields, step)
}

#[then("the synced file has these tags:")]
fn synced_file_tags(world: &mut ImportWorld, step: &Step) -> StepResult {
    check_tags(audio(world)?, step)
}
