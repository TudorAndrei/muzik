use crate::gates::Gate;
use muzik_store::watchlist::jobs::JobError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Persist(#[from] tempfile::PersistError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Store(#[from] muzik_store::Error),
    #[error(transparent)]
    Config(#[from] muzik_core::Error),
    #[error(transparent)]
    Choice(#[from] muzik_core::ChoiceError),
    #[error(transparent)]
    Workflow(#[from] muzik_workflow::Error),
    #[error(transparent)]
    Library(#[from] muzik_library::Error),
    #[error(transparent)]
    Agent(#[from] muzik_agent::Error),
    #[error(transparent)]
    Soulseek(#[from] muzik_soulseek::error::BridgeError),
    #[error("{0} queue wait cancelled")]
    GateCancelled(Gate),
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

impl From<Error> for JobError {
    fn from(error: Error) -> Self {
        Self::Operation(error.to_string())
    }
}
