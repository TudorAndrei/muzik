//! Candidate/progress types the bridge hands to Python, and their
//! conversions from `soulseek_rs` wire types. Kept free of PyO3 so this
//! module is unit-testable without a Python interpreter.

use soulseek_rs::types::{Download as WireDownload, DownloadStatus as WireDownloadStatus};
use soulseek_rs::{File as WireFile, SearchResult as WireSearchResult};

#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
}

impl From<&WireFile> for FileEntry {
    fn from(file: &WireFile) -> Self {
        Self {
            name: file.name.clone(),
            size: file.size,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub username: String,
    pub slots: u8,
    pub speed: u32,
    pub files: Vec<FileEntry>,
}

impl From<WireSearchResult> for Candidate {
    fn from(result: WireSearchResult) -> Self {
        Self {
            username: result.username,
            slots: result.slots,
            speed: result.speed,
            files: result.files.iter().map(FileEntry::from).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadProgress {
    Queued,
    InProgress {
        bytes_downloaded: u64,
        total_bytes: u64,
        speed_bytes_per_sec: f64,
    },
    Paused {
        bytes_downloaded: u64,
        total_bytes: u64,
    },
    Completed,
    Failed(Option<String>),
    TimedOut,
}

impl From<&WireDownloadStatus> for DownloadProgress {
    fn from(status: &WireDownloadStatus) -> Self {
        match status {
            WireDownloadStatus::Queued => Self::Queued,
            WireDownloadStatus::InProgress {
                bytes_downloaded,
                total_bytes,
                speed_bytes_per_sec,
            } => Self::InProgress {
                bytes_downloaded: *bytes_downloaded,
                total_bytes: *total_bytes,
                speed_bytes_per_sec: *speed_bytes_per_sec,
            },
            WireDownloadStatus::Paused {
                bytes_downloaded,
                total_bytes,
            } => Self::Paused {
                bytes_downloaded: *bytes_downloaded,
                total_bytes: *total_bytes,
            },
            WireDownloadStatus::Completed => Self::Completed,
            WireDownloadStatus::Failed(reason) => Self::Failed(reason.clone()),
            WireDownloadStatus::TimedOut => Self::TimedOut,
        }
    }
}

impl DownloadProgress {
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed(_) | Self::TimedOut)
    }
}

/// Identity of an in-flight or queued download, used to target
/// `Client::remove_download` on cancellation.
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadTarget {
    pub username: String,
    pub filename: String,
}

impl From<&WireDownload> for DownloadTarget {
    fn from(download: &WireDownload) -> Self {
        Self {
            username: download.username.clone(),
            filename: download.filename.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn wire_file(name: &str, size: u64) -> WireFile {
        WireFile {
            username: "peer".to_string(),
            name: name.to_string(),
            size,
            attribs: HashMap::new(),
        }
    }

    #[test]
    fn search_result_becomes_a_candidate_with_its_files() {
        let result = WireSearchResult {
            token: 1,
            files: vec![wire_file("01 Track.flac", 42_000_000)],
            slots: 3,
            speed: 512_000,
            username: "peer".to_string(),
        };

        let candidate = Candidate::from(result);

        assert_eq!(candidate.username, "peer");
        assert_eq!(candidate.slots, 3);
        assert_eq!(candidate.speed, 512_000);
        assert_eq!(candidate.files.len(), 1);
        assert_eq!(candidate.files[0].name, "01 Track.flac");
        assert_eq!(candidate.files[0].size, 42_000_000);
    }

    #[test]
    fn in_progress_and_paused_download_status_carry_their_byte_counts() {
        let status = WireDownloadStatus::InProgress {
            bytes_downloaded: 10,
            total_bytes: 100,
            speed_bytes_per_sec: 5.0,
        };
        assert_eq!(
            DownloadProgress::from(&status),
            DownloadProgress::InProgress {
                bytes_downloaded: 10,
                total_bytes: 100,
                speed_bytes_per_sec: 5.0,
            }
        );
        assert!(!DownloadProgress::from(&status).is_finished());

        let paused = WireDownloadStatus::Paused {
            bytes_downloaded: 10,
            total_bytes: 100,
        };
        assert_eq!(
            DownloadProgress::from(&paused),
            DownloadProgress::Paused {
                bytes_downloaded: 10,
                total_bytes: 100,
            }
        );
    }

    #[test]
    fn completed_failed_and_timed_out_are_terminal() {
        assert!(DownloadProgress::from(&WireDownloadStatus::Completed).is_finished());
        assert!(DownloadProgress::from(&WireDownloadStatus::Failed(None)).is_finished());
        assert!(DownloadProgress::from(&WireDownloadStatus::TimedOut).is_finished());
        assert!(!DownloadProgress::from(&WireDownloadStatus::Queued).is_finished());
    }

    #[test]
    fn failed_status_keeps_its_reason() {
        let status = WireDownloadStatus::Failed(Some("peer went offline".to_string()));
        assert_eq!(
            DownloadProgress::from(&status),
            DownloadProgress::Failed(Some("peer went offline".to_string()))
        );
    }
}
