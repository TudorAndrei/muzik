use std::collections::{HashMap, HashSet, VecDeque};

use muzik_core::{DecisionKind, JobEvent, Task};
use muzik_runner::AppEvent;
use parking_lot::Mutex;
use serde_json::{Value, json};

const LOG_LIMIT: usize = 200;
const FINISHED_LIMIT: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub status: String,
    pub task: Option<Task>,
    pub completed: u64,
    pub total: Option<u64>,
}

impl Progress {
    #[must_use]
    pub fn percent(&self) -> Option<u64> {
        let total = self.total.filter(|total| *total > 0)?;
        Some(
            self.completed
                .saturating_mul(100)
                .checked_div(total)
                .unwrap_or(0)
                .min(100),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveDecision {
    pub decision_id: String,
    pub job_id: String,
    pub question: Value,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub progress: HashMap<String, Progress>,
    pub decisions: Vec<LiveDecision>,
    pub log: Vec<String>,
}

#[derive(Default)]
struct State {
    progress: HashMap<String, Progress>,
    decisions: Vec<LiveDecision>,
    log: VecDeque<String>,
    finished: VecDeque<String>,
    finished_set: HashSet<String>,
}

#[derive(Default)]
pub struct Hub {
    state: Mutex<State>,
}

impl Hub {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&self, event: &AppEvent) {
        self.state.lock().apply(event);
    }

    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        let state = self.state.lock();
        Snapshot {
            progress: state.progress.clone(),
            decisions: state.decisions.clone(),
            log: state.log.iter().rev().cloned().collect(),
        }
    }

    #[must_use]
    pub fn decision(&self, decision_id: &str) -> Option<LiveDecision> {
        self.state
            .lock()
            .decisions
            .iter()
            .find(|decision| decision.decision_id == decision_id)
            .cloned()
    }

    pub fn resolve(&self, decision_id: &str) {
        self.state
            .lock()
            .decisions
            .retain(|decision| decision.decision_id != decision_id);
    }
}

impl State {
    fn apply(&mut self, event: &AppEvent) {
        if let AppEvent::JobStarted { job_id, .. } = event {
            self.finished_set.remove(job_id);
        }
        if event
            .job_id()
            .is_some_and(|id| self.finished_set.contains(id))
        {
            return;
        }
        match event {
            AppEvent::JobStarted { job_id, title, .. } => {
                self.entry(job_id).status = "Starting".into();
                self.push_log(format!("Started {title}"));
            }
            AppEvent::JobEvent { job_id, event, .. } => self.job_event(job_id, event),
            AppEvent::DecisionRequest {
                job_id,
                decision_id,
                kind,
                payload,
            } => {
                if !self
                    .decisions
                    .iter()
                    .any(|decision| decision.decision_id == *decision_id)
                {
                    self.decisions.push(LiveDecision {
                        decision_id: decision_id.clone(),
                        job_id: job_id.clone(),
                        question: question(*kind, payload),
                    });
                }
                self.entry(job_id).status = "Decision needed".into();
            }
            AppEvent::JobCompleted { job_id, .. } => self.finish(job_id, "completed"),
            AppEvent::JobCancelled { job_id } => self.finish(job_id, "cancelled"),
            AppEvent::JobFailed { job_id, message } => {
                self.finish(job_id, &format!("failed: {message}"));
            }
            _ => {}
        }
    }

    fn entry(&mut self, job_id: &str) -> &mut Progress {
        self.progress.entry(job_id.to_owned()).or_default()
    }

    fn finish(&mut self, job_id: &str, ending: &str) {
        self.progress.remove(job_id);
        self.decisions.retain(|decision| decision.job_id != job_id);
        self.push_log(format!("Job {job_id} {ending}"));
        if self.finished_set.insert(job_id.to_owned()) {
            self.finished.push_back(job_id.to_owned());
        }
        while self.finished.len() > FINISHED_LIMIT {
            if let Some(oldest) = self.finished.pop_front() {
                self.finished_set.remove(&oldest);
            }
        }
    }

    fn push_log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > LOG_LIMIT {
            self.log.pop_front();
        }
    }

