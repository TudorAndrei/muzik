use crate::gates::GatesSnapshot;
use crate::queue::QueueSnapshot;
use muzik_core::{DecisionKind, JobEvent};
use muzik_store::jobs::Kind;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Workflow,
    Native,
    Agent,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AppEvent {
    JobsUpdated(QueueSnapshot),
    QueuesUpdated(GatesSnapshot),
    JobStarted {
        job_id: String,
        title: String,
        kind: Kind,
    },
    JobEvent {
        job_id: String,
        source: Source,
        event: JobEvent,
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
    WatchlistUpdated(muzik_store::watchlist::WatchlistView),
    WatchlistError(String),
    RemoteRunner(String),
}

impl AppEvent {
    #[must_use]
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

#[cfg(test)]
mod tests {
    use super::{AppEvent, Source};
    use muzik_core::{JobEvent, Step};
    use muzik_store::jobs::Kind;
    use serde_json::json;

    #[test]
    fn events_serialize_with_a_type_tag() {
        let started = AppEvent::JobStarted {
            job_id: "queue-1".into(),
            title: "Song".into(),
            kind: Kind::Item,
        };
        assert_eq!(
            serde_json::to_value(&started).unwrap(),
            json!({"type": "job_started", "data": {"job_id": "queue-1", "title": "Song", "kind": "item"}})
        );
        let progress = AppEvent::JobEvent {
            job_id: "queue-1".into(),
            source: Source::Native,
            event: JobEvent::StepStarted(Step::Download),
        };
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            json!({"type": "job_event", "data": {"job_id": "queue-1", "source": "native", "event": {"step_started": "download"}}})
        );
        assert_eq!(
            serde_json::to_value(AppEvent::WatchlistError("bad".into())).unwrap(),
            json!({"type": "watchlist_error", "data": "bad"})
        );
        assert_eq!(
            serde_json::to_value(AppEvent::WatchlistSaved).unwrap(),
            json!({"type": "watchlist_saved"})
        );
    }
}
