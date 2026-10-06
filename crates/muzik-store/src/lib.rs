//! muzik.db: the migration list and every query on it.

pub mod db;
pub mod jobs;
pub mod sync_files;
pub mod watchlist;

pub use rusqlite::Connection;
