use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};
use serde_json::Value;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS jobs (
    id INTEGER PRIMARY KEY,
    queue TEXT NOT NULL,
    kind TEXT NOT NULL,
    item_key TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    params TEXT NOT NULL DEFAULT '{}',
    question TEXT,
    answer TEXT,
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS jobs_by_queue ON jobs (queue, status, id);
CREATE INDEX IF NOT EXISTS jobs_by_item ON jobs (item_key, kind, status);
";

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

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let connection = Connection::open(path).map_err(text)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(text)?;
        Self::prepare(connection)
    }

    pub fn open_in_memory() -> Result<Self, String> {
        Self::prepare(Connection::open_in_memory().map_err(text)?)
    }

    fn prepare(connection: Connection) -> Result<Self, String> {
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(text)?;
        connection.execute_batch(SCHEMA).map_err(text)?;
        Ok(Self { connection })
    }

    pub fn recover(&self) -> Result<usize, String> {
        self.connection
            .execute(
                "UPDATE jobs SET status = 'queued', updated_at = ?1 WHERE status = 'running'",
                params![now()],
            )
            .map_err(text)
    }

    pub fn enqueue(&self, job: &NewJob<'_>) -> Result<i64, String> {
        if let Some(id) = self.open_job(job.kind, job.item_key)? {
            return Ok(id);
        }
        self.insert(job, Status::Queued, None)
    }

    pub fn park(&self, job: &NewJob<'_>, question: &Value) -> Result<i64, String> {
        let question = question.to_string();
        let updated = self
            .connection
            .query_row(
                "UPDATE jobs SET question = ?1, params = ?2, title = ?3, answer = NULL, updated_at = ?4
                 WHERE kind = ?5 AND item_key = ?6 AND status = 'waiting' RETURNING id",
                params![
                    question,
                    job.params.to_string(),
                    job.title,
                    now(),
                    job.kind,
                    job.item_key
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(text)?;
        match updated {
            Some(id) => Ok(id),
            None => self.insert(job, Status::Waiting, Some(question)),
        }
    }

    pub fn claim(&self, queue: Queue) -> Result<Option<Job>, String> {
        self.claim_any(&[queue])
    }

    pub fn claim_any(&self, queues: &[Queue]) -> Result<Option<Job>, String> {
        let names: Vec<&str> = queues.iter().map(|queue| queue.as_ref()).collect();
        let names = serde_json::to_string(&names).map_err(|error| error.to_string())?;
        self.connection
            .query_row(
                &format!(
                    "UPDATE jobs SET status = 'running', updated_at = ?1
                     WHERE id = (SELECT jobs.id FROM jobs JOIN json_each(?2) AS names ON names.value = jobs.queue
                                 WHERE jobs.status = 'queued' ORDER BY names.key, jobs.id LIMIT 1)
                     RETURNING {COLUMNS}"
                ),
                params![now(), names],
                job,
            )
            .optional()
            .map_err(text)
    }

    pub fn list_open(&self) -> Result<Vec<Job>, String> {
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {COLUMNS} FROM jobs WHERE status IN ('queued', 'running') ORDER BY id"
            ))
            .map_err(text)?;
        let rows = statement.query_map([], job).map_err(text)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(text)
    }

    pub fn cancel_open(&self, id: i64) -> Result<bool, String> {
        self.connection
            .execute(
                "UPDATE jobs SET status = 'cancelled', updated_at = ?1
                 WHERE id = ?2 AND status IN ('queued', 'waiting')",
                params![now(), id],
            )
            .map(|changed| changed == 1)
            .map_err(text)
    }

    pub fn answer(&self, id: i64, answer: &Value) -> Result<bool, String> {
        self.connection
            .execute(
                "UPDATE jobs SET status = 'queued', answer = ?1, updated_at = ?2
                 WHERE id = ?3 AND status = 'waiting'",
                params![answer.to_string(), now(), id],
            )
            .map(|changed| changed == 1)
            .map_err(text)
    }

    pub fn reopen(&self, id: i64) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE jobs SET status = 'waiting', answer = NULL, updated_at = ?1
                 WHERE id = ?2 AND question IS NOT NULL",
                params![now(), id],
            )
            .map(|_| ())
            .map_err(text)
    }

    pub fn finish(&self, id: i64) -> Result<(), String> {
        self.set_status(id, Status::Done, None)
    }

    pub fn fail(&self, id: i64, error: &str) -> Result<(), String> {
        self.set_status(id, Status::Failed, Some(error))
    }

    pub fn cancel(&self, id: i64) -> Result<(), String> {
        self.set_status(id, Status::Cancelled, None)
    }

    pub fn get(&self, id: i64) -> Result<Option<Job>, String> {
        self.connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1"),
                params![id],
                job,
            )
            .optional()
            .map_err(text)
    }

    pub fn list(&self, status: Status) -> Result<Vec<Job>, String> {
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {COLUMNS} FROM jobs WHERE status = ?1 ORDER BY id"
            ))
            .map_err(text)?;
        let rows = statement.query_map(params![status], job).map_err(text)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(text)
    }

    pub fn find_open(&self, kind: Kind, item_key: &str) -> Result<Vec<Job>, String> {
        let mut statement = self
            .connection
            .prepare(&format!(
                "SELECT {COLUMNS} FROM jobs WHERE kind = ?1 AND item_key = ?2
                 AND status IN ('queued', 'running', 'waiting') ORDER BY id"
            ))
            .map_err(text)?;
        let rows = statement
            .query_map(params![kind, item_key], job)
            .map_err(text)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(text)
    }

    fn open_job(&self, kind: Kind, item_key: &str) -> Result<Option<i64>, String> {
        self.connection
            .query_row(
                "SELECT id FROM jobs WHERE kind = ?1 AND item_key = ?2
                 AND status IN ('queued', 'running', 'waiting') ORDER BY id LIMIT 1",
                params![kind, item_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(text)
    }

    fn insert(
        &self,
        job: &NewJob<'_>,
        status: Status,
        question: Option<String>,
    ) -> Result<i64, String> {
        let time = now();
        self.connection
            .execute(
                "INSERT INTO jobs (queue, kind, item_key, title, status, params, question, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    job.kind.queue(),
                    job.kind,
                    job.item_key,
                    job.title,
                    status,
                    job.params.to_string(),
                    question,
                    time
                ],
            )
            .map_err(text)?;
        Ok(self.connection.last_insert_rowid())
    }

    fn set_status(&self, id: i64, status: Status, error: Option<&str>) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE jobs SET status = ?1, error = ?2, updated_at = ?3 WHERE id = ?4",
                params![status, error, now(), id],
            )
            .map(|_| ())
            .map_err(text)
    }
}

