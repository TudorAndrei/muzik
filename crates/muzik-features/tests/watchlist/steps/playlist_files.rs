use crate::support::StepResult;
use crate::world::WatchlistWorld;
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use muzik_core::PlaylistFormat;
use muzik_library::{Fields, Library, SqlValue, path_to_sql};
use muzik_store::watchlist::{Playlist, SourceKind, WatchItem, playlist_files};
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn table(step: &Step) -> anyhow::Result<Vec<HashMap<String, String>>> {
    let rows = &step
        .table
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the step has no table"))?
        .rows;
    let (header, body) = rows
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("the table is empty"))?;
    Ok(body
        .iter()
        .map(|row| header.iter().cloned().zip(row.iter().cloned()).collect())
        .collect())
}

fn single_column(step: &Step) -> anyhow::Result<Vec<String>> {
    step.table
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the step has no table"))?
        .rows
        .iter()
        .map(|row| {
            row.first()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("a row is empty"))
        })
        .collect()
}

fn music(world: &WatchlistWorld) -> PathBuf {
    world.root().join("music")
}

fn folder(world: &WatchlistWorld) -> PathBuf {
    music(world).join("Playlists")
}

fn write(world: &mut WatchlistWorld, format: Option<PlaylistFormat>) -> StepResult {
    let config = world
        .config
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no music library"))?;
    let watchlist = world.repository().load()?;
    world.playlist_report = Some(playlist_files::write(&watchlist, &config, format)?);
    Ok(())
}

#[given("a music library with these songs:")]
fn library_with_songs(world: &mut WatchlistWorld, step: &Step) -> StepResult {
    let mut library = Library::open_or_create(&world.root().join("library.db"))?;
    for song in table(step)? {
        let mut fields = Fields::new();
        for (name, value) in song {
            let value = if name == "path" {
                path_to_sql(&music(world).join(value))
            } else {
                SqlValue::Text(value)
            };
            fields.insert(name, value);
        }
        library.insert_item(&fields, &Fields::new())?;
    }
    let config = world.root().join("config.yaml");
    fs::write(&config, "library: library.db\ndirectory: music\n")?;
    world.config = Some(config);
    Ok(())
}

#[given("the watchlist has the liked songs with these tracks:")]
fn liked_songs(world: &mut WatchlistWorld, step: &Step) -> StepResult {
    let tracks = table(step)?;
    world.repository().update(|document| {
        let mut playlist = Playlist::new(
            "spotify:liked",
            "https://open.spotify.com/collection/tracks",
            SourceKind::Spotify,
            Some("Liked Songs"),
        );
        for (track, position) in tracks.iter().zip(1_u64..) {
            let artist = track.get("artist").cloned().unwrap_or_default();
            let title = track.get("title").cloned().unwrap_or_default();
            let mut item = WatchItem::new(
                position,
                &format!("{artist} - {title}"),
                SourceKind::Spotify,
            );
            item.video_id = Some(format!("t{position}"));
            item.entry_id = Some(format!("spotify:track:t{position}#0"));
            item.track = Some(json!({"title": title, "artists": [artist]}));
            playlist.items.push(item);
        }
        document.playlists.push(playlist);
        Ok(())
    })?;
    Ok(())
}

#[given(expr = "the playlist folder has the file {string}")]
fn folder_has_file(world: &mut WatchlistWorld, name: String) -> StepResult {
    fs::create_dir_all(folder(world))?;
    fs::write(folder(world).join(name), "#EXTM3U\n../song.flac\n")?;
    Ok(())
}

#[given(expr = "muzik wrote the playlist files as {string}")]
#[when(expr = "muzik writes the playlist files as {string}")]
fn writes_as(world: &mut WatchlistWorld, format: String) -> StepResult {
    write(world, Some(format.parse()?))
}

#[when("muzik writes the playlist files with playlist files off")]
fn writes_off(world: &mut WatchlistWorld) -> StepResult {
    write(world, None)
}

#[when(expr = "the playlist file of {string} is turned off")]
fn turned_off(world: &mut WatchlistWorld, playlist_id: String) -> StepResult {
    anyhow::ensure!(
        world.repository().set_playlist_file(&playlist_id, false)?,
        "{playlist_id} is not in the watchlist"
    );
    Ok(())
}

#[then(expr = "the playlist {string} has these entries:")]
fn playlist_entries(world: &mut WatchlistWorld, name: String, step: &Step) -> StepResult {
    let text = fs::read_to_string(folder(world).join(name))?;
    let entries: Vec<&str> = text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    anyhow::ensure!(
        entries == single_column(step)?,
        "the playlist has {entries:?}"
    );
    Ok(())
}

#[then(expr = "{int} item(s) of {string} is/are not in the library")]
fn missing_items(world: &mut WatchlistWorld, count: usize, playlist_id: String) -> StepResult {
    let missing = world
        .playlist_report
        .as_ref()
        .and_then(|report| {
            report
                .written
                .iter()
                .find(|written| written.playlist_id == playlist_id)
        })
        .map(|written| written.missing);
    anyhow::ensure!(
        missing == Some(count),
        "{playlist_id} has {missing:?} missing items"
    );
    Ok(())
}

#[then(expr = "the playlist folder has only {string}")]
fn folder_has_only(world: &mut WatchlistWorld, name: String) -> StepResult {
    let mut names: Vec<String> = fs::read_dir(folder(world))?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    names.sort();
    anyhow::ensure!(names == [name], "the playlist folder has {names:?}");
    Ok(())
}
