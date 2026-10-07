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
    #[must_use]
    pub const fn queue(self) -> Queue {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewJob<'a> {
    pub kind: Kind,
    pub item_key: &'a str,
    pub title: &'a str,
    pub params: &'a Value,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    #[must_use]
    pub const fn id(self) -> i64 {
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
    /// # Errors
    /// Returns an error if the lock file cannot be created, opened, or locked.
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
    pub const fn from_connection(connection: Connection) -> Self {
        Self { connection }
    }

    /// # Errors
    /// Returns an error if the old jobs database cannot be read, copied, or renamed.
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

    /// # Errors
    /// Returns an error if the database query fails.
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

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn cancel_requests(&self) -> Result<Vec<i64>> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM jobs WHERE status = 'running' AND cancel_requested = 1")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn recover(&self) -> Result<usize> {
        Ok(self.connection.execute(
            "UPDATE jobs SET status = 'queued', updated_at = ?1 WHERE status = 'running'",
            params![now()],
        )?)
    }

    /// # Errors
    /// Returns an error if the database transaction fails.
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

    /// # Errors
    /// Returns an error if the database transaction fails.
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

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn park(&self, job: &NewJob<'_>, question: &Value) -> Result<i64> {
        park_on(&self.connection, job, question)
    }

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn claim(&self, queue: Queue) -> Result<Option<Job>> {
        self.claim_any(&[queue])
    }

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn claim_any(&self, queues: &[Queue]) -> Result<Option<Job>> {
        let names: Vec<&str> = queues.iter().map(AsRef::as_ref).collect();
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

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn list_open(&self) -> Result<Vec<Job>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE status IN ('queued', 'running') ORDER BY id"
        ))?;
        let rows = statement.query_map([], job)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn cancel_open(&self, id: i64) -> Result<bool> {
        let changed = self.connection.execute(
            "UPDATE jobs SET status = 'cancelled', updated_at = ?1
                 WHERE id = ?2 AND status IN ('queued', 'waiting')",
            params![now(), id],
        )?;
        Ok(changed == 1)
    }

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn answer(&self, id: i64, answer: &Value) -> Result<bool> {
        let changed = self.connection.execute(
            "UPDATE jobs SET status = 'queued', answer = ?1, updated_at = ?2
                 WHERE id = ?3 AND status = 'waiting'",
            params![answer, now(), id],
        )?;
        Ok(changed == 1)
    }

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn reopen(&self, id: i64) -> Result<()> {
        self.connection.execute(
            "UPDATE jobs SET status = 'waiting', answer = NULL, updated_at = ?1
                 WHERE id = ?2 AND question IS NOT NULL",
            params![now(), id],
        )?;
        Ok(())
    }

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn finish(&self, id: i64) -> Result<()> {
        self.set_status(id, Status::Done, None)
    }

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn fail(&self, id: i64, error: &str) -> Result<()> {
        self.set_status(id, Status::Failed, Some(error))
    }

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn cancel(&self, id: i64) -> Result<()> {
        self.set_status(id, Status::Cancelled, None)
    }

    /// # Errors
    /// Returns an error if the database query fails.
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

    /// # Errors
    /// Returns an error if the database query fails.
    pub fn list(&self, status: Status) -> Result<Vec<Job>> {
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM jobs WHERE status = ?1 ORDER BY id"
        ))?;
        let rows = statement.query_map(params![status], job)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// # Errors
    /// Returns an error if the database query fails.
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

