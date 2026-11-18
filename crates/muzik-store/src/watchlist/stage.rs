use super::{StageStatus, now};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stages {
    #[serde(default)]
    pub(super) download: StageRecord,
    #[serde(default)]
    pub(super) quality: StageRecord,
    #[serde(default)]
    pub(super) parse: StageRecord,
    #[serde(default)]
    pub(super) split: StageRecord,
    #[serde(default)]
    pub(super) organize: StageRecord,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum StageState {
    #[default]
    NotStarted,
    Running,
    Waiting {
        question: Value,
    },
    Complete {
        path: Option<PathBuf>,
    },
    Failed {
        error: String,
    },
    Skipped,
    Stale,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StageEvent {
    Start,
    Wait(Value),
    Complete(Option<PathBuf>),
    Fail(String),
    Skip,
    Invalidate,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IllegalTransition {
    pub from: StageStatus,
    pub event: &'static str,
}

impl StageEvent {
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Wait(_) => "wait",
            Self::Complete(_) => "complete",
            Self::Fail(_) => "fail",
            Self::Skip => "skip",
            Self::Invalidate => "invalidate",
            Self::Release => "release",
        }
    }
}

impl StageState {
    #[must_use]
    pub const fn status(&self) -> StageStatus {
        match self {
            Self::NotStarted => StageStatus::NotStarted,
            Self::Running => StageStatus::Running,
            Self::Waiting { .. } => StageStatus::Waiting,
            Self::Complete { .. } => StageStatus::Complete,
            Self::Failed { .. } => StageStatus::Failed,
            Self::Skipped => StageStatus::Skipped,
            Self::Stale => StageStatus::Stale,
        }
    }

    /// # Errors
    /// Returns the refused transition when `event` is not allowed from this state.
    pub fn after(&self, event: StageEvent) -> Result<Self, IllegalTransition> {
        let refused = IllegalTransition {
            from: self.status(),
            event: event.name(),
        };
        match (self, event) {
            (_, StageEvent::Start) => Ok(Self::Running),
            (_, StageEvent::Wait(question)) => Ok(Self::Waiting { question }),
            (_, StageEvent::Complete(path)) => Ok(Self::Complete { path }),
            (_, StageEvent::Fail(error)) => Ok(Self::Failed { error }),
            (
                Self::NotStarted
                | Self::Complete { .. }
                | Self::Failed { .. }
                | Self::Skipped
                | Self::Stale,
                StageEvent::Skip,
            ) => Ok(Self::Skipped),
            (
                Self::NotStarted
                | Self::Complete { .. }
                | Self::Failed { .. }
                | Self::Skipped
                | Self::Stale,
                StageEvent::Invalidate,
            ) => Ok(Self::Stale),
            (Self::Running | Self::Waiting { .. }, StageEvent::Release) => Ok(Self::NotStarted),
            (Self::Running | Self::Waiting { .. }, StageEvent::Skip | StageEvent::Invalidate)
            | (
                Self::NotStarted
                | Self::Complete { .. }
                | Self::Failed { .. }
                | Self::Skipped
                | Self::Stale,
                StageEvent::Release,
            ) => Err(refused),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StoredRecord", into = "StoredRecord")]
pub struct StageRecord {
    state: StageState,
    updated_at: Option<String>,
}

impl StageRecord {
    #[must_use]
    pub const fn state(&self) -> &StageState {
        &self.state
    }

    #[must_use]
    pub const fn status(&self) -> StageStatus {
        self.state.status()
    }

    #[must_use]
    pub fn updated_at(&self) -> Option<&str> {
        self.updated_at.as_deref()
    }

    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match &self.state {
            StageState::Complete { path } => path.as_deref(),
            _ => None,
        }
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match &self.state {
            StageState::Failed { error } if !error.is_empty() => Some(error),
            _ => None,
        }
    }

    #[must_use]
    pub fn question(&self) -> Option<&Value> {
        match &self.state {
            StageState::Waiting { question } if !question.is_null() => Some(question),
            _ => None,
        }
    }

    pub(super) fn apply(&mut self, event: StageEvent) -> Result<(), IllegalTransition> {
        self.state = self.state.after(event)?;
        self.updated_at = Some(now());
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct StoredRecord {
    #[serde(default)]
    status: StageStatus,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    question: Option<Value>,
}

impl From<StoredRecord> for StageRecord {
    fn from(stored: StoredRecord) -> Self {
        let state = match stored.status {
            StageStatus::NotStarted => StageState::NotStarted,
            StageStatus::Running => StageState::Running,
            StageStatus::Waiting => StageState::Waiting {
                question: stored.question.unwrap_or_default(),
            },
            StageStatus::Complete => StageState::Complete { path: stored.path },
            StageStatus::Failed => StageState::Failed {
                error: stored.error.unwrap_or_default(),
            },
            StageStatus::Skipped => StageState::Skipped,
            StageStatus::Stale => StageState::Stale,
        };
        Self {
            state,
            updated_at: stored.updated_at,
        }
    }
}

impl From<StageRecord> for StoredRecord {
    fn from(record: StageRecord) -> Self {
        let mut stored = Self {
            status: record.status(),
            updated_at: record.updated_at,
            ..Self::default()
        };
        match record.state {
            StageState::Waiting { question } => {
                stored.question = (!question.is_null()).then_some(question);
            }
            StageState::Complete { path } => stored.path = path,
            StageState::Failed { error } => stored.error = (!error.is_empty()).then_some(error),
            StageState::NotStarted
            | StageState::Running
            | StageState::Skipped
            | StageState::Stale => {}
        }
        stored
    }
}

#[cfg(test)]
mod tests {
    use super::{StageEvent, StageRecord, StageState};
    use crate::watchlist::StageStatus;
    use serde_json::json;

    fn states() -> Vec<StageState> {
        vec![
            StageState::NotStarted,
            StageState::Running,
            StageState::Waiting {
                question: json!({"kind":"import_match"}),
            },
            StageState::Complete {
                path: Some("/a.flac".into()),
            },
            StageState::Failed {
                error: "failed".into(),
            },
            StageState::Skipped,
            StageState::Stale,
        ]
    }

    #[test]
    fn a_job_hold_is_only_released_and_never_skipped_or_invalidated() {
        for state in states() {
            let held = state.status().is_held();
            assert_eq!(state.after(StageEvent::Release).is_ok(), held);
            assert_eq!(state.after(StageEvent::Skip).is_ok(), !held);
            assert_eq!(state.after(StageEvent::Invalidate).is_ok(), !held);
            for event in [
                StageEvent::Start,
                StageEvent::Wait(json!({"kind":"chapter_review"})),
                StageEvent::Complete(None),
                StageEvent::Fail("again".into()),
            ] {
                assert!(state.after(event).is_ok());
            }
        }
    }

    #[test]
    fn a_new_state_keeps_nothing_from_the_old_one() {
        let mut record = StageRecord::default();
        record
            .apply(StageEvent::Wait(json!({"kind":"import_match"})))
            .unwrap();
        record.apply(StageEvent::Fail("broken".into())).unwrap();
        assert_eq!(record.question(), None);
        assert_eq!(record.error(), Some("broken"));
        record
            .apply(StageEvent::Complete(Some("/a.flac".into())))
            .unwrap();
        assert_eq!(record.error(), None);
        assert_eq!(record.path(), Some(std::path::Path::new("/a.flac")));
        record.apply(StageEvent::Invalidate).unwrap();
        assert_eq!(record.path(), None);
        assert!(record.updated_at().is_some());
    }

    #[test]
    fn stored_records_load_without_leftover_fields() {
        let waiting: StageRecord = serde_json::from_value(json!({
            "status":"waiting","updated_at":"2026-09-30T21:03:00+02:00","path":null,
            "question":{"kind":"import_match","payload":{"task":{}}},"error":null
        }))
        .unwrap();
        assert_eq!(
            waiting.question().map(|question| &question["kind"]),
            Some(&json!("import_match"))
        );
        assert_eq!(waiting.updated_at(), Some("2026-09-30T21:03:00+02:00"));
        let complete: StageRecord =
            serde_json::from_value(json!({"status":"complete","path":"/x","error":"old"})).unwrap();
        assert_eq!(
            complete.state(),
            &StageState::Complete {
                path: Some("/x".into())
            }
        );
        assert_eq!(
            serde_json::to_value(&complete).unwrap(),
            json!({"status":"complete","updated_at":null,"path":"/x","error":null})
        );
        let stale: StageRecord = serde_json::from_value(
            json!({"status":"stale","path":"/old","question":{"kind":"import_match"}}),
        )
        .unwrap();
        assert_eq!(stale.state(), &StageState::Stale);
        let failed: StageRecord = serde_json::from_value(json!({"status":"failed"})).unwrap();
        assert_eq!(failed.error(), None);
        assert_eq!(failed.status(), StageStatus::Failed);
        assert_eq!(
            serde_json::from_value::<StageRecord>(json!({})).unwrap(),
            StageRecord::default()
        );
    }
}
