use crate::Result;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{Connection, OptionalExtension, Row, ToSql, TransactionBehavior, params};
use serde_json::Value;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr};

const COLUMNS: &str = "id, queue, kind, item_key, title, status, params, question, answer, error";

#[derive(Clone, Copy, Debug, PartialEq, Eq, AsRefStr, Display, EnumString, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Status {
    Queued,
    Running,
    Waiting,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, AsRefStr, Display, EnumString, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Queue {
    Sync,
    Workflow,
    Item,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, AsRefStr, Display, EnumString, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Kind {
    Refresh,
    Workflow,
    Item,
}

impl Kind {
    pub fn queue(self) -> Queue {
        match self {
            Self::Refresh => Queue::Sync,
            Self::Workflow => Queue::Workflow,
            Self::Item => Queue::Item,
        }
    }
}

macro_rules! sql_text {
    ($($name:ident),+) => {$(
        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(<&'static str>::from(*self).into())
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                value.as_str()?.parse().map_err(|_| FromSqlError::InvalidType)
            }
        }
    )+};
}

sql_text!(Status, Queue, Kind);

#[derive(Clone, Debug, PartialEq)]
pub struct NewJob<'a> {
    pub kind: Kind,
    pub item_key: &'a str,
    pub title: &'a str,
    pub params: &'a Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: i64,
    pub queue: Queue,
    pub kind: Kind,
    pub item_key: String,
    pub title: String,
    pub status: Status,
    pub params: Value,
    pub question: Option<Value>,
    pub answer: Option<Value>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Inserted(i64),
    Retained(i64),
}

