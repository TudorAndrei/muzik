use super::{
    at, cancel_or, check_cancelled, mark_full, required, safe_name, workflow_error, youtube, Source,
};
use crate::watchlist::Adapter;
use crate::{local_workflow, remote_workflow};
use muzik_core::watchlist::jobs::{JobError, LoadedSource};
use muzik_core::watchlist::{ItemAction, Playlist, SourceKind, Stage, WatchItem};
use muzik_spotify as spotify;
use muzik_workflow::playlist::{write_spotify_tags, SpotifyTags};
use muzik_workflow::process_audio_plan_with_events;
use serde_json::Value;
use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

pub(super) struct Spotify;

impl Source for Spotify {
    fn load(
        &self,
        adapter: &mut Adapter<'_, '_>,
        playlist: &Playlist,
    ) -> Result<LoadedSource, JobError> {
        let document = spotify::load_playlist_document(
            &adapter.prepared.settings.paths.config_file(),
            &adapter.prepared.settings.paths.spotify_token(),
            &playlist.playlist_id,
        )?;
        check_cancelled(adapter.cancelled)?;
        items(&document)
    }

    fn process(
        &self,
        adapter: &mut Adapter<'_, '_>,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        if action == ItemAction::OrganizeAgain {
            return match adapter.prepared.audio(item) {
                Some(file) => import_file(adapter, item, file, cancelled),
                None => youtube::local_stage(adapter, item, action, cancelled),
            };
        }
        if action.stage() != Stage::Download {
            return Err(JobError::Operation(format!(
                "A Spotify track does not support {action}."
            )));
        }
        let fresh = matches!(action, ItemAction::DownloadAgain | ItemAction::RunAllAgain);
        if !fresh {
            if let Some(file) = item.path(Stage::Download).filter(|path| path.is_file()) {
                return import_file(adapter, item, file.to_path_buf(), cancelled);
            }
        }
        let track = item
            .track
            .as_ref()
            .and_then(Value::as_object)
            .ok_or_else(|| {
                JobError::Operation("The Spotify track has no saved metadata.".into())
            })?;
        let title = track
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| JobError::Operation("Spotify track title is missing".into()))?;
        let artists = track
            .get("artists")
            .and_then(Value::as_array)
            .map(|artists| {
                artists
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let query = format!("{artists} - {title}");
        let entry_id = required(item.entry_id.as_deref(), "entry_id")?;
        let settings = adapter.prepared.settings;
        let root = settings
            .request
            .output
            .join("spotify-watchlist")
            .join(safe_name(entry_id));
        let files = remote_workflow::soulseek_download(
            &settings.paths,
            &query,
            &settings.options.prefer,
            false,
            cancelled,
            adapter.decide,
            true,
            Some(&root),
        )
        .map_err(|error| cancel_or(cancelled, error))?;
        let file = files
            .into_iter()
            .next()
            .ok_or_else(|| JobError::Operation("Soulseek returned no audio file.".into()))?;
        import_file(adapter, item, file, cancelled)
    }
}

fn import_file(
    adapter: &mut Adapter<'_, '_>,
    item: &WatchItem,
    file: PathBuf,
    cancelled: &AtomicBool,
) -> Result<WatchItem, JobError> {
    let track = item.track.clone().unwrap_or(Value::Null);
    write_spotify_tags(&file, &tags(&track)).map_err(JobError::Operation)?;
    let mut options = adapter.prepared.settings.options.clone();
    options.no_split = true;
    options.interactive = false;
    let events = adapter.events;
    let stage = Cell::new(Stage::Organize);
    let mut local = local_workflow::LocalOperations {
        decide: adapter.decide,
        on_import_event: adapter.on_import_event,
        cancelled,
        stage: &stage,
    };
    process_audio_plan_with_events(
        std::slice::from_ref(&file),
        &[],
        &adapter.prepared.settings.request.splits,
        &options,
        &mut local,
        cancelled,
        &mut |event| (events.borrow_mut())(local_workflow::event_record(event)),
    )
    .map_err(|error| at(stage.get(), workflow_error(error)))?;
    let mut updated = item.clone();
    mark_full(&mut updated, &options, false);
    if file.is_file() {
        updated.set_path(Stage::Download, Some(file));
    }
    Ok(updated)
}

fn tags(track: &Value) -> SpotifyTags {
    SpotifyTags {
        title: track["title"].as_str().unwrap_or("").to_owned(),
        artists: track["artists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        album: track["album"].as_str().map(str::to_owned),
        track: track["track_number"].as_u64(),
        disc: track["disc_number"].as_u64(),
        date: track["release_date"].as_str().map(str::to_owned),
    }
}

fn items(document: &Value) -> Result<LoadedSource, JobError> {
    let entries = document["entries"]
        .as_array()
        .ok_or_else(|| JobError::Operation("Spotify source has no entries.".into()))?;
    let mut occurrences = HashMap::<String, usize>::new();
    let mut items = Vec::new();
    for (index, track) in entries.iter().enumerate() {
        let source = track["source_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("spotify:{}", index + 1));
        let occurrence = occurrences.entry(source.clone()).or_default();
        let entry_id = format!("{source}#{occurrence}");
        *occurrence += 1;
        let artists = track["artists"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let title = track["title"].as_str().unwrap_or("Unknown track");
        let label = if artists.is_empty() {
            title.to_owned()
        } else {
            format!("{artists} - {title}")
        };
        let mut item = WatchItem::new(index as u64 + 1, &label, SourceKind::Spotify);
        item.video_id = Some(source.rsplit(':').next().unwrap_or("").to_owned());
        item.video_url = track["source_url"].as_str().map(str::to_owned);
        item.thumbnail_url = track["source_metadata"]["image"]
            .as_str()
            .map(str::to_owned);
        item.entry_id = Some(entry_id);
        item.track = Some(track.clone());
        items.push(item);
    }
    Ok(LoadedSource {
        title: document["title"].as_str().map(str::to_owned),
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::items;
    use crate::sources::of;
    use crate::sources::testing::{fixture, library_config, settings, with_adapter};
    use muzik_core::watchlist::{ItemAction, SourceKind, Stage, StageStatus, WatchItem};
    use serde_json::json;
    use std::fs;

    #[test]
    fn organize_again_imports_saved_audio() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let audio = directory.path().join("track.flac");
        fs::copy(fixture(), &audio)?;
        let config = library_config(directory.path())?;
        let settings = settings(
            directory.path(),
            &json!({"output":directory.path(),"config":config,"interactive":true,"quality_policy":"off"}),
        )?;
        let mut item = WatchItem::new(1, "Warhaus - Love's a Stranger", SourceKind::Spotify);
        item.set_path(Stage::Download, Some(audio));
        item.track = Some(json!({
            "title":"Love's a Stranger","artists":["Warhaus"],"album":"Warhaus",
            "track_number":2,"disc_number":1,"release_date":"2017-10-13"
        }));
        let result = with_adapter(&settings, |adapter, cancelled| {
            of(SourceKind::Spotify).process(adapter, &item, ItemAction::OrganizeAgain, cancelled)
        })?;
        assert_eq!(result.status(Stage::Organize), StageStatus::Complete);
        let imported =
            muzik_library::Library::open_read_only(&directory.path().join("library.db"))?
                .items()?;
        assert_eq!(imported.len(), 1);
        let text = |name: &str| match imported[0].field(name) {
            Some(muzik_library::SqlValue::Text(text)) => text.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(text("album"), "Warhaus");
        assert_eq!(text("title"), "Love's a Stranger");
        assert_eq!(text("albumartist"), "Warhaus");
        Ok(())
    }

    #[test]
    fn repeated_tracks_keep_separate_state_keys() -> Result<(), Box<dyn std::error::Error>> {
        let loaded = items(&json!({"title":"Album","entries":[
            {"title":"One","artists":["Alex"],"source_id":"spotify:track:t1","source_url":"https://open.spotify.com/track/t1"},
            {"title":"One","artists":["Alex"],"source_id":"spotify:track:t1","source_url":"https://open.spotify.com/track/t1"}
        ]}))?;
        assert_eq!(
            loaded.items[0].entry_id.as_deref(),
            Some("spotify:track:t1#0")
        );
        assert_eq!(
            loaded.items[1].entry_id.as_deref(),
            Some("spotify:track:t1#1")
        );
        assert_eq!(loaded.items[0].title, "Alex - One");
        Ok(())
    }
}
