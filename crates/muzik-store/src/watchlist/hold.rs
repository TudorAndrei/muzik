use super::{ItemAction, ItemId, Stage, StageStatus, WatchItem, Watchlist};
use crate::Result;
use crate::jobs::{self, Job, Status};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Hold {
    Running {
        stage: Stage,
        job_id: i64,
    },
    Waiting {
        stage: Stage,
        job_id: i64,
        question: Value,
    },
}

impl Hold {
    fn of(job: Job) -> Option<Self> {
        let action = job
            .params
            .get("action")
            .and_then(Value::as_str)
            .and_then(|action| action.parse::<ItemAction>().ok());
        match job.status {
            Status::Running => Some(Self::Running {
                stage: action.map_or(Stage::Download, ItemAction::stage),
                job_id: job.id,
            }),
            Status::Waiting => {
                let question = job.question.unwrap_or_default();
                let asked = question
                    .get("kind")
                    .and_then(Value::as_str)
                    .and_then(|kind| kind.parse().ok())
                    .map(Stage::of_decision);
                Some(Self::Waiting {
                    stage: asked
                        .or_else(|| action.map(ItemAction::stage))
                        .unwrap_or(Stage::Download),
                    job_id: job.id,
                    question,
                })
            }
            Status::Queued | Status::Done | Status::Failed | Status::Cancelled => None,
        }
    }

    #[must_use]
    pub const fn stage(&self) -> Stage {
        match self {
            Self::Running { stage, .. } | Self::Waiting { stage, .. } => *stage,
        }
    }

    #[must_use]
    pub const fn status(&self) -> StageStatus {
        match self {
            Self::Running { .. } => StageStatus::Running,
            Self::Waiting { .. } => StageStatus::Waiting,
        }
    }

    #[must_use]
    pub const fn job_id(&self) -> i64 {
        match self {
            Self::Running { job_id, .. } | Self::Waiting { job_id, .. } => *job_id,
        }
    }

    #[must_use]
    pub const fn question(&self) -> Option<&Value> {
        match self {
            Self::Waiting { question, .. } => Some(question),
            Self::Running { .. } => None,
        }
    }

    #[must_use]
    pub const fn is_waiting(&self) -> bool {
        matches!(self, Self::Waiting { .. })
    }
}

/// The Running and Waiting stages of watchlist items, read from the open item jobs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Holds(HashMap<ItemId, Hold>);

impl Holds {
    /// # Errors
    /// Returns an error if the jobs cannot be read.
    pub fn read(connection: &Connection) -> Result<Self> {
        let mut holds = HashMap::new();
        for job in jobs::held_on(connection)? {
            let Ok(id) = job.item_key.parse::<ItemId>() else {
                continue;
            };
            if let Some(hold) = Hold::of(job) {
                let running = matches!(hold, Hold::Running { .. });
                if running || !holds.contains_key(&id) {
                    holds.insert(id, hold);
                }
            }
        }
        Ok(Self(holds))
    }

    #[must_use]
    pub fn get(&self, id: &ItemId) -> Option<&Hold> {
        self.0.get(id)
    }

    #[must_use]
    pub fn is_waiting(&self, id: &ItemId) -> bool {
        self.get(id).is_some_and(Hold::is_waiting)
    }
}

/// The status of `stage` with the job that holds it, if any.
#[must_use]
pub fn status(item: &WatchItem, hold: Option<&Hold>, stage: Stage) -> StageStatus {
    hold.filter(|hold| hold.stage() == stage)
        .map_or_else(|| item.status(stage), Hold::status)
}

/// Saved items with the jobs that hold them, read in one transaction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub document: Watchlist,
    pub holds: Holds,
}

impl Snapshot {
    #[must_use]
    pub const fn new(document: Watchlist, holds: Holds) -> Self {
        Self { document, holds }
    }
}
