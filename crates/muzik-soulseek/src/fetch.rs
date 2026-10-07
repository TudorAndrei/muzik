use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use muzik_core::PreferredAudio;
use serde_json::Value;
use soulseek_rs::DownloadStatus;

use crate::error::{BridgeError, Result};
use crate::ranking::{format, rank, search_query, RankedCandidate};
use crate::session::{setting, Session};
use crate::types::Candidate;

const POLL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timeouts {
    pub search: f64,
    pub download: f64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            search: 15.0,
            download: 600.0,
        }
    }
}

impl Timeouts {
    pub fn configured(config: &Value) -> Self {
        let read = |environment, key, default, maximum| {
            setting(config, environment, key)
                .and_then(|value| value.parse::<f64>().ok())
                .filter(|value| value.is_finite() && (1.0..=maximum).contains(value))
                .unwrap_or(default)
        };
        let standard = Self::default();
        Self {
            search: read(
                "MUZIK_SOULSEEK_SEARCH_TIMEOUT",
                "search_timeout",
                standard.search,
                120.0,
            ),
            download: read(
                "MUZIK_SOULSEEK_DOWNLOAD_TIMEOUT",
                "download_timeout",
                standard.download,
                3_600.0,
            ),
        }
    }
}

impl Candidate {
    pub fn audio_only(&self, limit: usize) -> Self {
        Self {
            files: self
                .files
                .iter()
                .filter(|file| format(file).is_some())
                .take(limit)
                .cloned()
                .collect(),
            ..self.clone()
        }
    }
}

pub fn local_files(candidate: &Candidate, root: &Path) -> Result<Vec<PathBuf>> {
    if candidate.username.trim().is_empty() || candidate.username.chars().any(char::is_control) {
        return Err("Soulseek result has an invalid username.".into());
    }
    let mut names = HashSet::new();
    candidate
        .files
        .iter()
        .map(|remote| {
            let name = remote.name.rsplit(['/', '\\']).next().unwrap_or("");
            if matches!(name, "" | "." | "..")
                || name.chars().any(char::is_control)
                || !names.insert(name.to_ascii_lowercase())
            {
                return Err("Soulseek result has missing or duplicate file names.".into());
            }
            Ok(root.join(name))
        })
        .collect()
}

