use crate::support::StepResult;
use crate::world::{SyncWorld, rows};
use cucumber::gherkin::Step;
use cucumber::given;
use std::fs;

#[given("the library has these tracks:")]
fn library_has_tracks(world: &mut SyncWorld, step: &Step) -> StepResult {
    for name in rows(step)? {
        let path = world.library().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, b"audio")?;
        world.tracks.push(path);
    }
    Ok(())
}

#[given(expr = "a track {string} outside the library")]
fn track_outside_library(world: &mut SyncWorld, name: String) {
    let path = world.root().join(name);
    world.tracks.push(path);
}

#[given(expr = "the library has the cover {string}")]
fn library_has_cover(world: &mut SyncWorld, name: String) -> StepResult {
    let path = world.library().join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, b"audio")?;
    world.covers.push(path);
    Ok(())
}

#[given(expr = "muzik cannot read the track {string}")]
fn cannot_read_track(world: &mut SyncWorld, name: String) {
    world.unreadable.push(name);
}
