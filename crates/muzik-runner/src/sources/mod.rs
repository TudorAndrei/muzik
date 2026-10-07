//! One module for each watchlist source kind.

use crate::local_workflow;
use crate::watchlist::Adapter;
use muzik_core::paths::Paths;
use muzik_core::{AudioSource, QualityPolicy};
use muzik_store::watchlist::jobs::{JobError, LoadedSource};
use muzik_store::watchlist::{
    ItemAction, Playlist, Repository, SourceKind, Stage, StageStatus, WatchItem,
};
use muzik_workflow::{WorkflowOperations, WorkflowOptions};
use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

mod bandcamp;
mod spotify;
mod youtube;

pub trait Source {
    fn load(
        &self,
        adapter: &mut Adapter<'_, '_>,
        playlist: &Playlist,
    ) -> Result<LoadedSource, JobError>;

    fn process(
        &self,
        adapter: &mut Adapter<'_, '_>,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError>;
}

pub fn of(kind: SourceKind) -> &'static dyn Source {
    match kind {
        SourceKind::Youtube => &youtube::Youtube,
        SourceKind::Spotify => &spotify::Spotify,
        SourceKind::Bandcamp => &bandcamp::Bandcamp,
    }
}

pub fn ensure(repository: &Repository, paths: &Paths) -> crate::Result<bool> {
    bandcamp::ensure(repository, paths)
}

fn organize(
    adapter: &mut Adapter<'_, '_>,
    target: &Path,
    options: &WorkflowOptions,
    cancelled: &AtomicBool,
) -> Result<(), JobError> {
    let stage = Cell::new(Stage::Organize);
    let mut local = local_workflow::LocalOperations {
        decide: adapter.decide,
        on_import_event: adapter.on_import_event,
        cancelled,
        stage: &stage,
    };
    local
        .organize(target, options)
        .map_err(|error| at(Stage::Organize, JobError::Operation(error)))?;
    check_cancelled(cancelled)
}

fn at(stage: Stage, error: JobError) -> JobError {
    match error {
        JobError::Operation(message) => JobError::Failed { stage, message },
        other => other,
    }
}

fn mark_full(item: &mut WatchItem, options: &WorkflowOptions, split: bool) {
    let single_file = item.kind.single_file();
    for stage in Stage::ALL.iter().copied() {
        let skipped = match stage {
            Stage::Download => false,
            Stage::Quality => {
                single_file
                    || options.quality_policy == QualityPolicy::Off
                    || options.audio_source == AudioSource::Soulseek
            }
            Stage::Parse | Stage::Split => single_file || !split,
            Stage::Organize => options.no_organize,
        };
        item.set(
            stage,
            if skipped {
                StageStatus::Skipped
            } else {
                StageStatus::Complete
            },
        );
    }
}

fn required<'a>(value: Option<&'a str>, key: &str) -> Result<&'a str, JobError> {
    value
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| JobError::Operation(format!("{key} must be a non-empty string")))
}

fn safe_name(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn workflow_error(error: muzik_workflow::Error) -> JobError {
    match error {
        muzik_workflow::Error::Cancelled => JobError::Cancelled,
        other => JobError::Operation(other.to_string()),
    }
}

fn cancel_or(cancelled: &AtomicBool, error: impl std::fmt::Display) -> JobError {
    if cancelled.load(Ordering::SeqCst) {
        JobError::Cancelled
    } else {
        JobError::Operation(error.to_string())
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), JobError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
pub mod testing {
    use crate::settings::Settings;
    use crate::watchlist::{Adapter, Prepared};
    use muzik_core::paths::Paths;
    use muzik_core::{DecisionKind, JobEvent};
    use serde_json::Value;
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    pub fn library_config(directory: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let config = directory.join("config.yaml");
        std::fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                directory.join("music").display(),
                directory.join("library.db").display(),
                directory.join("state").display()
            ),
        )?;
        Ok(config)
    }

    pub fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/muzik-tags/tests/fixtures/blank.flac")
    }

    pub fn settings(
        directory: &Path,
        params: &Value,
    ) -> Result<Settings, Box<dyn std::error::Error>> {
        Ok(Settings::parse(&Paths::under(directory), params)?)
    }

    pub fn with_adapter<T>(
        settings: &Settings,
        work: impl FnOnce(&mut Adapter<'_, '_>, &AtomicBool) -> T,
    ) -> T {
        let prepared = Prepared::new(settings);
        let mut event = |_| {};
        let events: RefCell<&mut dyn FnMut(JobEvent)> = RefCell::new(&mut event);
        let mut imported = |_| {};
        let mut decide = |_: DecisionKind, _: Value| Err("unexpected decision".into());
        let cancelled = AtomicBool::new(false);
        let parked = RefCell::new(None);
        let mut adapter = Adapter {
            prepared: &prepared,
            params: &Value::Null,
            events: &events,
            on_import_event: &mut imported,
            decide: &mut decide,
            parked: &parked,
            cancelled: &cancelled,
        };
        work(&mut adapter, &cancelled)
    }
}
