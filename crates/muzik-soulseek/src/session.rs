//! Owns a connected `soulseek_rs::Client` and starts pollable jobs on it.
//!
//! `Client::connect` takes `&mut self`; every other operation used here takes
//! `&self` and is safe to call from multiple threads (its fields are each an
//! `Arc<RwLock<_>>` internally). So a session connects once, wraps the
//! client in an `Arc`, and spawns one plain OS thread per job — no async
//! runtime is needed because `soulseek_rs` itself is synchronous.

use std::env;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use soulseek_rs::types::DownloadMetadata;
use soulseek_rs::{Client, ClientSettings, PeerAddress};

use crate::error::BridgeError;
use crate::job::{JobHandle, JobOutcome, JobState};
use crate::types::{Candidate, DownloadProgress, DownloadTarget};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSettings {
    pub username: String,
    pub password: String,
    pub server_host: Option<String>,
    pub server_port: Option<u16>,
    pub enable_listen: Option<bool>,
    pub listen_port: Option<u16>,
}

impl SessionSettings {
    /// Read the settings used by both native apps. Missing credentials leave
    /// Soulseek unconfigured rather than attempting a network connection.
    pub fn configured(config: &Value) -> Option<Self> {
        let username = setting(config, "MUZIK_SOULSEEK_USERNAME", "username")?;
        let password = setting(config, "MUZIK_SOULSEEK_PASSWORD", "password")?;
        let host = setting(config, "MUZIK_SOULSEEK_SERVER_HOST", "server_host")
            .unwrap_or_else(|| "server.slsknet.org".into());
        let port = setting(config, "MUZIK_SOULSEEK_SERVER_PORT", "server_port")
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(2416);
        let listen_port = setting(config, "MUZIK_SOULSEEK_LISTEN_PORT", "listen_port")
            .and_then(|value| value.parse::<u16>().ok());
        Some(Self {
            username,
            password,
            server_host: Some(host),
            server_port: Some(port),
            enable_listen: None,
            listen_port,
        })
    }
}

pub fn setting(config: &Value, environment: &str, key: &str) -> Option<String> {
    env::var(environment)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            config
                .get("soulseek")?
                .get(key)
                .and_then(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| value.as_u64().map(|number| number.to_string()))
                })
                .filter(|value| !value.trim().is_empty())
        })
}

pub struct Session {
    client: Arc<Client>,
}

type Shared = Option<(SessionSettings, Arc<Session>)>;

static SHARED: Mutex<Shared> = Mutex::new(None);

impl Session {
    pub fn shared(settings: SessionSettings) -> Result<Arc<Self>, BridgeError> {
        let mut shared = SHARED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((current, session)) = shared.as_ref() {
            if *current == settings {
                return Ok(Arc::clone(session));
            }
        }
        let session = Arc::new(Self::connect(settings.clone())?);
        *shared = Some((settings, Arc::clone(&session)));
        Ok(session)
    }

    pub fn forget_shared() {
        SHARED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

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
            let timeout = if timeout_secs.is_finite() && timeout_secs >= 0.0 {
                Duration::from_secs_f64(timeout_secs.min(3_600.0))
            } else {
                Duration::from_secs(15)
            };
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
            worker.finish(JobState::Failed(
                "download stopped without a final status".into(),
            ));
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
