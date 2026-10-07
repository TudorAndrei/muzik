use crate::settings::Settings;
use muzik_core::paths::Paths;
use muzik_core::{DecisionKind, KEEP_CURRENT_TAGS};
use muzik_store::db;
use muzik_store::jobs::{Admission, CancelRequest, Job, Kind, NewJob, RunnerLock, Status, Store};
use muzik_store::watchlist::jobs::PendingItem;
use muzik_store::watchlist::{ItemAction, ItemId, SourceKind};
use parking_lot::{Mutex, MutexGuard};
use serde_json::{Value, json};
use std::fmt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueError {
    Invalid(String),
    Busy(String),
    Store(String),
}

impl fmt::Display for EnqueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Busy(message) | Self::Store(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for EnqueueError {}

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
        Ok(self
            .store()
            .import_legacy(&self.paths.data.join("jobs.db"))?)
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
        let key = validate_item(params)
            .map_err(|error| EnqueueError::Invalid(error.to_string()))?
            .to_string();
        let title = params["title"]
            .as_str()
            .or_else(|| params["video_id"].as_str())
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

    pub(crate) fn queue_pending(
        &self,
        params: &Value,
        pending: &[PendingItem],
    ) -> crate::Result<usize> {
        let mut store = self.store();
        let mut queued = 0_usize;
        for item in pending {
            let mut params = params.clone();
            item.id.write(&mut params);
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
    pub fn answer(&self, id: i64, value: &Value) -> crate::Result<bool> {
        let store = self.store();
        let kind = store
            .get(id)?
            .and_then(|job| job.question)
            .and_then(|question| question.get("kind").cloned())
            .unwrap_or(Value::Null);
        Ok(store.answer(id, &json!({"kind":kind,"value":value}))?)
    }

    pub(crate) fn release_import_questions(&self) -> crate::Result<usize> {
        let store = self.store();
        let mut released = 0_usize;
        for job in store.list(Status::Waiting)? {
            let keeps_tags = job
                .params
                .get("playlist_id")
                .and_then(Value::as_str)
                .is_some_and(|id| SourceKind::of_playlist_id(id).keeps_current_tags());
            let kind = job
                .question
                .as_ref()
                .and_then(|question| question["kind"].as_str())
                .and_then(|kind| kind.parse::<DecisionKind>().ok());
            if keeps_tags
                && matches!(
                    kind,
                    Some(DecisionKind::ImportMatch | DecisionKind::ImportDuplicate)
                )
                && store.answer(job.id, &json!({"kind":kind,"value":KEEP_CURRENT_TAGS}))?
            {
                released = released.saturating_add(1);
            }
        }
        Ok(released)
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

    pub fn snapshot(&self) -> Value {
        snapshot(&self.store())
    }
}

fn snapshot(store: &Store) -> Value {
    let open: Vec<Value> = store
        .list_open()
        .unwrap_or_default()
        .into_iter()
        .map(|job| {
            json!({"job_id":job_id(job.id),"title":job.title,"kind":job.kind.as_ref(),"status":job.status.as_ref(),"item":(job.kind == Kind::Item).then_some(job.item_key)})
        })
        .collect();
    let waiting: Vec<Value> = store
        .list(Status::Waiting)
        .unwrap_or_default()
        .into_iter()
        .map(|job| {
            let question = job.question.unwrap_or(Value::Null);
            json!({"id":job.id,"title":job.title,"kind":question.get("kind"),"payload":question.get("payload"),"item":job.item_key})
        })
        .collect();
    json!({"open":open,"waiting":waiting})
}

#[must_use]
pub fn job_id(id: i64) -> String {
    format!("queue-{id}")
}

#[must_use]
pub fn parse_job_id(text: &str) -> Option<i64> {
    text.strip_prefix("queue-").unwrap_or(text).parse().ok()
}

fn validate_item(params: &Value) -> crate::Result<ItemId> {
    let id = ItemId::from_params(params)?;
    let action = params
        .get("action")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("action must be a non-empty string.")?;
    action
        .parse::<ItemAction>()
        .map_err(|_| format!("'{action}' is not a valid ItemAction"))?;
    Ok(id)
}

fn unique() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::{EnqueueError, Jobs, parse_job_id};
    use muzik_core::paths::Paths;
    use muzik_store::jobs::{Kind, NewJob, Status};
    use muzik_store::watchlist::ItemId;
    use muzik_store::watchlist::jobs::PendingItem;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn an_item_has_one_open_job_and_a_waiting_one_gives_way() {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused"))).unwrap();
        let params =
            json!({"playlist_id":"PL1","position":2,"video_id":"abcdefghijk","action":"run"});
        let waiting = jobs
            .store()
            .park(
                &NewJob {
                    kind: Kind::Item,
                    item_key: "PL1:2:abcdefghijk",
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
            jobs.item(&json!({"playlist_id":"PL1","position":2,"action":"sing"})),
            Err(EnqueueError::Invalid(_))
        ));
        assert_eq!(jobs.snapshot()["open"][0]["item"], "PL1:2:abcdefghijk");
        jobs.cancel(first).unwrap();
        assert_eq!(jobs.snapshot()["open"], json!([]));
        assert_eq!(parse_job_id("queue-7"), Some(7));
        assert_eq!(parse_job_id("7"), Some(7));
    }

    #[test]
    fn a_refresh_keeps_open_item_jobs_and_counts_new_ones() {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused"))).unwrap();
        let existing = jobs
            .item(
                &json!({"playlist_id":"PL1","position":1,"video_id":"abcdefghijk","action":"run"}),
            )
            .unwrap();
        let pending = [
            PendingItem {
                id: ItemId::new("PL1", 1, Some("abcdefghijk")),
                title: "Old".into(),
            },
            PendingItem {
                id: ItemId::new("PL1", 2, Some("bcdefghijkl")),
                title: "New".into(),
            },
        ];
        assert_eq!(
            jobs.queue_pending(&json!({"playlist_id":"PL1"}), &pending)
                .unwrap(),
            1
        );
        let open: Vec<_> = jobs.snapshot()["open"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|job| (job["job_id"].clone(), job["item"].clone()))
            .collect();
        assert_eq!(
            open,
            [
                (json!(super::job_id(existing)), json!("PL1:1:abcdefghijk")),
                (
                    json!(super::job_id(existing + 1)),
                    json!("PL1:2:bcdefghijkl")
                ),
            ]
        );
    }

    #[test]
    fn a_busy_item_keeps_its_older_waiting_job() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let jobs = Jobs::open(&paths).unwrap();
        let params =
            json!({"playlist_id":"PL1","position":2,"video_id":"abcdefghijk","action":"run"});
        let question = json!({"kind":"import_match","payload":{}});
        let waiting = jobs
            .store()
            .park(
                &NewJob {
                    kind: Kind::Item,
                    item_key: "PL1:2:abcdefghijk",
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
             VALUES ('item', 'item', 'PL1:2:abcdefghijk', 'queued', 1, 1)",
                [],
            )
            .unwrap();
        assert!(matches!(jobs.item(&params), Err(EnqueueError::Busy(_))));
        let kept = jobs.get(waiting).unwrap().unwrap();
        assert_eq!(
            (kept.status, kept.question),
            (Status::Waiting, Some(question))
        );
    }

    #[test]
    fn waiting_spotify_import_questions_go_back_to_the_queue() {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused"))).unwrap();
        let park = |playlist: &str, kind: &str| {
            jobs.store().park(
                &muzik_store::jobs::NewJob {
                    kind: muzik_store::jobs::Kind::Item,
                    item_key: &format!("{playlist}:1:x"),
                    title: "Song",
                    params: &json!({"playlist_id":playlist,"position":1,"action":"organize_again"}),
                },
                &json!({"kind":kind,"payload":{}}),
            )
        };
        let spotify = park("spotify:liked", "import_match").unwrap();
        let youtube = park("PL1", "import_match").unwrap();
        let chapters = park("spotify:album:a", "chapter_review").unwrap();
        assert_eq!(jobs.release_import_questions().unwrap(), 1);
        let waiting: Vec<i64> = jobs.snapshot()["waiting"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|job| job["id"].as_i64())
            .collect();
        assert_eq!(waiting, [youtube, chapters]);
        let released = jobs.get(spotify).unwrap().unwrap();
        assert_eq!(
            released.answer,
            Some(json!({"kind":"import_match","value":"as_is"}))
        );
    }
}
