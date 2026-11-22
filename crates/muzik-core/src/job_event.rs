use serde::Serialize;
use serde_json::Value;
use strum_macros::Display;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, Serialize)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Step {
    Read,
    Download,
    Import,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Display, Serialize)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CandidateSource {
    Soulseek,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    LocalSplit,
    BandcampDownload,
    WatchlistRefresh,
    LibraryMaintenance,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobEvent {
    Message {
        message: String,
        severity: Severity,
    },
    StepStarted(Step),
    StepFinished(Step),
    ProgressStarted {
        task: Task,
        description: String,
        total: Option<u64>,
    },
    ProgressAdvanced {
        task: Task,
        completed: Option<u64>,
        total: Option<u64>,
    },
    ProgressFinished {
        task: Task,
        success: bool,
    },
    ItemWaiting {
        title: String,
        question: Value,
    },
    AgentDecided {
        label: String,
        confidence: f64,
        reason: String,
    },
    CandidatesFound {
        source: CandidateSource,
        candidates: Vec<Value>,
    },
    WatchlistSaved,
}

impl JobEvent {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message {
            message: message.into(),
            severity: Severity::Info,
        }
    }
}
