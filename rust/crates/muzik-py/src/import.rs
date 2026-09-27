//! Python boundary for native import plans and explicit choices.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use muzik_core::BeetsConfig;
use muzik_import::apply::{self, AlbumDecision, ApplyOptions, DuplicateDecision, MatchDecision};
use muzik_import::plan::{AlbumPlan, ImportPlan, ImportPlanner};
use muzik_import::sync;
use muzik_library::{Library, SqlValue};
use muzik_match::MatchConfig;
use muzik_metadata::MetadataClient;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::{beets_path, ImportError, MUSICBRAINZ_USER_AGENT};

fn import_error(error: impl std::fmt::Display) -> PyErr {
    ImportError::new_err(error.to_string())
}

#[pyclass(name = "NativeImporter")]
pub(crate) struct PyNativeImporter {
    config: BeetsConfig,
    library_path: PathBuf,
    directory: PathBuf,
    plan: Mutex<Option<ImportPlan>>,
}

#[pymethods]
impl PyNativeImporter {
    #[new]
    fn new(config_path: String, overrides_json: String) -> PyResult<Self> {
        let config_path = Path::new(&config_path);
        let overrides = serde_json::from_str(&overrides_json).map_err(import_error)?;
        let config = BeetsConfig::load(config_path, overrides).map_err(import_error)?;
        let raw_library = config
            .get(&["library"])
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| import_error("beets config has no library path"))?;
        let raw_directory = config
            .get(&["directory"])
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| import_error("beets config has no music directory"))?;
        Ok(Self {
            library_path: beets_path(raw_library, config_path),
            directory: beets_path(raw_directory, config_path),
            config,
            plan: Mutex::new(None),
        })
    }

    fn plan(&self, py: Python<'_>, paths: Vec<String>) -> PyResult<Py<PyAny>> {
        self.prepare_plan(py, paths, false)
    }

    fn plan_singletons(&self, py: Python<'_>, paths: Vec<String>) -> PyResult<Py<PyAny>> {
        self.prepare_plan(py, paths, true)
    }

    fn apply(
        &self,
        py: Python<'_>,
        choices: Vec<(String, Option<usize>, Option<String>)>,
    ) -> PyResult<Py<PyAny>> {
        let decisions = choices
            .into_iter()
            .map(|(choice, index, duplicate)| {
                let choice = match choice.as_str() {
                    "candidate" => MatchDecision::Candidate(
                        index.ok_or_else(|| import_error("candidate choice needs an index"))?,
                    ),
                    "as_is" => MatchDecision::AsIs,
                    "skip" => MatchDecision::Skip,
                    _ => return Err(import_error("invalid native import choice")),
                };
                let duplicate = match duplicate.as_deref() {
                    None => None,
                    Some("keep") => Some(DuplicateDecision::Keep),
                    Some("replace") => Some(DuplicateDecision::Replace),
                    Some("skip") => Some(DuplicateDecision::Skip),
                    Some(_) => return Err(import_error("invalid duplicate choice")),
                };
                Ok(AlbumDecision { choice, duplicate })
            })
            .collect::<PyResult<Vec<_>>>()?;
        let result = py.detach(|| {
            let guard = self.plan.lock().map_err(import_error)?;
            let plan = guard
                .as_ref()
                .ok_or_else(|| import_error("import has no plan"))?;
            let options = ApplyOptions::from_beets(&self.config, self.directory.clone())
                .map_err(import_error)?;
            let mut library = if options.dry_run {
                Library::open_read_only(&self.library_path)
            } else {
                Library::open_read_write(&self.library_path)
            }
            .map_err(import_error)?;
            apply::apply(&mut library, plan, &decisions, &options).map_err(import_error)
        })?;
        let output = PyDict::new(py);
        output.set_item("album_ids", result.album_ids)?;
        output.set_item("item_ids", result.item_ids)?;
        output.set_item(
            "destinations",
            result
                .destinations
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )?;
        output.set_item("skipped_albums", result.skipped_albums)?;
        output.set_item(
            "cleanup_failed",
            result
                .cleanup_failed
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )?;
        output.set_item(
            "source_cleanup_failed",
            result
                .source_cleanup_failed
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )?;
        Ok(output.into())
    }

    fn sync(&self, py: Python<'_>, query: String, write_tags: bool) -> PyResult<Py<PyAny>> {
        let result = py.detach(|| {
            let mut library = Library::open_read_write(&self.library_path).map_err(import_error)?;
            let provider = MetadataClient::new(MUSICBRAINZ_USER_AGENT);
            sync::sync(&mut library, &provider, &query, write_tags).map_err(import_error)
        })?;
        let output = PyDict::new(py);
        output.set_item("albums_updated", result.albums_updated)?;
        output.set_item("items_updated", result.items_updated)?;
        output.set_item(
            "albums_without_release_id",
            result.albums_without_release_id,
        )?;
        output.set_item("items_without_match", result.items_without_match)?;
        output.set_item("singletons_updated", result.singletons_updated)?;
        output.set_item(
            "singletons_without_recording_id",
            result.singletons_without_recording_id,
        )?;
        Ok(output.into())
    }
}

