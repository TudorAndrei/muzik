//! Beets-compatible import and organization entry points for front ends.

use std::fs;
use std::path::{Path, PathBuf};

use muzik_core::BeetsConfig;
use muzik_library::{Item, Library, SqlValue, path_from_sql, scalar_text};
use muzik_match::MatchConfig;
use muzik_metadata::MetadataClient;
use muzik_tags::TagData;
use serde_json::json;
use serde_pickle::value::{HashableValue, Value};

use crate::apply::{self, AlbumDecision, ApplyOptions, ApplyResult};
use crate::history::IncrementalHistory;
use crate::plan::{AlbumPlan, ImportMode, ImportPlan, ImportPlanner, PlanOptions};
use crate::sync::{self, SyncResult};

const USER_AGENT: &str = "muzik/0.1 (https://github.com/TudorAndrei/muzik)";

#[derive(Clone, Debug, Default)]
pub struct ImportRequest {
    pub source: PathBuf,
    pub config_path: Option<PathBuf>,
    pub copy: bool,
    pub link: bool,
    pub nowrite: bool,
    pub dry_run: bool,
    pub force: bool,
    pub no_prune: bool,
}

#[derive(Clone, Debug)]
pub struct BeetsPaths {
    pub config: PathBuf,
    pub library: PathBuf,
    pub directory: PathBuf,
    pub statefile: PathBuf,
}

pub struct ImportPreview {
    pub plan: ImportPlan,
    pub paths: BeetsPaths,
    config: BeetsConfig,
    request: ImportRequest,
}

#[derive(Debug)]
pub struct ImportOutcome {
    pub planned_albums: usize,
    pub apply: ApplyResult,
    pub pruned_items: usize,
    pub prune_error: Option<String>,
}

#[derive(Debug)]
pub enum SyncOutcome {
    Preview { albums: usize, items: usize },
    Updated(SyncResult),
}

pub fn configured_path(
    config: &BeetsConfig,
    config_path: &Path,
    key: &str,
) -> Result<PathBuf, String> {
    let raw = config
        .get(&[key])
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("beets config has no {key} path"))?;
    let expanded = if raw == "~" || raw.starts_with("~/") {
        std::env::var_os("HOME")
            .map(|home| {
                PathBuf::from(home).join(raw.trim_start_matches('~').trim_start_matches('/'))
            })
            .unwrap_or_else(|| PathBuf::from(raw))
    } else {
        PathBuf::from(raw)
    };
    Ok(if expanded.is_absolute() {
        expanded
    } else {
        config_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(expanded)
    })
}

pub fn load_paths(
    config_path: Option<&Path>,
    overrides: serde_json::Value,
) -> Result<(BeetsConfig, BeetsPaths), String> {
    let config_path = config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(muzik_core::default_config_path);
    let config = BeetsConfig::load(&config_path, overrides).map_err(|error| error.to_string())?;
    let paths = BeetsPaths {
        library: configured_path(&config, &config_path, "library")?,
        directory: configured_path(&config, &config_path, "directory")?,
        statefile: configured_path(&config, &config_path, "statefile")?,
        config: config_path,
    };
    Ok((config, paths))
}

pub fn plan_import(request: ImportRequest) -> Result<ImportPreview, String> {
    plan_import_with_cancel(request, &|| false)
}

