use crate::jobs;
use anyhow::{Context, bail};
use muzik_core::paths::Paths;
use muzik_runner::{Settings, job_id};
use muzik_store::watchlist::{self, ItemAction, Repository, Summary};
use serde_json::json;
use std::collections::HashMap;

fn repository() -> Repository {
    Repository::open(&Paths::user())
}

fn settings() -> anyhow::Result<Settings> {
    Ok(Settings::resolve(&Paths::user(), &json!({}))?)
}

pub fn list(items: bool) -> anyhow::Result<()> {
    let document = repository().load()?;
    let settings = settings()?;
    let visible = watchlist::view(&document, &settings.request.output, &settings.paths.cache);
    let playlists = &visible.playlists;
    if playlists.is_empty() {
        println!("The watchlist is empty. Add a playlist with `muzik watchlist add <url>`.");
        return Ok(());
    }
    for playlist in playlists {
        let entries = &playlist.items;
        let mut counts = HashMap::<Summary, usize>::new();
        for item in entries {
            let count = counts.entry(item.summary).or_default();
            *count = count.saturating_add(1);
        }
        let summary = Summary::ALL
            .iter()
            .filter_map(|state| {
                counts
                    .get(state)
                    .map(|count| format!("{count} {}", state.as_ref().to_lowercase()))
            })
            .collect::<Vec<_>>()
            .join(" · ");
        println!(
            "{}  {} ({}) · {} items{}{}",
            playlist.playlist_id,
            playlist.title.as_deref().unwrap_or("Untitled"),
            playlist.kind,
            entries.len(),
            if summary.is_empty() { "" } else { " · " },
            summary
        );
        if items {
            for item in entries {
                println!(
                    "  {:>4}. {} · {}",
                    item.item.position, item.item.title, item.summary
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
    action: ItemAction,
    queue_only: bool,
) -> anyhow::Result<()> {
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
