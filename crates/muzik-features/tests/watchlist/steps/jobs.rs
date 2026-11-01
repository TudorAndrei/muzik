use crate::documents::{field, list};
use crate::operations::{FakeOperations, Processing, Question, card};
use crate::support::StepResult;
use crate::world::WatchlistWorld;
use cucumber::{given, then, when};
use muzik_core::{DecisionKind, JobEvent};
use muzik_store::watchlist::jobs::{self, JobError, PendingItem};
use muzik_store::watchlist::{ItemAction, ItemId, Stage};
use serde_json::{Value, json};
use std::str::FromStr;
use std::sync::atomic::AtomicBool;

fn listed(videos: &str) -> anyhow::Result<Vec<muzik_store::watchlist::WatchItem>> {
    list(videos)
        .iter()
        .enumerate()
        .map(|(index, video)| {
            let position = u64::try_from(index)?
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("too many videos"))?;
            Ok(card(position, video, video))
        })
        .collect()
}

fn job_result(world: &WatchlistWorld) -> anyhow::Result<&Value> {
    match &world.job {
        Some(Ok(result)) => Ok(result),
        Some(Err(error)) => anyhow::bail!("the job failed: {error}"),
        None => anyhow::bail!("no job ran"),
    }
}

fn refresh(world: &mut WatchlistWorld) -> StepResult {
    world.snapshot()?;
    let mut operations = std::mem::take(&mut world.operations);
    operations.processed.clear();
    operations.calls.clear();
    let repository = world.repository();
    let cancelled = AtomicBool::new(false);
    let mut events = Vec::new();
    let result = world.with_job_options(|options| {
        jobs::refresh(
            &repository,
            options,
            &mut operations,
            &cancelled,
            &mut |event| events.push(event),
        )
    });
    world.operations = operations;
    world.events = events;
    world.job = Some(result);
    Ok(())
}

fn run_on_item(
    world: &mut WatchlistWorld,
    id: &ItemId,
    action: ItemAction,
    only_the_item: bool,
) -> StepResult {
    world.snapshot()?;
    let mut operations: FakeOperations = std::mem::take(&mut world.operations);
    let repository = world.repository();
    let cancelled = AtomicBool::new(false);
    let result = world.with_job_options(|options| {
        if only_the_item {
            jobs::run_item(
                &repository,
                options,
                id,
                action,
                &mut operations,
                &cancelled,
            )
            .map(|_| Value::Null)
        } else {
            jobs::action(
                &repository,
                options,
                id,
                action,
                &mut operations,
                &cancelled,
            )
        }
    });
    world.operations = operations;
    world.job = Some(result);
    Ok(())
}

#[given(expr = "the source now lists the videos {string}")]
fn source_lists(world: &mut WatchlistWorld, videos: String) -> StepResult {
    world.operations.listed = listed(&videos)?;
    Ok(())
}

#[given(expr = "the source now lists the videos {string} with the title {string}")]
fn source_lists_with_title(
    world: &mut WatchlistWorld,
    videos: String,
    title: String,
) -> StepResult {
    world.operations.listed = listed(&videos)?;
    world.operations.listed_title = Some(title);
    Ok(())
}

#[given(expr = "the listed video {string} is private")]
fn listed_video_is_private(world: &mut WatchlistWorld, video: String) -> StepResult {
    let item = world
        .operations
        .listed
        .iter_mut()
        .find(|item| item.video_id.as_deref() == Some(video.as_str()))
        .ok_or_else(|| anyhow::anyhow!("the video {video} is not listed"))?;
    item.unavailable = Some(true);
    Ok(())
}

#[given("each source lists one video named after the source")]
const fn each_source_lists_one_video(world: &mut WatchlistWorld) {
    world.operations.list_per_source = true;
}

#[given("processing leaves each video as it is")]
fn processing_leaves_videos(world: &mut WatchlistWorld) {
    world.operations.processing = Processing::Unchanged;
}

#[given(expr = "the video {string} asks an {string} question at the {word} stage")]
fn video_asks_question(
    world: &mut WatchlistWorld,
    video: String,
    kind: String,
    stage: String,
) -> StepResult {
    world.operations.question = Some(Question {
        video,
        stage: Stage::from_str(&stage)?,
        value: json!({"kind": kind, "payload": {"task": {}}}),
    });
    Ok(())
}

#[given(expr = "processing fails with the error {string}")]
fn processing_fails(world: &mut WatchlistWorld, message: String) {
    world.operations.processing = Processing::Fail(message);
}

#[given(expr = "the {word} stage fails with the error {string}")]
fn stage_fails(world: &mut WatchlistWorld, stage: String, message: String) -> StepResult {
    world.operations.processing = Processing::FailStage(Stage::from_str(&stage)?, message);
    Ok(())
}

#[given("processing downloads the video again")]
fn processing_downloads_again(world: &mut WatchlistWorld) {
    world.operations.processing = Processing::DownloadAgain;
}

#[given("the job is cancelled while the second video is processed")]
fn cancelled_at_second_video(world: &mut WatchlistWorld) {
    world.operations.processing = Processing::StopAtSecond;
}

#[given("jobs are dry runs")]
const fn jobs_are_dry_runs(world: &mut WatchlistWorld) {
    world.dry_run = true;
}

#[when("the watchlist is refreshed")]
fn watchlist_is_refreshed(world: &mut WatchlistWorld) -> StepResult {
    refresh(world)
}

