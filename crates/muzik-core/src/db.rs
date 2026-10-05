//! The shared muzik state database.

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
];

pub fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let connection = Connection::open(path).map_err(text)?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .map_err(text)?;
    prepare(connection)
}

pub fn open_in_memory() -> Result<Connection, String> {
    prepare(Connection::open_in_memory().map_err(text)?)
}

fn prepare(mut connection: Connection) -> Result<Connection, String> {
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(text)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(text)?;
    migrate(&mut connection)?;
    Ok(connection)
}

fn migrate(connection: &mut Connection) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(text)?;
    let version: i64 = transaction
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(text)?;
    let applied = usize::try_from(version).map_err(|error| error.to_string())?;
    if applied > MIGRATIONS.len() {
        return Err(format!(
            "{version} is a newer muzik database version than this build supports"
        ));
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(applied) {
        transaction.execute_batch(migration).map_err(text)?;
        transaction
            .pragma_update(None, "user_version", integer(index + 1)?)
            .map_err(text)?;
    }
    transaction.commit().map_err(text)
}

pub(crate) fn text(error: rusqlite::Error) -> String {
    error.to_string()
}

pub(crate) fn integer(value: usize) -> Result<i64, String> {
    i64::try_from(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_applies_every_migration_once() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state/muzik.db");
        drop(open(&path)?);
        let connection = open(&path)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        assert_eq!(usize::try_from(version)?, MIGRATIONS.len());
        Ok(())
    }

    #[test]
    fn a_version_one_database_keeps_its_rows_after_the_jobs_migration(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("muzik.db");
        {
            let connection = Connection::open(&path)?;
            connection.execute_batch(MIGRATIONS[0])?;
            connection.execute(
                "INSERT INTO watchlist_playlists (playlist_id, ordinal, data) VALUES ('PL1', 0, '{}')",
                [],
            )?;
            connection.pragma_update(None, "user_version", 1)?;
        }
        let connection = open(&path)?;
        let playlists: i64 =
            connection.query_row("SELECT COUNT(*) FROM watchlist_playlists", [], |row| {
                row.get(0)
            })?;
        let jobs: i64 = connection.query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))?;
        assert_eq!((playlists, jobs), (1, 0));
        Ok(())
    }

    #[test]
    fn open_refuses_a_newer_database() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("muzik.db");
        Connection::open(&path)?.pragma_update(None, "user_version", 99)?;
        assert!(open(&path).is_err());
        Ok(())
    }
}
