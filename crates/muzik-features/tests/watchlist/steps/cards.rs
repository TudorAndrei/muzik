use crate::documents::{field, item, playlist};
use crate::support::StepResult;
use crate::world::WatchlistWorld;
use cucumber::{then, when};
use muzik_store::watchlist::view;
use serde_json::{Value, json};

fn card(world: &WatchlistWorld, which: &str, ordinal: usize, id: &str) -> anyhow::Result<Value> {
    let document = world.document(which)?;
    Ok(item(&document, id, ordinal)?.clone())
}

#[when("the cards are shown")]
fn cards_are_shown(world: &mut WatchlistWorld) -> StepResult {
    world.snapshot()?;
    let document = world.repository().load()?;
    world.shown = Some(serde_json::to_value(view(
        &document,
        &world.downloads(),
        &world.cache(),
    ))?);
    Ok(())
}

#[then(expr = "the shown source {string} and its items keep the fields from the newer version")]
fn shown_keeps_fields(world: &mut WatchlistWorld, id: String) -> StepResult {
    let shown = world.document("shown")?;
    let source = playlist(&shown, &id)?;
    anyhow::ensure!(
        field(source, "/source_extension") == &json!({"revision": 7}),
        "the source is {source}"
    );
    let card = item(&shown, &id, 1)?;
    anyhow::ensure!(
        field(card, "/item_extension") == &json!(["kept"]),
        "the item is {card}"
    );
    Ok(())
}

#[then(expr = "the {word} card of item {int} in {string} reads {string}")]
fn card_reads(
    world: &mut WatchlistWorld,
    which: String,
    ordinal: usize,
    id: String,
    summary: String,
) -> StepResult {
    let card = card(world, &which, ordinal, &id)?;
    let actual = field(&card, "/summary");
    anyhow::ensure!(actual == summary.as_str(), "the card reads {actual}");
    Ok(())
}

#[then(
    expr = "the shown card of item {int} in {string} has the main action {string} labelled {string}"
)]
fn card_main_action_label(
    world: &mut WatchlistWorld,
    ordinal: usize,
    id: String,
    action: String,
    label: String,
) -> StepResult {
    let card = card(world, "shown", ordinal, &id)?;
    let actual = field(&card, "/primary_action");
    anyhow::ensure!(
        actual == &json!({"action": action, "label": label}),
        "the main action is {actual}"
    );
    Ok(())
}

#[then(expr = "the shown card of item {int} in {string} has the main action {string}")]
fn card_main_action(
    world: &mut WatchlistWorld,
    ordinal: usize,
    id: String,
    action: String,
) -> StepResult {
    let card = card(world, "shown", ordinal, &id)?;
    let actual = field(&card, "/primary_action/action");
    anyhow::ensure!(actual == action.as_str(), "the main action is {actual}");
    Ok(())
}

#[then(expr = "the shown card of item {int} in {string} has no main action")]
fn card_has_no_main_action(world: &mut WatchlistWorld, ordinal: usize, id: String) -> StepResult {
    let card = card(world, "shown", ordinal, &id)?;
    let actual = field(&card, "/primary_action");
    anyhow::ensure!(actual.is_null(), "the main action is {actual}");
    Ok(())
}

#[then(expr = "the shown card of item {int} in {string} allows the action {string}")]
fn card_allows(
    world: &mut WatchlistWorld,
    ordinal: usize,
    id: String,
    action: String,
) -> StepResult {
    let card = card(world, "shown", ordinal, &id)?;
    let actual = field(&card, &format!("/actions/{action}"));
    anyhow::ensure!(
        actual == &json!({"enabled": true, "reason": null}),
        "the {action} action is {actual}"
    );
    Ok(())
}

#[then(expr = "the shown card of item {int} in {string} does not allow the action {string}")]
fn card_does_not_allow(
    world: &mut WatchlistWorld,
    ordinal: usize,
    id: String,
    action: String,
) -> StepResult {
    let card = card(world, "shown", ordinal, &id)?;
    let actual = field(&card, &format!("/actions/{action}/enabled"));
    anyhow::ensure!(*actual == false, "the {action} action is enabled: {actual}");
    Ok(())
}

#[then(expr = "the shown card of item {int} in {string} has the thumbnail {string} from the cache")]
fn card_thumbnail(
    world: &mut WatchlistWorld,
    ordinal: usize,
    id: String,
    name: String,
) -> StepResult {
    let card = card(world, "shown", ordinal, &id)?;
    let expected = world.cache().join(name);
    let actual = field(&card, "/thumbnail_path");
    anyhow::ensure!(
        actual == expected.to_string_lossy().as_ref(),
        "the thumbnail is {actual}"
    );
    Ok(())
}
