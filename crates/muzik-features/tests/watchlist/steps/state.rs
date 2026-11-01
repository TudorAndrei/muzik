use crate::documents::{field, item, items, list, playlist, strings};
use crate::support::StepResult;
use crate::world::WatchlistWorld;
use cucumber::gherkin::Step;
use cucumber::then;
use serde_json::{Value, json};

fn item_field(
    world: &WatchlistWorld,
    which: &str,
    id: &str,
    ordinal: usize,
    pointer: &str,
) -> anyhow::Result<Value> {
    let document = world.document(which)?;
    Ok(field(item(&document, id, ordinal)?, pointer).clone())
}

fn processed(world: &WatchlistWorld, which: &str, id: &str) -> anyhow::Result<Vec<String>> {
    let document = world.document(which)?;
    Ok(strings(field(
        playlist(&document, id)?,
        "/processed_video_ids",
    )))
}

#[then(expr = "the {word} watchlist has the stages:")]
fn watchlist_has_stages(world: &mut WatchlistWorld, which: String, step: &Step) -> StepResult {
    let table = step
        .table
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the step has no table"))?;
    let document = world.document(&which)?;
    for row in table.rows.iter().skip(1) {
        let [id, ordinal, stage, status] = row.as_slice() else {
            anyhow::bail!("the row {row:?} does not have 4 cells");
        };
        let actual = field(
            item(&document, id, ordinal.parse()?)?,
            &format!("/stages/{stage}/status"),
        );
        anyhow::ensure!(
            actual.as_str() == Some(status.as_str()),
            "the {stage} stage of item {ordinal} of {id} is {actual}"
        );
    }
    Ok(())
}

#[then(
    expr = "in the {word} watchlist item {int} of {string} waits at the {word} stage for an {string} question"
)]
fn item_waits_for_question(
    world: &mut WatchlistWorld,
    which: String,
    ordinal: usize,
    id: String,
    stage: String,
    kind: String,
) -> StepResult {
    let record = item_field(world, &which, &id, ordinal, &format!("/stages/{stage}"))?;
    anyhow::ensure!(
        field(&record, "/status") == "waiting"
            && field(&record, "/question") == &json!({"kind": kind, "payload": {"task": {}}}),
        "the {stage} stage is {record}"
    );
    Ok(())
}

#[then(expr = "the {word} watchlist has no processed videos for {string}")]
fn no_processed_videos(world: &mut WatchlistWorld, which: String, id: String) -> StepResult {
    let actual = processed(world, &which, &id)?;
    anyhow::ensure!(actual.is_empty(), "the processed videos are {actual:?}");
    Ok(())
}

#[then(expr = "the {word} watchlist has the processed videos {string} for {string}")]
fn processed_videos(
    world: &mut WatchlistWorld,
    which: String,
    videos: String,
    id: String,
) -> StepResult {
    let actual = processed(world, &which, &id)?;
    anyhow::ensure!(
        actual == list(&videos),
        "the processed videos are {actual:?}"
    );
    Ok(())
}

#[then(expr = "the saved watchlist has {int} processed videos for {string}")]
fn processed_video_count(world: &mut WatchlistWorld, count: usize, id: String) -> StepResult {
    let actual = processed(world, "saved", &id)?;
    anyhow::ensure!(actual.len() == count, "the processed videos are {actual:?}");
    Ok(())
}

#[then(expr = "item {int} of {string} in the {word} watchlist has the {word} {string}")]
fn item_has(
    world: &mut WatchlistWorld,
    ordinal: usize,
    id: String,
    which: String,
    name: String,
    expected: String,
) -> StepResult {
    let pointer = match name.as_str() {
        "title" => "/title",
        "error" => "/last_error",
        other => anyhow::bail!("an item has no {other}"),
    };
    let actual = item_field(world, &which, &id, ordinal, pointer)?;
    anyhow::ensure!(actual == expected.as_str(), "the {name} is {actual}");
    Ok(())
}