fn job(row: &Row<'_>) -> rusqlite::Result<Job> {
    let json = |text: Option<String>| text.and_then(|text| serde_json::from_str(&text).ok());
    Ok(Job {
        id: row.get(0)?,
        queue: row.get(1)?,
        kind: row.get(2)?,
        item_key: row.get(3)?,
        title: row.get(4)?,
        status: row.get(5)?,
        params: json(row.get(6)?).unwrap_or(Value::Null),
        question: json(row.get(7)?),
        answer: json(row.get(8)?),
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

fn text(error: rusqlite::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::{Kind, NewJob, Queue, Status, Store};
    use serde_json::json;

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
        let store = Store::open_in_memory()?;
        let params = json!({});
        let first = store.enqueue(&job(Kind::Item, "a", &params))?;
        let second = store.enqueue(&job(Kind::Item, "b", &params))?;
        assert_eq!(store.enqueue(&job(Kind::Item, "a", &params))?, first);
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
        let store = Store::open_in_memory()?;
        let params = json!({});
        let first = store.enqueue(&job(Kind::Refresh, "refresh", &params))?;
        let second = store.enqueue(&job(Kind::Item, "a", &params))?;
        let third = store.enqueue(&job(Kind::Item, "b", &params))?;
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
        let older = store.enqueue(&job(Kind::Item, "c", &params))?;
        let newer = store.enqueue(&job(Kind::Refresh, "again", &params))?;
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
        let store = Store::open_in_memory()?;
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
    fn running_jobs_return_to_the_queue_after_a_restart() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = directory.path().join("jobs.db");
        let params = json!({});
        let id = {
            let store = Store::open(&path)?;
            let id = store.enqueue(&job(Kind::Workflow, "a", &params))?;
            store.claim(Queue::Workflow)?;
            id
        };
        let store = Store::open(&path)?;
        assert_eq!(store.recover()?, 1);
        assert_eq!(store.claim(Queue::Workflow)?.map(|job| job.id), Some(id));
        Ok(())
    }
}