#[when(expr = "the source {string} is refreshed")]
fn source_is_refreshed(world: &mut WatchlistWorld, id: String) -> StepResult {
    world.only = Some(id);
    refresh(world)
}

#[when("the watchlist is synced")]
fn watchlist_is_synced(world: &mut WatchlistWorld) -> StepResult {
    let mut operations = std::mem::take(&mut world.operations);
    operations.processed.clear();
    let repository = world.repository();
    let synced = world.with_job_options(|options| {
        jobs::sync(
            &repository,
            options,
            &mut operations,
            &AtomicBool::new(false),
            &mut |_| {},
        )
    });
    world.operations = operations;
    world.pending = Some(synced?.pending);
    Ok(())
}

#[when(expr = "the action {string} runs on the video {string} at position {int} of {string}")]
fn action_runs(
    world: &mut WatchlistWorld,
    action: String,
    video: String,
    position: u64,
    id: String,
) -> StepResult {
    let item = ItemId::new(&id, position, Some(&video));
    run_on_item(world, &item, ItemAction::from_str(&action)?, false)
}

#[when(expr = "the item {string} at position {int} of {string} runs")]
fn item_runs(world: &mut WatchlistWorld, video: String, position: u64, id: String) -> StepResult {
    let item = ItemId::new(&id, position, Some(&video));
    run_on_item(world, &item, ItemAction::Run, true)
}

#[when(expr = "a video asks an {string} question")]
fn a_video_asks(world: &mut WatchlistWorld, kind: String) -> StepResult {
    world.decision = Some(Stage::of_decision(DecisionKind::from_str(&kind)?));
    Ok(())
}

#[then(expr = "the video waits at the {word} stage")]
fn the_video_waits(world: &mut WatchlistWorld, stage: String) -> StepResult {
    let expected = Stage::from_str(&stage)?;
    anyhow::ensure!(
        world.decision == Some(expected),
        "the video waits at {:?}",
        world.decision
    );
    Ok(())
}

#[then(expr = "the videos {string} were processed")]
fn videos_were_processed(world: &mut WatchlistWorld, videos: String) -> StepResult {
    let actual = &world.operations.processed;
    anyhow::ensure!(
        *actual == list(&videos),
        "the processed videos are {actual:?}"
    );
    Ok(())
}

#[then("no video was processed")]
fn no_video_was_processed(world: &mut WatchlistWorld) -> StepResult {
    let actual = &world.operations.processed;
    anyhow::ensure!(actual.is_empty(), "the processed videos are {actual:?}");
    Ok(())
}

#[then(expr = "the watchlist was saved {int} times")]
fn watchlist_saved_times(world: &mut WatchlistWorld, count: usize) -> StepResult {
    let actual = world
        .events
        .iter()
        .filter(|event| **event == JobEvent::WatchlistSaved)
        .count();
    anyhow::ensure!(actual == count, "the watchlist was saved {actual} times");
    Ok(())
}

#[then(expr = "the refresh summary counts {int} {word} video(s)")]
fn summary_counts(world: &mut WatchlistWorld, count: u64, kind: String) -> StepResult {
    let actual = field(job_result(world)?, &format!("/summary/{kind}_videos"));
    anyhow::ensure!(
        *actual == count,
        "the summary counts {actual} {kind} videos"
    );
    Ok(())
}

#[then(expr = "the work was {string}")]
fn the_work_was(world: &mut WatchlistWorld, calls: String) -> StepResult {
    let actual = &world.operations.calls;
    anyhow::ensure!(*actual == list(&calls), "the work was {actual:?}");
    Ok(())
}

#[then("the job fails")]
fn job_fails(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.job, Some(Err(JobError::Operation(_)))),
        "the job ended with {:?}",
        world.job
    );
    Ok(())
}

#[then("the job is cancelled")]
fn job_is_cancelled(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.job, Some(Err(JobError::Cancelled))),
        "the job ended with {:?}",
        world.job
    );
    Ok(())
}

#[then("the item fails at a stage")]
fn item_fails_at_stage(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.job, Some(Err(JobError::Failed { .. }))),
        "the job ended with {:?}",
        world.job
    );
    Ok(())
}

#[then("the action reports a dry run")]
fn action_reports_dry_run(world: &mut WatchlistWorld) -> StepResult {
    let actual = field(job_result(world)?, "/action/dry_run");
    anyhow::ensure!(*actual == true, "the action reports {actual}");
    Ok(())
}

#[then(expr = "the action waits at the {word} stage")]
fn action_waits(world: &mut WatchlistWorld, stage: String) -> StepResult {
    let actual = field(job_result(world)?, "/action/waiting_stage");
    anyhow::ensure!(*actual == stage.as_str(), "the action waits at {actual}");
    Ok(())
}

#[then(expr = "the pending videos of {string} are {string}")]
fn pending_videos(world: &mut WatchlistWorld, id: String, videos: String) -> StepResult {
    let expected = list(&videos)
        .iter()
        .map(|entry| {
            let (video, position) = entry
                .split_once(" at ")
                .ok_or_else(|| anyhow::anyhow!("{entry} has no position"))?;
            Ok(PendingItem {
                id: ItemId::new(&id, position.parse()?, Some(video)),
                title: video.to_owned(),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let actual = world
        .pending
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the watchlist was not synced"))?;
    anyhow::ensure!(*actual == expected, "the pending videos are {actual:?}");
    Ok(())
}
