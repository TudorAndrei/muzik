use muzik_core::paths::Paths;
use muzik_core::{PreferredAudio, app_config, paths};
use muzik_import::beets;
use muzik_library::Library;
use muzik_media::quality::{self, MeasuredQuality};
use muzik_soulseek::fetch::{Timeouts, local_files};
use muzik_soulseek::ranking::RankedCandidate;
use muzik_soulseek::session::{
    DEFAULT_SERVER_HOST, DEFAULT_SERVER_PORT, Session, SessionSettings, setting,
};
use muzik_soulseek::types::FileEntry;
use muzik_workflow::upgrade::{
    CachedCandidate, candidate_id, load_candidate, save_candidate, scan_library, select_upgrade,
};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::{Import, SoulseekCheckLibrary, SoulseekDownload, import};

pub fn check() -> Result<(), String> {
    let config = app_config::load(&app_config::path()).map_err(|error| error.to_string())?;
    let settings = SessionSettings::configured(&config).ok_or(
        "Set MUZIK_SOULSEEK_USERNAME and MUZIK_SOULSEEK_PASSWORD, or save them with 'muzik config set-soulseek'.",
    )?;
    let username = settings.username.clone();
    let host = settings
        .server_host
        .clone()
        .unwrap_or_else(|| DEFAULT_SERVER_HOST.into());
    let port = settings.server_port.unwrap_or(DEFAULT_SERVER_PORT);
    let download_dir = setting(&config, "MUZIK_SOULSEEK_DOWNLOAD_DIR", "download_dir")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| Paths::user().soulseek());
    let _session =
        Session::connect(settings).map_err(|error| format!("Soulseek check failed: {error}"))?;
    println!("Soulseek reachable");
    println!("  Username: {username}");
    println!("  Server: {host}:{port}");
    println!("  Download dir: {}", download_dir.display());
    Ok(())
}

pub fn search(
    query: &str,
    prefer: PreferredAudio,
    limit: usize,
    json_output: bool,
) -> Result<(), String> {
    let config = app_config::load(&app_config::path())?;
    let session = connect(&config)?;
    let ranked = ranked_search(&session, &config, query, prefer, limit)?;
    show_candidates(&ranked, query, json_output)?;
    Ok(())
}

pub fn check_library(args: &SoulseekCheckLibrary) -> Result<(), String> {
    let prefer = args.prefer;
    let (_, paths) = beets::load_paths(args.config.as_deref(), json!({}))?;
    let library = Library::open_read_only(&paths.library)
        .map_err(|error| format!("Could not open the music library: {error}"))?;
    let items = library
        .query_items(args.query.as_deref().unwrap_or(""))
        .map_err(|error| error.to_string())?;
    let (scanned, mut flagged) =
        scan_library(items, &paths.directory, args.min_bitrate, quality::measure)?;
    flagged.sort_by(|left, right| {
        (&left.sort_artist, &left.album, &left.title).cmp(&(
            &right.sort_artist,
            &right.album,
            &right.title,
        ))
    });
    if flagged.is_empty() {
        println!(
            "No tracks below {}kbps out of {scanned} scanned.",
            args.min_bitrate
        );
        return Ok(());
    }
    let selected = flagged.len().min(args.limit);
    let skipped = flagged.len() - selected;
    println!("Artist\tTitle\tCurrent\tStatus\tSuggested\tCandidate ID");
    let config = app_config::load(&app_config::path())?;
    let session = if selected > 0 {
        Some(connect(&config)?)
    } else {
        None
    };
    let mut found = 0_usize;
    for track in flagged.iter().take(selected) {
        let current = quality_label(&track.quality);
        let query = format!("{} - {}", track.artist, track.title);
        let Some(session) = session.as_ref() else {
            break;
        };
        match ranked_search(session, &config, &query, prefer, 10) {
            Err(error) => println!(
                "{}\t{}\t{}\tsearch failed\t{}\t",
                safe_display(&track.artist),
                safe_display(&track.title),
                current,
                safe_display(&error)
            ),
            Ok(ranked) => match select_upgrade(track, &ranked, prefer) {
                None => println!(
                    "{}\t{}\t{}\tno safe match\t\t",
                    safe_display(&track.artist),
                    safe_display(&track.title),
                    current
                ),
                Some((candidate, score)) => {
                    let id = candidate_id(&candidate)?;
                    save_candidate(
                        &paths::cache_dir(),
                        &id,
                        &CachedCandidate {
                            query: query.clone(),
                            score,
                            candidate: candidate.clone(),
                        },
                    )?;
                    let suggested = candidate
                        .files
                        .first()
                        .map(file_quality_label)
                        .unwrap_or_default();
                    println!(
                        "{}\t{}\t{}\treplacement found\t{}\t{}",
                        safe_display(&track.artist),
                        safe_display(&track.title),
                        current,
                        suggested,
                        id
                    );
                    found += 1;
                }
            },
        }
    }
    println!(
        "Scanned {scanned} track(s); {} below {}kbps; {found} replacement(s) found.",
        flagged.len(),
        args.min_bitrate
    );
    if skipped > 0 {
        println!(
            "{skipped} more below-threshold track(s) not searched. Raise --limit or narrow --query."
        );
    }
    Ok(())
}

