//! The shared muzik state database.

use crate::{Error, Result};
use rusqlite::{Connection, TransactionBehavior};
use std::path::Path;
use std::time::Duration;

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE watchlist_playlists (
        playlist_id TEXT PRIMARY KEY,
        ordinal INTEGER NOT NULL,
        data TEXT NOT NULL
    );
    CREATE TABLE watchlist_items (
        playlist_id TEXT NOT NULL
            REFERENCES watchlist_playlists (playlist_id) ON DELETE CASCADE,
        ordinal INTEGER NOT NULL,
        data TEXT NOT NULL,
        PRIMARY KEY (playlist_id, ordinal)
    ) WITHOUT ROWID;
    CREATE TABLE watchlist_revision (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        revision INTEGER NOT NULL
    );
    INSERT INTO watchlist_revision (id, revision) VALUES (1, 0);",
    "CREATE TABLE jobs (
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
        cancel_requested INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX jobs_by_queue ON jobs (queue, status, id);
    CREATE INDEX jobs_by_item ON jobs (item_key, kind, status);",
    "CREATE TABLE meta (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );",
    "CREATE TABLE sync_files (
        destination TEXT PRIMARY KEY,
        encoding TEXT NOT NULL
    ) WITHOUT ROWID;",
    REKEY_ITEM_JOBS,
    PARK_STORED_HOLDS,
];

const PARK_STORED_HOLDS: &str = "
    UPDATE watchlist_revision SET revision = revision + 1
    WHERE EXISTS (
        SELECT 1 FROM watchlist_items, json_each(watchlist_items.data, '$.stages') AS stage
        WHERE json_valid(watchlist_items.data)
            AND json_extract(stage.value, '$.status') IN ('running', 'waiting')
    );
    INSERT INTO jobs (queue, kind, item_key, title, status, params, question, created_at, updated_at)
    WITH items AS (
        SELECT
            playlist_id,
            ordinal,
            data,
            coalesce(json_extract(data, '$.title'), '') AS title,
            nullif(
                CASE coalesce(json_extract(data, '$.kind'), 'youtube')
                    WHEN 'youtube' THEN json_extract(data, '$.video_id')
                    ELSE json_extract(data, '$.entry_id')
                END,
                ''
            ) AS key
        FROM watchlist_items
        WHERE json_valid(data)
    ),
    asked AS (
        SELECT
            items.playlist_id,
            items.key,
            items.title,
            stage.key AS stage,
            json_extract(stage.value, '$.question') AS question,
            row_number() OVER (
                PARTITION BY items.playlist_id, items.key
                ORDER BY CASE stage.key
                    WHEN 'download' THEN 0 WHEN 'quality' THEN 1 WHEN 'parse' THEN 2
                    WHEN 'split' THEN 3 ELSE 4
                END, items.ordinal
            ) AS rank
        FROM items, json_each(items.data, '$.stages') AS stage
        WHERE items.key IS NOT NULL
            AND json_extract(stage.value, '$.status') = 'waiting'
            AND json_type(stage.value, '$.question') = 'object'
    )
    SELECT
        'item',
        'item',
        json_array(playlist_id, key),
        title,
        'waiting',
        json_object(
            'playlist_id', playlist_id,
            'key', key,
            'title', title,
            'action', CASE stage
                WHEN 'organize' THEN 'organize_again'
                WHEN 'parse' THEN 'parse_again'
                WHEN 'quality' THEN 'check_quality_again'
                ELSE 'run'
            END
        ),
        question,
        CAST(strftime('%s', 'now') AS INTEGER),
        CAST(strftime('%s', 'now') AS INTEGER)
    FROM asked
    WHERE rank = 1 AND NOT EXISTS (
        SELECT 1 FROM jobs
        WHERE jobs.kind = 'item'
            AND jobs.item_key = json_array(asked.playlist_id, asked.key)
            AND jobs.status IN ('queued', 'running', 'waiting')
    );
    UPDATE watchlist_items SET data = json_set(data, '$.stages.download', json_object(
        'status', 'not_started', 'updated_at', json_extract(data, '$.stages.download.updated_at'),
        'path', NULL, 'error', NULL))
    WHERE json_valid(data) AND json_extract(data, '$.stages.download.status') IN ('running', 'waiting');
    UPDATE watchlist_items SET data = json_set(data, '$.stages.quality', json_object(
        'status', 'not_started', 'updated_at', json_extract(data, '$.stages.quality.updated_at'),
        'path', NULL, 'error', NULL))
    WHERE json_valid(data) AND json_extract(data, '$.stages.quality.status') IN ('running', 'waiting');
    UPDATE watchlist_items SET data = json_set(data, '$.stages.parse', json_object(
        'status', 'not_started', 'updated_at', json_extract(data, '$.stages.parse.updated_at'),
        'path', NULL, 'error', NULL))
    WHERE json_valid(data) AND json_extract(data, '$.stages.parse.status') IN ('running', 'waiting');
    UPDATE watchlist_items SET data = json_set(data, '$.stages.split', json_object(
        'status', 'not_started', 'updated_at', json_extract(data, '$.stages.split.updated_at'),
        'path', NULL, 'error', NULL))
    WHERE json_valid(data) AND json_extract(data, '$.stages.split.status') IN ('running', 'waiting');
    UPDATE watchlist_items SET data = json_set(data, '$.stages.organize', json_object(
        'status', 'not_started', 'updated_at', json_extract(data, '$.stages.organize.updated_at'),
        'path', NULL, 'error', NULL))
    WHERE json_valid(data) AND json_extract(data, '$.stages.organize.status') IN ('running', 'waiting');";

