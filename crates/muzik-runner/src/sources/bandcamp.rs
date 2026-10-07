use super::{check_cancelled, mark_full, organize, required, safe_name, Source};
use crate::gates::{self, Gate};
use crate::watchlist::Adapter;
use muzik_bandcamp as bandcamp;
use muzik_core::paths::Paths;
use muzik_core::{JobEvent, Task};
use muzik_store::watchlist::jobs::{JobError, LoadedSource};
use muzik_store::watchlist::{
    bandcamp_source, ItemAction, Playlist, Repository, SourceKind, Stage, WatchItem,
};
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};

const LOGIN: &str = "Set your Bandcamp login in Settings first.";
const MEGABYTE: u64 = 1024 * 1024;

pub(super) struct Bandcamp;

pub(super) fn ensure(repository: &Repository, paths: &Paths) -> crate::Result<bool> {
    match bandcamp::Login::load(paths) {
        Some(login) => Ok(repository.ensure(&bandcamp_source(&login.user))?),
        None => Ok(false),
    }
}

impl Source for Bandcamp {
    fn load(&self, adapter: &mut Adapter<'_, '_>, _: &Playlist) -> Result<LoadedSource, JobError> {
        let login = bandcamp::Login::load(&adapter.prepared.settings.paths)
            .ok_or_else(|| JobError::Operation(LOGIN.into()))?;
        let purchases = bandcamp::collection(&login).map_err(String::from)?;
        check_cancelled(adapter.cancelled)?;
        Ok(items(&purchases))
    }

    fn process(
        &self,
        adapter: &mut Adapter<'_, '_>,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        let entry_id = required(item.entry_id.as_deref(), "entry_id")?;
        let directory = adapter
            .prepared
            .settings
            .request
            .output
            .join("bandcamp-watchlist")
            .join(safe_name(entry_id));
        let fresh = matches!(action, ItemAction::DownloadAgain | ItemAction::RunAllAgain);
        let saved = !fresh && !bandcamp::audio_files(&directory).is_empty();
        if action == ItemAction::OrganizeAgain && !saved {
            return Err(JobError::Operation(
                "Download this purchase before you organize it again.".into(),
            ));
        }
        if action != ItemAction::OrganizeAgain && action.stage() != Stage::Download {
            return Err(JobError::Operation(format!(
                "A Bandcamp purchase does not support {action}."
            )));
        }
        if !saved {
            let page = item
                .track
                .as_ref()
                .and_then(|track| track["download_page"].as_str())
                .ok_or_else(|| {
                    JobError::Operation(
                        "The Bandcamp purchase has no download page. Refresh the collection."
                            .into(),
                    )
                })?;
            let login = bandcamp::Login::load(&adapter.prepared.settings.paths)
                .ok_or_else(|| JobError::Operation(LOGIN.into()))?;
            if directory.exists() {
                std::fs::remove_dir_all(&directory).map_err(|error| error.to_string())?;
            }
            let _permit =
                gates::enter(Gate::Download, cancelled).map_err(|_| JobError::Cancelled)?;
            let on_import_event = &mut *adapter.on_import_event;
            let mut reported = None;
            bandcamp::download(
                &login,
                page,
                bandcamp::BandcampFormat::default(),
                &directory,
                cancelled,
                &mut |received, total| {
                    let completed = received / MEGABYTE;
                    let total = total.map(|total| total.div_ceil(MEGABYTE));
                    if reported.is_none() {
                        on_import_event(JobEvent::ProgressStarted {
                            task: Task::BandcampDownload,
                            description: "Downloading from Bandcamp (MB)".into(),
                            total,
                        });
                    } else if reported == Some(completed) {
                        return;
                    }
                    reported = Some(completed);
                    on_import_event(JobEvent::ProgressAdvanced {
                        task: Task::BandcampDownload,
                        completed: Some(completed),
                        total: None,
                    });
                },
            )
            .map_err(|error| {
                if cancelled.load(Ordering::SeqCst) {
                    JobError::Cancelled
                } else {
                    JobError::Failed {
                        stage: Stage::Download,
                        message: error.to_string(),
                    }
                }
            })?;
            on_import_event(JobEvent::ProgressFinished {
                task: Task::BandcampDownload,
                success: true,
            });
        }
        let mut options = adapter.prepared.settings.options.clone();
        options.no_split = true;
        options.interactive = false;
        if action == ItemAction::OrganizeAgain {
            options.no_organize = false;
            options.force = true;
        }
        if !options.no_organize {
            organize(adapter, &directory, &options, cancelled)?;
        }
        let mut updated = item.clone();
        mark_full(&mut updated, &options, false);
        updated.set_path(Stage::Download, Some(directory));
        Ok(updated)
    }
}

fn items(purchases: &[bandcamp::Purchase]) -> LoadedSource {
    let items = purchases
        .iter()
        .zip(1_u64..)
        .map(|(purchase, position)| {
            let mut item = WatchItem::new(position, &purchase.label(), SourceKind::Bandcamp);
            item.video_id = Some(purchase.key.clone());
            item.entry_id = Some(purchase.key.clone());
            item.video_url = Some(
                purchase
                    .item_url
                    .clone()
                    .unwrap_or_else(|| purchase.download_page.clone()),
            );
            item.thumbnail_url = purchase.art_url.clone();
            item.track = Some(json!({
                "artist": purchase.artist,
                "title": purchase.title,
                "single": purchase.single,
                "download_page": purchase.download_page,
            }));
            item
        })
        .collect();
    LoadedSource {
        title: Some("Bandcamp collection".into()),
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::items;
    use muzik_store::watchlist::SourceKind;
    use serde_json::json;

    #[test]
    fn purchases_become_items_with_their_download_page() {
        let loaded = items(&[muzik_bandcamp::Purchase {
            key: "p12".into(),
            artist: "Band".into(),
            title: "Album".into(),
            single: false,
            download_page: "https://bandcamp.com/download?id=12".into(),
            item_url: Some("https://band.bandcamp.com/album/album".into()),
            art_url: None,
        }]);
        let item = &loaded.items[0];
        assert_eq!(item.kind, SourceKind::Bandcamp);
        assert_eq!(item.entry_id.as_deref(), Some("p12"));
        assert_eq!(item.video_id.as_deref(), Some("p12"));
        assert_eq!(item.title, "Band - Album");
        assert_eq!(
            item.video_url.as_deref(),
            Some("https://band.bandcamp.com/album/album")
        );
        assert_eq!(
            item.track.as_ref().map(|track| &track["download_page"]),
            Some(&json!("https://bandcamp.com/download?id=12"))
        );
    }
}
