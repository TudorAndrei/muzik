//! Watchlist jobs on the queue. Each source kind is one module in `sources`.

use crate::settings::Settings;
use crate::sources;
use muzik_core::{DecisionKind, JobEvent, Task};
use muzik_store::jobs::{Kind, NewJob, park_on};
use muzik_store::watchlist::jobs::{
    self, JobError, JobOptions, LoadedSource, Operations, PendingItem,
};
use muzik_store::watchlist::{
    AudioIndex, ItemAction, ItemId, Playlist, Repository, Stage, WatchItem, import_cache,
};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub fn sync(
    settings: &Settings,
    playlist_id: Option<&str>,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<Vec<PendingItem>, JobError> {
    let prepared = Prepared::new(settings);
    sources::ensure(&prepared.repository, &settings.paths)?;
    import_cache(&prepared.repository, settings.reconcile())?;
    let events = RefCell::new(on_event);
    let parked = RefCell::new(None);
    let mut adapter = Adapter {
        prepared: &prepared,
        params: &Value::Null,
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
    (events.borrow_mut())(JobEvent::ProgressFinished {
        task: Task::WatchlistRefresh,
        success: synced.errors == 0,
    });
    Ok(synced.pending)
}

pub fn ensure_sources(paths: &muzik_core::paths::Paths) -> crate::Result<bool> {
    sources::ensure(&Repository::open(paths), paths)
}

pub fn action(
    settings: &Settings,
    params: &Value,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
    on_import_event: &mut dyn FnMut(JobEvent),
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
        params,
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
    pub(crate) params: &'a Value,
    pub(crate) events: &'a RefCell<&'b mut dyn FnMut(JobEvent)>,
    pub(crate) on_import_event: &'a mut dyn FnMut(JobEvent),
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
        _playlist: &Playlist,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        check_cancelled(cancelled)?;
        self.parked.replace(None);
        let result = sources::of(item.kind).process(self, item, action, cancelled);
        result.map_err(|error| {
            let Some(parked) = self.parked.replace(None) else {
                return error;
            };
            let stage = Stage::of_decision(parked.kind);
            let question = parked.question();
            (self.events.borrow_mut())(JobEvent::ItemWaiting {
                title: item.title.clone(),
                question: question.clone(),
            });
            JobError::Waiting { stage, question }
        })
    }

    fn park(
        &mut self,
        connection: &Connection,
        id: &ItemId,
        title: &str,
        stage: Stage,
        question: &Value,
    ) -> muzik_store::Result<()> {
        let mut params = self.params.clone();
        id.write(&mut params);
        if let Some(fields) = params.as_object_mut() {
            fields.insert("action".into(), json!(stage.resume_action()));
        }
        park_on(
            connection,
            &NewJob {
                kind: Kind::Item,
                item_key: &id.to_string(),
                title,
                params: &params,
            },
            question,
        )
        .map(drop)
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), JobError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}
