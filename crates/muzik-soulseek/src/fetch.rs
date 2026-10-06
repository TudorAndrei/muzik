use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use muzik_core::PreferredAudio;
use serde_json::Value;

use crate::job::{JobHandle, JobOutcome, JobState};
use crate::ranking::{format, rank, search_query, RankedCandidate};
use crate::session::{setting, Session};
use crate::types::{Candidate, DownloadProgress};

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

pub fn local_files(candidate: &Candidate, root: &Path) -> Result<Vec<PathBuf>, String> {
    if candidate.username.trim().is_empty() || candidate.username.chars().any(char::is_control) {
        return Err("Soulseek result has an invalid username.".into());
    }
    let mut names = HashSet::new();
    candidate
        .files
        .iter()
        .map(|remote| {
            let name = remote.name.rsplit(['/', '\\']).next().unwrap_or("");
            if name.is_empty()
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
    ) -> Result<Vec<RankedCandidate>, String> {
        if query.trim().is_empty() || query.chars().any(char::is_control) {
            return Err("Enter a Soulseek search without control characters.".into());
        }
        let job = self.start_track_search(search_query(query, prefer), timeout);
        match wait(&job, timeout, cancelled, "search") {
            Ok(JobOutcome::Search(candidates)) => Ok(rank(candidates, query, prefer, limit)),
            Ok(JobOutcome::Download(_)) => Err("Soulseek returned a download for a search.".into()),
            Err(error) => {
                if !cancelled.load(Ordering::SeqCst) {
                    Session::forget_shared();
                }
                Err(error)
            }
        }
    }

    pub fn fetch(
        &self,
        candidate: &Candidate,
        destination: &Path,
        timeout: f64,
        cancelled: &AtomicBool,
    ) -> Result<Vec<PathBuf>, String> {
        let files = local_files(candidate, destination)?;
        std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        for (remote, local) in candidate.files.iter().zip(&files) {
            let job = self
                .start_download(
                    candidate.username.clone(),
                    remote.name.clone(),
                    remote.size,
                    destination.to_string_lossy().into_owned(),
                )
                .map_err(|error| format!("Soulseek download failed: {error}"))?;
            match wait(&job, timeout, cancelled, "download")? {
                JobOutcome::Download(DownloadProgress::Completed) if local.is_file() => {}
                JobOutcome::Download(DownloadProgress::Completed) => {
                    return Err(format!(
                        "Soulseek reported a completed transfer, but {} is missing.",
                        local.display()
                    ));
                }
                JobOutcome::Download(_) => {
                    return Err("Soulseek download did not complete.".into());
                }
                JobOutcome::Search(_) => {
                    return Err("Soulseek returned a search for a download.".into());
                }
            }
        }
        Ok(files)
    }
}

fn wait(
    job: &Arc<JobHandle>,
    timeout: f64,
    cancelled: &AtomicBool,
    name: &str,
) -> Result<JobOutcome, String> {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout.clamp(1.0, 3_600.0) + 5.0);
    loop {
        if cancelled.load(Ordering::SeqCst) {
            job.cancel();
            return Err(format!("Soulseek {name} cancelled"));
        }
        match job.snapshot() {
            JobState::Running if Instant::now() >= deadline => {
                job.cancel();
                return Err(format!("Soulseek {name} timed out"));
            }
            JobState::Running => std::thread::sleep(Duration::from_millis(100)),
            JobState::Completed(outcome) => return Ok(outcome),
            JobState::Failed(reason) => return Err(format!("Soulseek {name} failed: {reason}")),
            JobState::Cancelled => return Err(format!("Soulseek {name} cancelled")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{local_files, wait, Timeouts};
    use crate::job::{JobHandle, JobOutcome, JobState};
    use crate::types::{Candidate, FileEntry};
    use serde_json::json;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
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
        let mut nameless = candidate(&["Song.flac"]);
        nameless.username = " ".into();
        assert!(local_files(&nameless, root).is_err());
        Ok(())
    }

    #[test]
    fn audio_only_drops_other_files_and_keeps_the_limit() {
        let audio = candidate(&["A\\cover.jpg", "A\\01.flac", "A\\02.mp3"]).audio_only(1);
        assert_eq!(audio.files.len(), 1);
        assert_eq!(audio.files[0].name, "A\\01.flac");
    }

    #[test]
    fn wait_returns_the_outcome_or_stops_on_cancel() -> Result<(), String> {
        let job = JobHandle::new();
        let worker = Arc::clone(&job);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            worker.finish(JobState::Completed(JobOutcome::Search(Vec::new())));
        });
        assert_eq!(
            wait(&job, 5.0, &AtomicBool::new(false), "search")?,
            JobOutcome::Search(Vec::new())
        );
        let running = JobHandle::new();
        let started = Instant::now();
        let error = wait(&running, 5.0, &AtomicBool::new(true), "download")
            .err()
            .ok_or("a cancelled wait must fail")?;
        assert_eq!(error, "Soulseek download cancelled");
        assert!(running.is_cancelled());
        assert!(started.elapsed() < Duration::from_secs(1));
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
