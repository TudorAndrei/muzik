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

fn migrations() -> Migrations<'static> {
    Migrations::new(MIGRATIONS.iter().copied().map(M::up).collect())
}

fn migrate(connection: &mut Connection) -> Result<()> {
    let migrations = migrations();
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
    fn the_migrations_are_valid() {
        migrations().validate().unwrap();
    }

    const SEEDS: &[&str] = &[
        "INSERT INTO watchlist_playlists (playlist_id, ordinal, data) VALUES ('PL1', 0, '{}');
         INSERT INTO watchlist_items (playlist_id, ordinal, data) VALUES ('PL1', 0, '{}');
         UPDATE watchlist_revision SET revision = 3;",
        "INSERT INTO jobs (queue, kind, item_key, status, created_at, updated_at)
         VALUES ('item', 'item', 'PL1:1:x', 'queued', 1, 1);",
        "INSERT INTO meta (key, value) VALUES ('legacy_jobs_imported', '1');",
    ];

    const TABLES: &[&str] = &["watchlist_playlists", "jobs", "meta", "sync_files"];

    fn count(connection: &Connection, query: &str) -> i64 {
        connection.query_row(query, [], |row| row.get(0)).unwrap()
    }

    fn upgrade_from(version: usize) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("muzik.db");
        {
            let mut connection = Connection::open(&path).unwrap();
            migrations().to_version(&mut connection, version).unwrap();
            for seed in &SEEDS[..version] {
                connection.execute_batch(seed).unwrap();
            }
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
            [1, 1, 3, seeded(1), seeded(2), 0]
        );
        connection
            .execute(
                "INSERT INTO sync_files (destination, encoding) VALUES ('/device/a.mp3', '{}')",
                [],
            )
            .unwrap();
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