fn quality_label(quality: &MeasuredQuality) -> String {
    format!(
        "{} {}kbps",
        quality.format,
        quality
            .bitrate_kbps
            .map_or_else(|| "?".into(), |rate| rate.to_string())
    )
}

fn file_quality_label(file: &FileEntry) -> String {
    format!(
        "{} {}kbps",
        muzik_soulseek::ranking::format(file).map_or_else(String::new, |format| format.to_string()),
        file.bitrate_kbps
            .map_or_else(|| "?".into(), |rate| rate.to_string())
    )
}

fn connect(config: &Value) -> Result<Session, String> {
    let settings = SessionSettings::configured(config).ok_or(
        "Set MUZIK_SOULSEEK_USERNAME and MUZIK_SOULSEEK_PASSWORD, or save them with 'muzik config set-soulseek'.",
    )?;
    Session::connect(settings).map_err(|error| format!("Soulseek connection failed: {error}"))
}

fn ranked_search(
    session: &Session,
    config: &Value,
    query: &str,
    prefer: PreferredAudio,
    limit: usize,
) -> Result<Vec<RankedCandidate>, String> {
    if !(1..=100).contains(&limit) {
        return Err("limit must be from 1 to 100".into());
    }
    Ok(session.search(
        query,
        prefer,
        limit,
        Timeouts::configured(config).search,
        &AtomicBool::new(false),
    )?)
}

fn show_candidates(
    ranked: &[RankedCandidate],
    query: &str,
    json_output: bool,
) -> Result<(), String> {
    let mut rows = Vec::new();
    for item in ranked {
        let id = candidate_id(&item.candidate)?;
        save_candidate(
            &paths::cache_dir(),
            &id,
            &CachedCandidate {
                query: query.to_owned(),
                score: item.score,
                candidate: item.candidate.clone(),
            },
        )?;
        rows.push(candidate_row(item, &id));
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({"query": query, "results": rows}))
                .map_err(|error| error.to_string())?
        );
    } else if rows.is_empty() {
        println!("No candidates found.");
    } else {
        println!("#\tID\tScore\tFormat\tFiles\tUser\tPath");
        for (index, row) in rows.iter().enumerate() {
            println!(
                "{}\t{}\t{:.1}\t{}\t{}\t{}\t{}",
                index + 1,
                row["id"].as_str().unwrap_or(""),
                row["score"].as_f64().unwrap_or(0.0),
                row["format"].as_str().unwrap_or("?"),
                row["file_count"].as_u64().unwrap_or(0),
                safe_display(row["username"].as_str().unwrap_or("")),
                safe_display(row["path"].as_str().unwrap_or(""))
            );
        }
    }
    Ok(())
}

fn candidate_row(item: &RankedCandidate, id: &str) -> Value {
    let format = item
        .candidate
        .files
        .iter()
        .find_map(muzik_soulseek::ranking::format)
        .map_or_else(|| "?".to_owned(), |format| format.to_string());
    let path = item
        .candidate
        .files
        .first()
        .map(|file| file.name.as_str())
        .unwrap_or("");
    json!({"id": id, "score": item.score, "format": format,
        "file_count": item.candidate.files.len(), "username": item.candidate.username,
        "path": path})
}

