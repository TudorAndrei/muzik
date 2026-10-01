use muzik_core::DecisionKind;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum AppEvent {
    JobsUpdated(Value),
    QueuesUpdated(Value),
    JobStarted {
        job_id: String,
        title: String,
        kind: String,
    },
    JobEvent {
        job_id: String,
        source: String,
        name: String,
        data: Value,
    },
    JobCompleted {
        job_id: String,
        result: Value,
    },
    JobFailed {
        job_id: String,
        message: String,
    },
    JobCancelled {
        job_id: String,
    },
    DecisionRequest {
        job_id: String,
        decision_id: String,
        kind: DecisionKind,
        payload: Value,
    },
    WatchlistSaved,
    WatchlistUpdated(Value),
    WatchlistError(String),
    RemoteRunner(String),
}

impl AppEvent {
    pub fn job_id(&self) -> Option<&str> {
        match self {
            Self::JobStarted { job_id, .. }
            | Self::JobEvent { job_id, .. }
            | Self::JobCompleted { job_id, .. }
            | Self::JobFailed { job_id, .. }
            | Self::JobCancelled { job_id }
            | Self::DecisionRequest { job_id, .. } => Some(job_id),
            _ => None,
        }
    }
}
