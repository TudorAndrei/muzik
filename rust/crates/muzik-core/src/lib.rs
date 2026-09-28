//! Domain data shared by muzik's native libraries.

pub mod app_config;
pub mod chapters;
mod config;
pub mod downloads;
pub mod paths;
pub mod splitter;
pub mod spotify;
pub mod thumbnails;
mod types;
pub mod watchlist;

pub use config::{default_config_path, BeetsConfig, Error as ConfigError};
pub use types::{AlbumId, LocalTrack, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
