//! Native import planning and file placement.

use std::path::PathBuf;

use thiserror::Error as ThisError;

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("tag operation failed: {0}")]
    Tags(#[from] muzik_tags::TagsError),
    #[error("library operation failed: {0}")]
    Library(#[from] muzik_library::Error),
    #[error("MusicBrainz request failed: {0}")]
    Metadata(#[from] muzik_metadata::Error),
    #[error("cannot score album: {0}")]
    Match(#[from] muzik_match::Error),
    #[error("path format failed: {0}")]
    Path(#[from] fancy_regex::Error),
    #[error("trash operation failed: {0}")]
    Trash(#[from] trash::Error),
    #[error("source is not a regular file: {0}")]
    InvalidSource(PathBuf),
    #[error("destination exists: {0}")]
    DestinationExists(PathBuf),
    #[error("path is outside the prune root: {0}")]
    OutsideRoot(PathBuf),
    #[error("no audio files were found")]
    NoAudio,
    #[error("unsupported audio file: {0}")]
    UnsupportedAudio(PathBuf),
    #[error("decision count does not match album count")]
    DecisionCount,
    #[error("candidate index {index} does not exist")]
    CandidateIndex { index: usize },
    #[error("duplicate album needs an explicit duplicate decision")]
    DuplicateDecision,
    #[error("tag or art writes cannot use linked files")]
    LinkedWrite,
    #[error("two tracks have the same destination: {0}")]
    DestinationCollision(PathBuf),
    #[error("cannot use an empty relative destination")]
    InvalidDestination,
    #[error("old album path is outside the library root: {0}")]
    UnsafeReplacePath(PathBuf),
    #[error("item {0} has no audio path")]
    MissingPath(i64),
}

pub mod apply;
pub mod files;
pub mod ftclean;
pub mod paths;
pub mod plan;
pub mod sync;
