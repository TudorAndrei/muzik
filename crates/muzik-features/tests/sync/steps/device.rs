use crate::support::StepResult;
use crate::world::{SyncWorld, rows};
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use std::fs;
use std::path::{Path, PathBuf};

fn write(path: &Path, contents: &[u8]) -> StepResult {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

#[given(expr = "the device has the old file {string} of {int} bytes")]
fn device_has_old_file(world: &mut SyncWorld, name: String, bytes: usize) -> StepResult {
    write(&world.card().join(name), &vec![b'x'; bytes])
}

#[given("the device has these files:")]
fn device_has_files(world: &mut SyncWorld, step: &Step) -> StepResult {
    for name in rows(step)? {
        write(&world.card().join(name), b"x")?;
    }
    Ok(())
}

#[given(expr = "the sync plan has {string}")]
fn sync_plan_has(world: &mut SyncWorld, name: String) {
    let path = world.card().join(name);
    world.planned.insert(path);
}

#[when("muzik looks for old files on the device")]
fn looks_for_old_files(world: &mut SyncWorld) -> StepResult {
    world.stale = Some(muzik_sync::stale_files(&world.card(), &world.planned)?);
    Ok(())
}

#[when(expr = "someone removes {string} from the device")]
fn someone_removes(world: &mut SyncWorld, name: String) -> StepResult {
    fs::remove_file(world.card().join(name))?;
    Ok(())
}

#[when("the device is unplugged")]
fn device_is_unplugged(world: &mut SyncWorld) -> StepResult {
    fs::rename(world.card(), world.unplugged())?;
    Ok(())
}

#[then("the old files are:")]
fn old_files_are(world: &mut SyncWorld, step: &Step) -> StepResult {
    let card = world.card();
    let expected: Vec<PathBuf> = rows(step)?.iter().map(|name| card.join(name)).collect();
    anyhow::ensure!(
        world.stale.as_ref() == Some(&expected),
        "the old files are {:?}",
        world.stale
    );
    Ok(())
}

#[then(expr = "the device does not have {string}")]
fn device_does_not_have(world: &mut SyncWorld, name: String) -> StepResult {
    let path = world.card().join(name);
    anyhow::ensure!(!path.exists(), "{} exists", path.display());
    Ok(())
}

#[then(expr = "the unplugged device still has {string}")]
fn unplugged_device_has(world: &mut SyncWorld, name: String) -> StepResult {
    let path = world.unplugged().join(name);
    anyhow::ensure!(path.is_file(), "{} is missing", path.display());
    Ok(())
}

#[then("the device folder does not exist")]
fn device_folder_missing(world: &mut SyncWorld) -> StepResult {
    anyhow::ensure!(!world.card().exists(), "the device folder exists");
    Ok(())
}
