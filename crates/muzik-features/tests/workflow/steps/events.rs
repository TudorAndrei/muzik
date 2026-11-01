use crate::support::StepResult;
use crate::world::WorkflowWorld;
use cucumber::{given, then};
use muzik_workflow::{SplitProgress, WorkflowEvent};

#[given("the run is cancelled when the plan is ready")]
const fn cancelled_when_plan_ready(world: &mut WorkflowWorld) {
    world.cancel_when_plan_ready = true;
}

#[then("the first event is the input classification")]
fn first_event_is_classification(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        matches!(
            world.events.first(),
            Some(WorkflowEvent::InputClassified(_))
        ),
        "events {:?}",
        world.events
    );
    Ok(())
}

#[then(expr = "an event reports a plan with {int} album(s) and {int} single(s)")]
fn event_reports_plan(world: &mut WorkflowWorld, albums: usize, singles: usize) -> StepResult {
    anyhow::ensure!(
        world
            .events
            .contains(&WorkflowEvent::PlanReady { albums, singles }),
        "events {:?}",
        world.events
    );
    Ok(())
}

#[then(expr = "an event reports split progress {int} of {int} at chapter {int}")]
fn event_reports_progress(
    world: &mut WorkflowWorld,
    completed: usize,
    total: usize,
    chapter_index: u32,
) -> StepResult {
    let expected = SplitProgress {
        completed,
        total,
        chapter_index,
    };
    anyhow::ensure!(
        world.events.iter().any(|event| matches!(
            event,
            WorkflowEvent::SplitProgress { progress, .. } if *progress == expected
        )),
        "events {:?}",
        world.events
    );
    Ok(())
}

#[then("the last event is the completion")]
fn last_event_is_completion(world: &mut WorkflowWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.events.last(), Some(WorkflowEvent::Completed)),
        "events {:?}",
        world.events
    );
    Ok(())
}
