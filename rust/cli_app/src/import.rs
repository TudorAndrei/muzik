use std::fs;
use std::path::{Path, PathBuf};

use muzik_core::BeetsConfig;
use muzik_import::apply::{self, AlbumDecision, ApplyOptions, DuplicateDecision, MatchDecision};
use muzik_import::history::IncrementalHistory;
use muzik_import::plan::{ImportMode, ImportPlanner, PlanOptions};
use muzik_import::sync;
use muzik_library::Library;
use muzik_match::MatchConfig;
use muzik_metadata::MetadataClient;
use serde_json::json;
use serde_pickle::value::{HashableValue, Value};

use crate::Import;

const USER_AGENT: &str = "muzik/0.1 (https://github.com/TudorAndrei/muzik)";

pub fn run(args: &Import) -> Result<(), String> {
    if args.directory.is_none() && args.library.is_none() {
        return Err("give an audio path or --library QUERY".to_owned());
    }
    if args.directory.is_some() && args.library.is_some() {
        return Err("choose an audio path or --library QUERY".to_owned());
    }
    if args.link && !args.nowrite {
        return Err("--link requires --nowrite".to_owned());
    }
    if args.link && args.copy {
        return Err("--link and --copy cannot be used together".to_owned());
    }

    let config_path = args
        .config
        .clone()
        .unwrap_or_else(muzik_core::default_config_path);
    let overrides = json!({"import": {
        "copy": args.copy,
        "link": args.link,
        "move": !args.copy && !args.link,
        "write": !args.nowrite,
        "pretend": args.dry_run,
        "incremental": true
    }});
    let config = BeetsConfig::load(&config_path, overrides).map_err(|error| error.to_string())?;
    let db = configured_path(&config, &config_path, "library")?;
    let root = configured_path(&config, &config_path, "directory")?;

    if let Some(query) = &args.library {
        return sync_library(query, args, &db);
    }
    let path = args.directory.as_ref().ok_or("audio path is missing")?;
    if !path.exists() {
        return Err(format!("audio path does not exist: {}", path.display()));
    }
    let statefile = configured_path(&config, &config_path, "statefile")?;
    let seed = legacy_history(&statefile)?;
    let history =
        IncrementalHistory::open_or_seed(&statefile, &seed).map_err(|error| error.to_string())?;
    let planning_library = if db.exists() {
        Library::open_read_only(&db)
    } else {
        Library::empty()
    }
    .map_err(|error| error.to_string())?;
    let match_config = MatchConfig::from_beets(&config).map_err(|error| error.to_string())?;
    let provider = MetadataClient::new(USER_AGENT);
    let planner = ImportPlanner {
        provider: &provider,
        library: &planning_library,
        match_config: &match_config,
        search_limit: config
            .get(&["musicbrainz", "searchlimit"])
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u8::try_from(value).ok())
            .unwrap_or(5),
    };
    let plan = planner
        .plan_with_options(
            std::slice::from_ref(path),
            ImportMode::Album,
            PlanOptions {
                autotag: config
                    .get(&["import", "autotag"])
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(true),
                history: Some(history),
                incremental_skip_later: config
                    .get(&["import", "incremental_skip_later"])
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            },
        )
        .map_err(|error| error.to_string())?;
    let decisions = plan
        .albums
        .iter()
        .map(|album| {
            let title = album
                .items
                .first()
                .map(|item| item.match_item.album.as_str())
                .unwrap_or("");
            println!("Album: {} ({title})", album.source_dir.display());
            let choice = if args.quiet {
                MatchDecision::Skip
            } else {
                MatchDecision::AsIs
            };
            AlbumDecision {
                choice,
                duplicate: (!album.duplicates.is_empty()).then_some(DuplicateDecision::Skip),
            }
        })
        .collect::<Vec<_>>();
    let mut options =
        ApplyOptions::from_beets(&config, root.clone()).map_err(|error| error.to_string())?;
    options.dry_run = args.dry_run;
    let mut library = if args.dry_run {
        if db.exists() {
            Library::open_read_only(&db)
        } else {
            Library::empty()
        }
    } else {
        Library::open_or_create(&db)
    }
    .map_err(|error| error.to_string())?;
    let result = apply::apply(&mut library, &plan, &decisions, &options)
        .map_err(|error| error.to_string())?;
    for path in &result.destinations {
        println!("{}", path.display());
    }
    println!(
        "{} album(s), {} item(s), {} skipped",
        plan.albums.len().saturating_sub(result.skipped_albums),
        result.destinations.len(),
        result.skipped_albums + result.skipped_incremental
    );
    if !result.cleanup_failed.is_empty()
        || !result.source_cleanup_failed.is_empty()
        || !result.history_failed.is_empty()
    {
        eprintln!("Warning: some source files or history entries could not be cleaned up");
    }
    if !args.copy && !args.link && !args.dry_run && !args.no_prune {
        match library.prune_missing_items(&root, 0.1) {
            Ok(count) if count > 0 => println!("Pruned {count} missing library item(s)"),
            Ok(_) => {}
            Err(error) => eprintln!("Warning: skipped library prune: {error}"),
        }
    }
    Ok(())
}

