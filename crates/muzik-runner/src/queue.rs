use crate::{local_workflow, remote_workflow};
use muzik_core::paths;
use muzik_core::watchlist::ItemAction;
use muzik_jobs::{CancelRequest, Job, Kind, NewJob, RunnerLock, Status, Store};
use serde_json::{json, Value};
use std::fmt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
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

pub struct Jobs {
    store: Mutex<Store>,
    lock_path: Option<PathBuf>,
}

impl Jobs {
    pub fn open() -> Result<Self, String> {
        let directory = paths::data_dir();
        Ok(Self {
            store: Mutex::new(Store::open(&directory.join("jobs.db"))?),
            lock_path: Some(directory.join("jobs.lock")),
        })
    }

    pub fn in_memory() -> Result<Self, String> {
        Ok(Self {
            store: Mutex::new(Store::open_in_memory()?),
            lock_path: None,
        })
    }

    pub(crate) fn runner_lock(&self) -> Result<Option<Option<RunnerLock>>, String> {
        match &self.lock_path {
            None => Ok(Some(None)),
            Some(path) => Ok(RunnerLock::try_acquire(path)?.map(Some)),
        }
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn refresh(&self, params: &Value) -> Result<i64, EnqueueError> {
        Ok(self.store().enqueue(&NewJob {
            kind: Kind::Refresh,
            item_key: "refresh",
            title: "Watchlist check",
            params,
        })?)
    }

    pub fn workflow(&self, params: &Value) -> Result<i64, EnqueueError> {
        let raw = params["raw"].as_str().unwrap_or("").trim().to_owned();
        if raw.is_empty() {
            return Err(EnqueueError::Invalid("Enter a URL or path.".into()));
        }
        let checked = local_workflow::supported(params)
            .map(|request| request.map(drop))
            .or_else(|| remote_workflow::supported(params).map(|request| request.map(drop)));
        match checked {
            Some(Ok(())) => {}
            Some(Err(message)) => return Err(EnqueueError::Invalid(message)),
            None => return Err(EnqueueError::Invalid("Enter a URL or path.".into())),
        }
        let key = format!("{raw}#{}", unique());
        Ok(self.store().enqueue(&NewJob {
            kind: Kind::Workflow,
            item_key: &key,
            title: &raw,
            params,
        })?)
    }

    pub fn item(&self, params: &Value) -> Result<i64, EnqueueError> {
        validate_item(params).map_err(EnqueueError::Invalid)?;
        let key = item_key(params);
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
        store.answer(id, &json!({"kind":kind,"value":value}))
    }

    pub fn cancel(&self, id: i64) -> Result<CancelRequest, String> {
        self.store().request_cancel(id)
    }

    pub fn get(&self, id: i64) -> Result<Option<Job>, String> {
        self.store().get(id)
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

pub fn item_key(params: &Value) -> String {
    format!(
        "{}:{}:{}",
        params["playlist_id"].as_str().unwrap_or(""),
        params["position"],
        params["video_id"].as_str().unwrap_or("")
    )
}

fn validate_item(params: &Value) -> Result<(), String> {
    params
        .get("playlist_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("playlist_id must be a non-empty string.")?;
    if params
        .get("position")
        .is_none_or(|value| value.as_i64().is_none() && value.as_u64().is_none())
    {
        return Err("position must be an integer.".into());
    }
    let action = params
        .get("action")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("action must be a non-empty string.")?;
    action
        .parse::<ItemAction>()
        .map_err(|_| format!("'{action}' is not a valid ItemAction"))?;
    Ok(())
}

fn unique() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::{parse_job_id, EnqueueError, Jobs};
    use serde_json::json;

    #[test]
    fn an_item_has_one_open_job_and_a_waiting_one_gives_way() -> Result<(), String> {
        let jobs = Jobs::in_memory()?;
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
}
