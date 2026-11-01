use crate::support::{self, StepResult};
use crate::world::ImportWorld;
use cucumber::{given, then, when};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::Path;

fn add_song(world: &mut ImportWorld, name: &str, fixture: &str) -> StepResult {
    let source = world.incoming.join(name);
    fs::copy(support::fixture(fixture), &source)?;
    world.sources.push(source);
    Ok(())
}

fn update_sidecar(source: &Path, key: &str, value: Value) -> StepResult {
    let path = source.with_extension("muzik.json");
    let mut sidecar: Map<String, Value> = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path)?)?
    } else {
        Map::new()
    };
    sidecar.insert(key.to_owned(), value);
    fs::write(path, Value::Object(sidecar).to_string())?;
    Ok(())
}

fn add_known_song(world: &mut ImportWorld, name: &str, resolved: Value) -> StepResult {
    add_song(world, name, "muzik-tags/tests/fixtures/blank.flac")?;
    update_sidecar(world.source()?, "resolved", resolved)
}

#[given(expr = "the incoming song {string} that muzik knows as {string} by {string} on {string}")]
fn known_song(
    world: &mut ImportWorld,
    name: String,
    title: String,
    artist: String,
    album: String,
) -> StepResult {
    add_known_song(
        world,
        &name,
        json!({ "title": title, "artist": artist, "album": album }),
    )
}

#[given(
    expr = "the incoming song {string} that muzik knows as {string} by {string} on {string} from {int}"
)]
fn known_song_from(
    world: &mut ImportWorld,
    name: String,
    title: String,
    artist: String,
    album: String,
    year: i32,
) -> StepResult {
    add_known_song(
        world,
        &name,
        json!({ "title": title, "artist": artist, "album": album, "year": year.to_string() }),
    )
}

#[given(expr = "the song comes from the source {string}")]
fn song_source(world: &mut ImportWorld, source_id: String) -> StepResult {
    update_sidecar(world.source()?, "source_id", Value::String(source_id))
}

#[given("the incoming folder has a cover image")]
fn cover_image(world: &mut ImportWorld) -> StepResult {
    fs::copy(
        support::fixture("muzik-tags/tests/fixtures/cover.png"),
        world.incoming.join("cover.png"),
    )?;
    Ok(())
}

#[given(expr = "the tagged incoming song {string}")]
fn tagged_song(world: &mut ImportWorld, name: String) -> StepResult {
    add_song(world, &name, "muzik-tags/tests/fixtures/mediafile.flac")
}

#[given(expr = "the blank incoming song {string}")]
fn blank_song(world: &mut ImportWorld, name: String) -> StepResult {
    add_song(world, &name, "muzik-tags/tests/fixtures/blank.flac")
}

#[given(
    expr = "an incoming song for each track of the release, which muzik knows on the album {string} from {int}"
)]
fn song_per_track(world: &mut ImportWorld, album: String, year: i32) -> StepResult {
    let tracks = world.catalog.last_release_mut()?.tracks.clone();
    for track in tracks {
        add_known_song(
            world,
            &format!("{:02} {}.flac", track.index, track.title),
            json!({
                "title": track.title,
                "artist": track.artist,
                "album": album,
                "year": year.to_string(),
            }),
        )?;
    }
    Ok(())
}

#[when("the incoming song is deleted")]
fn delete_song(world: &mut ImportWorld) -> StepResult {
    fs::remove_file(world.source()?)?;
    Ok(())
}

#[then("the incoming songs are still there")]
fn songs_still_there(world: &mut ImportWorld) -> StepResult {
    for source in &world.sources {
        anyhow::ensure!(source.exists(), "{} is gone", source.display());
    }
    Ok(())
}

#[then("the incoming song is gone")]
fn song_is_gone(world: &mut ImportWorld) -> StepResult {
    let source = world.source()?;
    anyhow::ensure!(!source.exists(), "{} is still there", source.display());
    Ok(())
}
