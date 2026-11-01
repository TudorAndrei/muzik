use crate::support::StepResult;
use crate::world::WorkflowWorld;
use cucumber::{given, then, when};
use muzik_workflow::{Error, WorkflowEvent, plan_audio_processing, process_audio_plan};
use std::sync::Arc;
use std::sync::atomic::Ordering;

fn run(world: &mut WorkflowWorld, raw: &str) {
    let request = world.request(raw);
    let cancelled = Arc::clone(&world.cancelled);
    let cancel_when_plan_ready = world.cancel_when_plan_ready;
    let mut events = Vec::new();
    let outcome = muzik_workflow::run_workflow_with_events(
        &request,
        &world.options,
        &mut world.operations,
        &cancelled,
        &mut |event| {
            if cancel_when_plan_ready && matches!(event, WorkflowEvent::PlanReady { .. }) {
                cancelled.store(true, Ordering::SeqCst);
            }
            events.push(event);
        },
    );
    world.events = events;
    world.outcome = Some(outcome);
}

#[given("the workflow is cancelled")]
fn workflow_is_cancelled(world: &mut WorkflowWorld) {
    world.cancelled.store(true, Ordering::SeqCst);
}

#[given("organize is off")]
const fn organize_is_off(world: &mut WorkflowWorld) {
    world.options.no_organize = true;
}

#[when(expr = "the workflow runs for {string}")]
fn workflow_runs_for(world: &mut WorkflowWorld, raw: String) {
    run(world, &raw);
}

#[when("the workflow runs for the local file")]
fn workflow_runs_for_the_local_file(world: &mut WorkflowWorld) {
    let raw = world.source.to_string_lossy().into_owned();
    run(world, &raw);
}

#[when("the workflow processes the local file")]
fn workflow_processes_the_local_file(world: &mut WorkflowWorld) {
    let splits = world.root().join("splits");
    world.outcome = Some(process_audio_plan(
        std::slice::from_ref(&world.source),
        &[],
        &splits,
        &world.options,
        &mut world.operations,
        &world.cancelled,
    ));
}

#[when("the workflow plans the local file with no split")]
fn workflow_plans_with_no_split(world: &mut WorkflowWorld) -> StepResult {
    world.plan = Some(plan_audio_processing(
        std::slice::from_ref(&world.source),
        &[],
        true,
    )?);
    Ok(())
}

#[then(expr = "the plan has {int} album(s) and {int} single(s)")]
fn plan_has(world: &mut WorkflowWorld, albums: usize, singles: usize) -> StepResult {
    let plan = world.current_plan()?;
    anyhow::ensure!(
        plan.albums.len() == albums && plan.singles.len() == singles,
        "the plan has {} albums and {} singles",
        plan.albums.len(),
        plan.singles.len()
    );
    Ok(())
}

#[then("the workflow is cancelled")]
fn workflow_was_cancelled(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.outcome, Some(Err(Error::Cancelled))),
        "the outcome is {:?}",
        world.outcome
    );
    Ok(())
}

#[then(expr = "the workflow fails with {string}")]
fn workflow_fails_with(world: &mut WorkflowWorld, text: String) -> StepResult {
    anyhow::ensure!(
        matches!(&world.outcome, Some(Err(Error::Operation(message))) if message.contains(&text)),
        "the outcome is {:?}",
        world.outcome
    );
    Ok(())
}

#[then("nothing is organized")]
fn nothing_is_organized(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.organized.is_empty(),
        "organized {:?}",
        world.operations.organized
    );
    Ok(())
}

#[then("the source is organized")]
fn source_is_organized(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.organized == [world.source.clone()],
        "organized {:?}",
        world.operations.organized
    );
    Ok(())
}

#[then("nothing was split")]
fn nothing_was_split(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.split_sources.is_empty(),
        "split {:?}",
        world.operations.split_sources
    );
    Ok(())
}

#[then("the source was split")]
fn source_was_split(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        world.operations.split_sources == [world.source.clone()],
        "split {:?}",
        world.operations.split_sources
    );
    Ok(())
}