pub(crate) const REKEY_ITEM_JOBS: &str = "
    UPDATE jobs
    SET status = 'failed',
        error = 'The selected video is no longer available.',
        updated_at = CAST(strftime('%s', 'now') AS INTEGER)
    WHERE kind = 'item'
        AND status IN ('queued', 'running', 'waiting')
        AND json_valid(params)
        AND json_type(params, '$.position') IS NOT NULL
        AND json_type(params, '$.key') IS NULL
        AND NOT EXISTS (
            SELECT 1 FROM watchlist_items
            WHERE json_valid(watchlist_items.data)
                AND watchlist_items.playlist_id = json_extract(jobs.params, '$.playlist_id')
                AND json_extract(watchlist_items.data, '$.position')
                    = json_extract(jobs.params, '$.position')
                AND nullif(json_extract(watchlist_items.data, '$.video_id'), '')
                    IS nullif(json_extract(jobs.params, '$.video_id'), '')
        );
    WITH items AS (
        SELECT
            playlist_id,
            ordinal,
            json_extract(data, '$.position') AS position,
            nullif(json_extract(data, '$.video_id'), '') AS video_id,
            nullif(
                CASE coalesce(json_extract(data, '$.kind'), 'youtube')
                    WHEN 'youtube' THEN json_extract(data, '$.video_id')
                    ELSE json_extract(data, '$.entry_id')
                END,
                ''
            ) AS key
        FROM watchlist_items
        WHERE json_valid(data)
    ),
    old AS (
        SELECT id, CASE WHEN json_valid(params) THEN params END AS params
        FROM jobs
        WHERE kind = 'item'
    ),
    targets AS (
        SELECT
            old.id,
            json_extract(old.params, '$.playlist_id') AS playlist_id,
            coalesce(items.key, nullif(json_extract(old.params, '$.video_id'), '')) AS key
        FROM old
        JOIN items
            ON items.playlist_id = json_extract(old.params, '$.playlist_id')
            AND items.position = json_extract(old.params, '$.position')
            AND items.video_id IS nullif(json_extract(old.params, '$.video_id'), '')
        WHERE json_type(old.params, '$.position') IS NOT NULL
            AND json_type(old.params, '$.key') IS NULL
    )
    UPDATE jobs
    SET item_key = json_array(targets.playlist_id, targets.key),
        params = json_remove(
            json_set(jobs.params, '$.key', targets.key),
            '$.position',
            '$.video_id'
        )
    FROM targets
    WHERE jobs.id = targets.id;
    UPDATE jobs
    SET status = 'cancelled', updated_at = CAST(strftime('%s', 'now') AS INTEGER)
    WHERE kind = 'item'
        AND status IN ('queued', 'running', 'waiting')
        AND EXISTS (
            SELECT 1 FROM jobs AS other
            WHERE other.kind = 'item'
                AND other.item_key = jobs.item_key
                AND other.id != jobs.id
                AND other.status IN ('queued', 'running', 'waiting')
                AND (
                    CASE other.status WHEN 'running' THEN 2 WHEN 'waiting' THEN 1 ELSE 0 END
                        > CASE jobs.status WHEN 'running' THEN 2 WHEN 'waiting' THEN 1 ELSE 0 END
                    OR (
                        other.status = jobs.status AND other.id < jobs.id
                    )
                )
        );";