pub fn plan_import_with_cancel(
    request: ImportRequest,
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportPreview, String> {
    check_cancelled(cancelled)?;
    if request.source.as_os_str().is_empty() {
        return Err("audio path is missing".to_owned());
    }
    if request.link && !request.nowrite {
        return Err("--link requires --nowrite".to_owned());
    }
    if request.link && request.copy {
        return Err("--link and --copy cannot be used together".to_owned());
    }
    let overrides = json!({"import": {
        "copy": request.copy,
        "link": request.link,
        "move": !request.copy && !request.link,
        "write": !request.nowrite,
        "pretend": request.dry_run,
        "incremental": !request.force
    }});
    let (config, paths) = load_paths(request.config_path.as_deref(), overrides)?;
    check_cancelled(cancelled)?;
    if !request.source.exists() {
        return Err(format!(
            "audio path does not exist: {}",
            request.source.display()
        ));
    }
    let history = if request.force {
        None
    } else {
        let seed = legacy_history(&paths.statefile)?;
        check_cancelled(cancelled)?;
        Some(
            IncrementalHistory::open_or_seed(&paths.statefile, &seed)
                .map_err(|error| error.to_string())?,
        )
    };
    let library = if paths.library.exists() {
        Library::open_read_only(&paths.library)
    } else {
        Library::empty()
    }
    .map_err(|error| error.to_string())?;
    let match_config = MatchConfig::from_beets(&config).map_err(|error| error.to_string())?;
    let provider = MetadataClient::new(USER_AGENT);
    let planner = ImportPlanner {
        provider: &provider,
        library: &library,
        match_config: &match_config,
        search_limit: config
            .get(&["musicbrainz", "searchlimit"])
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u8::try_from(value).ok())
            .unwrap_or(5),
    };
    let plan = planner
        .plan_with_options_and_cancel(
            std::slice::from_ref(&request.source),
            ImportMode::Album,
            PlanOptions {
                autotag: config
                    .get(&["import", "autotag"])
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                history,
                incremental_skip_later: config
                    .get(&["import", "incremental_skip_later"])
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            },
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    Ok(ImportPreview {
        plan,
        paths,
        config,
        request,
    })
}

pub fn apply_import(
    preview: ImportPreview,
    decisions: &[AlbumDecision],
) -> Result<ImportOutcome, String> {
    apply_import_with_cancel(preview, decisions, &|| false)
}

pub fn apply_import_with_cancel(
    preview: ImportPreview,
    decisions: &[AlbumDecision],
    cancelled: &dyn Fn() -> bool,
) -> Result<ImportOutcome, String> {
    check_cancelled(cancelled)?;
    let mut options = ApplyOptions::from_beets(&preview.config, preview.paths.directory.clone())
        .map_err(|error| error.to_string())?;
    options.dry_run = preview.request.dry_run;
    let mut library = if preview.request.dry_run {
        if preview.paths.library.exists() {
            Library::open_read_only(&preview.paths.library)
        } else {
            Library::empty()
        }
    } else {
        Library::open_or_create(&preview.paths.library)
    }
    .map_err(|error| error.to_string())?;
    let result =
        apply::apply_with_cancel(&mut library, &preview.plan, decisions, &options, cancelled)
            .map_err(|error| error.to_string())?;
    let mut outcome = ImportOutcome {
        planned_albums: preview.plan.albums.len(),
        apply: result,
        pruned_items: 0,
        prune_error: None,
    };
    if !cancelled()
        && !preview.request.copy
        && !preview.request.link
        && !preview.request.dry_run
        && !preview.request.no_prune
    {
        match library.prune_missing_items(&preview.paths.directory, 0.1) {
            Ok(count) => outcome.pruned_items = count,
            Err(error) => outcome.prune_error = Some(error.to_string()),
        }
    }
    Ok(outcome)
}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err(crate::Error::Cancelled.to_string())
    } else {
        Ok(())
    }
}

pub fn import_with(
    request: ImportRequest,
    mut decide: impl FnMut(&AlbumPlan) -> AlbumDecision,
) -> Result<ImportOutcome, String> {
    let preview = plan_import(request)?;
    let decisions = preview
        .plan
        .albums
        .iter()
        .map(&mut decide)
        .collect::<Vec<_>>();
    apply_import(preview, &decisions)
}

pub fn sync_library(
    query: &str,
    config_path: Option<&Path>,
    dry_run: bool,
    nowrite: bool,
) -> Result<SyncOutcome, String> {
    let (_, paths) = load_paths(config_path, json!({}))?;
    if !paths.library.exists() {
        return Err(format!(
            "library database does not exist: {}",
            paths.library.display()
        ));
    }
    if dry_run {
        let library = Library::open_read_only(&paths.library).map_err(|error| error.to_string())?;
        return Ok(SyncOutcome::Preview {
            albums: library
                .query_albums(query)
                .map_err(|error| error.to_string())?
                .len(),
            items: library
                .query_items(query)
                .map_err(|error| error.to_string())?
                .len(),
        });
    }
    let mut library =
        Library::open_read_write(&paths.library).map_err(|error| error.to_string())?;
    let provider = MetadataClient::new(USER_AGENT);
    let result =
        sync::sync(&mut library, &provider, query, !nowrite).map_err(|error| error.to_string())?;
    Ok(SyncOutcome::Updated(result))
}

