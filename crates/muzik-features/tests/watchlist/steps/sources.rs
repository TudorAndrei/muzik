use crate::documents::{field, playlist};
use crate::support::StepResult;
use crate::world::WatchlistWorld;
use cucumber::{given, then, when};
use muzik_store::watchlist::{Playlist, bandcamp_source, parse_source};
use serde_json::Value;

fn source(world: &WatchlistWorld) -> anyhow::Result<&Playlist> {
    match &world.source {
        Some(Ok(source)) => Ok(source),
        Some(Err(error)) => anyhow::bail!("the source was rejected: {error}"),
        None => anyhow::bail!("no source was read"),
    }
}

fn changed(world: &WatchlistWorld) -> anyhow::Result<bool> {
    world
        .changed
        .ok_or_else(|| anyhow::anyhow!("no change was asked for"))
}

#[when(expr = "the link {string} is read as a source")]
fn link_is_read(world: &mut WatchlistWorld, link: String) {
    world.source = Some(parse_source(&link));
}

#[when(expr = "the source {string} is added")]
fn source_is_added(world: &mut WatchlistWorld, link: String) {
    world.source = Some(world.repository().add(&link));
}

#[given(expr = "the watchlist has the source {string}")]
fn watchlist_has_source(world: &mut WatchlistWorld, link: String) -> StepResult {
    world.repository().add(&link)?;
    Ok(())
}

#[then(expr = "the source has the ID {string}")]
fn source_has_id(world: &mut WatchlistWorld, id: String) -> StepResult {
    let actual = &source(world)?.playlist_id;
    anyhow::ensure!(*actual == id, "the source has the ID {actual}");
    Ok(())
}

#[then(expr = "the source has the link {string}")]
fn source_has_link(world: &mut WatchlistWorld, link: String) -> StepResult {
    let actual = &source(world)?.url;
    anyhow::ensure!(*actual == link, "the source has the link {actual}");
    Ok(())
}

#[then(expr = "the source has the title {string}")]
fn source_has_title(world: &mut WatchlistWorld, title: String) -> StepResult {
    let actual = source(world)?.title.as_deref();
    anyhow::ensure!(
        actual == Some(title.as_str()),
        "the source has the title {actual:?}"
    );
    Ok(())
}

#[then("the source is rejected")]
fn source_is_rejected(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(
        matches!(world.source, Some(Err(_))),
        "the source was {:?}",
        world.source
    );
    Ok(())
}

#[when(expr = "the source {string} is renamed to {string}")]
fn source_is_renamed(world: &mut WatchlistWorld, id: String, title: String) -> StepResult {
    world.changed = Some(world.repository().rename(&id, &title)?);
    Ok(())
}

#[when(expr = "the source {string} is removed")]
fn source_is_removed(world: &mut WatchlistWorld, id: String) -> StepResult {
    world.changed = Some(world.repository().remove(&id)?);
    Ok(())
}

#[then("the change is made")]
fn change_is_made(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(changed(world)?, "nothing was changed");
    Ok(())
}

#[then("nothing is changed")]
fn nothing_is_changed(world: &mut WatchlistWorld) -> StepResult {
    anyhow::ensure!(!changed(world)?, "the watchlist was changed");
    Ok(())
}

#[given(expr = "the Bandcamp collection of {string} was added")]
fn bandcamp_was_added(world: &mut WatchlistWorld, user: String) -> StepResult {
    anyhow::ensure!(
        world.repository().ensure(&bandcamp_source(&user))?,
        "the Bandcamp collection was not added"
    );
    Ok(())
}

#[when(expr = "the Bandcamp collection of {string} is added")]
fn bandcamp_is_added(world: &mut WatchlistWorld, user: String) -> StepResult {
    world.snapshot()?;
    world.changed = Some(world.repository().ensure(&bandcamp_source(&user))?);
    Ok(())
}

#[then(expr = "the saved watchlist has {int} source(s)")]
fn saved_source_count(world: &mut WatchlistWorld, count: usize) -> StepResult {
    let saved = world.document("saved")?;
    let actual = field(&saved, "/playlists").as_array().map(Vec::len);
    anyhow::ensure!(
        actual == Some(count),
        "the watchlist has {actual:?} sources"
    );
    Ok(())
}

#[then(expr = "the saved source {string} is a {string} source with the link {string}")]
fn saved_source_kind_and_link(
    world: &mut WatchlistWorld,
    id: String,
    kind: String,
    link: String,
) -> StepResult {
    let saved = world.document("saved")?;
    let source = playlist(&saved, &id)?;
    anyhow::ensure!(
        source.get("kind") == Some(&Value::from(kind))
            && source.get("url") == Some(&Value::from(link)),
        "the source is {source}"
    );
    Ok(())
}