impl PyNativeImporter {
    fn prepare_plan(
        &self,
        py: Python<'_>,
        paths: Vec<String>,
        singletons: bool,
    ) -> PyResult<Py<PyAny>> {
        let plan = py.detach(|| {
            let library = Library::open_read_only(&self.library_path).map_err(import_error)?;
            let match_config = MatchConfig::from_beets(&self.config).map_err(import_error)?;
            let provider = MetadataClient::new(MUSICBRAINZ_USER_AGENT);
            let planner = ImportPlanner {
                provider: &provider,
                library: &library,
                match_config: &match_config,
                search_limit: self
                    .config
                    .get(&["musicbrainz", "searchlimit"])
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u8::try_from(value).ok())
                    .unwrap_or(5),
            };
            let paths = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
            if singletons {
                planner.plan_singletons(&paths).map_err(import_error)
            } else {
                planner.plan(&paths).map_err(import_error)
            }
        })?;
        let list = PyList::empty(py);
        let library = Library::open_read_only(&self.library_path).map_err(import_error)?;
        for album in &plan.albums {
            list.append(album_to_dict(py, album, &library)?)?;
        }
        *self.plan.lock().map_err(import_error)? = Some(plan);
        Ok(list.into())
    }
}

fn album_to_dict<'py>(
    py: Python<'py>,
    album: &AlbumPlan,
    library: &Library,
) -> PyResult<Bound<'py, PyDict>> {
    let output = PyDict::new(py);
    output.set_item(
        "paths",
        album
            .items
            .iter()
            .map(|item| item.source.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
    )?;
    if let Some(first) = album.items.first() {
        output.set_item("current_artist", &first.match_item.artist)?;
        output.set_item("current_album", &first.match_item.album)?;
        output.set_item("current_year", first.match_item.year.to_string())?;
    }
    let candidates = PyList::empty(py);
    for candidate in &album.candidates {
        let row = PyDict::new(py);
        row.set_item("id", &candidate.release.id.0)?;
        row.set_item("artist", &candidate.release.artist)?;
        row.set_item("album", &candidate.release.title)?;
        row.set_item("distance", candidate.distance)?;
        candidates.append(row)?;
    }
    output.set_item("candidates", candidates)?;
    let duplicates = PyList::empty(py);
    for duplicate in &album.duplicates {
        let row = PyDict::new(py);
        row.set_item("album_id", duplicate.album_id)?;
        if let Some(existing) = library.album(duplicate.album_id).map_err(import_error)? {
            row.set_item("artist", field_text(existing.field("albumartist")))?;
            row.set_item("album", field_text(existing.field("album")))?;
            if let Some(item) = library
                .items_for_album(duplicate.album_id)
                .map_err(import_error)?
                .first()
            {
                row.set_item("path", field_text(item.field("path")))?;
            }
        }
        duplicates.append(row)?;
    }
    output.set_item("duplicates", duplicates)?;
    Ok(output)
}

fn field_text(value: Option<&SqlValue>) -> Option<String> {
    match value {
        Some(SqlValue::Text(value)) => Some(value.clone()),
        Some(SqlValue::Blob(value)) => Some(String::from_utf8_lossy(value).into_owned()),
        _ => None,
    }
}
