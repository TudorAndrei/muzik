//! Job queue runtime shared by the CLI and the desktop app.

pub mod agent;
pub mod app;
pub mod choices;
mod events;
pub mod gates;
mod local_workflow;
mod queue;
mod remote_workflow;
mod runner;
pub mod settings;
mod sources;
pub mod watchlist;

pub use app::{App, AppOptions};
pub use events::AppEvent;
pub use queue::{job_id, parse_job_id, EnqueueError, Jobs};
pub use runner::{Ask, Options, Prompt, Runner, Running, Sink};
pub use settings::Settings;
