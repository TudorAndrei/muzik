//! Domain data shared by muzik's native libraries.

mod config;
mod types;

pub use config::{BeetsConfig, Error as ConfigError};
pub use types::{AlbumId, LocalTrack, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
