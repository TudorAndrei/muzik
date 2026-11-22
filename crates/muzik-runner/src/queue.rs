use crate::gates::GatesSnapshot;
use crate::library_tasks::LibraryRequest;
use crate::settings::Settings;
use muzik_core::DecisionKind;
use muzik_core::paths::Paths;
use muzik_store::db;
use muzik_store::jobs::{Admission, CancelRequest, Job, Kind, NewJob, RunnerLock, Status, Store};
use muzik_store::watchlist::jobs::{self as watchlist_jobs, PendingItem};
use muzik_store::watchlist::{ItemAction, ItemId};
use parking_lot::{Mutex, MutexGuard};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnqueueError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Busy(String),
    #[error("{0}")]
    Store(String),
}

impl From<String> for EnqueueError {
    fn from(message: String) -> Self {
        Self::Store(message)
    }
}

impl From<muzik_store::Error> for EnqueueError {
    fn from(error: muzik_store::Error) -> Self {
        Self::Store(error.to_string())
    }
}

pub const QUESTION_CHANGED: &str = "The question changed. Review the new question.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answered {
    Saved,
    Changed,
    Closed,
}

pub enum RunnerClaim {
    Unlocked,
    Locked(RunnerLock),
    Taken,
}

pub struct Jobs {
    store: Mutex<Store>,
    lock_path: Option<PathBuf>,
    paths: Paths,
}

impl Jobs {
    /// # Errors
    /// Returns an error when the database cannot be opened.
    pub fn open(paths: &Paths) -> crate::Result<Self> {
        Ok(Self {
            store: Mutex::new(Store::from_connection(db::open(&paths.database())?)),
            lock_path: Some(paths.data.join("jobs.lock")),
            paths: paths.clone(),
        })
    }

    pub(crate) fn in_memory(paths: &Paths) -> crate::Result<Self> {
        Ok(Self {
            store: Mutex::new(Store::from_connection(db::open_in_memory()?)),
            lock_path: None,
            paths: paths.clone(),
        })
    }

    pub(crate) fn import_legacy(&self) -> crate::Result<usize> {
        if self.lock_path.is_none() {
            return Ok(0);
        }
        Ok(watchlist_jobs::import_legacy_files(
            &mut self.store(),
            &self.paths.config.join("watchlist.json"),
            &self.paths.data.join("jobs.db"),
        )?)
    }

    pub const fn paths(&self) -> &Paths {
        &self.paths
    }

    pub(crate) fn runner_lock(&self) -> crate::Result<RunnerClaim> {
        match &self.lock_path {
            None => Ok(RunnerClaim::Unlocked),
            Some(path) => {
                Ok(RunnerLock::try_acquire(path)?.map_or(RunnerClaim::Taken, RunnerClaim::Locked))
            }
        }
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock()
    }

    /// # Errors
    /// Returns an error when the job cannot be added to the queue.
    pub fn refresh(&self, params: &Value) -> Result<i64, EnqueueError> {
        let source = params["playlist_id"].as_str().filter(|id| !id.is_empty());
        let key = source.map_or_else(|| "refresh".to_owned(), |id| format!("refresh:{id}"));
        let title = source.map_or_else(
            || "Watchlist check".to_owned(),
            |_| {
                format!(
                    "Check {}",
                    params["playlist_title"].as_str().unwrap_or("source")
                )
            },
        );
        Ok(self
            .store()
            .enqueue(&NewJob {
                kind: Kind::Refresh,
                item_key: &key,
                title: &title,
                params,
            })?
            .id())
    }

    /// # Errors
    /// Returns an error when the input or settings are invalid, or the job cannot be added to the queue.
    pub fn workflow(&self, params: &Value) -> Result<i64, EnqueueError> {
        let raw = params["raw"].as_str().unwrap_or("").trim().to_owned();
        if raw.is_empty() {
            return Err(EnqueueError::Invalid("Enter a URL or path.".into()));
        }
        Settings::resolve(&self.paths, params)
            .map_err(|error| EnqueueError::Invalid(error.to_string()))?;
        let key = format!("{raw}#{}", unique());
        Ok(self
            .store()
            .enqueue(&NewJob {
                kind: Kind::Workflow,
                item_key: &key,
                title: &raw,
                params,
            })?
            .id())
    }

    /// # Errors
    /// Returns an error when the item is invalid, already has an open job, or cannot be added to the queue.
    pub fn item(&self, params: &Value) -> Result<i64, EnqueueError> {
        let (id, _) =
            item_request(params).map_err(|error| EnqueueError::Invalid(error.to_string()))?;
        let key = id.to_string();
        let title = params["title"]
            .as_str()
            .or_else(|| params["key"].as_str())
            .unwrap_or("Item")
            .to_owned();
        self.store()
            .replace_waiting(&NewJob {
                kind: Kind::Item,
                item_key: &key,
                title: &title,
                params,
            })?
            .ok_or_else(|| EnqueueError::Busy("This item already has a job in the queue.".into()))
    }

