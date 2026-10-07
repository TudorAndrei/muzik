//! The shared muzik state database.

use crate::{Error, Result};
use rusqlite::{Connection, TransactionBehavior};
use rusqlite_migration::{M, MigrationDefinitionError, Migrations};
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
    let migrations = Migrations::new(MIGRATIONS.iter().copied().map(M::up).collect());
    connection.set_transaction_behavior(TransactionBehavior::Immediate);
    let migrated = migrations
        .to_latest(connection)
        .or_else(|_| migrations.to_latest(connection));
    connection.set_transaction_behavior(TransactionBehavior::Deferred);
    migrated.map_err(|error| match error {
        rusqlite_migration::Error::MigrationDefinition(
            MigrationDefinitionError::DatabaseTooFarAhead,
        ) => Error::from("muzik.db has a newer version than this build supports"),
        error => Error::from(error),
    })
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
    fn a_version_one_database_keeps_its_rows_after_the_jobs_migration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("muzik.db");
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(MIGRATIONS[0]).unwrap();
            connection
                .execute(
                    "INSERT INTO watchlist_playlists (playlist_id, ordinal, data) VALUES ('PL1', 0, '{}')",
                    [],
                )
                .unwrap();
            connection.pragma_update(None, "user_version", 1).unwrap();
        }
        let connection = open(&path).unwrap();
        let playlists: i64 = connection
            .query_row("SELECT COUNT(*) FROM watchlist_playlists", [], |row| {
                row.get(0)
            })
            .unwrap();
        let jobs: i64 = connection
            .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!((playlists, jobs), (1, 0));
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
