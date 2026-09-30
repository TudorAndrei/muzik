use crate::jobs::{self, field, text};
use muzik_core::watchlist::{self, ItemAction, Repository, Summary};
use muzik_core::{app_config, paths};
use muzik_runner::job_id;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn repository() -> Repository {
    Repository::default()
}

fn output() -> PathBuf {
    app_config::load_gui_defaults(&app_config::path())
        .ok()
        .and_then(|defaults| field(&defaults, "output").as_str().map(PathBuf::from))
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(paths::download_dir)
}

fn list_of(value: &Value, key: &str) -> Vec<Value> {
    field(value, key).as_array().cloned().unwrap_or_default()
}

pub fn list(items: bool) -> Result<(), String> {
    let document = repository().load()?;
    let visible = watchlist::view(document, &output(), &paths::cache_dir())?;
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

pub fn add(url: &str) -> Result<(), String> {
    let playlist = repository().add(url)?;
    println!(
        "Added {}. Run `muzik watchlist refresh` to sync it.",
        field(&playlist, "title")
            .as_str()
            .or_else(|| field(&playlist, "playlist_id").as_str())
            .unwrap_or("the playlist")
    );
    Ok(())
}

pub fn remove(playlist_id: &str) -> Result<(), String> {
    if repository().remove(playlist_id)? {
        println!("Removed {playlist_id}.");
        Ok(())
    } else {
        Err(format!("{playlist_id} is not in the watchlist."))
    }
}

pub fn refresh(queue_only: bool) -> Result<(), String> {
    let jobs = jobs::open()?;
    let id = jobs
        .refresh(&json!({}))
        .map_err(|error| error.to_string())?;
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
) -> Result<(), String> {
    let action: ItemAction = action.parse().map_err(|_| {
        let names = ItemAction::ALL
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(", ");
        format!("Unknown action {action}. Use one of: {names}.")
    })?;
    let document = repository().load()?;
    let playlist = list_of(&document, "playlists")
        .into_iter()
        .find(|playlist| field(playlist, "playlist_id") == playlist_id)
        .ok_or_else(|| format!("{playlist_id} is not in the watchlist."))?;
    let item = list_of(&playlist, "items")
        .into_iter()
        .find(|item| field(item, "position") == position)
        .ok_or_else(|| format!("{playlist_id} has no item at position {position}."))?;
    let title = field(&item, "title")
        .as_str()
        .unwrap_or("the item")
        .to_owned();
    let params = json!({
        "playlist_id": playlist_id,
        "position": position,
        "video_id": field(&item, "video_id"),
        "title": title,
        "action": action,
    });
    let jobs = jobs::open()?;
    let id = jobs.item(&params).map_err(|error| error.to_string())?;
    println!("Queued {action} for {title} as {}.", job_id(id));
    if queue_only {
        return Ok(());
    }
    jobs::drain(&jobs)
}