impl Admission {
    pub fn id(self) -> i64 {
        match self {
            Self::Inserted(id) | Self::Retained(id) => id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelRequest {
    Removed,
    Requested,
    NotOpen,
}

pub struct RunnerLock {
    _file: File,
}

impl RunnerLock {
    pub fn try_acquire(path: &Path) -> Result<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn from_connection(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn import_legacy(&self, path: &Path) -> Result<usize> {
        if !path.is_file() {
            return Ok(0);
        }
        let jobs = {
            let legacy = Connection::open(path)?;
            let mut statement = legacy.prepare(
                "SELECT queue, kind, item_key, title, status, params, question, answer, created_at
                     FROM jobs WHERE status IN ('queued', 'running', 'waiting') ORDER BY id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let time = now();
        for (queue, kind, item_key, title, status, params, question, answer, created) in &jobs {
            let status = if status == "running" {
                "queued"
            } else {
                status
            };
            self.connection
                .execute(
                    "INSERT INTO jobs (queue, kind, item_key, title, status, params, question, answer, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![queue, kind, item_key, title, status, params, question, answer, created, time],
                )?;
        }
        let mut backup = path.as_os_str().to_owned();
        backup.push(".migrated");
        std::fs::rename(path, backup)?;
        Ok(jobs.len())
    }

    pub fn request_cancel(&self, id: i64) -> Result<CancelRequest> {
        if self.cancel_open(id)? {
            return Ok(CancelRequest::Removed);
        }
        let changed = self.connection.execute(
            "UPDATE jobs SET cancel_requested = 1, updated_at = ?1
                 WHERE id = ?2 AND status = 'running'",
            params![now(), id],
        )?;
        Ok(if changed == 1 {
            CancelRequest::Requested
        } else {
            CancelRequest::NotOpen
        })
    }

    pub fn cancel_requests(&self) -> Result<Vec<i64>> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM jobs WHERE status = 'running' AND cancel_requested = 1")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn recover(&self) -> Result<usize> {
        Ok(self.connection.execute(
            "UPDATE jobs SET status = 'queued', updated_at = ?1 WHERE status = 'running'",
            params![now()],
        )?)
    }

    pub fn enqueue(&mut self, job: &NewJob<'_>) -> Result<Admission> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let open = transaction
            .query_row(
                "SELECT id FROM jobs WHERE kind = ?1 AND item_key = ?2
                 AND status IN ('queued', 'running', 'waiting') ORDER BY id LIMIT 1",
                params![job.kind, job.item_key],
                |row| row.get(0),
            )
            .optional()?;
        let admission = match open {
            Some(id) => Admission::Retained(id),
            None => Admission::Inserted(insert_on(&transaction, job, Status::Queued, None)?),
        };
        transaction.commit()?;
        Ok(admission)
    }

    pub fn replace_waiting(&mut self, job: &NewJob<'_>) -> Result<Option<i64>> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: bool = transaction.query_row(
            "SELECT EXISTS (SELECT 1 FROM jobs WHERE kind = ?1 AND item_key = ?2
             AND status IN ('queued', 'running'))",
            params![job.kind, job.item_key],
            |row| row.get(0),
        )?;
        if active {
            return Ok(None);
        }
        transaction.execute(
            "UPDATE jobs SET status = 'cancelled', updated_at = ?1
             WHERE kind = ?2 AND item_key = ?3 AND status = 'waiting'",
            params![now(), job.kind, job.item_key],
        )?;
        let id = insert_on(&transaction, job, Status::Queued, None)?;
        transaction.commit()?;
        Ok(Some(id))
    }

    pub fn park(&self, job: &NewJob<'_>, question: &Value) -> Result<i64> {
        park_on(&self.connection, job, question)
    }

    pub fn claim(&self, queue: Queue) -> Result<Option<Job>> {
        self.claim_any(&[queue])
    }

    pub fn claim_any(&self, queues: &[Queue]) -> Result<Option<Job>> {
        let names: Vec<&str> = queues.iter().map(|queue| queue.as_ref()).collect();
        let names = serde_json::to_string(&names)?;
        Ok(self
            .connection
            .query_row(
                &format!(
                    "UPDATE jobs SET status = 'running', cancel_requested = 0, updated_at = ?1
                     WHERE id = (SELECT jobs.id FROM jobs JOIN json_each(?2) AS names ON names.value = jobs.queue
                                 WHERE jobs.status = 'queued' ORDER BY names.key, jobs.id LIMIT 1)
                     RETURNING {COLUMNS}"
                ),
                params![now(), names],
                job,
            )
            .optional()?)
    }

    pub fn list_open(&self) -> Result<Vec<Job>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE status IN ('queued', 'running') ORDER BY id"
        ))?;
        let rows = statement.query_map([], job)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn cancel_open(&self, id: i64) -> Result<bool> {
        let changed = self.connection.execute(
            "UPDATE jobs SET status = 'cancelled', updated_at = ?1
                 WHERE id = ?2 AND status IN ('queued', 'waiting')",
            params![now(), id],
        )?;
        Ok(changed == 1)
    }

    pub fn answer(&self, id: i64, answer: &Value) -> Result<bool> {
        let changed = self.connection.execute(
            "UPDATE jobs SET status = 'queued', answer = ?1, updated_at = ?2
                 WHERE id = ?3 AND status = 'waiting'",
            params![answer, now(), id],
        )?;
        Ok(changed == 1)
    }

    pub fn reopen(&self, id: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE jobs SET status = 'waiting', answer = NULL, updated_at = ?1
                 WHERE id = ?2 AND question IS NOT NULL",
            params![now(), id],
        )?;
        Ok(())
    }

    pub fn finish(&self, id: i64) -> Result<()> {
        self.set_status(id, Status::Done, None)
    }

    pub fn fail(&self, id: i64, error: &str) -> Result<()> {
        self.set_status(id, Status::Failed, Some(error))
    }

    pub fn cancel(&self, id: i64) -> Result<()> {
        self.set_status(id, Status::Cancelled, None)
    }

    pub fn get(&self, id: i64) -> Result<Option<Job>> {
        Ok(self
            .connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1"),
                params![id],
                job,
            )
            .optional()?)
    }

    pub fn list(&self, status: Status) -> Result<Vec<Job>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE status = ?1 ORDER BY id"
        ))?;
        let rows = statement.query_map(params![status], job)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn find_open(&self, kind: Kind, item_key: &str) -> Result<Vec<Job>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE kind = ?1 AND item_key = ?2
                 AND status IN ('queued', 'running', 'waiting') ORDER BY id"
        ))?;
        let rows = statement.query_map(params![kind, item_key], job)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn set_status(&self, id: i64, status: Status, error: Option<&str>) -> Result<()> {
        self.connection.execute(
            "UPDATE jobs SET status = ?1, error = ?2, updated_at = ?3 WHERE id = ?4",
            params![status, error, now(), id],
        )?;
        Ok(())
    }
}

