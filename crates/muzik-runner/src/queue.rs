use crate::settings::Settings;
use muzik_core::paths::Paths;
use muzik_core::{DecisionKind, KEEP_CURRENT_TAGS};
use muzik_store::db;
use muzik_store::jobs::{CancelRequest, Job, Kind, NewJob, RunnerLock, Status, Store};
use muzik_store::watchlist::{ItemAction, ItemId, SourceKind};
use parking_lot::{Mutex, MutexGuard};
use serde_json::{json, Value};
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

pub struct Jobs {
    store: Mutex<Store>,
    lock_path: Option<PathBuf>,
    paths: Paths,
}

impl Jobs {
    pub fn open(paths: &Paths) -> Result<Self, String> {
        Ok(Self {
            store: Mutex::new(Store::from_connection(db::open(&paths.database())?)),
            lock_path: Some(paths.data.join("jobs.lock")),
            paths: paths.clone(),
        })
    }

    pub fn in_memory(paths: &Paths) -> Result<Self, String> {
        Ok(Self {
            store: Mutex::new(Store::from_connection(db::open_in_memory()?)),
            lock_path: None,
            paths: paths.clone(),
        })
    }

    pub(crate) fn import_legacy(&self) -> Result<usize, String> {
        if self.lock_path.is_none() {
            return Ok(0);
        }
        Ok(self
            .store()
            .import_legacy(&self.paths.data.join("jobs.db"))?)
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub(crate) fn runner_lock(&self) -> Result<Option<Option<RunnerLock>>, String> {
        match &self.lock_path {
            None => Ok(Some(None)),
            Some(path) => Ok(RunnerLock::try_acquire(path)?.map(Some)),
        }
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock()
    }

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
        Ok(self.store().enqueue(&NewJob {
            kind: Kind::Refresh,
            item_key: &key,
            title: &title,
            params,
        })?)
    }

    pub fn workflow(&self, params: &Value) -> Result<i64, EnqueueError> {
        let raw = params["raw"].as_str().unwrap_or("").trim().to_owned();
        if raw.is_empty() {
            return Err(EnqueueError::Invalid("Enter a URL or path.".into()));
        }
        Settings::resolve(&self.paths, params).map_err(EnqueueError::Invalid)?;
        let key = format!("{raw}#{}", unique());
        Ok(self.store().enqueue(&NewJob {
            kind: Kind::Workflow,
            item_key: &key,
            title: &raw,
            params,
        })?)
    }

    pub fn item(&self, params: &Value) -> Result<i64, EnqueueError> {
        let key = validate_item(params)
            .map_err(EnqueueError::Invalid)?
            .to_string();
        let store = self.store();
        for open in store.find_open(Kind::Item, &key)? {
            if open.status == Status::Waiting {
                store.cancel_open(open.id)?;
            } else {
                return Err(EnqueueError::Busy(
                    "This item already has a job in the queue.".into(),
                ));
            }
        }
        let title = params["title"]
            .as_str()
            .or_else(|| params["video_id"].as_str())
            .unwrap_or("Item")
            .to_owned();
        Ok(store.enqueue(&NewJob {
            kind: Kind::Item,
            item_key: &key,
            title: &title,
            params,
        })?)
    }

    pub fn answer(&self, id: i64, value: &Value) -> Result<bool, String> {
        let store = self.store();
        let kind = store
            .get(id)?
            .and_then(|job| job.question)
            .map(|question| question["kind"].clone())
            .unwrap_or(Value::Null);
        Ok(store.answer(id, &json!({"kind":kind,"value":value}))?)
    }

    pub fn release_import_questions(&self) -> Result<usize, String> {
        let store = self.store();
        let mut released = 0;
        for job in store.list(Status::Waiting)? {
            let keeps_tags = job.params["playlist_id"]
                .as_str()
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
                released += 1;
            }
        }
        Ok(released)
    }

    pub fn cancel(&self, id: i64) -> Result<CancelRequest, String> {
        Ok(self.store().request_cancel(id)?)
    }

    pub fn get(&self, id: i64) -> Result<Option<Job>, String> {
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
            json!({"id":job.id,"title":job.title,"kind":question["kind"],"payload":question["payload"],"item":job.item_key})
        })
        .collect();
    json!({"open":open,"waiting":waiting})
}

pub fn job_id(id: i64) -> String {
    format!("queue-{id}")
}

pub fn parse_job_id(text: &str) -> Option<i64> {
    text.strip_prefix("queue-").unwrap_or(text).parse().ok()
}

fn validate_item(params: &Value) -> Result<ItemId, String> {
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
    use super::{parse_job_id, EnqueueError, Jobs};
    use muzik_core::paths::Paths;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn an_item_has_one_open_job_and_a_waiting_one_gives_way() -> Result<(), String> {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused")))?;
        let params =
            json!({"playlist_id":"PL1","position":2,"video_id":"abcdefghijk","action":"run"});
        let first = jobs.item(&params).map_err(|error| error.to_string())?;
        assert!(matches!(jobs.item(&params), Err(EnqueueError::Busy(_))));
        assert!(matches!(
            jobs.item(&json!({"playlist_id":"PL1","position":2,"action":"sing"})),
            Err(EnqueueError::Invalid(_))
        ));
        assert_eq!(jobs.snapshot()["open"][0]["item"], "PL1:2:abcdefghijk");
        jobs.cancel(first)?;
        assert_eq!(jobs.snapshot()["open"], json!([]));
        assert_eq!(parse_job_id("queue-7"), Some(7));
        assert_eq!(parse_job_id("7"), Some(7));
        Ok(())
    }

    #[test]
    fn waiting_spotify_import_questions_go_back_to_the_queue() -> Result<(), String> {
        let jobs = Jobs::in_memory(&Paths::under(Path::new("unused")))?;
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
        let spotify = park("spotify:liked", "import_match")?;
        let youtube = park("PL1", "import_match")?;
        let chapters = park("spotify:album:a", "chapter_review")?;
        assert_eq!(jobs.release_import_questions()?, 1);
        let waiting: Vec<i64> = jobs.snapshot()["waiting"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|job| job["id"].as_i64())
            .collect();
        assert_eq!(waiting, [youtube, chapters]);
        let released = jobs.get(spotify)?.ok_or("job is missing")?;
        assert_eq!(
            released.answer,
            Some(json!({"kind":"import_match","value":"as_is"}))
        );
        Ok(())
    }
}
