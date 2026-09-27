//! Python bindings for muzik's Rust libraries.
//!
//! The Rust boundary returns owned values only (candidate ids, peers, file
//! names, sizes, queue state, transfer progress, error text) — never a
//! `soulseek_rs` connection, channel, or internal reference.

mod import;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};

use import::PyNativeImporter;
use muzik_core::{default_config_path, BeetsConfig};

#[pyfunction]
fn library_config_path() -> String {
    default_config_path().to_string_lossy().into_owned()
}
use muzik_library::{Fields, Library as RustLibrary, SqlValue};
use muzik_match::{rank_albums, MatchAlbum, MatchConfig, MatchItem};
use muzik_metadata::{MetadataClient, ReleaseSearch};
use muzik_soulseek::error::BridgeError;
use muzik_soulseek::job::{JobHandle, JobOutcome, JobState};
use muzik_soulseek::session::{Session, SessionSettings};
use muzik_soulseek::types::{Candidate, DownloadProgress};

create_exception!(_native, SeakarrError, PyException);
create_exception!(_native, MetadataError, PyException);
create_exception!(_native, MatchError, PyException);
create_exception!(_native, LibraryError, PyException);
create_exception!(_native, ImportError, PyException);
create_exception!(_native, TagsError, PyException);

fn tags_error(error: impl std::fmt::Display) -> PyErr {
    TagsError::new_err(error.to_string())
}

#[pyfunction]
fn probe_audio(py: Python<'_>, path: String) -> PyResult<Py<PyAny>> {
    let (properties, tags) = py.detach(|| {
        let properties = muzik_tags::probe(&path).map_err(tags_error)?;
        let tags = muzik_tags::read(&path, &[]).map_err(tags_error)?;
        Ok::<_, PyErr>((properties, tags))
    })?;
    let result = PyDict::new(py);
    result.set_item("format", properties.format)?;
    result.set_item("codec", properties.codec)?;
    result.set_item("duration", properties.duration_seconds)?;
    result.set_item("bitrate_kbps", properties.bitrate_kbps)?;
    result.set_item("sample_rate_hz", properties.sample_rate_hz)?;
    result.set_item("bit_depth", properties.bit_depth)?;
    result.set_item("channels", properties.channels)?;
    result.set_item("size_bytes", properties.size_bytes)?;
    result.set_item("tags", tags.fields)?;
    Ok(result.into())
}

#[pyfunction]
fn read_audio_tags(py: Python<'_>, path: String) -> PyResult<Py<PyAny>> {
    let data = py.detach(|| muzik_tags::read(&path, &[]).map_err(tags_error))?;
    let result = PyDict::new(py);
    result.set_item("fields", data.fields)?;
    result.set_item("lists", data.lists)?;
    result.set_item("custom", data.custom)?;
    Ok(result.into())
}

