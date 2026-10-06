//! muzik.db: the migration list and every query on it.

pub mod db;
pub mod error;
pub mod jobs;
pub mod sync_files;
pub mod watchlist;

pub use error::{Error, Result};
pub use rusqlite::Connection;
