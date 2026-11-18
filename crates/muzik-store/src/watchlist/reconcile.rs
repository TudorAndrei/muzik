use super::library_lookup::MusicLibrary;
use super::{AudioIndex, LastAction, Stage, StageEvent, StageStatus, WatchItem, Watchlist};
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

impl Watchlist {
    /// # Errors
    /// Returns an error if the updated watchlist is not valid.
    pub fn reconcile(&mut self, options: ReconcileOptions<'_>) -> Result<()> {
        self.reconcile_items(options, false)
    }

    pub(super) fn reconcile_keeping_running(
        &mut self,
        options: ReconcileOptions<'_>,
    ) -> Result<()> {
        self.reconcile_items(options, true)
    }

    fn reconcile_items(&mut self, options: ReconcileOptions<'_>, keep_running: bool) -> Result<()> {
        let music_library = MusicLibrary::open(options.config);
        let audio = AudioIndex::scan(options.output);
        for playlist in &mut self.playlists {
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
                if keep_running && item.statuses().any(|status| status == StageStatus::Running) {
                    continue;
                }
                for stage in Stage::ALL {
                    if item.status(*stage) == StageStatus::Running {
                        item.apply(*stage, StageEvent::Release)?;
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
                            item.apply(stage, StageEvent::Skip)?;
                        }
                    }
                    continue;
                }
                let Some(video_id) = item.video_id.clone().filter(|id| !id.is_empty()) else {
                    continue;
                };
                if processed.contains(&video_id) {
                    mark_completed(item, options)?;
                    continue;
                }
                if let Some(path) = audio.find(&video_id) {
                    if item
                        .path(Stage::Download)
                        .is_none_or(|saved| !saved.is_file())
                    {
                        item.apply(Stage::Download, StageEvent::Complete(Some(path)))?;
                    }
                } else if let Some(path) = music_library
                    .as_ref()
                    .and_then(|library| library.find(&video_id, &item.title))
                {
                    item.apply(Stage::Download, StageEvent::Complete(Some(path)))?;
                    let parse = item.kept_path(Stage::Parse);
                    item.apply(Stage::Parse, StageEvent::Complete(parse))?;
                    item.apply(Stage::Split, StageEvent::Skip)?;
                    item.apply(Stage::Organize, StageEvent::Complete(None))?;
                    processed.push(video_id);
                }
            }
            playlist.processed_video_ids = processed;
        }
        *self = std::mem::take(self).normalized()?;
        Ok(())
    }
}

fn mark_completed(item: &mut WatchItem, options: ReconcileOptions<'_>) -> Result<()> {
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
        return Ok(());
    }
    item.last_action = Some(LastAction::Refresh);
    item.last_error = None;
    for (stage, status) in wanted {
        let event = if status == StageStatus::Skipped {
            StageEvent::Skip
        } else {
            StageEvent::Complete(item.kept_path(stage))
        };
        item.apply(stage, event)?;
    }
    Ok(())
}
