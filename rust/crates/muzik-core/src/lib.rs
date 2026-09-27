//! Domain data shared by muzik's native libraries.

mod config;
pub mod downloads;
pub mod paths;
mod types;

pub use config::{default_config_path, BeetsConfig, Error as ConfigError};
pub use types::{AlbumId, LocalTrack, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
