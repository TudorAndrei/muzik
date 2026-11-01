use crate::support::StepResult;
use crate::world::{SyncWorld, probe, rows};
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use muzik_sync::{self as sync, Action, Encoding, Plan, Transfer};
use std::fs;
use std::path::PathBuf;

fn plan(world: &SyncWorld) -> anyhow::Result<Plan> {
    let encodings = sync::encodings(world.records()?, &world.target.path)?;
    let unreadable = &world.unreadable;
    Ok(sync::plan(
        &world.target,
        &world.library(),
        &world.tracks,
        &world.covers,
        &encodings,
        2,
        &|path| Ok(probe(unreadable, path)),
    ))
}

fn recorded_transfer(world: &SyncWorld) -> anyhow::Result<&Transfer> {
    world
        .transfer
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no converted file is on the device"))
}

fn expect_paths(found: &[PathBuf], expected: &[PathBuf], what: &str) -> StepResult {
    anyhow::ensure!(found == expected, "the {what} are {found:?}");
    Ok(())
}

#[given("the device has the files that need no conversion")]
fn device_has_copies(world: &mut SyncWorld) -> StepResult {
    for transfer in &plan(world)?.pending {
        if transfer.action == Action::Copy {
            sync::transfer(transfer)?;
        }
    }
    Ok(())
}

#[given("the converted file is on the device")]
fn converted_file_on_device(world: &mut SyncWorld) -> StepResult {
    let first = plan(world)?;
    let transfer = first
        .pending
        .first()
        .ok_or_else(|| anyhow::anyhow!("the plan has no pending file"))?;
    if let Some(parent) = transfer.destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&transfer.destination, b"converted")?;
    world.transfer = Some(transfer.clone());
    Ok(())
}

#[given("muzik recorded the encoding of the converted file")]
fn recorded_encoding(world: &mut SyncWorld) -> StepResult {
    sync::record(world.records()?, recorded_transfer(world)?)?;
    Ok(())
}

#[when("muzik records the file as a plain copy")]
fn records_plain_copy(world: &mut SyncWorld) -> StepResult {
    let copy = Transfer {
        action: Action::Copy,
        ..recorded_transfer(world)?.clone()
    };
    sync::record(world.records()?, &copy)?;
    Ok(())
}

#[when("muzik plans the sync")]
fn plans_sync(world: &mut SyncWorld) -> StepResult {
    world.plan = Some(plan(world)?);
    Ok(())
}

#[then("the planned destinations on the device are:")]
fn planned_destinations(world: &mut SyncWorld, step: &Step) -> StepResult {
    let card = world.card();
    let expected: Vec<PathBuf> = rows(step)?.iter().map(|name| card.join(name)).collect();
    let found: Vec<PathBuf> = world
        .current_plan()?
        .pending
        .iter()
        .map(|transfer| transfer.destination.clone())
        .collect();
    expect_paths(&found, &expected, "destinations")
}

#[then("the planned library tracks are:")]
fn planned_sources(world: &mut SyncWorld, step: &Step) -> StepResult {
    let library = world.library();
    let expected: Vec<PathBuf> = rows(step)?.iter().map(|name| library.join(name)).collect();
    let found: Vec<PathBuf> = world
        .current_plan()?
        .pending
        .iter()
        .map(|transfer| transfer.source.clone())
        .collect();
    expect_paths(&found, &expected, "sources")
}

#[then("the skipped duplicates are:")]
fn skipped_duplicates(world: &mut SyncWorld, step: &Step) -> StepResult {
    let library = world.library();
    let expected: Vec<PathBuf> = rows(step)?.iter().map(|name| library.join(name)).collect();
    expect_paths(&world.current_plan()?.duplicates, &expected, "duplicates")
}

#[then(expr = "the track {string} is outside the library")]
fn track_is_outside(world: &mut SyncWorld, name: String) -> StepResult {
    let expected = world.root().join(name);
    expect_paths(
        &world.current_plan()?.outside,
        std::slice::from_ref(&expected),
        "outside tracks",
    )
}

#[then(expr = "{string} needs {int} bytes")]
fn destination_needs_bytes(world: &mut SyncWorld, name: String, bytes: u64) -> StepResult {
    let destination = world.card().join(name);
    let transfer = world
        .current_plan()?
        .pending
        .iter()
        .find(|transfer| transfer.destination == destination)
        .ok_or_else(|| anyhow::anyhow!("{} is not planned", destination.display()))?;
    anyhow::ensure!(transfer.bytes == bytes, "it needs {} bytes", transfer.bytes);
    Ok(())
}

#[then(expr = "the plan has {int} current file(s)")]
fn plan_has_current(world: &mut SyncWorld, fresh: usize) -> StepResult {
    let plan = world.current_plan()?;
    anyhow::ensure!(
        plan.fresh == fresh,
        "the plan has {} current files",
        plan.fresh
    );
    Ok(())
}

#[then(expr = "the plan has {int} current file(s) and {int} pending file(s)")]
fn plan_has_current_and_pending(world: &mut SyncWorld, fresh: usize, pending: usize) -> StepResult {
    let plan = world.current_plan()?;
    anyhow::ensure!(
        plan.fresh == fresh && plan.pending.len() == pending,
        "the plan has {} current and {} pending files",
        plan.fresh,
        plan.pending.len()
    );
    Ok(())
}

#[then(expr = "the plan converts the file to MP3 at {int} kbps")]
fn plan_converts_to_mp3(world: &mut SyncWorld, kbps: u32) -> StepResult {
    let action = world
        .current_plan()?
        .pending
        .first()
        .map(|transfer| &transfer.action);
    anyhow::ensure!(
        action == Some(&Action::Convert(Encoding::Mp3 { kbps })),
        "the action is {action:?}"
    );
    Ok(())
}

#[then("the device has no recorded encodings")]
fn no_recorded_encodings(world: &mut SyncWorld) -> StepResult {
    let encodings = sync::encodings(world.records()?, &world.card())?;
    anyhow::ensure!(encodings.is_empty(), "the encodings are {encodings:?}");
    Ok(())
}
