use crate::jobs::{self, field, text};
use anyhow::{Context, anyhow, bail};
use muzik_core::paths::Paths;
use muzik_runner::{Settings, job_id};
use muzik_store::watchlist::{self, ItemAction, Repository, Summary};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn repository() -> Repository {
    Repository::open(&Paths::user())
}

fn settings() -> anyhow::Result<Settings> {
    Ok(Settings::resolve(&Paths::user(), &json!({}))?)
}

fn list_of(value: &Value, key: &str) -> Vec<Value> {
    field(value, key).as_array().cloned().unwrap_or_default()
}

pub fn list(items: bool) -> anyhow::Result<()> {
    let document = repository().load()?;
    let settings = settings()?;
    let visible = watchlist::view(&document, &settings.request.output, &settings.paths.cache)?;
    let playlists = list_of(&visible, "playlists");
    if playlists.is_empty() {
        println!("The watchlist is empty. Add a playlist with `muzik watchlist add <url>`.");
        return Ok(());
    }
    for playlist in &playlists {
        let entries = list_of(playlist, "items");
        let mut counts = BTreeMap::<String, usize>::new();
        for item in &entries {
            *counts.entry(text(field(item, "summary"))).or_default() += 1;
        }
        let summary = Summary::ALL
            .iter()
            .filter_map(|state| {
                counts
                    .get(state.as_ref())
                    .map(|count| format!("{count} {}", state.as_ref().to_lowercase()))
            })
            .collect::<Vec<_>>()
            .join(" · ");
        println!(
            "{}  {} ({}) · {} items{}{}",
            text(field(playlist, "playlist_id")),
            field(playlist, "title").as_str().unwrap_or("Untitled"),
            text(field(playlist, "kind")),
            entries.len(),
            if summary.is_empty() { "" } else { " · " },
            summary
        );
        if items {
            for item in &entries {
                println!(
                    "  {:>4}. {} · {}",
                    text(field(item, "position")),
                    field(item, "title").as_str().unwrap_or("Untitled"),
                    text(field(item, "summary"))
                );
            }
        }
    }
    Ok(())
}

pub fn add(url: &str) -> anyhow::Result<()> {
    let playlist = repository().add(url)?;
    println!(
        "Added {}. Run `muzik watchlist refresh` to sync it.",
        playlist.title.as_deref().unwrap_or(&playlist.playlist_id)
    );
    Ok(())
}

pub fn remove(playlist_id: &str) -> anyhow::Result<()> {
    if repository().remove(playlist_id)? {
        println!("Removed {playlist_id}.");
        Ok(())
    } else {
        bail!("{playlist_id} is not in the watchlist.")
    }
}

pub fn refresh(queue_only: bool) -> anyhow::Result<()> {
    let jobs = jobs::open()?;
    let id = jobs.refresh(&json!({}))?;
    println!("Queued the watchlist check as {}.", job_id(id));
    if queue_only {
        return Ok(());
    }
    jobs::drain(&jobs)
}

pub fn item(
    playlist_id: &str,
    position: u64,
    action: &str,
    queue_only: bool,
) -> anyhow::Result<()> {
    let action: ItemAction = action.parse().map_err(|_| {
        let names = ItemAction::ALL
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow!("Unknown action {action}. Use one of: {names}.")
    })?;
    let document = repository().load()?;
    let playlist = document
        .playlist(playlist_id)
        .with_context(|| format!("{playlist_id} is not in the watchlist."))?;
    let item = playlist
        .items
        .iter()
        .find(|item| item.position == position)
        .with_context(|| format!("{playlist_id} has no item at position {position}."))?;
    let title = item.title.clone();
    let params = json!({
        "playlist_id": playlist_id,
        "position": position,
        "video_id": item.video_id,
        "title": title,
        "action": action,
    });
    let jobs = jobs::open()?;
    let id = jobs.item(&params)?;
    println!("Queued {action} for {title} as {}.", job_id(id));
    if queue_only {
        return Ok(());
    }
    jobs::drain(&jobs)
}
