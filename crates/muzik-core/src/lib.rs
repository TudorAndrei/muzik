//! Domain data shared by muzik's native libraries.

pub mod app_config;
pub mod chapters;
mod config;
pub mod config_choices;
pub mod db;
mod decision;
pub mod downloads;
pub mod paths;
pub mod process;
pub mod quality;
pub mod splitter;
pub mod spotify;
pub mod thumbnails;
mod types;
pub mod watchlist;

pub use config::{default_config_path, BeetsConfig, Error as ConfigError};
pub use config_choices::{
    AudioFallback, AudioSource, ChoiceError, DuplicatePolicy, MetadataSource, QualityPolicy,
};
pub use decision::{ChapterAnswer, DecisionKind, DuplicateAnswer, KEEP_CURRENT_TAGS};
pub use types::{AlbumId, LocalTrack, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