pub fn park_on(connection: &Connection, job: &NewJob<'_>, question: &Value) -> Result<i64> {
    let updated = connection
        .query_row(
            "UPDATE jobs SET question = ?1, params = ?2, title = ?3, answer = NULL, updated_at = ?4
             WHERE kind = ?5 AND item_key = ?6 AND status = 'waiting' RETURNING id",
            params![
                question,
                job.params,
                job.title,
                now(),
                job.kind,
                job.item_key
            ],
            |row| row.get(0),
        )
        .optional()?;
    match updated {
        Some(id) => Ok(id),
        None => insert_on(connection, job, Status::Waiting, Some(question)),
    }
}

fn insert_on(
    connection: &Connection,
    job: &NewJob<'_>,
    status: Status,
    question: Option<&Value>,
) -> Result<i64> {
    let time = now();
    connection.execute(
        "INSERT INTO jobs (queue, kind, item_key, title, status, params, question, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![
            job.kind.queue(),
            job.kind,
            job.item_key,
            job.title,
            status,
            job.params,
            question,
            time
        ],
    )?;
    Ok(connection.last_insert_rowid())
}

fn job(row: &Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        queue: row.get(1)?,
        kind: row.get(2)?,
        item_key: row.get(3)?,
        title: row.get(4)?,
        status: row.get(5)?,
        params: row.get(6)?,
        question: row.get(7)?,
        answer: row.get(8)?,
        error: row.get(9)?,
    })
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::{Admission, CancelRequest, Kind, NewJob, Queue, RunnerLock, Status, Store};
    use serde_json::json;

    fn memory() -> Result<Store, String> {
        Ok(Store::from_connection(crate::db::open_in_memory()?))
    }

    fn job<'a>(kind: Kind, item_key: &'a str, params: &'a serde_json::Value) -> NewJob<'a> {
        NewJob {
            kind,
            item_key,
            title: "Album",
            params,
        }
    }

    #[test]
    fn claim_takes_the_oldest_queued_job_once() -> Result<(), String> {
        let mut store = memory()?;
        let params = json!({});
        let first = store.enqueue(&job(Kind::Item, "a", &params))?.id();
        let second = store.enqueue(&job(Kind::Item, "b", &params))?.id();
        assert_eq!(
            store.enqueue(&job(Kind::Item, "a", &params))?,
            Admission::Retained(first)
        );
        let claimed = store.claim(Queue::Item)?.ok_or("job was not queued")?;
        assert_eq!(
            (claimed.id, claimed.queue, claimed.kind),
            (first, Queue::Item, Kind::Item)
        );
        assert_eq!(store.claim(Queue::Item)?.map(|job| job.id), Some(second));
        assert_eq!(store.claim(Queue::Item)?, None);
        assert_eq!(store.claim(Queue::Workflow)?, None);
        Ok(())
    }

    #[test]
    fn claim_any_takes_the_oldest_job_of_the_named_queues() -> Result<(), String> {
        let mut store = memory()?;
        let params = json!({});
        let first = store.enqueue(&job(Kind::Refresh, "refresh", &params))?.id();
        let second = store.enqueue(&job(Kind::Item, "a", &params))?.id();
        let third = store.enqueue(&job(Kind::Item, "b", &params))?.id();
        assert_eq!(
            store
                .list_open()?
                .iter()
                .map(|job| job.id)
                .collect::<Vec<_>>(),
            [first, second, third]
        );
        assert!(store.cancel_open(second)?);
        assert_eq!(
            store.claim_any(&[Queue::Item])?.map(|job| job.id),
            Some(third)
        );
        assert!(!store.cancel_open(third)?);
        assert_eq!(
            store
                .claim_any(&[Queue::Item, Queue::Sync])?
                .map(|job| job.id),
            Some(first)
        );
        assert_eq!(store.list_open()?.len(), 2);
        let older = store.enqueue(&job(Kind::Item, "c", &params))?.id();
        let newer = store.enqueue(&job(Kind::Refresh, "again", &params))?.id();
        assert_eq!(
            store
                .find_open(Kind::Item, "c")?
                .iter()
                .map(|job| job.id)
                .collect::<Vec<_>>(),
            [older]
        );
        assert_eq!(
            store
                .claim_any(&[Queue::Sync, Queue::Item])?
                .map(|job| job.id),
            Some(newer)
        );
        assert_eq!(
            store
                .claim_any(&[Queue::Sync, Queue::Item])?
                .map(|job| job.id),
            Some(older)
        );
        Ok(())
    }

    #[test]
    fn a_parked_job_waits_until_it_has_an_answer() -> Result<(), String> {
        let store = memory()?;
        let params = json!({"playlist_id": "PL1", "position": 3});
        let id = store.park(
            &job(Kind::Item, "PL1:3", &params),
            &json!({"kind": "import_match"}),
        )?;
        assert_eq!(store.claim(Queue::Item)?, None);
        assert_eq!(store.list(Status::Waiting)?.len(), 1);
        let again = store.park(
            &job(Kind::Item, "PL1:3", &params),
            &json!({"kind": "chapter_review"}),
        )?;
        assert_eq!(again, id);
        assert!(store.answer(id, &json!("as_is"))?);
        assert!(!store.answer(id, &json!("skip"))?);
        let claimed = store.claim(Queue::Item)?.ok_or("job was not queued")?;
        assert_eq!(claimed.answer, Some(json!("as_is")));
        assert_eq!(claimed.question, Some(json!({"kind": "chapter_review"})));
        assert_eq!(claimed.params, params);
        store.reopen(claimed.id)?;
        let reopened = store.get(id)?.ok_or("job is missing")?;
        assert_eq!(reopened.status, Status::Waiting);
        assert_eq!(reopened.answer, None);
        assert!(store.answer(id, &json!("as_is"))?);
        store.claim(Queue::Item)?;
        store.finish(id)?;
        assert_eq!(store.get(id)?.map(|job| job.status), Some(Status::Done));
        Ok(())
    }

    #[test]
    fn a_cancel_removes_a_queued_job_and_flags_a_running_one() -> Result<(), String> {
        let mut store = memory()?;
        let params = json!({});
        let queued = store.enqueue(&job(Kind::Item, "a", &params))?.id();
        let running = store.enqueue(&job(Kind::Workflow, "b", &params))?.id();
        store.claim(Queue::Workflow)?;
        assert_eq!(store.request_cancel(queued)?, CancelRequest::Removed);
        assert_eq!(store.request_cancel(running)?, CancelRequest::Requested);
        assert_eq!(store.cancel_requests()?, [running]);
        store.cancel(running)?;
        assert_eq!(store.request_cancel(running)?, CancelRequest::NotOpen);
        assert!(store.cancel_requests()?.is_empty());
        Ok(())
    }

    #[test]
    fn competing_admissions_leave_one_open_job() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("muzik.db");
        let params = json!({});
        Store::from_connection(crate::db::open(&path)?).park(
            &job(Kind::Item, "a", &params),
            &json!({"kind":"import_match"}),
        )?;
        let barrier = std::sync::Barrier::new(4);
        let replaced = std::thread::scope(|scope| {
            let admissions: Vec<_> = (0..4)
                .map(|index| {
                    let (barrier, path, params) = (&barrier, &path, &params);
                    scope.spawn(move || -> Result<bool, String> {
                        let mut store = Store::from_connection(crate::db::open(path)?);
                        barrier.wait();
                        let job = job(Kind::Item, "a", params);
                        if index % 2 == 0 {
                            Ok(store.replace_waiting(&job)?.is_some())
                        } else {
                            store.enqueue(&job)?;
                            Ok(false)
                        }
                    })
                })
                .collect();
            admissions
                .into_iter()
                .map(|admission| {
                    admission
                        .join()
                        .map_err(|_| "an admission panicked".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()
        })?;
        let replaced = replaced.into_iter().collect::<Result<Vec<_>, _>>()?;
        assert_eq!(replaced.iter().filter(|inserted| **inserted).count(), 1);
        let open = Store::from_connection(crate::db::open(&path)?).find_open(Kind::Item, "a")?;
        assert_eq!(
            open.iter().map(|job| job.status).collect::<Vec<_>>(),
            [Status::Queued]
        );
        Ok(())
    }

    #[test]
    fn a_replacement_touches_only_its_own_item() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("muzik.db");
        let params = json!({});
        let question = json!({"kind":"import_match"});
        let mut first = Store::from_connection(crate::db::open(&path)?);
        let mut second = Store::from_connection(crate::db::open(&path)?);
        let busy = first.park(&job(Kind::Item, "a", &params), &question)?;
        super::insert_on(
            &second.connection,
            &job(Kind::Item, "a", &params),
            Status::Queued,
            None,
        )?;
        let free = first.park(&job(Kind::Item, "b", &params), &question)?;
        assert_eq!(
            second.replace_waiting(&job(Kind::Item, "a", &params))?,
            None
        );
        let inserted = second
            .replace_waiting(&job(Kind::Item, "b", &params))?
            .ok_or("the free item was not admitted")?;
        assert_eq!(
            first.enqueue(&job(Kind::Item, "b", &params))?,
            Admission::Retained(inserted)
        );
        assert!(matches!(
            first.enqueue(&job(Kind::Item, "c", &params))?,
            Admission::Inserted(_)
        ));
        let kept = first.get(busy)?.ok_or("job is missing")?;
        assert_eq!(
            (kept.status, kept.question),
            (Status::Waiting, Some(question))
        );
        assert_eq!(
            first.get(free)?.map(|job| job.status),
            Some(Status::Cancelled)
        );
        Ok(())
    }

    #[test]
    fn a_failed_insertion_keeps_the_waiting_job() -> Result<(), Box<dyn std::error::Error>> {
        let mut store = memory()?;
        let params = json!({});
        let question = json!({"kind":"import_match"});
        let waiting = store.park(&job(Kind::Item, "a", &params), &question)?;
        store.connection.execute_batch(
            "CREATE TRIGGER refuse BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )?;
        assert!(
            store
                .replace_waiting(&job(Kind::Item, "a", &params))
                .is_err()
        );
        let kept = store.get(waiting)?.ok_or("job is missing")?;
        assert_eq!(
            (kept.status, kept.question),
            (Status::Waiting, Some(question))
        );
        assert_eq!(store.find_open(Kind::Item, "a")?.len(), 1);
        Ok(())
    }

    #[test]
    fn one_runner_holds_the_lock() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = directory.path().join("jobs.lock");
        let first = RunnerLock::try_acquire(&path)?;
        assert!(first.is_some());
        assert!(RunnerLock::try_acquire(&path)?.is_none());
        drop(first);
        assert!(RunnerLock::try_acquire(&path)?.is_some());
        Ok(())
    }

    #[test]
    fn running_jobs_return_to_the_queue_after_a_restart() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = directory.path().join("muzik.db");
        let params = json!({});
        let id = {
            let mut store = Store::from_connection(crate::db::open(&path)?);
            let id = store.enqueue(&job(Kind::Workflow, "a", &params))?.id();
            store.claim(Queue::Workflow)?;
            id
        };
        let store = Store::from_connection(crate::db::open(&path)?);
        assert_eq!(store.recover()?, 1);
        assert_eq!(store.claim(Queue::Workflow)?.map(|job| job.id), Some(id));
        Ok(())
    }

    #[test]
    fn open_jobs_of_the_old_database_move_once() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let legacy = directory.path().join("jobs.db");
        {
            let old = rusqlite::Connection::open(&legacy)?;
            old.execute_batch(
                "CREATE TABLE jobs (id INTEGER PRIMARY KEY, queue TEXT NOT NULL, kind TEXT NOT NULL,
                 item_key TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', status TEXT NOT NULL,
                 params TEXT NOT NULL DEFAULT '{}', question TEXT, answer TEXT, error TEXT,
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 INSERT INTO jobs (queue, kind, item_key, title, status, params, question, created_at, updated_at) VALUES
                 ('item', 'item', 'PL1:1:a', 'Waiting', 'waiting', '{}', '{\"kind\":\"import_match\"}', 1, 1),
                 ('item', 'item', 'PL1:2:b', 'Running', 'running', '{}', NULL, 1, 1),
                 ('item', 'item', 'PL1:3:c', 'Done', 'done', '{}', NULL, 1, 1);",
            )?;
        }
        let store = memory()?;
        assert_eq!(store.import_legacy(&legacy)?, 2);
        assert!(!legacy.exists());
        assert!(directory.path().join("jobs.db.migrated").is_file());
        assert_eq!(store.import_legacy(&legacy)?, 0);
        let waiting = store.list(Status::Waiting)?;
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].question, Some(json!({"kind":"import_match"})));
        assert_eq!(store.list(Status::Queued)?[0].item_key, "PL1:2:b");
        Ok(())
    }
}
