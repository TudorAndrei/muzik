//! Job queue runtime shared by the CLI and the desktop app.

pub mod agent;
pub mod app;
pub mod choices;
mod error;
mod events;
pub mod gates;
pub mod library_info;
pub mod library_tasks;
mod local_workflow;
mod queue;
mod remote_workflow;
mod runner;
pub mod service;
pub mod settings;
pub mod setup;
mod sources;
pub mod thumbnails;
pub mod watchlist;
mod workflow_result;

pub use app::{App, AppOptions, PlaylistFile, RunnerMode};
pub use error::{Error, Result};
pub use events::{AppEvent, Source};
pub use muzik_bandcamp::Status as BandcampStatus;
pub use queue::{
    Answered, EnqueueError, Jobs, JobsSnapshot, OpenJob, QUESTION_CHANGED, QuestionVersion,
    QueueSnapshot, WaitingJob, job_id, parse_job_id,
};
pub use runner::{Ask, Options, Prompt, Runner, Running, Sink};
pub use settings::Settings;
