#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Choice(#[from] muzik_core::ChoiceError),
    #[error(transparent)]
    Library(#[from] muzik_library::Error),
    #[error(transparent)]
    Tags(#[from] muzik_tags::TagsError),
    #[error(transparent)]
    Ffmpeg(#[from] muzik_media::ffmpeg::Error),
    #[error(transparent)]
    Store(#[from] muzik_store::Error),
    #[error(transparent)]
    Config(#[from] muzik_core::Error),
    #[error("{0}")]
    Message(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}
