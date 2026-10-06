use muzik_core::chapters;
use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("workflow cancelled")]
    Cancelled,
    #[error("file operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("chapter lookup failed: {0}")]
    Chapters(#[from] chapters::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Csv(#[from] csv::Error),
    #[error(transparent)]
    Metadata(#[from] muzik_metadata::Error),
    #[error(transparent)]
    Tags(#[from] muzik_tags::TagsError),
    #[error(transparent)]
    Soulseek(#[from] muzik_soulseek::error::BridgeError),
    #[error(transparent)]
    Config(#[from] muzik_core::Error),
    #[error("{0}")]
    Operation(String),
    #[error("playlist workflow needs a playlist adapter")]
    PlaylistAdapterRequired,
    #[error("Spotify export workflow needs a playlist adapter")]
    SpotifyAdapterRequired,
    #[error("no audio files found in output directory")]
    NoAudio,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Operation(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::Operation(message.to_owned())
    }
}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}
