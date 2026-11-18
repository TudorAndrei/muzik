use super::{StageStatus, now};
use serde::{Deserialize, Serialize};
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
    Complete(Option<PathBuf>),
    Fail(String),
    Skip,
    Invalidate,
}

impl StageState {
    #[must_use]
    pub const fn status(&self) -> StageStatus {
        match self {
            Self::NotStarted => StageStatus::NotStarted,
            Self::Complete { .. } => StageStatus::Complete,
            Self::Failed { .. } => StageStatus::Failed,
            Self::Skipped => StageStatus::Skipped,
            Self::Stale => StageStatus::Stale,
        }
    }

    #[must_use]
    pub fn after(event: StageEvent) -> Self {
        match event {
            StageEvent::Complete(path) => Self::Complete { path },
            StageEvent::Fail(error) => Self::Failed { error },
            StageEvent::Skip => Self::Skipped,
            StageEvent::Invalidate => Self::Stale,
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

    pub(super) fn apply(&mut self, event: StageEvent) {
        self.state = StageState::after(event);
        self.updated_at = Some(now());
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
}

impl From<StoredRecord> for StageRecord {
    fn from(stored: StoredRecord) -> Self {
        let state = match stored.status {
            StageStatus::NotStarted | StageStatus::Running | StageStatus::Waiting => {
                StageState::NotStarted
            }
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
            StageState::Complete { path } => stored.path = path,
            StageState::Failed { error } => stored.error = (!error.is_empty()).then_some(error),
            StageState::NotStarted | StageState::Skipped | StageState::Stale => {}
        }
        stored
    }
}

#[cfg(test)]
mod tests {
    use super::{StageEvent, StageRecord, StageState};
    use crate::watchlist::StageStatus;
    use serde_json::json;

    #[test]
    fn a_new_state_keeps_nothing_from_the_old_one() {
        let mut record = StageRecord::default();
        record.apply(StageEvent::Fail("broken".into()));
        assert_eq!(record.error(), Some("broken"));
        record.apply(StageEvent::Complete(Some("/a.flac".into())));
        assert_eq!(record.error(), None);
        assert_eq!(record.path(), Some(std::path::Path::new("/a.flac")));
        record.apply(StageEvent::Invalidate);
        assert_eq!(record.path(), None);
        assert!(record.updated_at().is_some());
    }

    #[test]
    fn stored_records_load_without_leftover_fields() {
        let held: StageRecord = serde_json::from_value(json!({
            "status":"waiting","updated_at":"2026-09-30T21:03:00+02:00","path":null,
            "question":{"kind":"import_match","payload":{"task":{}}},"error":null
        }))
        .unwrap();
        assert_eq!(held.state(), &StageState::NotStarted);
        assert_eq!(held.updated_at(), Some("2026-09-30T21:03:00+02:00"));
        assert_eq!(
            serde_json::to_value(&held).unwrap(),
            json!({"status":"not_started","updated_at":"2026-09-30T21:03:00+02:00","path":null,"error":null})
        );
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
        let stale: StageRecord =
            serde_json::from_value(json!({"status":"stale","path":"/old"})).unwrap();
        assert_eq!(stale.state(), &StageState::Stale);
        let failed: StageRecord = serde_json::from_value(json!({"status":"failed"})).unwrap();
        assert_eq!(failed.error(), None);
        assert_eq!(failed.status(), StageStatus::Failed);
        assert_eq!(
            serde_json::from_value::<StageRecord>(json!({"status":"running"})).unwrap(),
            StageRecord::default()
        );
        assert_eq!(
            serde_json::from_value::<StageRecord>(json!({})).unwrap(),
            StageRecord::default()
        );
    }
}