fn legacy_history(statefile: &Path) -> Result<Vec<Vec<PathBuf>>, String> {
    if IncrementalHistory::path_for_statefile(statefile).exists() || !statefile.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(statefile).map_err(|error| error.to_string())?;
    let state = serde_pickle::value_from_slice(&bytes, serde_pickle::DeOptions::new())
        .map_err(|error| format!("invalid beets import state: {error}"))?;
    let Value::Dict(fields) = state else {
        return Err("beets import state is not a dictionary".to_owned());
    };
    let Some(entries) = fields.get(&HashableValue::String("taghistory".to_owned())) else {
        return Ok(Vec::new());
    };
    let Value::Set(entries) = entries else {
        return Err("beets import history is not a set".to_owned());
    };
    entries
        .iter()
        .map(|entry| {
            let HashableValue::Tuple(paths) = entry else {
                return Err("beets import history entry is not a tuple".to_owned());
            };
            if paths.is_empty() {
                return Err("beets import history entry is empty".to_owned());
            }
            paths.iter().map(legacy_path).collect()
        })
        .collect()
}

fn legacy_path(value: &HashableValue) -> Result<PathBuf, String> {
    match value {
        HashableValue::String(path) => Ok(PathBuf::from(path)),
        HashableValue::Bytes(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                Ok(std::ffi::OsString::from_vec(bytes.clone()).into())
            }
            #[cfg(not(unix))]
            {
                Ok(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
            }
        }
        _ => Err("beets import history path is not text or bytes".to_owned()),
    }
}

pub fn write_library_tags(
    directory: &Path,
    config_path: Option<&Path>,
    dry_run: bool,
) -> Result<usize, String> {
    if !directory.exists() {
        return Err(format!("Directory not found: {}", directory.display()));
    }
    let (_, paths) = load_paths(config_path, json!({}))?;
    if !paths.library.exists() {
        return Err(format!(
            "library database does not exist: {}",
            paths.library.display()
        ));
    }
    let library = Library::open_read_only(&paths.library).map_err(|error| error.to_string())?;
    let requested = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut count = 0;
    for item in library.items().map_err(|error| error.to_string())? {
        let Some(path) = item.field("path").and_then(path_from_sql) else {
            continue;
        };
        let path = if path.is_absolute() {
            path
        } else {
            paths.directory.join(path)
        };
        let path = path.canonicalize().unwrap_or(path);
        if path != requested && !path.starts_with(&requested) {
            continue;
        }
        count += 1;
        if dry_run {
            continue;
        }
        muzik_tags::write(&path, &tags_from_item(&item))
            .map_err(|error| format!("cannot write tags to {}: {error}", path.display()))?;
        if let Some(parent) = path.parent()
            && let Some(cover) = muzik_tags::find_cover(parent)
        {
            let mime = if cover
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
            {
                "image/png"
            } else {
                "image/jpeg"
            };
            let bytes = fs::read(&cover).map_err(|error| error.to_string())?;
            muzik_tags::embed_cover(&path, &bytes, mime)
                .map_err(|error| format!("cannot embed cover in {}: {error}", path.display()))?;
        }
    }
    if count == 0 {
        return Err(format!("No library items match {}", directory.display()));
    }
    Ok(count)
}