#[pyfunction]
fn write_audio_tags(py: Python<'_>, path: String, data_json: String) -> PyResult<()> {
    let data: serde_json::Value = serde_json::from_str(&data_json).map_err(tags_error)?;
    let fields = serde_json::from_value(
        data.get("fields")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(tags_error)?;
    let lists = serde_json::from_value(
        data.get("lists")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(tags_error)?;
    let custom = serde_json::from_value(
        data.get("custom")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
    )
    .map_err(tags_error)?;
    py.detach(move || {
        muzik_tags::write(
            path,
            &muzik_tags::TagData {
                fields,
                lists,
                custom,
            },
        )
        .map_err(tags_error)
    })
}

#[pyfunction]
fn find_audio_cover(directory: String) -> Option<String> {
    muzik_tags::find_cover(directory).map(|path| path.to_string_lossy().into_owned())
}

#[pyfunction]
fn embed_audio_cover(
    py: Python<'_>,
    path: String,
    image: Vec<u8>,
    mime_type: String,
) -> PyResult<()> {
    py.detach(move || muzik_tags::embed_cover(path, &image, &mime_type).map_err(tags_error))
}

#[pyfunction]
fn audio_has_front_cover(py: Python<'_>, path: String) -> PyResult<bool> {
    py.detach(move || muzik_tags::has_front_cover(path).map_err(tags_error))
}

const MUSICBRAINZ_USER_AGENT: &str = "muzik/0.1 (https://github.com/TudorAndrei/muzik)";

fn soulseek_error(error: BridgeError) -> PyErr {
    SeakarrError::new_err(error.to_string())
}

fn metadata_error(error: muzik_metadata::Error) -> PyErr {
    MetadataError::new_err(error.to_string())
}

fn library_error(error: impl std::fmt::Display) -> PyErr {
    LibraryError::new_err(error.to_string())
}

pub(crate) fn beets_path(raw: &str, config_path: &Path) -> PathBuf {
    let expanded = if raw == "~" || raw.starts_with("~/") {
        std::env::var_os("HOME")
            .map(|home| {
                PathBuf::from(home).join(raw.trim_start_matches('~').trim_start_matches('/'))
            })
            .unwrap_or_else(|| PathBuf::from(raw))
    } else {
        PathBuf::from(raw)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(expanded)
    }
}

#[pyclass(name = "NativeLibrary")]
struct PyNativeLibrary {
    inner: Mutex<RustLibrary>,
    library_path: PathBuf,
    directory: String,
}

#[pymethods]
impl PyNativeLibrary {
    #[new]
    fn new(config_path: String) -> PyResult<Self> {
        let config_path = Path::new(&config_path);
        let config =
            BeetsConfig::load(config_path, serde_json::json!({})).map_err(library_error)?;
        let library_path = config
            .get(&["library"])
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| LibraryError::new_err("beets config has no library path"))?;
        let directory = config
            .get(&["directory"])
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| LibraryError::new_err("beets config has no music directory"))?;
        let library_path = beets_path(library_path, config_path);
        let directory = beets_path(directory, config_path);
        let inner = if library_path.exists() {
            RustLibrary::open_read_only(&library_path)
        } else {
            RustLibrary::empty()
        }
        .map_err(library_error)?;
        Ok(Self {
            inner: Mutex::new(inner),
            library_path,
            directory: directory.to_string_lossy().into_owned(),
        })
    }

    #[getter]
    fn directory(&self) -> &str {
        &self.directory
    }

    #[pyo3(signature = (query=""))]
    fn items(&self, py: Python<'_>, query: &str) -> PyResult<Py<PyAny>> {
        let items = py.detach(|| {
            self.inner
                .lock()
                .map_err(library_error)?
                .query_items(query)
                .map_err(library_error)
        })?;
        let list = PyList::empty(py);
        for item in items {
            list.append(library_row(py, item.id, &item.fields, &item.attributes)?)?;
        }
        Ok(list.into())
    }

    #[pyo3(signature = (query=""))]
    fn albums(&self, py: Python<'_>, query: &str) -> PyResult<Py<PyAny>> {
        let albums = py.detach(|| {
            self.inner
                .lock()
                .map_err(library_error)?
                .query_albums(query)
                .map_err(library_error)
        })?;
        let list = PyList::empty(py);
        for album in albums {
            list.append(library_row(py, album.id, &album.fields, &album.attributes)?)?;
        }
        Ok(list.into())
    }

    fn items_for_album(&self, py: Python<'_>, album_id: i64) -> PyResult<Py<PyAny>> {
        let items = py.detach(|| {
            self.inner
                .lock()
                .map_err(library_error)?
                .items_for_album(album_id)
                .map_err(library_error)
        })?;
        let list = PyList::empty(py);
        for item in items {
            list.append(library_row(py, item.id, &item.fields, &item.attributes)?)?;
        }
        Ok(list.into())
    }

    #[pyo3(signature = (safety_fraction=0.5))]
    fn prune_missing_items(
        &self,
        py: Python<'_>,
        safety_fraction: f64,
    ) -> PyResult<(usize, Option<(usize, usize)>)> {
        py.detach(|| {
            let mut library =
                RustLibrary::open_read_write(&self.library_path).map_err(library_error)?;
            match library.prune_missing_items(Path::new(&self.directory), safety_fraction) {
                Ok(removed) => Ok((removed, None)),
                Err(muzik_library::Error::PruneAborted { missing, total }) => {
                    Ok((0, Some((missing, total))))
                }
                Err(error) => Err(library_error(error)),
            }
        })
    }
}

fn library_row<'py>(
    py: Python<'py>,
    id: i64,
    fields: &Fields,
    attributes: &Fields,
) -> PyResult<Bound<'py, PyDict>> {
    let row = PyDict::new(py);
    row.set_item("id", id)?;
    row.set_item("fields", fields_to_dict(py, fields)?)?;
    row.set_item("attributes", fields_to_dict(py, attributes)?)?;
    Ok(row)
}

fn fields_to_dict<'py>(py: Python<'py>, fields: &Fields) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (name, value) in fields {
        match value {
            SqlValue::Null => dict.set_item(name, py.None())?,
            SqlValue::Integer(value) => dict.set_item(name, value)?,
            SqlValue::Real(value) => dict.set_item(name, value)?,
            SqlValue::Text(value) => dict.set_item(name, value)?,
            SqlValue::Blob(value) => dict.set_item(name, PyBytes::new(py, value))?,
        }
    }
    Ok(dict)
}

