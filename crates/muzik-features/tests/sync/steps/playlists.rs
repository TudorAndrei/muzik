use crate::support::StepResult;
use crate::world::{SyncWorld, rows};
use cucumber::gherkin::Step;
use cucumber::{given, then};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

fn write(path: &Path, text: &str) -> StepResult {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)?;
    Ok(())
}

#[given(expr = "the library has the playlist {string} with these entries:")]
fn library_playlist(world: &mut SyncWorld, name: String, step: &Step) -> StepResult {
    let mut text = "#EXTM3U\n#MUZIK-SOURCE:source\n".to_owned();
    for entry in rows(step)? {
        writeln!(text, "#EXTINF:1,{entry}\n{entry}")?;
    }
    let path = world.library().join("Playlists").join(name);
    write(&path, &text)?;
    world.playlists.push(path);
    Ok(())
}

#[given(expr = "the device has the muzik playlist {string}")]
fn device_muzik_playlist(world: &mut SyncWorld, name: String) -> StepResult {
    write(
        &world.card().join("Playlists").join(name),
        "#EXTM3U\n#MUZIK-SOURCE:old\n../Old/song.mp3\n",
    )
}

#[given(expr = "the device has the playlist {string}")]
fn device_playlist(world: &mut SyncWorld, name: String) -> StepResult {
    write(
        &world.card().join("Playlists").join(name),
        "#EXTM3U\n../Mine/song.mp3\n",
    )
}

#[then(expr = "the device playlist {string} has these entries:")]
fn device_playlist_entries(world: &mut SyncWorld, name: String, step: &Step) -> StepResult {
    let text = fs::read_to_string(world.card().join("Playlists").join(name))?;
    let entries: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .collect();
    anyhow::ensure!(
        entries == rows(step)?,
        "the device playlist has {entries:?}"
    );
    Ok(())
}

#[then(expr = "the device still has {string}")]
fn device_still_has(world: &mut SyncWorld, name: String) -> StepResult {
    anyhow::ensure!(world.card().join(&name).is_file(), "{name} is gone");
    Ok(())
}
