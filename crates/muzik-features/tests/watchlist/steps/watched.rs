use crate::documents::list;
use crate::operations::card;
use crate::support::StepResult;
use crate::world::WatchlistWorld;
use cucumber::given;
use muzik_store::watchlist::{ItemAction, SourceKind, Stage, StageStatus, WatchItem, Watchlist};
use serde_json::json;
use std::path::PathBuf;
use std::str::FromStr;

fn watch_playlist(world: &WatchlistWorld, id: &str) -> StepResult {
    let repository = world.repository();
    if repository.load()?.playlist(id).is_none() {
        repository.add(&format!("https://www.youtube.com/playlist?list={id}"))?;
    }
    Ok(())
}

fn add_items(world: &WatchlistWorld, id: &str, videos: &[(String, String)]) -> StepResult {
    watch_playlist(world, id)?;
    world.repository().update(|document: &mut Watchlist| {
        let playlist = document
            .playlist_mut(id)
            .ok_or("the playlist is not watched")?;
        for (video, title) in videos {
            let position = playlist
                .items
                .iter()
                .map(|item| item.position)
                .max()
                .unwrap_or_default()
                .checked_add(1)
                .ok_or("too many items")?;
            playlist.items.push(card(position, video, title));
        }
        Ok(())
    })?;
    Ok(())
}

fn edit_item(
    world: &WatchlistWorld,
    id: &str,
    video: &str,
    change: impl FnOnce(&mut WatchItem),
) -> StepResult {
    world.repository().update(|document| {
        let item = document
            .playlist_mut(id)
            .and_then(|playlist| {
                playlist
                    .items
                    .iter_mut()
                    .find(|item| item.video_id.as_deref() == Some(video))
            })
            .ok_or("the video is not watched")?;
        change(item);
        Ok(())
    })?;
    Ok(())
}

#[given(expr = "the watched playlist {string} has the video {string} titled {string}")]
fn playlist_has_video(
    world: &mut WatchlistWorld,
    id: String,
    video: String,
    title: String,
) -> StepResult {
    add_items(world, &id, &[(video, title)])
}

#[given(expr = "the watched playlist {string} has the videos {string}")]
fn playlist_has_videos(world: &mut WatchlistWorld, id: String, videos: String) -> StepResult {
    let videos: Vec<_> = list(&videos)
        .into_iter()
        .map(|video| (video.clone(), video))
        .collect();
    add_items(world, &id, &videos)
}

#[given(expr = "the watched playlist {string} has no videos")]
fn playlist_has_no_videos(world: &mut WatchlistWorld, id: String) -> StepResult {
    watch_playlist(world, &id)
}

#[given(expr = "the watchlist has the Spotify source {string} with the track {string}")]
fn spotify_source_with_track(
    world: &mut WatchlistWorld,
    source: String,
    track: String,
) -> StepResult {
    let repository = world.repository();
    let id = repository.add(&source)?.playlist_id;
    repository.update(|document| {
        let mut item = WatchItem::new(1, "Track", SourceKind::Spotify);
        item.video_id = Some(track.clone());
        item.entry_id = Some(track);
        document
            .playlist_mut(&id)
            .ok_or("the source is not watched")?
            .items = vec![item];
        Ok(())
    })?;
    Ok(())
}

#[given(expr = "the video {string} in {string} waits at the {word} stage for an {string} question")]
fn video_waits(
    world: &mut WatchlistWorld,
    video: String,
    id: String,
    stage: String,
    kind: String,
) -> StepResult {
    let stage = Stage::from_str(&stage)?;
    edit_item(world, &id, &video, |item| {
        let record = item.stage_mut(stage);
        record.status = StageStatus::Waiting;
        record.question = Some(json!({"kind": kind, "payload": {"task": {}}}));
    })
}

#[given(expr = "the video {string} in {string} was processed")]
fn video_was_processed(world: &mut WatchlistWorld, video: String, id: String) -> StepResult {
    world.repository().update(|document| {
        document
            .playlist_mut(&id)
            .ok_or("the playlist is not watched")?
            .mark_processed(&video, true);
        Ok(())
    })?;
    Ok(())
}

#[given(expr = "the download of the video {string} in {string} is running")]
fn download_is_running(world: &mut WatchlistWorld, video: String, id: String) -> StepResult {
    edit_item(world, &id, &video, |item| {
        item.set(Stage::Download, StageStatus::Running);
    })
}

#[given(expr = "the download of the video {string} in {string} failed with the error {string}")]
fn download_failed(
    world: &mut WatchlistWorld,
    video: String,
    id: String,
    error: String,
) -> StepResult {
    edit_item(world, &id, &video, |item| {
        item.last_error = Some(error.clone());
        let record = item.stage_mut(Stage::Download);
        record.status = StageStatus::Failed;
        record.error = Some(error);
    })
}

#[given(expr = "the video {string} in {string} was last retried")]
fn video_was_retried(world: &mut WatchlistWorld, video: String, id: String) -> StepResult {
    edit_item(world, &id, &video, |item| {
        item.last_action = Some(ItemAction::Retry.into());
    })
}

#[given(expr = "the video {string} in {string} was downloaded to {string}")]
fn video_was_downloaded(
    world: &mut WatchlistWorld,
    video: String,
    id: String,
    path: String,
) -> StepResult {
    add_items(world, &id, &[(video.clone(), "Song".to_owned())])?;
    edit_item(world, &id, &video, |item| {
        item.complete(Stage::Download, Some(PathBuf::from(path)));
    })
}

#[given(expr = "the source {string} and its items have fields from a newer version of muzik")]
fn newer_fields(world: &mut WatchlistWorld, id: String) -> StepResult {
    world.repository().update(|document| {
        let playlist = document
            .playlist_mut(&id)
            .ok_or("the playlist is not watched")?;
        playlist
            .extra
            .insert("source_extension".into(), json!({"revision": 7}));
        for item in &mut playlist.items {
            item.extra.insert("item_extension".into(), json!(["kept"]));
        }
        Ok(())
    })?;
    Ok(())
}