#[then(expr = "item {int} of {string} in the {word} watchlist has the download path {string}")]
fn item_has_download_path(
    world: &mut WatchlistWorld,
    ordinal: usize,
    id: String,
    which: String,
    path: String,
) -> StepResult {
    let actual = item_field(world, &which, &id, ordinal, "/stages/download/path")?;
    anyhow::ensure!(actual == path.as_str(), "the download path is {actual}");
    Ok(())
}

#[then(expr = "item {int} of {string} in the {word} watchlist has the last action {string}")]
fn item_has_last_action(
    world: &mut WatchlistWorld,
    ordinal: usize,
    id: String,
    which: String,
    action: String,
) -> StepResult {
    let actual = item_field(world, &which, &id, ordinal, "/last_action")?;
    anyhow::ensure!(actual == action.as_str(), "the last action is {actual}");
    Ok(())
}

#[then(
    expr = "the {word} stage of item {int} of {string} in the {word} watchlist has the error {string}"
)]
fn stage_has_error(
    world: &mut WatchlistWorld,
    stage: String,
    ordinal: usize,
    id: String,
    which: String,
    error: String,
) -> StepResult {
    let actual = item_field(
        world,
        &which,
        &id,
        ordinal,
        &format!("/stages/{stage}/error"),
    )?;
    anyhow::ensure!(actual == error.as_str(), "the {stage} error is {actual}");
    Ok(())
}

#[then(expr = "the {word} source {string} has the title {string}")]
fn source_title(
    world: &mut WatchlistWorld,
    which: String,
    id: String,
    title: String,
) -> StepResult {
    let document = world.document(&which)?;
    let actual = field(playlist(&document, &id)?, "/title");
    anyhow::ensure!(actual == title.as_str(), "the title is {actual}");
    Ok(())
}

#[then(expr = "the saved source {string} has no title")]
fn source_has_no_title(world: &mut WatchlistWorld, id: String) -> StepResult {
    let document = world.document("saved")?;
    let actual = field(playlist(&document, &id)?, "/title");
    anyhow::ensure!(actual.is_null(), "the title is {actual}");
    Ok(())
}

#[then(expr = "the saved source {string} has {int} item(s)")]
fn source_item_count(world: &mut WatchlistWorld, id: String, count: usize) -> StepResult {
    let document = world.document("saved")?;
    let actual = items(&document, &id)?.len();
    anyhow::ensure!(actual == count, "the source has {actual} items");
    Ok(())
}

#[then(expr = "the {word} items of {string} are at the positions {string}")]
fn item_positions(
    world: &mut WatchlistWorld,
    which: String,
    id: String,
    positions: String,
) -> StepResult {
    let document = world.document(&which)?;
    let actual = items(&document, &id)?
        .iter()
        .map(|item| field(item, "/position").to_string())
        .collect::<Vec<_>>();
    anyhow::ensure!(actual == list(&positions), "the positions are {actual:?}");
    Ok(())
}

#[then("the saved watchlist did not change")]
fn saved_watchlist_unchanged(world: &mut WatchlistWorld) -> StepResult {
    let (revision, document) = world
        .before
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the saved watchlist was not noted"))?;
    let repository = world.repository();
    let now = (repository.revision()?, repository.load()?);
    anyhow::ensure!(
        now.0 == *revision && now.1 == *document,
        "the saved watchlist changed from revision {revision} to {}",
        now.0
    );
    Ok(())
}

#[then("the saved revision did not change")]
fn saved_revision_unchanged(world: &mut WatchlistWorld) -> StepResult {
    let (revision, _) = world
        .before
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the saved watchlist was not noted"))?;
    let actual = world.repository().revision()?;
    anyhow::ensure!(actual == *revision, "the revision changed to {actual}");
    Ok(())
}

#[then(expr = "the loaded watchlist is version {int}")]
fn loaded_version(world: &mut WatchlistWorld, version: u64) -> StepResult {
    let actual = field(&world.document("loaded")?, "/version").clone();
    anyhow::ensure!(actual == version, "the version is {actual}");
    Ok(())
}