    /// # Errors
    /// Returns an error when the request or settings are invalid, or the job cannot be added to the queue.
    pub fn library(&self, request: &LibraryRequest) -> Result<i64, EnqueueError> {
        let params = request
            .to_params()
            .map_err(|error| EnqueueError::Invalid(error.to_string()))?;
        Settings::resolve(&self.paths, &params)
            .map_err(|error| EnqueueError::Invalid(error.to_string()))?;
        Ok(self
            .store()
            .enqueue(&NewJob {
                kind: Kind::Library,
                item_key: &request.item_key(),
                title: request.task.title(),
                params: &params,
            })?
            .id())
    }

    pub(crate) fn queue_pending(
        &self,
        params: &Value,
        pending: &[PendingItem],
    ) -> crate::Result<usize> {
        let mut store = self.store();
        let mut queued = 0_usize;
        for item in pending {
            let mut params = params.clone();
            item.id.write(&mut params)?;
            if let Some(fields) = params.as_object_mut() {
                fields.insert("title".into(), json!(item.title));
                fields.insert("action".into(), json!(ItemAction::Run));
            }
            let admission = store.enqueue(&NewJob {
                kind: Kind::Item,
                item_key: &item.id.to_string(),
                title: &item.title,
                params: &params,
            })?;
            if matches!(admission, Admission::Inserted(_)) {
                queued = queued.saturating_add(1);
            }
        }
        drop(store);
        Ok(queued)
    }

    /// # Errors
    /// Returns an error when the job queue cannot be read or written.
    pub fn answer_version(
        &self,
        id: i64,
        version: QuestionVersion,
        value: &Value,
    ) -> crate::Result<Answered> {
        let store = self.store();
        let Some(job) = store.get(id)?.filter(|job| job.status == Status::Waiting) else {
            return Ok(Answered::Closed);
        };
        if job.question_version != version.0 {
            return Ok(Answered::Changed);
        }
        let kind = job
            .question
            .and_then(|question| question.get("kind").cloned())
            .unwrap_or(Value::Null);
        if store.answer_version(id, version.0, &json!({"kind":kind,"value":value}))? {
            return Ok(Answered::Saved);
        }
        let waiting = store
            .get(id)?
            .is_some_and(|job| job.status == Status::Waiting);
        Ok(if waiting {
            Answered::Changed
        } else {
            Answered::Closed
        })
    }

    pub(crate) fn repair(&self) -> crate::Result<usize> {
        let paths = &self.paths;
        let repaired = watchlist_jobs::repair(&mut self.store(), |params| {
            Settings::resolve(paths, params)
                .ok()
                .map(|settings| settings.request.output)
        })
        .map_err(muzik_store::Error::from)?;
        Ok(repaired)
    }

    /// # Errors
    /// Returns an error when the job queue cannot record the cancel request.
    pub fn cancel(&self, id: i64) -> crate::Result<CancelRequest> {
        Ok(self.store().request_cancel(id)?)
    }

    /// # Errors
    /// Returns an error when the job queue cannot be read.
    pub fn get(&self, id: i64) -> crate::Result<Option<Job>> {
        Ok(self.store().get(id)?)
    }