fn tags_from_item(item: &Item) -> TagData {
    let mut tags = TagData::default();
    for field in muzik_tags::FIELDS {
        if matches!(field.name, "date" | "original_date") {
            continue;
        }
        if let Some(value) = item.field(field.name).and_then(scalar_text)
            && !value.is_empty()
        {
            tags.fields.insert(field.name.to_owned(), value);
        }
    }
    if let Some(value) = item.field("albumdisambig").and_then(scalar_text)
        && !value.is_empty()
    {
        tags.fields.insert("albumdisambig".into(), value);
    }
    for name in ["rg_track_gain", "rg_album_gain"] {
        if let Some(SqlValue::Real(value)) = item.field(name) {
            tags.fields.insert(name.into(), format!("{value:.2} dB"));
        }
    }
    for name in ["rg_track_peak", "rg_album_peak"] {
        if let Some(SqlValue::Real(value)) = item.field(name) {
            tags.fields.insert(name.into(), format!("{value:.6}"));
        }
    }
    for (prefix, target) in [("", "date"), ("original_", "original_date")] {
        let Some(year) = item.field(&format!("{prefix}year")).and_then(scalar_text) else {
            continue;
        };
        let mut date = format!("{year:0>4}");
        if let Some(month) = item.field(&format!("{prefix}month")).and_then(scalar_text) {
            date.push_str(&format!("-{month:0>2}"));
            if let Some(day) = item.field(&format!("{prefix}day")).and_then(scalar_text) {
                date.push_str(&format!("-{day:0>2}"));
            }
        }
        tags.fields.insert(target.to_owned(), date);
    }
    if let Some(comp) = item.field("comp").and_then(scalar_text) {
        tags.fields
            .insert("comp".into(), if comp == "0" { "0" } else { "1" }.into());
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::{AlbumDecision, MatchDecision};
    use muzik_library::Fields;

    #[test]
    fn reads_existing_beets_history() {
        let temp = tempfile::tempdir().unwrap();
        let statefile = temp.path().join("state.pickle");
        fs::write(
            &statefile,
            include_bytes!("../../../apps/cli/tests/fixtures/beets-state.pickle"),
        )
        .unwrap();
        let entries = legacy_history(&statefile).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&vec![PathBuf::from("/music/old album")]));
    }

    #[test]
    fn dry_run_import_keeps_source_and_database_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("incoming.flac");
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../muzik-tags/tests/fixtures/blank.flac");
        fs::copy(fixture, &source).unwrap();
        let config_path = temp.path().join("config.yaml");
        let database = temp.path().join("library.db");
        let statefile = temp.path().join("state.pickle");
        fs::write(
            &config_path,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                temp.path().join("Music").display(),
                database.display(),
                statefile.display()
            ),
        )
        .unwrap();
        let result = import_with(
            ImportRequest {
                source: source.clone(),
                config_path: Some(config_path),
                dry_run: true,
                ..ImportRequest::default()
            },
            |_| AlbumDecision {
                choice: MatchDecision::AsIs,
                duplicate: None,
            },
        )
        .unwrap();
        assert_eq!(result.apply.destinations.len(), 1);
        assert!(source.exists());
        assert!(!database.exists());
        assert!(!IncrementalHistory::path_for_statefile(&statefile).exists());
    }

    #[test]
    fn force_replans_an_incremental_source() {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("incoming");
        fs::create_dir(&source_dir).unwrap();
        let source = source_dir.join("track.flac");
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../muzik-tags/tests/fixtures/blank.flac"),
            &source,
        )
        .unwrap();
        let config_path = temp.path().join("config.yaml");
        let statefile = temp.path().join("state.pickle");
        fs::write(
            &config_path,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                temp.path().join("Music").display(),
                temp.path().join("library.db").display(),
                statefile.display(),
            ),
        )
        .unwrap();
        IncrementalHistory::open_or_seed(&statefile, &[])
            .unwrap()
            .record(&[source_dir.canonicalize().unwrap()])
            .unwrap();
        let request = ImportRequest {
            source: source.clone(),
            config_path: Some(config_path),
            dry_run: true,
            ..ImportRequest::default()
        };
        let skipped = plan_import(request.clone()).unwrap();
        assert!(skipped.plan.albums.is_empty());
        assert_eq!(skipped.plan.skipped_incremental, 1);
        let forced = plan_import(ImportRequest {
            force: true,
            ..request
        })
        .unwrap();
        assert_eq!(forced.plan.albums.len(), 1);
        assert_eq!(forced.plan.skipped_incremental, 0);
    }

    #[test]
    fn tag_only_uses_library_row_and_dry_run_is_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("music");
        fs::create_dir(&root).unwrap();
        let audio = root.join("song.mp3");
        fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../muzik-tags/tests/fixtures/blank.mp3"),
            &audio,
        )
        .unwrap();
        let database = temp.path().join("library.db");
        let mut library = Library::open_or_create(&database).unwrap();
        let mut fields = Fields::new();
        fields.insert(
            "path".into(),
            SqlValue::Blob(audio.as_os_str().as_encoded_bytes().to_vec()),
        );
        fields.insert("title".into(), SqlValue::Text("Library title".into()));
        library.insert_item(&fields, &Fields::new()).unwrap();
        drop(library);
        let config_path = temp.path().join("config.yaml");
        fs::write(
            &config_path,
            format!(
                "library: {}\ndirectory: {}\n",
                database.display(),
                root.display()
            ),
        )
        .unwrap();
        let before = fs::read(&audio).unwrap();
        assert_eq!(
            write_library_tags(&root, Some(&config_path), true).unwrap(),
            1
        );
        assert_eq!(fs::read(&audio).unwrap(), before);
        assert_eq!(
            write_library_tags(&root, Some(&config_path), false).unwrap(),
            1
        );
        let tags = muzik_tags::read(&audio, &[]).unwrap();
        assert_eq!(
            tags.fields.get("title").map(String::as_str),
            Some("Library title")
        );
    }
}