pub fn download(args: &SoulseekDownload) -> Result<(), String> {
    if args.query.is_some() && args.candidate.is_some() {
        return Err("give a query or --candidate, not both".into());
    }
    let config = app_config::load(&app_config::path())?;
    let mut session = None;
    let (id, saved) = if let Some(id) = &args.candidate {
        (id.clone(), load_candidate(&paths::cache_dir(), id)?)
    } else {
        let query = args.query.as_deref().ok_or("give a query or --candidate")?;
        let connected = connect(&config)?;
        let ranked = ranked_search(&connected, &config, query, args.prefer, args.limit)?;
        if ranked.is_empty() {
            println!("No candidates found.");
            return Ok(());
        }
        show_candidates(&ranked, query, false)?;
        let index = choose_index(ranked.len(), args.no_interactive)?;
        let selected = ranked
            .get(index)
            .ok_or("candidate number is out of range")?;
        let id = candidate_id(&selected.candidate)?;
        session = Some(connected);
        (
            id,
            CachedCandidate {
                query: query.to_owned(),
                score: selected.score,
                candidate: selected.candidate.clone(),
            },
        )
    };
    if saved.candidate.files.is_empty() || saved.candidate.username.trim().is_empty() {
        return Err("selected Soulseek result has no user or files".into());
    }
    if saved.candidate.username.chars().any(char::is_control) {
        return Err("Soulseek result has an invalid username".into());
    }
    println!(
        "Selected {} ({} file(s), score {:.1})",
        id,
        saved.candidate.files.len(),
        saved.score
    );
    let output = args
        .output
        .clone()
        .or_else(|| {
            setting(&config, "MUZIK_SOULSEEK_DOWNLOAD_DIR", "download_dir").map(PathBuf::from)
        })
        .unwrap_or_else(|| paths::data_dir().join("soulseek"));
    let root = paths::expand_home(&output).join(format!("soulseek_{id}"));
    let files = local_files(&saved.candidate, &root)?;
    for file in &files {
        if file.exists() {
            return Err(format!(
                "download target already exists: {}",
                file.display()
            ));
        }
    }
    if args.dry_run {
        println!(
            "Would request {} file(s) from {}:",
            saved.candidate.files.len(),
            saved.candidate.username
        );
        for (remote, local) in saved.candidate.files.iter().zip(&files) {
            println!("  {} -> {}", safe_display(&remote.name), local.display());
        }
        return Ok(());
    }
    let connected = if let Some(connected) = session {
        connected
    } else {
        connect(&config)?
    };
    for local in connected.fetch(
        &saved.candidate,
        &root,
        Timeouts::configured(&config).download,
        &AtomicBool::new(false),
    )? {
        println!("Downloaded {}", local.display());
    }
    let sidecar = root.join(".muzik.json");
    let title = saved
        .candidate
        .files
        .first()
        .map(|file| remote_parent(&file.name))
        .filter(|parent| !parent.is_empty())
        .unwrap_or(saved.query.as_str());
    let (artist, album) = saved
        .query
        .split_once(" - ")
        .map_or((None, title), |(artist, _)| (Some(artist), title));
    let source_id = saved
        .candidate
        .files
        .first()
        .map(|file| format!("{}:{}", saved.candidate.username, file.name))
        .unwrap_or_default();
    let metadata = json!({
        "version": 1,
        "source": "soulseek",
        "source_id": source_id,
        "requested": saved.query,
        "resolved": {"title": title, "artist": artist, "album": album, "tracks": []},
        "candidate": saved.candidate,
    });
    let mut bytes = serde_json::to_vec_pretty(&metadata).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    fs::write(&sidecar, bytes)
        .map_err(|error| format!("cannot write {}: {error}", sidecar.display()))?;
    println!("Metadata: {}", sidecar.display());
    if !args.no_organize {
        let import_args = Import {
            directory: Some(root),
            library: None,
            copy: false,
            link: false,
            nowrite: false,
            quiet: false,
            dry_run: false,
            no_prune: true,
            duplicates: muzik_core::DuplicatePolicy::default(),
            config: None,
        };
        import::run(&import_args)?;
    }
    Ok(())
}

fn choose_index(count: usize, no_interactive: bool) -> Result<usize, String> {
    if no_interactive {
        return Ok(0);
    }
    let number = dialoguer::Input::<usize>::new()
        .with_prompt("Candidate number")
        .default(1)
        .validate_with(|number: &usize| {
            if (1..=count).contains(number) {
                Ok(())
            } else {
                Err(format!("Enter a number from 1 to {count}."))
            }
        })
        .interact_text()
        .map_err(
            |_| "no candidate was selected; use --no-interactive to select the first result",
        )?;
    Ok(number - 1)
}

fn safe_display(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                '�'
            } else {
                character
            }
        })
        .collect()
}

fn remote_parent(path: &str) -> &str {
    path.rsplit_once(['/', '\\'])
        .map(|(parent, _)| parent.rsplit(['/', '\\']).next().unwrap_or(parent))
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::{candidate_row, check_library};
    use crate::SoulseekCheckLibrary;
    use muzik_library::Library;
    use muzik_soulseek::ranking::RankedCandidate;
    use muzik_soulseek::types::{Candidate, FileEntry};
    use std::fs;

    fn candidate(names: &[&str]) -> Candidate {
        Candidate {
            username: "peer".into(),
            slots: 1,
            speed: 100_000,
            files: names
                .iter()
                .map(|name| FileEntry {
                    name: (*name).to_owned(),
                    size: 100,
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
    fn search_result_has_a_small_structured_row() {
        let result = candidate_row(
            &RankedCandidate {
                candidate: candidate(&["Album\\01 Song.flac"]),
                score: 120.0,
            },
            "0123456789abcdef",
        );
        assert_eq!(result["id"], "0123456789abcdef");
        assert_eq!(result["format"], "flac");
        assert_eq!(result["file_count"], 1);
        assert_eq!(result["username"], "peer");
    }

    #[test]
    fn check_library_keeps_existing_beets_files_unchanged() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let database = dir.path().join("library.db");
        Library::open_or_create(&database)?;
        let config = dir.path().join("config.yaml");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\n",
                dir.path().join("Music").display(),
                database.display(),
                dir.path().join("state.pickle").display()
            ),
        )?;
        let database_before = fs::read(&database)?;
        let config_before = fs::read(&config)?;
        check_library(&SoulseekCheckLibrary {
            query: Some("artist:Mara".into()),
            min_bitrate: 256,
            prefer: muzik_core::PreferredAudio::default(),
            limit: 20,
            config: Some(config.clone()),
        })?;
        assert_eq!(fs::read(&database)?, database_before);
        assert_eq!(fs::read(&config)?, config_before);
        Ok(())
    }
}