/// # Errors
/// Returns an error if the directory or database cannot be created, opened, or migrated.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    prepare(connection)
}

/// # Errors
/// Returns an error if the in-memory database cannot be opened or migrated.
pub fn open_in_memory() -> Result<Connection> {
    prepare(Connection::open_in_memory()?)
}

fn prepare(mut connection: Connection) -> Result<Connection> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    migrate(&mut connection)?;
    Ok(connection)
}

fn migrate(connection: &mut Connection) -> Result<()> {
    // Read the version only after taking the write lock. Another opener may
    // have completed the migrations while this connection waited for it.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let version = usize::try_from(version)?;
    if version > MIGRATIONS.len() {
        return Err(Error::from(
            "muzik.db has a newer version than this build supports",
        ));
    }
    for sql in MIGRATIONS.iter().skip(version) {
        transaction.execute_batch(sql)?;
    }
    transaction.pragma_update(None, "user_version", integer(MIGRATIONS.len())?)?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn integer(value: usize) -> Result<i64> {
    Ok(i64::try_from(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_applies_every_migration_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state/muzik.db");
        drop(open(&path).unwrap());
        let connection = open(&path).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(usize::try_from(version).unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn failed_migrations_roll_back_schema_and_version() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO meta VALUES ('existing', 'keep');",
            )
            .unwrap();
        assert!(migrate(&mut connection).is_err());
        assert_eq!(count(&connection, "PRAGMA user_version"), 0);
        assert_eq!(
            count(
                &connection,
                "SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('jobs', 'watchlist_playlists')"
            ),
            0
        );
        let value: String = connection
            .query_row("SELECT value FROM meta WHERE key = 'existing'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value, "keep");
    }

    const SEEDS: &[&str] = &[
        r#"INSERT INTO watchlist_playlists (playlist_id, ordinal, data)
         VALUES ('PL1', 0, '{}'), ('spotify:playlist:p', 1, '{}');
         INSERT INTO watchlist_items (playlist_id, ordinal, data) VALUES
         ('PL1', 0, '{"position":1,"title":"A","video_id":"aaaaaaaaaaa"}'),
         ('PL1', 1, '{"position":2,"title":"B","video_id":"bbbbbbbbbbb"}'),
         ('PL1', 2, '{"position":3,"title":"A","video_id":"aaaaaaaaaaa"}'),
         ('spotify:playlist:p', 0,
          '{"position":1,"title":"T","kind":"spotify","video_id":"t","entry_id":"spotify:track:t#0"}');
         UPDATE watchlist_revision SET revision = 3;"#,
        r#"INSERT INTO jobs (queue, kind, item_key, status, params, created_at, updated_at) VALUES
         ('item', 'item', 'PL1:3:aaaaaaaaaaa', 'queued',
          '{"playlist_id":"PL1","position":3,"video_id":"aaaaaaaaaaa","action":"run"}', 1, 1),
         ('item', 'item', 'spotify:playlist:p:1:t', 'waiting',
          '{"playlist_id":"spotify:playlist:p","position":1,"video_id":"t","action":"organize_again"}', 1, 1),
         ('item', 'item', 'PL1:9:zzzzzzzzzzz', 'queued',
          '{"playlist_id":"PL1","position":9,"video_id":"zzzzzzzzzzz","action":"run"}', 1, 1),
         ('refresh', 'refresh', 'refresh', 'queued', '{}', 1, 1);
         INSERT INTO jobs (queue, kind, item_key, status, params, question, created_at, updated_at)
         VALUES ('item', 'item', 'PL1:1:aaaaaaaaaaa', 'waiting',
          '{"playlist_id":"PL1","position":1,"video_id":"aaaaaaaaaaa","action":"organize_again"}',
          '{"kind":"import_match"}', 1, 1);"#,
        "INSERT INTO meta (key, value) VALUES ('legacy_jobs_imported', '1');",
        "INSERT INTO sync_files (destination, encoding) VALUES ('/device/b.mp3', '{}');",
        r#"UPDATE watchlist_items
         SET data = '{"position":2,"title":"B","video_id":"bbbbbbbbbbb","stages":{
             "download":{"status":"complete","path":"/b.opus"},
             "organize":{"status":"waiting","updated_at":"2026-09-30T21:03:00+02:00",
                         "question":{"kind":"import_match","payload":{}}}}}'
         WHERE playlist_id = 'PL1' AND ordinal = 1;
         UPDATE watchlist_items
         SET data = json_set(data, '$.stages', json('{"download":{"status":"running"}}'))
         WHERE playlist_id = 'spotify:playlist:p';"#,
    ];

    const TABLES: &[&str] = &["watchlist_playlists", "jobs", "meta", "sync_files"];

    fn count(connection: &Connection, query: &str) -> i64 {
        connection.query_row(query, [], |row| row.get(0)).unwrap()
    }

    fn upgrade_from(version: usize) -> (tempfile::TempDir, Connection) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("muzik.db");
        {
            let connection = Connection::open(&path).unwrap();
            for (sql, seed) in MIGRATIONS[..version].iter().zip(SEEDS) {
                connection.execute_batch(sql).unwrap();
                connection.execute_batch(seed).unwrap();
            }
            connection
                .pragma_update(None, "user_version", integer(version).unwrap())
                .unwrap();
        }
        let connection = open(&path).unwrap();
        let version_after: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(usize::try_from(version_after).unwrap(), MIGRATIONS.len());
        let tables = TABLES
            .iter()
            .map(|table| {
                count(
                    &connection,
                    &format!("SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = '{table}'"),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(tables, vec![1; TABLES.len()]);
        let seeded = |at: usize| i64::from(version > at);
        assert_eq!(
            [
                count(&connection, "SELECT COUNT(*) FROM watchlist_playlists"),
                count(&connection, "SELECT COUNT(*) FROM watchlist_items"),
                count(&connection, "SELECT revision FROM watchlist_revision"),
                count(&connection, "SELECT COUNT(*) FROM jobs"),
                count(&connection, "SELECT COUNT(*) FROM meta"),
                count(&connection, "SELECT COUNT(*) FROM sync_files"),
            ],
            [
                2,
                4,
                if version > 4 { 4 } else { 3 },
                [0, 0, 5, 5, 5, 6][version],
                seeded(2),
                seeded(3)
            ]
        );
        let held: i64 = count(
            &connection,
            "SELECT COUNT(*) FROM watchlist_items, json_each(data, '$.stages') AS stage
             WHERE json_extract(stage.value, '$.status') IN ('running', 'waiting')",
        );
        assert_eq!(held, 0);
        let jobs: Vec<(String, String)> = connection
            .prepare("SELECT item_key, params FROM jobs ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let expected = [
            (
                r#"["PL1","aaaaaaaaaaa"]"#,
                r#"{"playlist_id":"PL1","action":"run","key":"aaaaaaaaaaa"}"#,
            ),
            (
                r#"["spotify:playlist:p","spotify:track:t#0"]"#,
                r#"{"playlist_id":"spotify:playlist:p","action":"organize_again","key":"spotify:track:t#0"}"#,
            ),
            (
                "PL1:9:zzzzzzzzzzz",
                r#"{"playlist_id":"PL1","position":9,"video_id":"zzzzzzzzzzz","action":"run"}"#,
            ),
            ("refresh", "{}"),
            (
                r#"["PL1","aaaaaaaaaaa"]"#,
                r#"{"playlist_id":"PL1","action":"organize_again","key":"aaaaaaaaaaa"}"#,
            ),
            (
                r#"["PL1","bbbbbbbbbbb"]"#,
                r#"{"playlist_id":"PL1","key":"bbbbbbbbbbb","title":"B","action":"organize_again"}"#,
            ),
        ];
        let expected: Vec<(String, String)> = expected
            .iter()
            .take([0, 0, 5, 5, 5, 6][version])
            .map(|(key, params)| ((*key).to_owned(), (*params).to_owned()))
            .collect();
        assert_eq!(jobs, expected);
        connection
            .execute(
                "INSERT INTO sync_files (destination, encoding) VALUES ('/device/a.mp3', '{}')",
                [],
            )
            .unwrap();
        (directory, connection)
    }

    #[test]
    fn a_version_one_database_upgrades_with_its_rows() {
        upgrade_from(1);
    }

    #[test]
    fn a_version_two_database_upgrades_with_its_rows() {
        upgrade_from(2);
    }

    #[test]
    fn a_version_three_database_upgrades_with_its_rows() {
        upgrade_from(3);
    }

    #[test]
    fn a_version_four_database_upgrades_with_its_rows() {
        upgrade_from(4);
    }

    #[test]
    fn legacy_jobs_of_one_item_keep_the_most_advanced_and_a_job_whose_item_is_gone_fails() {
        let (_directory, connection) = upgrade_from(4);
        let statuses: Vec<(String, String)> = connection
            .prepare("SELECT item_key, status FROM jobs WHERE kind = 'item' ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            statuses,
            [
                (
                    r#"["PL1","aaaaaaaaaaa"]"#.to_owned(),
                    "cancelled".to_owned()
                ),
                (
                    r#"["spotify:playlist:p","spotify:track:t#0"]"#.to_owned(),
                    "waiting".to_owned()
                ),
                ("PL1:9:zzzzzzzzzzz".to_owned(), "failed".to_owned()),
                (r#"["PL1","aaaaaaaaaaa"]"#.to_owned(), "waiting".to_owned()),
            ]
        );
    }

    #[test]
    fn a_version_five_database_parks_its_stored_questions_as_jobs() {
        let (_directory, connection) = upgrade_from(5);
        let (status, question): (String, String) = connection
            .query_row(
                r#"SELECT status, question FROM jobs WHERE item_key = '["PL1","bbbbbbbbbbb"]'"#,
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "waiting");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&question).unwrap(),
            serde_json::json!({"kind":"import_match","payload":{}})
        );
        let stages: String = connection
            .query_row(
                "SELECT json_extract(data, '$.stages') FROM watchlist_items
                 WHERE playlist_id = 'PL1' AND ordinal = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let stages: serde_json::Value = serde_json::from_str(&stages).unwrap();
        assert_eq!(stages["download"]["path"], "/b.opus");
        assert_eq!(stages["organize"]["status"], "not_started");
        assert_eq!(
            stages["organize"]["updated_at"],
            "2026-09-30T21:03:00+02:00"
        );
    }

    #[test]
    fn several_openers_can_migrate_the_database_at_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("muzik.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        std::thread::scope(|scope| {
            let openers: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| open(&path).map(drop)))
                .collect();
            for opener in openers {
                opener.join().unwrap().unwrap();
            }
        });
    }

    #[test]
    fn open_refuses_a_newer_database() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("muzik.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(open(&path).is_err());
    }
}