/// # Errors
/// Returns an error if the database query fails.
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
    updated.map_or_else(
        || insert_on(connection, job, Status::Waiting, Some(question)),
        Ok,
    )
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

    fn memory() -> Store {
        Store::from_connection(crate::db::open_in_memory().unwrap())
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
    fn claim_takes_the_oldest_queued_job_once() {
        let mut store = memory();
        let params = json!({});
        let first = store.enqueue(&job(Kind::Item, "a", &params)).unwrap().id();
        let second = store.enqueue(&job(Kind::Item, "b", &params)).unwrap().id();
        assert_eq!(
            store.enqueue(&job(Kind::Item, "a", &params)).unwrap(),
            Admission::Retained(first)
        );
        let claimed = store.claim(Queue::Item).unwrap().unwrap();
        assert_eq!(
            (claimed.id, claimed.queue, claimed.kind),
            (first, Queue::Item, Kind::Item)
        );
        assert_eq!(
            store.claim(Queue::Item).unwrap().map(|job| job.id),
            Some(second)
        );
        assert_eq!(store.claim(Queue::Item).unwrap(), None);
        assert_eq!(store.claim(Queue::Workflow).unwrap(), None);
    }

    #[test]
    fn claim_any_takes_the_oldest_job_of_the_named_queues() {
        let mut store = memory();
        let params = json!({});
        let first = store
            .enqueue(&job(Kind::Refresh, "refresh", &params))
            .unwrap()
            .id();
        let second = store.enqueue(&job(Kind::Item, "a", &params)).unwrap().id();
        let third = store.enqueue(&job(Kind::Item, "b", &params)).unwrap().id();
        assert_eq!(
            store
                .list_open()
                .unwrap()
                .iter()
                .map(|job| job.id)
                .collect::<Vec<_>>(),
            [first, second, third]
        );
        assert!(store.cancel_open(second).unwrap());
        assert_eq!(
            store.claim_any(&[Queue::Item]).unwrap().map(|job| job.id),
            Some(third)
        );
        assert!(!store.cancel_open(third).unwrap());
        assert_eq!(
            store
                .claim_any(&[Queue::Item, Queue::Sync])
                .unwrap()
                .map(|job| job.id),
            Some(first)
        );
        assert_eq!(store.list_open().unwrap().len(), 2);
        let older = store.enqueue(&job(Kind::Item, "c", &params)).unwrap().id();
        let newer = store
            .enqueue(&job(Kind::Refresh, "again", &params))
            .unwrap()
            .id();
        assert_eq!(
            store
                .find_open(Kind::Item, "c")
                .unwrap()
                .iter()
                .map(|job| job.id)
                .collect::<Vec<_>>(),
            [older]
        );
        assert_eq!(
            store
                .claim_any(&[Queue::Sync, Queue::Item])
                .unwrap()
                .map(|job| job.id),
            Some(newer)
        );
        assert_eq!(
            store
                .claim_any(&[Queue::Sync, Queue::Item])
                .unwrap()
                .map(|job| job.id),
            Some(older)
        );
    }

    #[test]
    fn a_parked_job_waits_until_it_has_an_answer() {
        let store = memory();
        let params = json!({"playlist_id": "PL1", "position": 3});
        let id = store
            .park(
                &job(Kind::Item, "PL1:3", &params),
                &json!({"kind": "import_match"}),
            )
            .unwrap();
        assert_eq!(store.claim(Queue::Item).unwrap(), None);
        assert_eq!(store.list(Status::Waiting).unwrap().len(), 1);
        let again = store
            .park(
                &job(Kind::Item, "PL1:3", &params),
                &json!({"kind": "chapter_review"}),
            )
            .unwrap();
        assert_eq!(again, id);
        assert!(store.answer(id, &json!("as_is")).unwrap());
        assert!(!store.answer(id, &json!("skip")).unwrap());
        let claimed = store.claim(Queue::Item).unwrap().unwrap();
        assert_eq!(claimed.answer, Some(json!("as_is")));
        assert_eq!(claimed.question, Some(json!({"kind": "chapter_review"})));
        assert_eq!(claimed.params, params);
        store.reopen(claimed.id).unwrap();
        let reopened = store.get(id).unwrap().unwrap();
        assert_eq!(reopened.status, Status::Waiting);
        assert_eq!(reopened.answer, None);
        assert!(store.answer(id, &json!("as_is")).unwrap());
        store.claim(Queue::Item).unwrap();
        store.finish(id).unwrap();
        assert_eq!(
            store.get(id).unwrap().map(|job| job.status),
            Some(Status::Done)
        );
    }

    #[test]
    fn a_cancel_removes_a_queued_job_and_flags_a_running_one() {
        let mut store = memory();
        let params = json!({});
        let queued = store.enqueue(&job(Kind::Item, "a", &params)).unwrap().id();
        let running = store
            .enqueue(&job(Kind::Workflow, "b", &params))
            .unwrap()
            .id();
        store.claim(Queue::Workflow).unwrap();
        assert_eq!(
            store.request_cancel(queued).unwrap(),
            CancelRequest::Removed
        );
        assert_eq!(
            store.request_cancel(running).unwrap(),
            CancelRequest::Requested
        );
        assert_eq!(store.cancel_requests().unwrap(), [running]);
        store.cancel(running).unwrap();
        assert_eq!(
            store.request_cancel(running).unwrap(),
            CancelRequest::NotOpen
        );
        assert!(store.cancel_requests().unwrap().is_empty());
    }

    #[test]
    fn competing_admissions_leave_one_open_job() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("muzik.db");
        let params = json!({});
        Store::from_connection(crate::db::open(&path).unwrap())
            .park(
                &job(Kind::Item, "a", &params),
                &json!({"kind":"import_match"}),
            )
            .unwrap();
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
        })
        .unwrap();
        let replaced = replaced.into_iter().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(replaced.iter().filter(|inserted| **inserted).count(), 1);
        let open = Store::from_connection(crate::db::open(&path).unwrap())
            .find_open(Kind::Item, "a")
            .unwrap();
        assert_eq!(
            open.iter().map(|job| job.status).collect::<Vec<_>>(),
            [Status::Queued]
        );
    }

    #[test]
    fn a_replacement_touches_only_its_own_item() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("muzik.db");
        let params = json!({});
        let question = json!({"kind":"import_match"});
        let mut first = Store::from_connection(crate::db::open(&path).unwrap());
        let mut second = Store::from_connection(crate::db::open(&path).unwrap());
        let busy = first
            .park(&job(Kind::Item, "a", &params), &question)
            .unwrap();
        super::insert_on(
            &second.connection,
            &job(Kind::Item, "a", &params),
            Status::Queued,
            None,
        )
        .unwrap();
        let free = first
            .park(&job(Kind::Item, "b", &params), &question)
            .unwrap();
        assert_eq!(
            second
                .replace_waiting(&job(Kind::Item, "a", &params))
                .unwrap(),
            None
        );
        let inserted = second
            .replace_waiting(&job(Kind::Item, "b", &params))
            .unwrap()
            .unwrap();
        assert_eq!(
            first.enqueue(&job(Kind::Item, "b", &params)).unwrap(),
            Admission::Retained(inserted)
        );
        assert!(matches!(
            first.enqueue(&job(Kind::Item, "c", &params)).unwrap(),
            Admission::Inserted(_)
        ));
        let kept = first.get(busy).unwrap().unwrap();
        assert_eq!(
            (kept.status, kept.question),
            (Status::Waiting, Some(question))
        );
        assert_eq!(
            first.get(free).unwrap().map(|job| job.status),
            Some(Status::Cancelled)
        );
    }

    #[test]
    fn a_failed_insertion_keeps_the_waiting_job() {
        let mut store = memory();
        let params = json!({});
        let question = json!({"kind":"import_match"});
        let waiting = store
            .park(&job(Kind::Item, "a", &params), &question)
            .unwrap();
        store
            .connection
            .execute_batch(
                "CREATE TRIGGER refuse BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT, 'refused'); END;",
            )
            .unwrap();
        assert!(
            store
                .replace_waiting(&job(Kind::Item, "a", &params))
                .is_err()
        );
        let kept = store.get(waiting).unwrap().unwrap();
        assert_eq!(
            (kept.status, kept.question),
            (Status::Waiting, Some(question))
        );
        assert_eq!(store.find_open(Kind::Item, "a").unwrap().len(), 1);
    }

    #[test]
    fn one_runner_holds_the_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("jobs.lock");
        let first = RunnerLock::try_acquire(&path).unwrap();
        assert!(first.is_some());
        assert!(RunnerLock::try_acquire(&path).unwrap().is_none());
        drop(first);
        assert!(RunnerLock::try_acquire(&path).unwrap().is_some());
    }

    #[test]
    fn running_jobs_return_to_the_queue_after_a_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("muzik.db");
        let params = json!({});
        let id = {
            let mut store = Store::from_connection(crate::db::open(&path).unwrap());
            let id = store
                .enqueue(&job(Kind::Workflow, "a", &params))
                .unwrap()
                .id();
            store.claim(Queue::Workflow).unwrap();
            id
        };
        let store = Store::from_connection(crate::db::open(&path).unwrap());
        assert_eq!(store.recover().unwrap(), 1);
        assert_eq!(
            store.claim(Queue::Workflow).unwrap().map(|job| job.id),
            Some(id)
        );
    }

    #[test]
    fn open_jobs_of_the_old_database_move_once() {
        let directory = tempfile::tempdir().unwrap();
        let legacy = directory.path().join("jobs.db");
        {
            let old = rusqlite::Connection::open(&legacy).unwrap();
            old.execute_batch(
                "CREATE TABLE jobs (id INTEGER PRIMARY KEY, queue TEXT NOT NULL, kind TEXT NOT NULL,
                 item_key TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', status TEXT NOT NULL,
                 params TEXT NOT NULL DEFAULT '{}', question TEXT, answer TEXT, error TEXT,
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
                 INSERT INTO jobs (queue, kind, item_key, title, status, params, question, created_at, updated_at) VALUES
                 ('item', 'item', 'PL1:1:a', 'Waiting', 'waiting', '{}', '{\"kind\":\"import_match\"}', 1, 1),
                 ('item', 'item', 'PL1:2:b', 'Running', 'running', '{}', NULL, 1, 1),
                 ('item', 'item', 'PL1:3:c', 'Done', 'done', '{}', NULL, 1, 1);",
            )
            .unwrap();
        }
        let store = memory();
        assert_eq!(store.import_legacy(&legacy).unwrap(), 2);
        assert!(!legacy.exists());
        assert!(directory.path().join("jobs.db.migrated").is_file());
        assert_eq!(store.import_legacy(&legacy).unwrap(), 0);
        let waiting = store.list(Status::Waiting).unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].question, Some(json!({"kind":"import_match"})));
        assert_eq!(store.list(Status::Queued).unwrap()[0].item_key, "PL1:2:b");
    }
}
