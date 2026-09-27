use muzik_core::{app_config, paths};
use muzik_soulseek::job::{JobOutcome, JobState};
use muzik_soulseek::ranking::{RankedCandidate, rank, search_query};
use muzik_soulseek::session::{Session, SessionSettings, setting};
use muzik_soulseek::types::Candidate;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::{Import, SoulseekDownload, import};

#[derive(Deserialize, Serialize)]
struct CachedCandidate {
    query: String,
    score: f64,
    candidate: Candidate,
}

pub fn check() -> Result<(), String> {
    let config = app_config::load(&app_config::path()).map_err(|error| error.to_string())?;
    let settings = SessionSettings::configured(&config).ok_or(
        "Set MUZIK_SOULSEEK_USERNAME and MUZIK_SOULSEEK_PASSWORD, or save them with 'muzik config set-soulseek'.",
    )?;
    let username = settings.username.clone();
    let host = settings
        .server_host
        .clone()
        .unwrap_or_else(|| "server.slsknet.org".into());
    let port = settings.server_port.unwrap_or(2416);
    let download_dir = setting(&config, "MUZIK_SOULSEEK_DOWNLOAD_DIR", "download_dir")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| paths::data_dir().join("soulseek"));
    let session =
        Session::connect(settings).map_err(|error| format!("Soulseek check failed: {error}"))?;
    session.close();
    println!("Soulseek reachable");
    println!("  Username: {username}");
    println!("  Server: {host}:{port}");
    println!("  Download dir: {}", download_dir.display());
    Ok(())
}

pub fn search(query: &str, prefer: &str, limit: usize, json_output: bool) -> Result<(), String> {
    let config = app_config::load(&app_config::path())?;
    let session = connect(&config)?;
    let ranked = ranked_search(&session, &config, query, prefer, limit)?;
    show_candidates(&ranked, query, json_output)?;
    session.close();
    Ok(())
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
    prefer: &str,
    limit: usize,
) -> Result<Vec<RankedCandidate>, String> {
    if query.trim().is_empty() {
        return Err("search query must not be empty".into());
    }
    if query.chars().any(char::is_control) {
        return Err("search query must not contain control characters".into());
    }
    if !(1..=100).contains(&limit) {
        return Err("limit must be from 1 to 100".into());
    }
    let timeout = setting(config, "MUZIK_SOULSEEK_SEARCH_TIMEOUT", "search_timeout")
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|timeout| timeout.is_finite() && (1.0..=120.0).contains(timeout))
        .unwrap_or(15.0);
    let query = search_query(query, prefer);
    let job = session.start_track_search(query.clone(), timeout);
    let deadline = Instant::now() + Duration::from_secs_f64(timeout + 5.0);
    let candidates = loop {
        match job.snapshot() {
            JobState::Running if Instant::now() >= deadline => {
                job.cancel();
                return Err("Timed out waiting for Soulseek search".into());
            }
            JobState::Running => thread::sleep(Duration::from_millis(200)),
            JobState::Completed(JobOutcome::Search(candidates)) => break candidates,
            JobState::Completed(JobOutcome::Download(_)) => {
                return Err("Soulseek returned a download for a search".into());
            }
            JobState::Failed(reason) => return Err(format!("Soulseek search failed: {reason}")),
            JobState::Cancelled => return Err("Soulseek search was cancelled".into()),
        }
    };
    Ok(rank(candidates, &query, prefer, limit))
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
        .map(muzik_soulseek::ranking::format)
        .find(|format| !format.is_empty())
        .unwrap_or("?");
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
        let ranked = ranked_search(&connected, &config, query, &args.prefer, args.limit)?;
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
    let root = expand_home(&output).join(format!("soulseek_{id}"));
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
    fs::create_dir_all(&root)
        .map_err(|error| format!("cannot create {}: {error}", root.display()))?;
    let timeout = setting(
        &config,
        "MUZIK_SOULSEEK_DOWNLOAD_TIMEOUT",
        "download_timeout",
    )
    .and_then(|text| text.parse::<f64>().ok())
    .filter(|timeout| timeout.is_finite() && (1.0..=3_600.0).contains(timeout))
    .unwrap_or(600.0);
    for (remote, local) in saved.candidate.files.iter().zip(&files) {
        let job = connected
            .start_download(
                saved.candidate.username.clone(),
                remote.name.clone(),
                remote.size,
                root.to_string_lossy().into_owned(),
            )
            .map_err(|error| format!("Soulseek download failed: {error}"))?;
        wait_download(&job, timeout)?;
        if !local.is_file() {
            return Err(format!(
                "Soulseek reported a completed transfer, but {} is missing",
                local.display()
            ));
        }
        println!("Downloaded {}", local.display());
    }
    connected.close();
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
    print!("Candidate number [1]: ");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let mut answer = String::new();
    let count_read = io::stdin()
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    if count_read == 0 {
        return Err(
            "no candidate was selected; use --no-interactive to select the first result".into(),
        );
    }
    let number = if answer.trim().is_empty() {
        1
    } else {
        answer
            .trim()
            .parse::<usize>()
            .map_err(|_| "candidate number must be an integer")?
    };
    if !(1..=count).contains(&number) {
        return Err("candidate number is out of range".into());
    }
    Ok(number - 1)
}

