use super::library_lookup::MusicLibrary;
use super::{AudioIndex, Stage, StageStatus, WatchItem, Watchlist};
use crate::Result;
use muzik_core::QualityPolicy;
use std::path::Path;

/// Refresh saved stage state from the files on disk and the music library.
/// Save the document with `Repository::save` after this call succeeds.
#[derive(Clone, Copy)]
pub struct ReconcileOptions<'a> {
    pub output: &'a Path,
    pub splits: &'a Path,
    pub cache: &'a Path,
    pub config: Option<&'a Path>,
    pub no_organize: bool,
    pub no_split: bool,
    pub quality_policy: QualityPolicy,
}

/// # Errors
/// Returns an error if the updated watchlist is not valid.
pub fn reconcile(document: &mut Watchlist, options: ReconcileOptions<'_>) -> Result<()> {
    let music_library = MusicLibrary::open(options.config);
    let audio = AudioIndex::scan(options.output);
    for playlist in &mut document.playlists {
        let mut processed = playlist.processed_video_ids.clone();
        for item in &mut playlist.items {
            if item.is_waiting() {
                for key in [item.video_id.as_deref(), item.entry_id.as_deref()]
                    .into_iter()
                    .flatten()
                {
                    processed.retain(|processed| processed != key);
                }
                continue;
            }
            for stage in Stage::ALL {
                if item.status(*stage) == StageStatus::Running {
                    item.set(*stage, StageStatus::NotStarted);
                }
            }
            if item.statuses().any(|status| status == StageStatus::Stale) {
                if let Some(key) = item.key() {
                    processed.retain(|processed| processed != key);
                }
                continue;
            }
            if item.kind.single_file() {
                for stage in [Stage::Quality, Stage::Parse, Stage::Split] {
                    if item.status(stage) != StageStatus::Skipped {
                        item.set(stage, StageStatus::Skipped);
                    }
                }
                continue;
            }
            let Some(video_id) = item.video_id.clone().filter(|id| !id.is_empty()) else {
                continue;
            };
            if processed.contains(&video_id) {
                mark_completed(item, options);
                continue;
            }
            if let Some(path) = audio.find(&video_id) {
                if item.status(Stage::Download) != StageStatus::Complete
                    || item
                        .path(Stage::Download)
                        .is_none_or(|saved| !saved.is_file())
                {
                    item.complete(Stage::Download, Some(path));
                }
            } else if let Some(path) = music_library
                .as_ref()
                .and_then(|library| library.find(&video_id, &item.title))
            {
                item.complete(Stage::Download, Some(path));
                item.set(Stage::Parse, StageStatus::Complete);
                item.set(Stage::Split, StageStatus::Skipped);
                item.set(Stage::Organize, StageStatus::Complete);
                processed.push(video_id);
            }
        }
        playlist.processed_video_ids = processed;
    }
    *document = std::mem::take(document).normalized()?;
    Ok(())
}

fn mark_completed(item: &mut WatchItem, options: ReconcileOptions<'_>) {
    let done = |skipped: bool| {
        if skipped {
            StageStatus::Skipped
        } else {
            StageStatus::Complete
        }
    };
    let split = if options.no_split || item.status(Stage::Split) != StageStatus::Complete {
        StageStatus::Skipped
    } else {
        StageStatus::Complete
    };
    let wanted = [
        (Stage::Download, StageStatus::Complete),
        (
            Stage::Quality,
            done(options.quality_policy == QualityPolicy::Off),
        ),
        (Stage::Parse, done(options.no_split)),
        (Stage::Split, split),
        (Stage::Organize, done(options.no_organize)),
    ];
    if wanted
        .iter()
        .all(|(stage, status)| item.status(*stage) == *status)
        && item.last_error.is_none()
    {
        return;
    }
    item.last_action = Some("refresh".into());
    item.last_error = None;
    for (stage, status) in wanted {
        item.set(stage, status);
    }
}
