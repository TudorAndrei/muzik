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
#[serde(tag = "type", rename_all = "snake_case")]
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
    WatchlistError {
        message: String,
    },
    RemoteRunner {
        message: String,
    },
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
    use crate::queue::QueueSnapshot;
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
            json!({"type": "job_started", "job_id": "queue-1", "title": "Song", "kind": "item"})
        );
        let progress = AppEvent::JobEvent {
            job_id: "queue-1".into(),
            source: Source::Native,
            event: JobEvent::StepStarted {
                step: Step::Download,
            },
        };
        assert_eq!(
            serde_json::to_value(&progress).unwrap(),
            json!({"type": "job_event", "job_id": "queue-1", "source": "native", "event": {"type": "step_started", "step": "download"}})
        );
        assert_eq!(
            serde_json::to_value(AppEvent::WatchlistError {
                message: "bad".into()
            })
            .unwrap(),
            json!({"type": "watchlist_error", "message": "bad"})
        );
        assert_eq!(
            serde_json::to_value(AppEvent::RemoteRunner {
                message: "busy".into()
            })
            .unwrap(),
            json!({"type": "remote_runner", "message": "busy"})
        );
        assert_eq!(
            serde_json::to_value(AppEvent::JobsUpdated(QueueSnapshot::default())).unwrap(),
            json!({"type": "jobs_updated", "open": [], "waiting": []})
        );
        let updated = serde_json::to_value(AppEvent::WatchlistUpdated(
            muzik_store::watchlist::WatchlistView::default(),
        ))
        .unwrap();
        assert_eq!(updated["type"], "watchlist_updated");
        assert_eq!(updated["playlists"], json!([]));
        assert_eq!(
            serde_json::to_value(JobEvent::WatchlistSaved).unwrap(),
            json!({"type": "watchlist_saved"})
        );
        assert_eq!(
            serde_json::to_value(AppEvent::WatchlistSaved).unwrap(),
            json!({"type": "watchlist_saved"})
        );
    }
}
