//! Pollable job state shared between a background worker thread and the
//! host-facing `SeakarrJob`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::error::BridgeError;
use crate::types::{Candidate, DownloadProgress};

#[derive(Debug, Clone, PartialEq)]
pub enum JobOutcome {
    Search(Vec<Candidate>),
    Download(DownloadProgress),
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    Running,
    Completed(JobOutcome),
    Failed(String),
    Cancelled,
}

/// Shared handle a worker thread finishes and the Python wrapper polls.
///
/// A worker only ever calls [`JobHandle::finish`] once; a caller can call
/// [`JobHandle::cancel`] at any time, but the worker decides when — and
/// whether — that request lands, since a job that already completed (or
/// failed) on its own must not be overwritten by a late cancel.
pub struct JobHandle {
    state: Mutex<JobState>,
    cancel: Arc<AtomicBool>,
}

impl JobHandle {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(JobState::Running),
            cancel: Arc::new(AtomicBool::new(false)),
        })
    }

    #[must_use]
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn snapshot(&self) -> JobState {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Record the job's terminal state, unless it is already terminal.
    pub fn finish(&self, state: JobState) {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if matches!(*guard, JobState::Running) {
            *guard = state;
        }
    }

    pub fn result(&self) -> Result<JobOutcome, BridgeError> {
        match self.snapshot() {
            JobState::Running => Err(BridgeError::JobNotFinished),
            JobState::Completed(outcome) => Ok(outcome),
            JobState::Failed(reason) => Err(BridgeError::JobFailed(reason)),
            JobState::Cancelled => Err(BridgeError::JobCancelled),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_job_is_running_and_not_cancelled() {
        let job = JobHandle::new();
        assert_eq!(job.snapshot(), JobState::Running);
        assert!(!job.is_cancelled());
        assert_eq!(job.result(), Err(BridgeError::JobNotFinished));
    }

    #[test]
    fn cancel_sets_the_flag_but_does_not_finish_the_job_by_itself() {
        let job = JobHandle::new();
        job.cancel();
        assert!(job.is_cancelled());
        // Only the worker thread observing the flag decides the outcome.
        assert_eq!(job.snapshot(), JobState::Running);
    }

    #[test]
    fn finish_records_the_outcome_and_result_returns_it() {
        let job = JobHandle::new();
        let outcome = JobOutcome::Search(vec![]);
        job.finish(JobState::Completed(outcome.clone()));
        assert_eq!(job.result(), Ok(outcome));
    }

    #[test]
    fn a_late_finish_cannot_overwrite_an_already_terminal_job() {
        let job = JobHandle::new();
        job.finish(JobState::Cancelled);
        // A worker racing a cancel must not clobber it with a late success.
        job.finish(JobState::Completed(JobOutcome::Search(vec![])));
        assert_eq!(job.snapshot(), JobState::Cancelled);
    }

    #[test]
    fn failed_and_cancelled_results_carry_the_matching_bridge_error() {
        let failed = JobHandle::new();
        failed.finish(JobState::Failed("peer offline".to_string()));
        assert_eq!(
            failed.result(),
            Err(BridgeError::JobFailed("peer offline".to_string()))
        );

        let cancelled = JobHandle::new();
        cancelled.finish(JobState::Cancelled);
        assert_eq!(cancelled.result(), Err(BridgeError::JobCancelled));
    }

    #[test]
    fn cancel_flag_clone_observes_cancellation_from_another_handle() {
        let job = JobHandle::new();
        let flag = job.cancel_flag();
        assert!(!flag.load(Ordering::Relaxed));
        job.cancel();
        assert!(flag.load(Ordering::Relaxed));
    }
}