/// Rank the supplied beets album candidates. Indices refer to the input list.
#[pyfunction]
fn rank_album_candidates(
    py: Python<'_>,
    items_json: String,
    albums_json: String,
    config_json: String,
) -> PyResult<(Vec<(usize, f64)>, String)> {
    py.detach(move || {
        let result = (|| -> Result<_, String> {
            let items: Vec<MatchItem> =
                serde_json::from_str(&items_json).map_err(|error| error.to_string())?;
            let albums: Vec<MatchAlbum> =
                serde_json::from_str(&albums_json).map_err(|error| error.to_string())?;
            let overrides: serde_json::Value =
                serde_json::from_str(&config_json).map_err(|error| error.to_string())?;
            let config = BeetsConfig::from_layers("", overrides.clone())
                .map_err(|error| error.to_string())?;
            let mut config = MatchConfig::from_beets(&config).map_err(|error| error.to_string())?;
            if let Some(count) = overrides
                .get("metadata_source_count")
                .and_then(|value| value.as_u64())
            {
                config.metadata_source_count = count as usize;
            }
            if let Some(penalties) = overrides.get("data_source_penalties") {
                config.data_source_penalties =
                    serde_json::from_value(penalties.clone()).map_err(|error| error.to_string())?;
            }
            let ranking =
                rank_albums(&items, &albums, &config).map_err(|error| error.to_string())?;
            let candidates = ranking
                .candidates
                .iter()
                .map(|candidate| {
                    candidate
                        .distance
                        .score(&config)
                        .map(|score| (candidate.input_index, score))
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((
                candidates,
                format!("{:?}", ranking.recommendation).to_lowercase(),
            ))
        })();
        result.map_err(MatchError::new_err)
    })
}

#[pyfunction]
#[pyo3(signature = (artist, album, year=None, limit=5))]
fn search_musicbrainz_releases(
    py: Python<'_>,
    artist: String,
    album: String,
    year: Option<String>,
    limit: u8,
) -> PyResult<Py<PyAny>> {
    let releases = py
        .detach(move || {
            MetadataClient::new(MUSICBRAINZ_USER_AGENT).search_releases(
                &ReleaseSearch {
                    release: album,
                    artist: Some(artist),
                    year,
                    ..ReleaseSearch::default()
                },
                limit,
            )
        })
        .map_err(metadata_error)?;
    let output = PyList::empty(py);
    for release in releases {
        let row = PyDict::new(py);
        row.set_item("id", release.id.0)?;
        row.set_item("title", release.title)?;
        row.set_item("artist", release.artist)?;
        row.set_item("score", release.score.unwrap_or(0))?;
        output.append(row)?;
    }
    Ok(output.into())
}

#[pyfunction]
fn get_musicbrainz_tracklist(py: Python<'_>, release_id: String) -> PyResult<Py<PyAny>> {
    let release = py
        .detach(move || MetadataClient::new(MUSICBRAINZ_USER_AGENT).lookup_release(&release_id))
        .map_err(metadata_error)?;
    let output = PyList::empty(py);
    for track in release.tracks {
        let row = PyDict::new(py);
        row.set_item("title", track.title)?;
        row.set_item("position", track.medium_index)?;
        row.set_item(
            "length",
            track
                .length_seconds
                .map(|seconds| (seconds * 1000.0).round() as u64),
        )?;
        output.append(row)?;
    }
    Ok(output.into())
}

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
            .map_err(soulseek_error)?;
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
            .map_err(soulseek_error)?;
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
        match self.handle.result().map_err(soulseek_error)? {
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
        file_dict.set_item("bitrate_kbps", file.bitrate_kbps)?;
        file_dict.set_item("duration_seconds", file.duration_seconds)?;
        file_dict.set_item("vbr", file.vbr)?;
        file_dict.set_item("sample_rate_hz", file.sample_rate_hz)?;
        file_dict.set_item("bit_depth", file.bit_depth)?;
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
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(library_config_path, m)?)?;
    m.add_class::<PyNativeLibrary>()?;
    m.add_class::<PyNativeImporter>()?;
    m.add_class::<PySeakarrSession>()?;
    m.add_class::<PySeakarrJob>()?;
    m.add("SeakarrError", m.py().get_type::<SeakarrError>())?;
    m.add("MetadataError", m.py().get_type::<MetadataError>())?;
    m.add("MatchError", m.py().get_type::<MatchError>())?;
    m.add("LibraryError", m.py().get_type::<LibraryError>())?;
    m.add("ImportError", m.py().get_type::<ImportError>())?;
    m.add("TagsError", m.py().get_type::<TagsError>())?;
    m.add_function(wrap_pyfunction!(rank_album_candidates, m)?)?;
    m.add_function(wrap_pyfunction!(probe_audio, m)?)?;
    m.add_function(wrap_pyfunction!(read_audio_tags, m)?)?;
    m.add_function(wrap_pyfunction!(write_audio_tags, m)?)?;
    m.add_function(wrap_pyfunction!(find_audio_cover, m)?)?;
    m.add_function(wrap_pyfunction!(embed_audio_cover, m)?)?;
    m.add_function(wrap_pyfunction!(audio_has_front_cover, m)?)?;
    m.add_function(wrap_pyfunction!(search_musicbrainz_releases, m)?)?;
    m.add_function(wrap_pyfunction!(get_musicbrainz_tracklist, m)?)?;
    Ok(())
}
