#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Persist(#[from] tempfile::PersistError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    ReadYaml(Box<serde_saphyr::Error>),
    #[error(transparent)]
    WriteYaml(Box<serde_saphyr::ser::Error>),
    #[error("{0}")]
    Message(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<serde_saphyr::Error> for Error {
    fn from(error: serde_saphyr::Error) -> Self {
        Self::ReadYaml(Box::new(error))
    }
}

impl From<serde_saphyr::ser::Error> for Error {
    fn from(error: serde_saphyr::ser::Error) -> Self {
        Self::WriteYaml(Box::new(error))
    }
}

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
