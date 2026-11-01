use crate::support::StepResult;
use crate::world::SyncWorld;
use cucumber::{given, then, when};
use muzik_sync::{Action, Transfer};
use std::fs;
use std::path::PathBuf;

fn source(world: &SyncWorld) -> PathBuf {
    world.root().join("source.mp3")
}

fn copy(world: &mut SyncWorld, length: usize) -> StepResult {
    let source = source(world);
    let transfer = Transfer {
        destination: world.card().join(format!("{}.mp3", "x".repeat(length))),
        action: Action::Copy,
        bytes: fs::metadata(&source)?.len(),
        cover: false,
        tags_in_stream: false,
        source,
    };
    world.transfer_result = Some(muzik_sync::transfer(&transfer));
    world.transfer = Some(transfer);
    Ok(())
}

fn destination(world: &SyncWorld) -> anyhow::Result<PathBuf> {
    world
        .transfer
        .as_ref()
        .map(|transfer| transfer.destination.clone())
        .ok_or_else(|| anyhow::anyhow!("muzik did not copy the source"))
}

#[given(expr = "a source file with the content {string}")]
fn source_file(world: &mut SyncWorld, content: String) -> StepResult {
    fs::write(source(world), content)?;
    Ok(())
}

#[given(expr = "muzik copied the source to a device file with a {int} character name")]
fn copied_source(world: &mut SyncWorld, length: usize) -> StepResult {
    copy(world, length)?;
    if let Some(Err(error)) = world.transfer_result.take() {
        anyhow::bail!("the copy failed: {error}");
    }
    Ok(())
}

#[given("the source file is removed")]
fn source_removed(world: &mut SyncWorld) -> StepResult {
    fs::remove_file(source(world))?;
    Ok(())
}

#[when(expr = "muzik copies the source to a device file with a {int} character name")]
fn copies_source(world: &mut SyncWorld, length: usize) -> StepResult {
    copy(world, length)
}

#[when("muzik copies the source again")]
fn copies_source_again(world: &mut SyncWorld) -> StepResult {
    let transfer = world
        .transfer
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("muzik did not copy the source"))?;
    world.transfer_result = Some(muzik_sync::transfer(transfer));
    Ok(())
}

#[then(expr = "the device file has the content {string}")]
fn device_file_content(world: &mut SyncWorld, content: String) -> StepResult {
    let found = fs::read(destination(world)?)?;
    anyhow::ensure!(
        found == content.as_bytes(),
        "the device file has {:?}",
        String::from_utf8_lossy(&found)
    );
    Ok(())
}

#[then("the device file has the same permissions as the source")]
fn same_permissions(world: &mut SyncWorld) -> StepResult {
    if let Some(Err(error)) = &world.transfer_result {
        anyhow::bail!("the copy failed: {error}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let device = fs::metadata(destination(world)?)?.permissions().mode();
        let original = fs::metadata(source(world))?.permissions().mode();
        anyhow::ensure!(
            device == original,
            "the device file mode is {device:o}, the source mode is {original:o}"
        );
    }
    Ok(())
}

#[then("the copy fails")]
fn copy_fails(world: &mut SyncWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.transfer_result, Some(Err(_))),
        "the copy result is {:?}",
        world.transfer_result
    );
    Ok(())
}

#[then("the device folder has only the device file")]
fn device_folder_has_only_file(world: &mut SyncWorld) -> StepResult {
    let entries: Vec<PathBuf> = fs::read_dir(world.card())?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    let expected = destination(world)?;
    anyhow::ensure!(
        entries == std::slice::from_ref(&expected),
        "the device folder has {entries:?}"
    );
    Ok(())
}
