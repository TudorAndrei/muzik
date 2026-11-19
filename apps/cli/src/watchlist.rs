use crate::jobs;
use anyhow::{Context, bail};
use muzik_core::paths::Paths;
use muzik_core::{PlaylistFormat, app_config};
use muzik_runner::{Settings, job_id};
use muzik_store::watchlist::{self, ItemAction, Repository, Summary, playlist_files};
use serde_json::{Value, json};
use std::collections::HashMap;

fn repository() -> Repository {
    Repository::open(&Paths::user())
}

fn settings() -> anyhow::Result<Settings> {
    Ok(Settings::resolve(&Paths::user(), &json!({}))?)
}

pub fn list(items: bool) -> anyhow::Result<()> {
    let snapshot = repository().snapshot()?;
    let settings = settings()?;
    let visible = watchlist::view(&snapshot, &settings.request.output, &settings.paths.cache);
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

pub fn playlists(on: Option<bool>, format: Option<PlaylistFormat>) -> anyhow::Result<()> {
    let paths = Paths::user();
    let mut changes = serde_json::Map::new();
    if let Some(on) = on {
        changes.insert("playlists".into(), json!(on));
    }
    if let Some(format) = format {
        changes.insert("playlist_format".into(), json!(format));
    }
    if !changes.is_empty() {
        app_config::save_gui_defaults(&paths, &Value::Object(changes))?;
    }
    let settings = settings()?;
    let report = muzik_runner::watchlist::write_playlists(&settings)?;
    if settings.playlists.is_none() {
        println!(
            "Playlist files are off. Removed {} file(s). Turn them on with `muzik watchlist playlists --on`.",
            report.removed.len()
        );
        return Ok(());
    }
    print_report(&report);
    Ok(())
}

pub fn playlist(playlist_id: &str, on: bool) -> anyhow::Result<()> {
    if !repository().set_playlist_file(playlist_id, on)? {
        bail!("{playlist_id} is not in the watchlist.");
    }
    println!(
        "The playlist file of {playlist_id} is {}.",
        if on { "on" } else { "off" }
    );
    let settings = settings()?;
    if settings.playlists.is_some() {
        print_report(&muzik_runner::watchlist::write_playlists(&settings)?);
    }
    Ok(())
}

fn print_report(report: &playlist_files::Report) {
    for written in &report.written {
        println!(
            "{} · {} song(s){}{}",
            written.path.display(),
            written.tracks,
            if written.missing > 0 {
                format!(" · {} item(s) not in the library", written.missing)
            } else {
                String::new()
            },
            if written.changed { "" } else { " · unchanged" },
        );
    }
    for removed in &report.removed {
        println!("Removed {}", removed.display());
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
    let (id, item) = playlist
        .item_ids()
        .into_iter()
        .zip(&playlist.items)
        .find(|(_, item)| item.position == position)
        .with_context(|| format!("{playlist_id} has no item at position {position}."))?;
    let title = item.title.clone();
    let mut params = json!({"title": title, "action": action});
    id.write(&mut params)?;
    let jobs = jobs::open()?;
    let id = jobs.item(&params)?;
    println!("Queued {action} for {title} as {}.", job_id(id));
    if queue_only {
        return Ok(());
    }
    jobs::drain(&jobs)
}
