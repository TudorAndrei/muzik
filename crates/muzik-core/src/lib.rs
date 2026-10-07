//! Domain data shared by muzik's native libraries.

pub mod app_config;
pub mod audio;
pub mod chapters;
mod config;
pub mod config_choices;
mod decision;
pub mod downloads;
mod error;
mod job_event;
pub mod paths;
pub mod thumbnails;
mod types;

pub use config::{BeetsConfig, Error as ConfigError, default_config_path};
pub use config_choices::{
    AudioFallback, AudioSource, ChoiceError, DuplicatePolicy, MetadataSource, PreferredAudio,
    QualityPolicy, SyncPreset,
};
pub use decision::{ChapterAnswer, DecisionKind, DuplicateAnswer, KEEP_CURRENT_TAGS};
pub use error::{Error, Result};
pub use job_event::{JobEvent, Severity, Step, Task};
pub use types::{RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