    pub fn has_running(&self) -> bool {
        self.store()
            .list_open()
            .is_ok_and(|jobs| jobs.iter().any(|job| job.status == Status::Running))
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        snapshot(&self.store())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuestionVersion(pub i64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenJob {
    pub job_id: String,
    pub title: String,
    pub kind: Kind,
    pub status: Status,
    pub item: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingJob {
    pub id: i64,
    pub title: String,
    pub kind: Option<DecisionKind>,
    pub payload: Option<Value>,
    pub item: String,
    pub version: QuestionVersion,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueSnapshot {
    pub open: Vec<OpenJob>,
    pub waiting: Vec<WaitingJob>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobsSnapshot {
    #[serde(flatten)]
    pub queue: QueueSnapshot,
    pub gates: GatesSnapshot,
    pub runner: bool,
}

fn snapshot(store: &Store) -> QueueSnapshot {
    let open = store
        .list_open()
        .unwrap_or_default()
        .into_iter()
        .map(|job| OpenJob {
            job_id: job_id(job.id),
            title: job.title,
            kind: job.kind,
            status: job.status,
            item: (job.kind == Kind::Item).then_some(job.item_key),
        })
        .collect();
    let waiting = store
        .list(Status::Waiting)
        .unwrap_or_default()
        .into_iter()
        .map(|job| {
            let question = job.question.unwrap_or(Value::Null);
            WaitingJob {
                id: job.id,
                title: job.title,
                kind: question
                    .get("kind")
                    .and_then(|kind| serde_json::from_value(kind.clone()).ok()),
                payload: question.get("payload").cloned(),
                item: job.item_key,
                version: QuestionVersion(job.question_version),
            }
        })
        .collect();
    QueueSnapshot { open, waiting }
}

#[must_use]
pub fn job_id(id: i64) -> String {
    format!("queue-{id}")
}

#[must_use]
pub fn parse_job_id(text: &str) -> Option<i64> {
    text.strip_prefix("queue-").unwrap_or(text).parse().ok()
}

pub fn item_request(params: &Value) -> crate::Result<(ItemId, ItemAction)> {
    let id = ItemId::from_params(params)?;
    let action = params
        .get("action")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("action must be a non-empty string.")?;
    let action = action
        .parse::<ItemAction>()
        .map_err(|_| format!("'{action}' is not a valid ItemAction"))?;
    Ok((id, action))
}

fn unique() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::{Answered, EnqueueError, Jobs, QuestionVersion, parse_job_id};
    use crate::library_tasks::{LibraryRequest, LibraryTask};
    use muzik_core::paths::Paths;
    use muzik_store::jobs::{Kind, NewJob, Status};
    use muzik_store::watchlist::ItemId;
    use muzik_store::watchlist::jobs::PendingItem;
    use serde_json::json;
    use std::path::Path;

    const KEY: &str = r#"["PL1","abcdefghijk"]"#;

    #[test]
    fn an_answer_is_refused_when_the_question_changed_after_it_was_shown() {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused"))).unwrap();
        let params = json!({"playlist_id":"PL1","key":"abcdefghijk","action":"run"});
        let new_job = NewJob {
            kind: Kind::Item,
            item_key: KEY,
            title: "Song",
            params: &params,
        };
        let id = jobs
            .store()
            .park(&new_job, &json!({"kind":"import_match","payload":{}}))
            .unwrap();
        let shown = QuestionVersion(jobs.get(id).unwrap().unwrap().question_version);
        jobs.store()
            .park(&new_job, &json!({"kind":"import_duplicate","payload":{}}))
            .unwrap();
        assert_eq!(
            jobs.answer_version(id, shown, &json!("skip")).unwrap(),
            Answered::Changed
        );
        assert_eq!(jobs.get(id).unwrap().unwrap().status, Status::Waiting);
        let current = QuestionVersion(jobs.get(id).unwrap().unwrap().question_version);
        assert_eq!(
            jobs.answer_version(id, current, &json!("skip")).unwrap(),
            Answered::Saved
        );
        assert_eq!(
            jobs.answer_version(id, current, &json!("skip")).unwrap(),
            Answered::Closed
        );
    }

    #[test]
    fn the_jobs_snapshot_keeps_its_wire_shape() {
        let snapshot = super::JobsSnapshot {
            queue: super::QueueSnapshot {
                open: vec![super::OpenJob {
                    job_id: "queue-1".into(),
                    title: "Song".into(),
                    kind: Kind::Item,
                    status: Status::Queued,
                    item: Some(KEY.into()),
                }],
                waiting: vec![super::WaitingJob {
                    id: 2,
                    title: "Album".into(),
                    kind: Some(muzik_core::DecisionKind::ImportMatch),
                    payload: Some(json!({})),
                    item: KEY.into(),
                    version: QuestionVersion(3),
                }],
            },
            gates: crate::gates::GatesSnapshot::default(),
            runner: true,
        };
        let wire = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(
            wire["open"][0],
            json!({"job_id":"queue-1","title":"Song","kind":"item","status":"queued","item":KEY})
        );
        assert_eq!(
            wire["waiting"][0],
            json!({"id":2,"title":"Album","kind":"import_match","payload":{},"item":KEY,"version":3})
        );
        assert_eq!(wire["runner"], true);
        assert_eq!(
            wire["gates"]["download"],
            json!({"limit":0,"active":[],"waiting":[]})
        );
        assert_eq!(
            serde_json::from_value::<super::JobsSnapshot>(wire).unwrap(),
            snapshot
        );
    }

    #[test]
    fn an_item_has_one_open_job_and_a_waiting_one_gives_way() {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused"))).unwrap();
        let params = json!({"playlist_id":"PL1","key":"abcdefghijk","action":"run"});
        let waiting = jobs
            .store()
            .park(
                &NewJob {
                    kind: Kind::Item,
                    item_key: KEY,
                    title: "Song",
                    params: &params,
                },
                &json!({"kind":"import_match","payload":{}}),
            )
            .unwrap();
        let first = jobs.item(&params).unwrap();
        assert_eq!(
            jobs.get(waiting).unwrap().map(|job| job.status),
            Some(Status::Cancelled)
        );
        assert!(matches!(jobs.item(&params), Err(EnqueueError::Busy(_))));
        assert!(matches!(
            jobs.item(&json!({"playlist_id":"PL1","key":"abcdefghijk","action":"sing"})),
            Err(EnqueueError::Invalid(_))
        ));
        assert_eq!(jobs.snapshot().open[0].item.as_deref(), Some(KEY));
        jobs.cancel(first).unwrap();
        assert_eq!(jobs.snapshot().open, Vec::new());
        assert_eq!(parse_job_id("queue-7"), Some(7));
        assert_eq!(parse_job_id("7"), Some(7));
    }

    #[test]
    fn a_library_task_has_one_open_job_with_its_title() {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused"))).unwrap();
        let request = LibraryRequest::new(LibraryTask::ReplayGain);
        let first = jobs.library(&request).unwrap();
        assert_eq!(jobs.library(&request).unwrap(), first);
        let other = jobs
            .library(&LibraryRequest::new(LibraryTask::Covers))
            .unwrap();
        assert_ne!(other, first);
        let open = jobs.snapshot().open;
        assert_eq!(open[0].kind, Kind::Library);
        assert_eq!(open[0].title, LibraryTask::ReplayGain.title());
        assert_eq!(open[0].item, None);
        assert_eq!(
            jobs.get(first).unwrap().map(|job| job.params),
            Some(json!({"task":"replaygain","dry_run":false}))
        );
    }

    #[test]
    fn a_real_library_task_does_not_reuse_an_open_dry_run_job() {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused"))).unwrap();
        let dry_run = jobs
            .library(&LibraryRequest {
                dry_run: true,
                ..LibraryRequest::new(LibraryTask::ReplayGain)
            })
            .unwrap();
        let real = jobs
            .library(&LibraryRequest::new(LibraryTask::ReplayGain))
            .unwrap();
        assert_ne!(real, dry_run);
        assert_eq!(
            jobs.get(real).unwrap().map(|job| job.params),
            Some(json!({"task":"replaygain","dry_run":false}))
        );
    }

    #[test]
    fn a_refresh_keeps_open_item_jobs_and_counts_new_ones() {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused"))).unwrap();
        let existing = jobs
            .item(&json!({"playlist_id":"PL1","key":"abcdefghijk","action":"run"}))
            .unwrap();
        let pending = [
            PendingItem {
                id: ItemId::new("PL1", Some("abcdefghijk")),
                title: "Old".into(),
            },
            PendingItem {
                id: ItemId::new("PL1", Some("bcdefghijkl")),
                title: "New".into(),
            },
        ];
        assert_eq!(
            jobs.queue_pending(&json!({"playlist_id":"PL1"}), &pending)
                .unwrap(),
            1
        );
        let open: Vec<_> = jobs
            .snapshot()
            .open
            .into_iter()
            .map(|job| (job.job_id, job.item))
            .collect();
        assert_eq!(
            open,
            [
                (
                    super::job_id(existing),
                    Some(r#"["PL1","abcdefghijk"]"#.to_owned())
                ),
                (
                    super::job_id(existing + 1),
                    Some(r#"["PL1","bcdefghijkl"]"#.to_owned())
                ),
            ]
        );
    }

    #[test]
    fn a_busy_item_keeps_its_older_waiting_job() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let jobs = Jobs::open(&paths).unwrap();
        let params = json!({"playlist_id":"PL1","key":"abcdefghijk","action":"run"});
        let question = json!({"kind":"import_match","payload":{}});
        let waiting = jobs
            .store()
            .park(
                &NewJob {
                    kind: Kind::Item,
                    item_key: KEY,
                    title: "Song",
                    params: &params,
                },
                &question,
            )
            .unwrap();
        muzik_store::db::open(&paths.database())
            .unwrap()
            .execute(
                "INSERT INTO jobs (queue, kind, item_key, status, created_at, updated_at)
             VALUES ('item', 'item', ?1, 'queued', 1, 1)",
                [KEY],
            )
            .unwrap();
        assert!(matches!(jobs.item(&params), Err(EnqueueError::Busy(_))));
        let kept = jobs.get(waiting).unwrap().unwrap();
        assert_eq!(
            (kept.status, kept.question),
            (Status::Waiting, Some(question))
        );
    }
}
