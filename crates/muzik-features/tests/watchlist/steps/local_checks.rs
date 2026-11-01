use crate::documents::{field, playlist};
use crate::support::StepResult;
use crate::world::WatchlistWorld;
use cucumber::{given, then, when};
use muzik_store::watchlist::{LocalCheck, WatchlistView};
use std::cell::{Cell, RefCell};
use std::sync::atomic::Ordering;

fn prepare(world: &WatchlistWorld) -> muzik_store::Result<(WatchlistView, LocalCheck)> {
    world.with_options(|options| {
        world
            .local
            .checks
            .prepare::<muzik_store::Error>(world.repository(), || Ok(options.into()))
    })
}

fn keep(world: &mut WatchlistWorld, prepared: (WatchlistView, LocalCheck)) -> StepResult {
    let (saved, check) = prepared;
    world.prepared = Some(serde_json::to_value(saved)?);
    world.local.check = Some(check);
    Ok(())
}

fn run(world: &mut WatchlistWorld, busy: impl Fn() -> bool) -> StepResult {
    world.snapshot()?;
    let check = world
        .local
        .check
        .take()
        .ok_or_else(|| anyhow::anyhow!("no local check was prepared"))?;
    let mut published = None;
    check.run(busy, |result| published = Some(result));
    world.published = published.map(|result| {
        result
            .map_err(|error| error.to_string())
            .and_then(|view| serde_json::to_value(view).map_err(|error| error.to_string()))
    });
    Ok(())
}

fn run_with_outside_edits(
    world: &mut WatchlistWorld,
    id: &str,
    title: impl Fn(u32) -> Option<String>,
) -> StepResult {
    let repository = world.repository();
    let polls = Cell::new(0_u32);
    let failure = RefCell::new(None);
    run(world, || {
        polls.set(polls.get().saturating_add(1));
        if let Some(title) = title(polls.get())
            && let Err(error) = repository.rename(id, &title)
        {
            failure.replace(Some(error.to_string()));
        }
        false
    })?;
    if let Some(error) = failure.into_inner() {
        anyhow::bail!("the outside edit failed: {error}");
    }
    world.outside_edits = polls.get();
    Ok(())
}

#[given("a local check is prepared")]
#[when("a local check is prepared")]
fn local_check_is_prepared(world: &mut WatchlistWorld) -> StepResult {
    let prepared = prepare(world)?;
    keep(world, prepared)
}

#[given("a local check is prepared and another check starts while it reads the settings")]
fn prepared_during_settings(world: &mut WatchlistWorld) -> StepResult {
    let prepared = world.with_options(|options| {
        world
            .local
            .checks
            .prepare::<muzik_store::Error>(world.repository(), || {
                let _ = prepare(world)?;
                Ok(options.into())
            })
    })?;
    keep(world, prepared)
}

#[when("another local check is prepared")]
fn another_check_is_prepared(world: &mut WatchlistWorld) -> StepResult {
    prepare(world)?;
    Ok(())
}

#[when("another local check cannot read its settings")]
fn another_check_fails(world: &mut WatchlistWorld) -> StepResult {
    let failed = world
        .local
        .checks
        .prepare::<muzik_store::Error>(world.repository(), || Err("settings unavailable".into()));
    anyhow::ensure!(failed.is_err(), "the other local check read its settings");
    Ok(())
}

#[when(expr = "the source {string} is renamed to {string} by the user")]
fn user_renames(world: &mut WatchlistWorld, id: String, title: String) -> StepResult {
    world
        .local
        .checks
        .edit(|| world.repository().rename(&id, &title))?;
    Ok(())
}

#[when("a runner starts a job")]
fn runner_starts(world: &mut WatchlistWorld) {
    world.local.generation.fetch_add(1, Ordering::SeqCst);
}

#[when("the local check runs")]
fn local_check_runs(world: &mut WatchlistWorld) -> StepResult {
    run(world, || false)
}

#[when("the local check runs while a job runs")]
fn local_check_runs_while_busy(world: &mut WatchlistWorld) -> StepResult {
    run(world, || true)
}

#[when(
    expr = "the local check runs and another process renames the source {string} to {string} at the third poll"
)]
fn outside_edit_at_third_poll(world: &mut WatchlistWorld, id: String, title: String) -> StepResult {
    run_with_outside_edits(world, &id, |poll| (poll == 3).then(|| title.clone()))
}

#[when(expr = "the local check runs and another process renames the source {string} at each poll")]
fn outside_edit_at_each_poll(world: &mut WatchlistWorld, id: String) -> StepResult {
    run_with_outside_edits(world, &id, |poll| Some(format!("Edit {poll}")))
}

#[then("the local check publishes nothing")]
fn publishes_nothing(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        world.published.is_none(),
        "the local check published {:?}",
        world.published
    );
    Ok(())
}

#[then(expr = "the local check reports {string}")]
fn local_check_reports(world: &mut WatchlistWorld, message: String) -> StepResult {
    match &world.published {
        Some(Err(error)) => anyhow::ensure!(error.contains(&message), "the error is {error}"),
        other => anyhow::bail!("the local check published {other:?}"),
    }
    Ok(())
}

#[then(expr = "the saved source {string} has the title of the last outside edit")]
fn title_of_last_edit(world: &mut WatchlistWorld, id: String) -> StepResult {
    let saved = world.document("saved")?;
    let expected = format!("Edit {}", world.outside_edits);
    let actual = field(playlist(&saved, &id)?, "/title");
    anyhow::ensure!(actual == expected.as_str(), "the title is {actual}");
    Ok(())
}
