//! Embedded Soulseek bridge: wraps `soulseek_rs::Client` as the private
//! Python extension module `muzik._seakarr`.
//!
//! The Rust boundary returns owned values only (candidate ids, peers, file
//! names, sizes, queue state, transfer progress, error text) — never a
//! `soulseek_rs` connection, channel, or internal reference.

mod error;
mod job;
mod session;
mod types;

use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use error::SeakarrError;
use job::{JobHandle, JobOutcome, JobState};
use session::{Session, SessionSettings};
use types::{Candidate, DownloadProgress};

/// `SeakarrSession.connect(...)` — logs into the Soulseek server and starts
/// jobs on the resulting `soulseek_rs::Client`.
#[pyclass(name = "SeakarrSession")]
struct PySeakarrSession {
    inner: Session,
}

#[pymethods]
impl PySeakarrSession {
    #[staticmethod]
    #[pyo3(signature = (username, password, server_host=None, server_port=None, enable_listen=None, listen_port=None))]
    fn connect(
        py: Python<'_>,
        username: String,
        password: String,
        server_host: Option<String>,
        server_port: Option<u16>,
        enable_listen: Option<bool>,
        listen_port: Option<u16>,
    ) -> PyResult<Self> {
        let settings = SessionSettings {
            username,
            password,
            server_host,
            server_port,
            enable_listen,
            listen_port,
        };
        // Blocking network I/O: release the GIL so a caller on another
        // Python thread (e.g. the GUI's worker-thread setup) is not stalled.
        let inner = py
            .detach(|| Session::connect(settings))
            .map_err(PyErr::from)?;
        Ok(Self { inner })
    }

    fn start_track_search(&self, query: String, timeout_secs: f64) -> PySeakarrJob {
        PySeakarrJob {
            handle: self.inner.start_track_search(query, timeout_secs),
        }
    }

    fn start_download(
        &self,
        username: String,
        filename: String,
        size: u64,
        destination: String,
    ) -> PyResult<PySeakarrJob> {
        let handle = self
            .inner
            .start_download(username, filename, size, destination)
            .map_err(PyErr::from)?;
        Ok(PySeakarrJob { handle })
    }

    fn close(&self) {
        self.inner.close();
    }
}

/// `SeakarrJob` — a pollable handle to one search or download.
#[pyclass(name = "SeakarrJob")]
struct PySeakarrJob {
    handle: Arc<JobHandle>,
}

#[pymethods]
impl PySeakarrJob {
    /// Return the current status without blocking: `"running"`,
    /// `"completed"`, `"failed"`, or `"cancelled"`.
    fn poll(&self) -> &'static str {
        match self.handle.snapshot() {
            JobState::Running => "running",
            JobState::Completed(_) => "completed",
            JobState::Failed(_) => "failed",
            JobState::Cancelled => "cancelled",
        }
    }

    fn cancel(&self) {
        self.handle.cancel();
    }

    /// Return the finished result, or raise `SeakarrError` when the job is
    /// still running, failed, or was cancelled.
    fn result(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        match self.handle.result().map_err(PyErr::from)? {
            JobOutcome::Search(candidates) => {
                let list = PyList::empty(py);
                for candidate in &candidates {
                    list.append(candidate_to_dict(py, candidate)?)?;
                }
                Ok(list.into())
            }
            JobOutcome::Download(progress) => Ok(download_progress_to_dict(py, &progress)?.into()),
        }
    }
}

fn candidate_to_dict<'py>(py: Python<'py>, candidate: &Candidate) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("username", &candidate.username)?;
    dict.set_item("slots", candidate.slots)?;
    dict.set_item("speed", candidate.speed)?;
    let files = PyList::empty(py);
    for file in &candidate.files {
        let file_dict = PyDict::new(py);
        file_dict.set_item("name", &file.name)?;
        file_dict.set_item("size", file.size)?;
        files.append(file_dict)?;
    }
    dict.set_item("files", files)?;
    Ok(dict)
}

fn download_progress_to_dict<'py>(
    py: Python<'py>,
    progress: &DownloadProgress,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    match progress {
        DownloadProgress::Queued => {
            dict.set_item("state", "queued")?;
        }
        DownloadProgress::InProgress {
            bytes_downloaded,
            total_bytes,
            speed_bytes_per_sec,
        } => {
            dict.set_item("state", "in_progress")?;
            dict.set_item("bytes_downloaded", *bytes_downloaded)?;
            dict.set_item("total_bytes", *total_bytes)?;
            dict.set_item("speed_bytes_per_sec", *speed_bytes_per_sec)?;
        }
        DownloadProgress::Paused {
            bytes_downloaded,
            total_bytes,
        } => {
            dict.set_item("state", "paused")?;
            dict.set_item("bytes_downloaded", *bytes_downloaded)?;
            dict.set_item("total_bytes", *total_bytes)?;
        }
        DownloadProgress::Completed => {
            dict.set_item("state", "completed")?;
        }
        DownloadProgress::Failed(reason) => {
            dict.set_item("state", "failed")?;
            dict.set_item("reason", reason.clone())?;
        }
        DownloadProgress::TimedOut => {
            dict.set_item("state", "timed_out")?;
        }
    }
    Ok(dict)
}

#[pymodule]
fn _seakarr(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PySeakarrSession>()?;
    m.add_class::<PySeakarrJob>()?;
    m.add("SeakarrError", m.py().get_type::<SeakarrError>())?;
    Ok(())
}