fn wait_download(job: &muzik_soulseek::job::JobHandle, timeout: f64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs_f64(timeout + 5.0);
    loop {
        match job.snapshot() {
            JobState::Running if Instant::now() >= deadline => {
                job.cancel();
                return Err("Timed out waiting for Soulseek download".into());
            }
            JobState::Running => thread::sleep(Duration::from_millis(200)),
            JobState::Completed(JobOutcome::Download(
                muzik_soulseek::types::DownloadProgress::Completed,
            )) => return Ok(()),
            JobState::Completed(JobOutcome::Download(_)) => {
                return Err("Soulseek download did not complete".into());
            }
            JobState::Completed(JobOutcome::Search(_)) => {
                return Err("Soulseek returned a search for a download".into());
            }
            JobState::Failed(reason) => return Err(format!("Soulseek download failed: {reason}")),
            JobState::Cancelled => return Err("Soulseek download was cancelled".into()),
        }
    }
}

fn local_files(candidate: &Candidate, root: &Path) -> Result<Vec<PathBuf>, String> {
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
                return Err("Soulseek result has missing or duplicate file names".into());
            }
            Ok(root.join(name))
        })
        .collect()
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

fn candidate_id(candidate: &Candidate) -> Result<String, String> {
    let bytes = serde_json::to_vec(candidate).map_err(|error| error.to_string())?;
    let digest = Sha256::digest(bytes);
    let hex = format!("{digest:x}");
    Ok(hex.get(..16).unwrap_or(&hex).to_owned())
}

fn cache_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.len() != 16 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("candidate ID must contain 16 hexadecimal digits".into());
    }
    Ok(root.join(format!("soulseek_{id}.json")))
}

fn save_candidate(root: &Path, id: &str, candidate: &CachedCandidate) -> Result<(), String> {
    let path = cache_path(root, id)?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(candidate).map_err(|error| error.to_string())?;
    fs::write(&path, bytes).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn load_candidate(root: &Path, id: &str) -> Result<CachedCandidate, String> {
    let path = cache_path(root, id)?;
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let candidate: CachedCandidate = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid Soulseek candidate: {error}"))?;
    if candidate_id(&candidate.candidate)? != id {
        return Err("cached Soulseek candidate ID does not match its files".into());
    }
    Ok(candidate)
}

fn expand_home(path: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::{
        CachedCandidate, candidate_id, candidate_row, load_candidate, local_files, save_candidate,
    };
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
    fn saved_candidate_round_trips_and_keeps_its_identity() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let selected = candidate(&["Album\\01 Song.flac"]);
        let id = candidate_id(&selected)?;
        save_candidate(
            dir.path(),
            &id,
            &CachedCandidate {
                query: "Artist Song".into(),
                score: 120.0,
                candidate: selected,
            },
        )?;
        let loaded = load_candidate(dir.path(), &id)?;
        assert_eq!(loaded.query, "Artist Song");
        assert_eq!(
            loaded
                .candidate
                .files
                .first()
                .map(|file| file.name.as_str()),
            Some("Album\\01 Song.flac")
        );
        let path = dir.path().join(format!("soulseek_{id}.json"));
        let changed =
            fs::read_to_string(&path)?.replace("Album\\\\01 Song.flac", "Album\\\\02 Song.flac");
        fs::write(&path, changed)?;
        assert!(load_candidate(dir.path(), &id).is_err());
        Ok(())
    }

    #[test]
    fn local_targets_reject_duplicate_remote_names() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let files = local_files(&candidate(&["Album\\01 Song.flac"]), dir.path())?;
        assert_eq!(
            files
                .first()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str()),
            Some("01 Song.flac")
        );
        assert!(local_files(&candidate(&["A\\Song.flac", "B\\song.FLAC"]), dir.path()).is_err());
        Ok(())
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
}
