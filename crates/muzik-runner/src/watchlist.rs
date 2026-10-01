//! Watchlist jobs on the queue. Each source kind is one module in `sources`.

use crate::gates;
use crate::settings::Settings;
use crate::sources;
use muzik_core::watchlist::jobs::{
    self, JobError, JobOptions, LoadedSource, Operations, PendingItem,
};
use muzik_core::watchlist::{AudioIndex, ItemAction, ItemId, Playlist, Repository, WatchItem};
use muzik_core::DecisionKind;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn sync(
    settings: &Settings,
    playlist_id: Option<&str>,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
) -> Result<Vec<PendingItem>, JobError> {
    let prepared = Prepared::new(settings);
    sources::ensure(&prepared.repository)?;
    let events = RefCell::new(on_event);
    let parked = RefCell::new(None);
    let mut adapter = Adapter {
        prepared: &prepared,
        events: &events,
        on_import_event: &mut |_| {},
        decide: &mut |_, _| Err("A playlist check does not ask for choices.".into()),
        parked: &parked,
        cancelled,
    };
    let mut options = prepared.job_options();
    options.playlist_id = playlist_id.filter(|id| !id.is_empty());
    let synced = jobs::sync(
        &prepared.repository,
        options,
        &mut adapter,
        cancelled,
        &mut |record| {
            (events.borrow_mut())(record);
        },
    )?;
    (events.borrow_mut())(
        json!({"event":"progress_finished","data":{"task_id":"watchlist-refresh","success":synced.errors == 0}}),
    );
    Ok(synced.pending)
}

pub fn ensure_sources(repository: &Repository) -> Result<bool, String> {
    sources::ensure(repository)
}

pub fn action(
    settings: &Settings,
    params: &Value,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
    on_import_event: &mut dyn FnMut(Value),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    parked: &RefCell<Option<Parked>>,
) -> Result<Value, JobError> {
    let prepared = Prepared::new(settings);
    let id = ItemId::from_params(params)?;
    let name = params["action"]
        .as_str()
        .ok_or_else(|| JobError::Operation("action must be a non-empty string".into()))?;
    let name: ItemAction = name
        .parse()
        .map_err(|_| JobError::Operation(format!("Unknown item action: {name}")))?;
    let events = RefCell::new(on_event);
    let mut adapter = Adapter {
        prepared: &prepared,
        events: &events,
        on_import_event,
        decide,
        parked,
        cancelled,
    };
    jobs::action(
        &prepared.repository,
        prepared.job_options(),
        &id,
        name,
        &mut adapter,
        cancelled,
    )
}

pub(crate) struct Prepared<'a> {
    pub(crate) settings: &'a Settings,
    repository: Repository,
}

pub struct Parked {
    pub kind: DecisionKind,
    pub payload: Value,
}

impl Parked {
    fn question(&self) -> Value {
        json!({"kind":self.kind,"payload":self.payload})
    }
}

impl<'a> Prepared<'a> {
    pub(crate) fn new(settings: &'a Settings) -> Self {
        Self {
            settings,
            repository: Repository::open(&settings.paths),
        }
    }

    fn job_options(&self) -> JobOptions<'_> {
        JobOptions {
            reconcile: self.settings.reconcile(),
            output: &self.settings.request.output,
            cache: &self.settings.paths.cache,
            dry_run: self.settings.options.dry_run,
            playlist_id: None,
        }
    }

    pub(crate) fn audio(&self, item: &WatchItem) -> Option<PathBuf> {
        item.downloaded_audio(&AudioIndex::scan(&self.settings.request.output))
    }
}

pub(crate) struct Adapter<'a, 'b> {
    pub(crate) prepared: &'a Prepared<'a>,
    pub(crate) events: &'a RefCell<&'b mut dyn FnMut(Value)>,
    pub(crate) on_import_event: &'a mut dyn FnMut(Value),
    pub(crate) decide: &'a mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    pub(crate) parked: &'a RefCell<Option<Parked>>,
    pub(crate) cancelled: &'a AtomicBool,
}

impl Operations for Adapter<'_, '_> {
    fn load(&mut self, playlist: &Playlist) -> Result<LoadedSource, JobError> {
        check_cancelled(self.cancelled)?;
        sources::of(playlist.kind).load(self, playlist)
    }

    fn process(
        &mut self,
        playlist: &Playlist,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        check_cancelled(cancelled)?;
        self.parked.replace(None);
        gates::take_stage();
        let result = sources::of(item.kind).process(self, item, action, cancelled);
        let stage = gates::take_stage();
        result.map_err(|error| {
            let Some(parked) = self.parked.replace(None) else {
                return match (error, stage) {
                    (JobError::Operation(message), Some(stage)) => {
                        JobError::Failed { stage, message }
                    }
                    (error, _) => error,
                };
            };
            let stage = parked.kind.stage();
            let question = parked.question();
            (self.events.borrow_mut())(json!({"event":"item_waiting","data":{
                "playlist_id":playlist.playlist_id,
                "position":item.position,
                "video_id":item.video_id.as_deref().or(item.entry_id.as_deref()),
                "title":item.title,
                "stage":stage,
                "question":question,
            }}));
            JobError::Waiting { stage, question }
        })
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), JobError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}