fn sync_library(query: &str, args: &Import, db: &Path) -> Result<(), String> {
    if !db.exists() {
        return Err(format!("library database does not exist: {}", db.display()));
    }
    if args.dry_run {
        let library = Library::open_read_only(db).map_err(|error| error.to_string())?;
        let albums = library
            .query_albums(query)
            .map_err(|error| error.to_string())?;
        let items = library
            .query_items(query)
            .map_err(|error| error.to_string())?;
        println!(
            "Sync preview: {} albums and {} items selected",
            albums.len(),
            items.len()
        );
        return Ok(());
    }
    let mut library = Library::open_read_write(db).map_err(|error| error.to_string())?;
    let provider = MetadataClient::new(USER_AGENT);
    let result = sync::sync(&mut library, &provider, query, !args.nowrite)
        .map_err(|error| error.to_string())?;
    println!(
        "Updated {} album(s), {} item(s)",
        result.albums_updated, result.items_updated
    );
    Ok(())
}

fn configured_path(config: &BeetsConfig, config_path: &Path, key: &str) -> Result<PathBuf, String> {
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
    let key = HashableValue::String("taghistory".to_owned());
    let Some(entries) = fields.get(&key) else {
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{Import, legacy_history, run};

    #[test]
    fn reads_existing_beets_import_history() {
        let temp = tempfile::tempdir().expect("create test directory");
        let statefile = temp.path().join("state.pickle");
        fs::write(
            &statefile,
            include_bytes!("../tests/fixtures/beets-state.pickle"),
        )
        .expect("write beets fixture");

        let entries = legacy_history(&statefile).expect("read beets history");
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&vec![PathBuf::from("/music/old album")]));
        assert!(entries.contains(&vec![PathBuf::from("/music/new album")]));
    }

    #[test]
    fn dry_run_import_does_not_create_a_library_or_move_audio() {
        let temp = tempfile::tempdir().expect("create test directory");
        let source = temp.path().join("incoming.flac");
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/muzik-tags/tests/fixtures/blank.flac");
        fs::copy(fixture, &source).expect("copy audio fixture");
        let config = temp.path().join("config.yaml");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                temp.path().join("Music").display(),
                temp.path().join("library.db").display(),
                temp.path().join("state.pickle").display()
            ),
        )
        .expect("write beets config");

        run(&Import {
            directory: Some(source.clone()),
            library: None,
            copy: false,
            link: false,
            nowrite: false,
            quiet: false,
            dry_run: true,
            no_prune: false,
            config: Some(config),
        })
        .expect("preview import");

        assert!(source.exists());
        assert!(!temp.path().join("library.db").exists());
        assert!(!temp.path().join("state.muzik-history.json").exists());
    }
}
