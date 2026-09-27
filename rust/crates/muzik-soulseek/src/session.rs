//! Owns a connected `soulseek_rs::Client` and starts pollable jobs on it.
//!
//! `Client::connect` takes `&mut self`; every other operation used here takes
//! `&self` and is safe to call from multiple threads (its fields are each an
//! `Arc<RwLock<_>>` internally). So a session connects once, wraps the
//! client in an `Arc`, and spawns one plain OS thread per job — no async
//! runtime is needed because `soulseek_rs` itself is synchronous.

use std::sync::mpsc::RecvTimeoutError;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use soulseek_rs::types::DownloadMetadata;
use soulseek_rs::{Client, ClientSettings, PeerAddress};

use crate::error::BridgeError;
use crate::job::{JobHandle, JobOutcome, JobState};
use crate::types::{Candidate, DownloadProgress, DownloadTarget};

pub struct SessionSettings {
    pub username: String,
    pub password: String,
    pub server_host: Option<String>,
    pub server_port: Option<u16>,
    pub enable_listen: Option<bool>,
    pub listen_port: Option<u16>,
}

pub struct Session {
    client: Arc<Client>,
}

impl Session {
    pub fn connect(settings: SessionSettings) -> Result<Self, BridgeError> {
        tracing::debug!("connect to Soulseek server");
        let mut client_settings = ClientSettings::new(settings.username, settings.password);
        if let (Some(host), Some(port)) = (settings.server_host, settings.server_port) {
            client_settings.server_address = PeerAddress::new(host, port);
        }
        if let Some(enable_listen) = settings.enable_listen {
            client_settings.enable_listen = enable_listen;
        }
        if let Some(listen_port) = settings.listen_port {
            client_settings.listen_port = listen_port;
        }

        let mut client = Client::with_settings(client_settings);
        client.connect()?;
        if !client.login()? {
            return Err(BridgeError::AuthenticationFailed);
        }
        tracing::info!("Soulseek login succeeded");
        Ok(Self {
            client: Arc::new(client),
        })
    }

    #[must_use]
    pub fn start_track_search(&self, query: String, timeout_secs: f64) -> Arc<JobHandle> {
        tracing::debug!(timeout_secs, "start Soulseek track search");
        let handle = JobHandle::new();
        let cancel = handle.cancel_flag();
        let worker = Arc::clone(&handle);
        let client = Arc::clone(&self.client);
        thread::spawn(move || {
            let timeout = Duration::from_secs_f64(timeout_secs.max(0.0));
            match client.search_with_cancel(&query, timeout, Some(cancel)) {
                Ok(results) if worker.is_cancelled() => {
                    worker.finish(JobState::Cancelled);
                    let _ = results; // whatever arrived before cancel, discarded
                }
                Ok(results) => {
                    let candidates: Vec<Candidate> =
                        results.into_iter().map(Candidate::from).collect();
                    worker.finish(JobState::Completed(JobOutcome::Search(candidates)));
                }
                Err(err) => {
                    worker.finish(JobState::Failed(BridgeError::from(err).to_string()));
                }
            }
        });
        handle
    }

    pub fn start_download(
        &self,
        username: String,
        filename: String,
        size: u64,
        destination: String,
    ) -> Result<Arc<JobHandle>, BridgeError> {
        tracing::debug!("start Soulseek download");
        let handle = JobHandle::new();
        let worker = Arc::clone(&handle);

        let (download, receiver) = self.client.download_with_metadata(
            filename,
            username,
            size,
            destination,
            DownloadMetadata::default(),
        )?;
        let target = DownloadTarget::from(&download);
        let client = Arc::clone(&self.client);

        thread::spawn(move || {
            // Poll rather than block on the channel forever, so a cancel
            // request is noticed even between status messages.
            loop {
                match receiver.recv_timeout(Duration::from_millis(200)) {
                    Ok(status) => {
                        let progress = DownloadProgress::from(&status);
                        if progress.is_finished() {
                            finish_from_progress(&worker, progress);
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if worker.is_cancelled() {
                            let _ = client.remove_download(&target.username, &target.filename);
                            worker.finish(JobState::Cancelled);
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        worker.finish(JobState::Failed(
                            "download channel closed unexpectedly".to_string(),
                        ));
                        return;
                    }
                }
            }
        });
        Ok(handle)
    }

    /// `soulseek_rs::Client` has no explicit teardown call; dropping the
    /// session's last `Arc<Client>` ends the connection. This method exists
    /// so the Python-facing lifecycle stays symmetric with `connect()` — it
    /// is intentionally a no-op beyond that.
    pub fn close(&self) {}
}

fn finish_from_progress(worker: &JobHandle, progress: DownloadProgress) {
    match progress {
        DownloadProgress::Completed => {
            worker.finish(JobState::Completed(JobOutcome::Download(progress)));
        }
        DownloadProgress::Failed(reason) => {
            worker.finish(JobState::Failed(
                reason.unwrap_or_else(|| "download failed".to_string()),
            ));
        }
        DownloadProgress::TimedOut => {
            worker.finish(JobState::Failed("download timed out".to_string()));
        }
        DownloadProgress::Queued
        | DownloadProgress::InProgress { .. }
        | DownloadProgress::Paused { .. } => {
            unreachable!("finish_from_progress is only called when is_finished() is true")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DownloadProgress;

    #[test]
    fn finish_from_progress_maps_completed_to_a_completed_job() {
        let job = JobHandle::new();
        finish_from_progress(&job, DownloadProgress::Completed);
        assert_eq!(
            job.result(),
            Ok(JobOutcome::Download(DownloadProgress::Completed))
        );
    }

    #[test]
    fn finish_from_progress_maps_failed_to_a_failed_job_with_its_reason() {
        let job = JobHandle::new();
        finish_from_progress(
            &job,
            DownloadProgress::Failed(Some("peer went offline".to_string())),
        );
        assert_eq!(
            job.result(),
            Err(BridgeError::JobFailed("peer went offline".to_string()))
        );
    }

    #[test]
    fn finish_from_progress_gives_failed_downloads_without_a_reason_a_message() {
        let job = JobHandle::new();
        finish_from_progress(&job, DownloadProgress::Failed(None));
        assert_eq!(
            job.result(),
            Err(BridgeError::JobFailed("download failed".to_string()))
        );
    }

    #[test]
    fn finish_from_progress_maps_timed_out_to_a_failed_job() {
        let job = JobHandle::new();
        finish_from_progress(&job, DownloadProgress::TimedOut);
        assert_eq!(
            job.result(),
            Err(BridgeError::JobFailed("download timed out".to_string()))
        );
    }

    #[test]
    fn session_settings_defaults_leave_optional_fields_unset() {
        let settings = SessionSettings {
            username: "alice".to_string(),
            password: "secret".to_string(),
            server_host: None,
            server_port: None,
            enable_listen: None,
            listen_port: None,
        };
        assert_eq!(settings.username, "alice");
        assert!(settings.server_host.is_none());
    }

    #[test]
    fn a_cancel_flag_set_before_any_worker_reads_it_is_observed() {
        // Regression guard for the search worker's cancel check: setting the
        // flag through one Arc must be visible through a clone taken before
        // the flag was set, since the worker thread clones it up front.
        use std::sync::atomic::Ordering;

        let job = JobHandle::new();
        let flag = job.cancel_flag();
        job.cancel();
        assert!(flag.load(Ordering::Relaxed));
    }
}
