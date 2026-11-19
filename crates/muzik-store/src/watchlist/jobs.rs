//! Durable watchlist jobs. Callers supply source and audio operations.

use super::source::availability;
use super::{
    AudioIndex, Holds, ItemAction, ItemId, Playlist, Readable, ReconcileOptions, Repository,
    Snapshot, Stage, WatchItem, Watchlist, import_legacy, now, read_readable, view,
};
use crate::Result;
use crate::jobs::{self, Ending, Kind, NewJob, Status};
use muzik_core::{DecisionKind, JobEvent, MatchAnswer, Severity, Task};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("watchlist job cancelled")]
    Cancelled,
    #[error("{0}")]
    Operation(String),
    #[error("waiting for a choice in the {stage} stage")]
    Waiting { stage: Stage, question: Value },
    #[error("{message}")]
    Failed { stage: Stage, message: String },
}

impl From<String> for JobError {
    fn from(value: String) -> Self {
        Self::Operation(value)
    }
}

impl From<crate::Error> for JobError {
    fn from(error: crate::Error) -> Self {
        Self::Operation(error.to_string())
    }
}

pub struct LoadedSource {
    pub title: Option<String>,
    pub items: Vec<WatchItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingItem {
    pub id: ItemId,
    pub title: String,
}

pub struct Synced {
    pub checked: usize,
    pub errors: usize,
    pub pending: Vec<PendingItem>,
    pub document: Watchlist,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemOutcome {
    Completed { stage: Stage },
    Waiting { stage: Stage, question: Value },
}

/// Implement source lookup and item processing with Rust adapters.
/// `process` records each stage on `item` when the stage ends.
pub trait Operations {
    /// # Errors
    /// Returns an error if the source cannot be read, or `JobError::Cancelled` after a cancel.
    fn load(&mut self, playlist: &Playlist) -> Result<LoadedSource, JobError>;
    /// # Errors
    /// Returns an error if a stage fails, waits for a choice, or is cancelled.
    fn process(
        &mut self,
        playlist: &Playlist,
        item: &mut WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<(), JobError>;
}

/// The running queue job that runs an item. Its status changes with the item result.
#[derive(Clone, Copy)]
pub struct RunningJob<'a> {
    pub id: i64,
    pub params: &'a Value,
}

#[derive(Clone, Copy)]
pub struct JobOptions<'a> {
    pub reconcile: ReconcileOptions<'a>,
    pub output: &'a Path,
    pub cache: &'a Path,
    pub dry_run: bool,
    pub playlist_id: Option<&'a str>,
    pub job: Option<RunningJob<'a>>,
}

/// # Errors
/// Returns an error if the watchlist has no matching source, cannot be saved, or the job is cancelled.
pub fn sync(
    repository: &Repository,
    options: JobOptions<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<Synced, JobError> {
    let mut draft = repository.snapshot()?;
    let mut ids: Vec<String> = draft
        .document
        .playlists
        .iter()
        .map(|playlist| playlist.playlist_id.clone())
        .collect();
    if ids.is_empty() {
        return Err(JobError::Operation(
            "Add a playlist before you refresh.".into(),
        ));
    }
    if let Some(only) = options.playlist_id {
        ids.retain(|id| id == only);
        if ids.is_empty() {
            return Err(JobError::Operation(format!(
                "The source {only} is not in the watchlist."
            )));
        }
    }
    on_event(JobEvent::ProgressStarted {
        task: Task::WatchlistRefresh,
        description: "Checking watchlist playlists.".into(),
        total: u64::try_from(ids.len()).ok(),
    });
    let mut errors: usize = 0;
    let mut loaded_ids = Vec::new();
    for id in &ids {
        check_cancelled(cancelled)?;
        let Some(playlist) = draft.document.playlist(id) else {
            continue;
        };
        match operations.load(playlist) {
            Err(JobError::Cancelled) => return Err(JobError::Cancelled),
            Err(error) => {
                let message = error.to_string();
                write(repository, options.dry_run, &mut draft, |document, _| {
                    if let Some(saved) = document.playlist_mut(id) {
                        saved.last_checked_at = Some(now());
                        saved.last_error = Some(message.clone());
                    }
                    Ok(())
                })?;
                on_event(JobEvent::WatchlistSaved);
                errors = errors.saturating_add(1);
                on_event(JobEvent::Message {
                    message,
                    severity: Severity::Error,
                });
            }
            Ok(loaded) => {
                check_cancelled(cancelled)?;
                write(
                    repository,
                    options.dry_run,
                    &mut draft,
                    |document, holds| {
                        if let Some(saved) = document.playlist_mut(id) {
                            if let Some(title) =
                                loaded.title.filter(|title| !title.trim().is_empty())
                            {
                                saved.title = Some(title);
                            }
                            saved.merge_items(loaded.items);
                            saved.last_checked_at = Some(now());
                            saved.last_error = None;
                        }
                        document.reconcile(options.reconcile, holds)
                    },
                )?;
                on_event(JobEvent::WatchlistSaved);
                loaded_ids.push(id.clone());
            }
        }
        on_event(JobEvent::ProgressAdvanced {
            task: Task::WatchlistRefresh,
            completed: None,
            total: None,
        });
    }
    let mut pending = Vec::new();
    for id in loaded_ids {
        let items = pending_of(&draft, &id);
        on_event(JobEvent::message(format!(
            "Playlist {id} has {} pending item(s).",
            items.len()
        )));
        pending.extend(items);
    }
    Ok(Synced {
        checked: ids.len(),
        errors,
        pending,
        document: draft.document,
    })
}

fn pending_of(snapshot: &Snapshot, playlist_id: &str) -> Vec<PendingItem> {
    snapshot
        .document
        .playlist(playlist_id)
        .map(|playlist| {
            playlist
                .pending(&snapshot.holds)
                .into_iter()
                .map(|(id, item)| PendingItem {
                    id,
                    title: item.title.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// # Errors
/// Returns an error if the sync fails, the watchlist cannot be read, or the job is cancelled.
pub fn refresh(
    repository: &Repository,
    options: JobOptions<'_>,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<Value, JobError> {
    let synced = sync(repository, options, operations, cancelled, on_event)?;
    let mut completed: usize = 0;
    let mut failed: usize = 0;
    let mut waiting: usize = 0;
    if !options.dry_run {
        let options = JobOptions {
            job: None,
            ..options
        };
        for item in &synced.pending {
            check_cancelled(cancelled)?;
            match run_item(
                repository,
                options,
                &item.id,
                ItemAction::Run,
                operations,
                cancelled,
            ) {
                Ok(ItemOutcome::Completed { .. }) => completed = completed.saturating_add(1),
                Ok(ItemOutcome::Waiting { .. }) => waiting = waiting.saturating_add(1),
                Err(JobError::Cancelled) => return Err(JobError::Cancelled),
                Err(_) => failed = failed.saturating_add(1),
            }
            on_event(JobEvent::WatchlistSaved);
        }
    }
    check_cancelled(cancelled)?;
    on_event(JobEvent::ProgressFinished {
        task: Task::WatchlistRefresh,
        success: failed == 0 && synced.errors == 0,
    });
    let snapshot = if options.dry_run {
        Snapshot::new(synced.document, repository.snapshot()?.holds)
    } else {
        repository.snapshot()?
    };
    let summary = json!({"playlists_checked":synced.checked, "pending_videos":synced.pending.len(), "completed_videos":completed, "failed_videos":failed, "waiting_videos":waiting, "playlist_errors":synced.errors});
    Ok(json!({"summary":summary, "watchlist":view(&snapshot, options.output, options.cache)}))
}

/// # Errors
/// Returns an error if the item is missing or unavailable, its stage fails, or the job is cancelled.
pub fn action(
    repository: &Repository,
    options: JobOptions<'_>,
    id: &ItemId,
    action: ItemAction,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
) -> Result<Value, JobError> {
    if options.dry_run {
        check_cancelled(cancelled)?;
        let snapshot = repository.snapshot()?;
        let item = find_item(&snapshot.document, id)?;
        check_available(item, action, options.output)?;
        return Ok(
            json!({"action":{"action":action,"planned_stage":action.stage(),"dry_run":true},
            "watchlist":view(&snapshot, options.output, options.cache)}),
        );
    }
    let outcome = run_item(repository, options, id, action, operations, cancelled)?;
    let summary = match outcome {
        ItemOutcome::Completed { stage } => json!({"action":action,"completed_stage":stage}),
        ItemOutcome::Waiting { stage, .. } => json!({"action":action,"waiting_stage":stage}),
    };
    Ok(
        json!({"action":summary, "watchlist":view(&repository.snapshot()?, options.output, options.cache)}),
    )
}

/// Runs `action` on one item. The item result and the status of `options.job` are saved
/// in one transaction.
/// # Errors
/// Returns an error if the item is missing or unavailable, its stage fails, or the job is cancelled.
pub fn run_item(
    repository: &Repository,
    options: JobOptions<'_>,
    id: &ItemId,
    action: ItemAction,
    operations: &mut impl Operations,
    cancelled: &AtomicBool,
) -> Result<ItemOutcome, JobError> {
    let stage = action.stage();
    let key = id.to_string();
    let job_id = options.job.map(|job| job.id);
    let (playlist, mut run) = repository.update_with(|document, connection| {
        if cancelled.load(Ordering::SeqCst) {
            end_job(connection, options, Ending::Cancelled)?;
            return Ok(Err(JobError::Cancelled));
        }
        jobs::cancel_waiting_on(connection, &key, job_id)?;
        let (playlist, index) = match find_item_mut(document, id) {
            Ok(found) => found,
            Err(error) => {
                end_job(connection, options, Ending::Failed(&error.to_string()))?;
                return Ok(Err(error.into()));
            }
        };
        let source = playlist.clone();
        let saved = playlist.items.get(index).ok_or(MISSING_VIDEO)?.clone();
        if let Err(error) = check_available(&saved, action, options.output) {
            let message = error.to_string();
            let mut failed = saved;
            failed.fail_before_start(stage, action, &message);
            playlist.record_copies(id, &failed);
            end_job(connection, options, Ending::Failed(&message))?;
            return Ok(Err(error.into()));
        }
        Ok(Ok((source, saved)))
    })??;
    let result = operations.process(&playlist, &mut run, action, cancelled);
    let result = match result {
        Ok(()) if cancelled.load(Ordering::SeqCst) => Err(JobError::Cancelled),
        other => other,
    };
    let saved = repository.update_with(|document, connection| {
        let Ok((playlist, index)) = find_item_mut(document, id) else {
            end_job(connection, options, Ending::Failed(GONE_VIDEO))?;
            return Ok(false);
        };
        match &result {
            Ok(()) => {
                run.record_action(action);
                if let Some(key) = run.key() {
                    playlist.mark_processed(key, run.is_done());
                }
                end_job(connection, options, Ending::Done)?;
            }
            Err(JobError::Waiting {
                stage: waiting,
                question,
            }) => {
                run.record_action(action);
                let title = &playlist.items.get(index).ok_or(MISSING_VIDEO)?.title;
                park(connection, options, id, title, *waiting, question)?;
                end_job(connection, options, Ending::Done)?;
            }
            Err(JobError::Cancelled) => {
                end_job(connection, options, Ending::Cancelled)?;
            }
            Err(error) => {
                let failed = match error {
                    JobError::Failed { stage, .. } => *stage,
                    _ => stage,
                };
                let message = error.to_string();
                run.fail(failed, action, &message);
                end_job(connection, options, Ending::Failed(&message))?;
            }
        }
        playlist.record_copies(id, &run);
        Ok(true)
    })?;
    match result {
        Err(JobError::Cancelled) => Err(JobError::Cancelled),
        _ if !saved => Err(JobError::Operation(GONE_VIDEO.into())),
        Ok(()) => Ok(ItemOutcome::Completed { stage }),
        Err(JobError::Waiting { stage, question }) => Ok(ItemOutcome::Waiting { stage, question }),
        Err(error) => Err(error),
    }
}

fn end_job(connection: &Connection, options: JobOptions<'_>, ending: Ending<'_>) -> Result<()> {
    if let Some(job) = options.job {
        jobs::end_on(connection, job.id, ending)?;
    }
    Ok(())
}

fn park(
    connection: &Connection,
    options: JobOptions<'_>,
    id: &ItemId,
    title: &str,
    stage: Stage,
    question: &Value,
) -> Result<()> {
    let mut params = options.job.map_or(Value::Null, |job| job.params.clone());
    id.write(&mut params)?;
    if let Some(fields) = params.as_object_mut() {
        fields.insert("action".into(), json!(stage.resume_action()));
    }
    jobs::park_on(
        connection,
        &NewJob {
            kind: Kind::Item,
            item_key: &id.to_string(),
            title,
            params: &params,
        },
        question,
    )?;
    Ok(())
}

const MISSING_PLAYLIST: &str = "The selected playlist is no longer available.";
const MISSING_VIDEO: &str = "The selected video is no longer available.";
const GONE_VIDEO: &str =
    "The video left its playlist while it was processed. Muzik could not save the result.";

/// Fails a running item job that ends before `run_item` starts, and saves the failure on
/// the stage of `action` in the same transaction unless that stage is done.
/// # Errors
/// Returns an error if the watchlist or the job cannot be saved.
pub fn abandon(
    repository: &Repository,
    job_id: i64,
    id: &ItemId,
    action: ItemAction,
    message: &str,
) -> Result<()> {
    repository.update_with(|document, connection| {
        if jobs::end_on(connection, job_id, Ending::Failed(message))?
            && let Ok((playlist, index)) = find_item_mut(document, id)
            && let Some(item) = playlist.items.get(index)
        {
            let mut failed = item.clone();
            failed.fail_before_start(action.stage(), action, message);
            playlist.record_copies(id, &failed);
        }
        Ok(())
    })
}

/// Imports `watchlist` (watchlist.json) when muzik.db has no watchlist yet, and then the open
/// jobs of `jobs_db` (jobs.db), so that each old item job finds its item.
/// # Errors
/// Returns an error only if muzik.db or jobs.db cannot be read or written.
pub fn import_legacy_files(
    store: &mut jobs::Store,
    watchlist: &Path,
    jobs_db: &Path,
) -> Result<usize> {
    Repository::locked(|| {
        if let Err(error) = import_legacy(store.connection_mut(), watchlist) {
            tracing::warn!(path = %watchlist.display(), %error, "watchlist.json was not imported");
        }
        store.import_legacy(jobs_db)
    })
}

/// Brings the jobs back in line with the items after a stop.
///
/// Running jobs return to the queue, a job that cannot be read or does not name an item and an
/// action fails, each item keeps one open job, jobs of missing items end, a waiting job
/// whose input is gone ends so that its item can run again, and a waiting import question
/// of a source that keeps its tags goes back to the queue with the current tags.
/// Items of missing playlists are removed. Rows that cannot be read stay as they are,
/// with the jobs of their playlist. `output_of` gives the download folder for the
/// parameters of a job.
/// # Errors
/// Returns an error only if muzik.db cannot be read or written.
pub fn repair(
    store: &mut jobs::Store,
    output_of: impl Fn(&Value) -> Option<PathBuf>,
) -> rusqlite::Result<usize> {
    Repository::locked(|| {
        let connection = store.connection_mut();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = repair_on(&transaction, &output_of)?;
        transaction.commit()?;
        Ok(changed)
    })
}

fn repair_on(
    connection: &Connection,
    output_of: &impl Fn(&Value) -> Option<PathBuf>,
) -> rusqlite::Result<usize> {
    let mut changed = jobs::fail_unreadable_on(connection)?.len();
    changed = changed.saturating_add(jobs::recover_on(connection)?);
    let removed = connection.execute(
        "DELETE FROM watchlist_items WHERE NOT EXISTS (
             SELECT 1 FROM watchlist_playlists
             WHERE watchlist_playlists.playlist_id = watchlist_items.playlist_id
         )",
        [],
    )?;
    if removed > 0 {
        tracing::warn!(removed, "watchlist items of missing playlists were removed");
    }
    let Readable { document, unread } = read_readable(connection)?;
    for job in jobs::open_items_on(connection)? {
        if item_request(&job).is_none() && jobs::fail_open_on(connection, job.id, NOT_AN_ITEM)? {
            changed = changed.saturating_add(1);
        }
    }
    changed = changed.saturating_add(jobs::cancel_duplicates_on(connection)?);
    let ids: HashSet<ItemId> = document
        .playlists
        .iter()
        .flat_map(Playlist::item_ids)
        .collect();
    let mut indexes = HashMap::<PathBuf, AudioIndex>::new();
    for job in jobs::open_items_on(connection)? {
        let Some((id, action)) = item_request(&job) else {
            continue;
        };
        if unread.contains(&id.playlist_id) {
            continue;
        }
        let keep = ids.contains(&id)
            && (job.status != Status::Waiting
                || output_of(&job.params).is_none_or(|output| {
                    let index = indexes
                        .entry(output)
                        .or_insert_with_key(|output| AudioIndex::scan(output));
                    find_item(&document, &id)
                        .is_ok_and(|item| check_available_in(item, action, index).is_ok())
                }));
        if !keep && jobs::cancel_open_on(connection, job.id)? {
            changed = changed.saturating_add(1);
        }
    }
    Ok(changed.saturating_add(release_import_questions(connection, &document)?))
}

const NOT_AN_ITEM: &str =
    "This job does not name a watchlist item and an action. Run the item again.";

fn item_request(job: &jobs::Job) -> Option<(ItemId, ItemAction)> {
    let id = job.item_key.parse::<ItemId>().ok()?;
    let action = job.params.get("action")?.as_str()?.parse().ok()?;
    (ItemId::from_params(&job.params).ok()? == id).then_some((id, action))
}

fn release_import_questions(
    connection: &Connection,
    document: &Watchlist,
) -> rusqlite::Result<usize> {
    let mut released: usize = 0;
    for job in jobs::open_items_on(connection)? {
        let keeps_tags = item_request(&job)
            .and_then(|(id, _)| document.playlist(&id.playlist_id))
            .is_some_and(|playlist| playlist.kind.keeps_current_tags());
        let kind = job
            .question
            .as_ref()
            .and_then(|question| question.get("kind")?.as_str()?.parse::<DecisionKind>().ok());
        if job.status == Status::Waiting
            && keeps_tags
            && let Some(kind @ (DecisionKind::ImportMatch | DecisionKind::ImportDuplicate)) = kind
            && jobs::answer_on(
                connection,
                job.id,
                &json!({"kind":kind,"value":MatchAnswer::KeepCurrentTags}),
            )?
        {
            released = released.saturating_add(1);
        }
    }
    Ok(released)
}

fn find_item<'a>(document: &'a Watchlist, id: &ItemId) -> Result<&'a WatchItem> {
    let playlist = document
        .playlists
        .iter()
        .find(|playlist| playlist.playlist_id == id.playlist_id)
        .ok_or(MISSING_PLAYLIST)?;
    let item = playlist
        .find(id)
        .and_then(|index| playlist.items.get(index))
        .ok_or(MISSING_VIDEO)?;
    Ok(item)
}

fn find_item_mut<'a>(
    document: &'a mut Watchlist,
    id: &ItemId,
) -> Result<(&'a mut Playlist, usize)> {
    let playlist = document
        .playlists
        .iter_mut()
        .find(|playlist| playlist.playlist_id == id.playlist_id)
        .ok_or(MISSING_PLAYLIST)?;
    let item = playlist.find(id).ok_or(MISSING_VIDEO)?;
    Ok((playlist, item))
}

fn check_available(item: &WatchItem, action: ItemAction, output: &Path) -> Result<()> {
    check_available_in(item, action, &AudioIndex::scan(output))
}

fn check_available_in(item: &WatchItem, action: ItemAction, index: &AudioIndex) -> Result<()> {
    let audio = item.downloaded_audio(index);
    let (enabled, reason) = availability(item, action, audio.as_deref());
    if enabled {
        Ok(())
    } else {
        Err(reason.unwrap_or("This command is not available.").into())
    }
}

fn write<T>(
    repository: &Repository,
    dry_run: bool,
    draft: &mut Snapshot,
    change: impl FnOnce(&mut Watchlist, &Holds) -> Result<T>,
) -> Result<T, JobError> {
    if dry_run {
        return Ok(change(&mut draft.document, &draft.holds)?);
    }
    let (result, snapshot) = repository.update_with(|document, connection| {
        let holds = Holds::read(connection)?;
        let result = change(document, &holds)?;
        Ok((result, Snapshot::new(document.clone(), holds)))
    })?;
    *draft = snapshot;
    Ok(result)
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), JobError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::repair;
    use crate::jobs::{Kind, NewJob, Queue, Status, Store};
    use crate::watchlist::{ItemId, Playlist, Repository, SourceKind, WatchItem};
    use serde_json::json;

    fn item(kind: SourceKind, key: &str) -> WatchItem {
        let mut item = WatchItem::new(1, key, kind);
        item.video_id = Some(key.into());
        item.entry_id = Some(key.into());
        item
    }

    #[test]
    fn repair_keeps_unreadable_rows_and_repairs_the_rest() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("muzik.db");
        Repository::new(database.clone())
            .update(|document| {
                let mut youtube = Playlist::new("PL1", "u", SourceKind::Youtube, None);
                youtube.merge_items(vec![item(SourceKind::Youtube, "aaaaaaaaaaa")]);
                let mut spotify = Playlist::new("spotify:liked", "u", SourceKind::Spotify, None);
                spotify.merge_items(vec![item(SourceKind::Spotify, "x")]);
                let broken = Playlist::new("PL2", "u", SourceKind::Youtube, None);
                document.playlists = vec![youtube, spotify, broken];
                Ok(())
            })
            .unwrap();
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        connection
            .execute_batch(
                r#"INSERT INTO watchlist_items (playlist_id, ordinal, data) VALUES
                 ('PL2', 0, 'not json'),
                 ('gone', 0, '{"position":1,"title":"A","video_id":"aaaaaaaaaaa"}');
                 INSERT INTO watchlist_playlists (playlist_id, ordinal, data) VALUES ('PL3', 3, 'broken');
                 INSERT INTO jobs (queue, kind, item_key, status, params, question, created_at, updated_at) VALUES
                 ('item', 'item', '["PL1","aaaaaaaaaaa"]', 'running',
                  '{"playlist_id":"PL1","key":"aaaaaaaaaaa","action":"run"}', NULL, 1, 1),
                 ('item', 'item', '["PL1","aaaaaaaaaaa"]', 'queued',
                  '{"playlist_id":"PL1","key":"aaaaaaaaaaa","action":"run"}', NULL, 1, 1),
                 ('item', 'item', '["PL1","aaaaaaaaaaa"]', 'queued', '{oops', NULL, 1, 1),
                 ('item', 'item', 'PL1:1:aaaaaaaaaaa', 'queued', '{"playlist_id":"PL1","action":"run"}', NULL, 1, 1),
                 ('item', 'item', '["PL1","bbbbbbbbbbb"]', 'queued',
                  '{"playlist_id":"PL1","key":"bbbbbbbbbbb","action":"run"}', NULL, 1, 1),
                 ('item', 'item', '["PL2","ccccccccccc"]', 'queued',
                  '{"playlist_id":"PL2","key":"ccccccccccc","action":"run"}', NULL, 1, 1),
                 ('item', 'item', '["spotify:liked","x"]', 'waiting',
                  '{"playlist_id":"spotify:liked","key":"x","action":"organize_again"}',
                  '{"kind":"import_match","payload":{}}', 1, 1),
                 ('sync', 'refresh', 'refresh', 'queued', '{}', NULL, 1, 1),
                 ('nowhere', 'refresh', 'refresh:x', 'queued', '{}', NULL, 1, 1);"#,
            )
            .unwrap();
        let mut store = Store::from_connection(crate::db::open(&database).unwrap());
        assert_eq!(repair(&mut store, |_| None).unwrap(), 7);
        let column = |query: &str| -> Vec<String> {
            connection
                .prepare(query)
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(
            column("SELECT status FROM jobs ORDER BY id"),
            [
                "queued",
                "cancelled",
                "failed",
                "failed",
                "cancelled",
                "queued",
                "queued",
                "queued",
                "failed"
            ]
        );
        assert_eq!(
            column(
                "SELECT playlist_id || ':' || ordinal FROM watchlist_items ORDER BY playlist_id"
            ),
            ["PL1:0", "PL2:0", "spotify:liked:0"]
        );
        assert_eq!(
            column("SELECT data FROM watchlist_items WHERE playlist_id = 'PL2'"),
            ["not json"]
        );
        assert_eq!(
            column("SELECT data FROM watchlist_playlists WHERE playlist_id = 'PL3'"),
            ["broken"]
        );
        let claimed = store.claim(Queue::Item).unwrap().unwrap();
        assert_eq!(claimed.item_key, r#"["PL1","aaaaaaaaaaa"]"#);
        assert_eq!(repair(&mut store, |_| None).unwrap(), 1);
    }

    #[test]
    fn old_item_jobs_find_their_items_in_the_old_watchlist() {
        let directory = tempfile::tempdir().unwrap();
        let watchlist = directory.path().join("watchlist.json");
        let jobs_db = directory.path().join("jobs.db");
        std::fs::write(
            &watchlist,
            json!({"version":1,"playlists":[{"playlist_id":"PL1","url":"u","items":[
                {"position":1,"title":"A","video_id":"aaaaaaaaaaa"},
                {"position":2,"title":"B","video_id":"bbbbbbbbbbb"}
            ]}]})
            .to_string(),
        )
        .unwrap();
        rusqlite::Connection::open(&jobs_db)
            .unwrap()
            .execute_batch(
                r#"CREATE TABLE jobs (id INTEGER PRIMARY KEY, queue TEXT NOT NULL, kind TEXT NOT NULL,
                 item_key TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', status TEXT NOT NULL,
                 params TEXT NOT NULL DEFAULT '{}', question TEXT, answer TEXT, error TEXT,
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 INSERT INTO jobs (queue, kind, item_key, status, params, question, created_at, updated_at) VALUES
                 ('item', 'item', 'PL1:1:aaaaaaaaaaa', 'running',
                  '{"playlist_id":"PL1","position":1,"video_id":"aaaaaaaaaaa","action":"run"}', NULL, 1, 1),
                 ('item', 'item', 'PL1:2:bbbbbbbbbbb', 'waiting',
                  '{"playlist_id":"PL1","position":2,"video_id":"bbbbbbbbbbb","action":"organize_again"}',
                  '{"kind":"import_match"}', 1, 1);"#,
            )
            .unwrap();
        let mut store = Store::from_connection(crate::db::open_in_memory().unwrap());
        assert_eq!(
            super::import_legacy_files(&mut store, &watchlist, &jobs_db).unwrap(),
            2
        );
        assert!(store.list(Status::Failed).unwrap().is_empty());
        let queued = store.list(Status::Queued).unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].item_key, r#"["PL1","aaaaaaaaaaa"]"#);
        let waiting = store.list(Status::Waiting).unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].item_key, r#"["PL1","bbbbbbbbbbb"]"#);
        assert_eq!(waiting[0].question, Some(json!({"kind":"import_match"})));
    }

    #[test]
    fn waiting_spotify_import_questions_go_back_to_the_queue() {
        let mut store = Store::from_connection(crate::db::open_in_memory().unwrap());
        let sources = [
            ("spotify:liked", SourceKind::Spotify),
            ("PL1", SourceKind::Youtube),
            ("spotify:album:a", SourceKind::Spotify),
        ];
        let mut document = crate::watchlist::Watchlist::default();
        for (id, kind) in sources {
            let mut playlist = Playlist::new(id, "u", kind, None);
            playlist.merge_items(vec![item(kind, "xxxxxxxxxxx")]);
            document.playlists.push(playlist);
        }
        super::super::write_changes(
            store.connection_mut(),
            &crate::watchlist::Watchlist::default(),
            &document,
        )
        .unwrap();
        let park = |playlist: &str, kind: &str| {
            store
                .park(
                    &NewJob {
                        kind: Kind::Item,
                        item_key: &ItemId::new(playlist, Some("xxxxxxxxxxx")).to_string(),
                        title: "Song",
                        params: &json!({"playlist_id":playlist,"key":"xxxxxxxxxxx","action":"organize_again"}),
                    },
                    &json!({"kind":kind,"payload":{}}),
                )
                .unwrap()
        };
        let spotify = park("spotify:liked", "import_match");
        let youtube = park("PL1", "import_match");
        let chapters = park("spotify:album:a", "chapter_review");
        assert_eq!(repair(&mut store, |_| None).unwrap(), 1);
        let waiting: Vec<i64> = store
            .list(Status::Waiting)
            .unwrap()
            .into_iter()
            .map(|job| job.id)
            .collect();
        assert_eq!(waiting, [youtube, chapters]);
        assert_eq!(
            store.get(spotify).unwrap().unwrap().answer,
            Some(json!({"kind":"import_match","value":"as_is"}))
        );
    }
}