impl Session {
    pub fn search(
        &self,
        query: &str,
        prefer: PreferredAudio,
        limit: usize,
        timeout: f64,
        cancelled: &AtomicBool,
    ) -> Result<Vec<RankedCandidate>> {
        if query.trim().is_empty() || query.chars().any(char::is_control) {
            return Err("Enter a Soulseek search without control characters.".into());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let found = thread::scope(|scope| {
            let search = scope.spawn(|| {
                self.client.search_with_cancel(
                    &search_query(query, prefer),
                    seconds(timeout),
                    Some(Arc::clone(&stop)),
                )
            });
            while !search.is_finished() {
                if cancelled.load(Ordering::SeqCst) {
                    stop.store(true, Ordering::SeqCst);
                }
                thread::sleep(POLL);
            }
            search.join()
        });
        if cancelled.load(Ordering::SeqCst) {
            return Err("Soulseek search cancelled".into());
        }
        match found {
            Ok(Ok(results)) => Ok(rank(
                results.into_iter().map(Candidate::from).collect(),
                query,
                prefer,
                limit,
            )),
            Ok(Err(error)) => {
                Session::forget_shared();
                Err(format!("Soulseek search failed: {}", BridgeError::from(error)).into())
            }
            Err(_) => Err("Soulseek search failed: the search thread stopped".into()),
        }
    }

    pub fn fetch(
        &self,
        candidate: &Candidate,
        destination: &Path,
        timeout: f64,
        cancelled: &AtomicBool,
    ) -> Result<Vec<PathBuf>> {
        let files = local_files(candidate, destination)?;
        std::fs::create_dir_all(destination)?;
        for (remote, local) in candidate.files.iter().zip(&files) {
            let (download, receiver) = self
                .client
                .download(
                    remote.name.clone(),
                    candidate.username.clone(),
                    remote.size,
                    destination.to_string_lossy().into_owned(),
                )
                .map_err(|error| {
                    format!("Soulseek download failed: {}", BridgeError::from(error))
                })?;
            let finished = Instant::now()
                .checked_add(seconds(timeout).saturating_add(Duration::from_secs(5)))
                .ok_or_else(|| BridgeError::from("Soulseek download timeout is too long."))
                .and_then(|deadline| finish(&receiver, deadline, cancelled));
            if let Err(error) = finished {
                let _ = self
                    .client
                    .cancel_download(&download.username, &download.filename);
                return Err(error);
            }
            if !local.is_file() {
                return Err(format!(
                    "Soulseek reported a completed transfer, but {} is missing.",
                    local.display()
                )
                .into());
            }
        }
        Ok(files)
    }
}

fn seconds(timeout: f64) -> Duration {
    Duration::try_from_secs_f64(timeout.clamp(1.0, 3_600.0)).unwrap_or(Duration::from_secs(15))
}

fn finish(
    receiver: &Receiver<DownloadStatus>,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<()> {
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err("Soulseek download cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("Soulseek download timed out".into());
        }
        match receiver.recv_timeout(POLL) {
            Ok(DownloadStatus::Completed) => return Ok(()),
            Ok(DownloadStatus::Cancelled) => return Err("Soulseek download cancelled".into()),
            Ok(DownloadStatus::Failed(reason)) => {
                return Err(format!(
                    "Soulseek download failed: {}",
                    reason.unwrap_or_else(|| "download failed".into())
                )
                .into());
            }
            Ok(DownloadStatus::TimedOut) => {
                return Err("Soulseek download failed: download timed out".into());
            }
            Ok(
                DownloadStatus::Queued
                | DownloadStatus::InProgress { .. }
                | DownloadStatus::Paused { .. },
            )
            | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(
                    "Soulseek download failed: download channel closed unexpectedly".into(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{finish, local_files, Timeouts};
    use crate::types::{Candidate, FileEntry};
    use serde_json::json;
    use soulseek_rs::DownloadStatus;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn candidate(names: &[&str]) -> Candidate {
        Candidate {
            username: "peer".into(),
            slots: 1,
            speed: 1,
            files: names
                .iter()
                .map(|name| FileEntry {
                    name: (*name).into(),
                    size: 1,
                    bitrate_kbps: None,
                    duration_seconds: None,
                    vbr: None,
                    sample_rate_hz: None,
                    bit_depth: None,
                })
                .collect(),
        }
    }

    #[test]
    fn local_files_keep_the_last_name_and_refuse_duplicates() -> Result<(), String> {
        let root = Path::new("/music");
        assert_eq!(
            local_files(&candidate(&["Album\\01 Song.flac"]), root)?,
            [root.join("01 Song.flac")]
        );
        assert!(local_files(&candidate(&["A\\Song.flac", "B/song.FLAC"]), root).is_err());
        assert!(local_files(&candidate(&["Album\\"]), root).is_err());
        assert!(local_files(&candidate(&["Album\\.."]), root).is_err());
        assert!(local_files(&candidate(&["Album/."]), root).is_err());
        let mut nameless = candidate(&["Song.flac"]);
        nameless.username = " ".into();
        assert!(local_files(&nameless, root).is_err());
        Ok(())
    }

    #[test]
    fn local_files_stay_directly_inside_the_root_for_any_peer_names() {
        let root = Path::new("/music");
        bolero::check!()
            .with_type::<(String, Vec<Vec<u8>>)>()
            .for_each(|(username, raw)| {
                let names: Vec<String> = raw
                    .iter()
                    .map(|bytes| {
                        bytes
                            .iter()
                            .map(|byte| {
                                ['a', '.', '/', '\\', ' ', '\0', 'é'][usize::from(*byte) % 7]
                            })
                            .collect()
                    })
                    .collect();
                let mut peer = candidate(&names.iter().map(String::as_str).collect::<Vec<_>>());
                peer.username.clone_from(username);
                if let Ok(paths) = local_files(&peer, root) {
                    assert_eq!(paths.len(), names.len());
                    for path in paths {
                        assert_eq!(path.parent(), Some(root), "{path:?}");
                        assert!(path.file_name().is_some(), "{path:?}");
                    }
                }
            });
    }

    #[test]
    fn audio_only_drops_other_files_and_keeps_the_limit() {
        let audio = candidate(&["A\\cover.jpg", "A\\01.flac", "A\\02.mp3"]).audio_only(1);
        assert_eq!(audio.files.len(), 1);
        assert_eq!(audio.files[0].name, "A\\01.flac");
    }

    #[test]
    fn finish_waits_for_a_final_status_or_stops_on_cancel() -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let running = AtomicBool::new(false);
        let (sender, receiver) = mpsc::channel();
        sender
            .send(DownloadStatus::Queued)
            .map_err(|e| e.to_string())?;
        sender
            .send(DownloadStatus::Completed)
            .map_err(|e| e.to_string())?;
        finish(&receiver, deadline, &running)?;

        let (sender, receiver) = mpsc::channel();
        sender
            .send(DownloadStatus::Failed(Some("peer went offline".into())))
            .map_err(|e| e.to_string())?;
        assert_eq!(
            finish(&receiver, deadline, &running).map_err(String::from),
            Err("Soulseek download failed: peer went offline".into())
        );

        let (_sender, receiver) = mpsc::channel();
        let started = Instant::now();
        assert_eq!(
            finish(&receiver, deadline, &AtomicBool::new(true)).map_err(String::from),
            Err("Soulseek download cancelled".into())
        );
        assert!(started.elapsed() < Duration::from_secs(1));

        let (sender, receiver) = mpsc::channel::<DownloadStatus>();
        drop(sender);
        assert!(finish(&receiver, deadline, &running).is_err());
        Ok(())
    }

    #[test]
    fn timeouts_stay_in_range() {
        assert_eq!(Timeouts::configured(&json!({})), Timeouts::default());
        let set = Timeouts::configured(
            &json!({"soulseek":{"search_timeout":"30","download_timeout":"99999"}}),
        );
        assert!((set.search - 30.0).abs() < f64::EPSILON);
        assert!((set.download - 600.0).abs() < f64::EPSILON);
    }
}