    fn job_event(&mut self, job_id: &str, event: &JobEvent) {
        match event {
            JobEvent::ProgressStarted {
                task,
                description,
                total,
            } => {
                let progress = self.entry(job_id);
                progress.task = Some(*task);
                progress.status.clone_from(description);
                progress.completed = 0;
                progress.total = total.filter(|total| *total > 0);
            }
            JobEvent::ProgressAdvanced {
                task,
                completed,
                total,
            } => {
                let progress = self.entry(job_id);
                if progress.task != Some(*task) {
                    return;
                }
                if let Some(total) = total.filter(|total| *total > 0) {
                    progress.total = Some(total);
                }
                progress.completed =
                    completed.unwrap_or_else(|| progress.completed.saturating_add(1));
            }
            JobEvent::ProgressFinished { task, .. } => {
                let progress = self.entry(job_id);
                if progress.task != Some(*task) {
                    return;
                }
                if let Some(total) = progress.total {
                    progress.completed = total;
                }
            }
            JobEvent::StepStarted(step) => {
                self.entry(job_id).status = step.to_string();
            }
            JobEvent::StepFinished(_) => {
                let progress = self.entry(job_id);
                if progress.total.is_some() {
                    progress.completed = progress.completed.saturating_add(1);
                }
            }
            JobEvent::Message { message, .. } => {
                self.entry(job_id).status.clone_from(message);
                self.push_log(message.clone());
            }
            JobEvent::ItemWaiting { title, .. } => {
                self.entry(job_id).status = "Waiting for you".into();
                self.push_log(format!("{title} waits for you"));
            }
            JobEvent::AgentDecided {
                label,
                confidence,
                reason,
            } => {
                self.entry(job_id).status = format!("Chose {label}");
                self.push_log(format!("Chose {label} ({confidence:.0}): {reason}"));
            }
            JobEvent::CandidatesFound { .. } | JobEvent::WatchlistSaved => {}
        }
    }
}

fn question(kind: DecisionKind, payload: &Value) -> Value {
    json!({"kind": kind, "payload": payload})
}

#[cfg(test)]
mod tests {
    use super::Hub;
    use muzik_core::{JobEvent, Task};
    use muzik_runner::{AppEvent, Source};
    use serde_json::json;

    fn event(event: JobEvent) -> AppEvent {
        AppEvent::JobEvent {
            job_id: "queue-1".into(),
            source: Source::Native,
            event,
        }
    }

    #[test]
    fn progress_follows_the_events_of_one_job_until_it_finishes() {
        let hub = Hub::new();
        hub.apply(&event(JobEvent::ProgressStarted {
            task: Task::LocalSplit,
            description: "Split tracks".into(),
            total: Some(10),
        }));
        hub.apply(&event(JobEvent::ProgressAdvanced {
            task: Task::LocalSplit,
            completed: Some(4),
            total: None,
        }));
        hub.apply(&event(JobEvent::ProgressAdvanced {
            task: Task::WatchlistRefresh,
            completed: Some(9),
            total: None,
        }));
        let progress = hub.snapshot().progress["queue-1"].clone();
        assert_eq!(progress.status, "Split tracks");
        assert_eq!(progress.percent(), Some(40));
        hub.apply(&AppEvent::JobCompleted {
            job_id: "queue-1".into(),
            result: json!(null),
        });
        hub.apply(&event(JobEvent::message("late")));
        assert!(hub.snapshot().progress.is_empty());
    }

    #[test]
    fn a_decision_waits_until_it_is_resolved_or_its_job_ends() {
        let hub = Hub::new();
        let request = |id: &str, job: &str| AppEvent::DecisionRequest {
            job_id: job.into(),
            decision_id: id.into(),
            kind: muzik_core::DecisionKind::ImportDuplicate,
            payload: json!({}),
        };
        hub.apply(&request("d1", "queue-1"));
        hub.apply(&request("d1", "queue-1"));
        hub.apply(&request("d2", "queue-2"));
        assert_eq!(hub.snapshot().decisions.len(), 2);
        assert_eq!(
            hub.decision("d1").unwrap().question["kind"],
            "import_duplicate"
        );
        hub.resolve("d1");
        assert!(hub.decision("d1").is_none());
        hub.apply(&AppEvent::JobCancelled {
            job_id: "queue-2".into(),
        });
        assert_eq!(hub.snapshot().decisions, Vec::new());
    }
}
