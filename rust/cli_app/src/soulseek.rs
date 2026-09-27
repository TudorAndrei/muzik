use muzik_core::{app_config, paths};
use muzik_soulseek::job::{JobOutcome, JobState};
use muzik_soulseek::ranking::{rank, search_query};
use muzik_soulseek::session::{Session, SessionSettings, setting};
use std::thread;
use std::time::{Duration, Instant};

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

pub fn search(query: &str, prefer: &str, limit: usize) -> Result<(), String> {
    if query.trim().is_empty() {
        return Err("search query must not be empty".into());
    }
    if !(1..=100).contains(&limit) {
        return Err("limit must be from 1 to 100".into());
    }
    let config = app_config::load(&app_config::path())?;
    let settings = SessionSettings::configured(&config).ok_or(
        "Set MUZIK_SOULSEEK_USERNAME and MUZIK_SOULSEEK_PASSWORD, or save them with 'muzik config set-soulseek'.",
    )?;
    let timeout = setting(&config, "MUZIK_SOULSEEK_SEARCH_TIMEOUT", "search_timeout")
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|timeout| timeout.is_finite() && (1.0..=120.0).contains(timeout))
        .unwrap_or(15.0);
    let query = search_query(query, prefer);
    let session =
        Session::connect(settings).map_err(|error| format!("Soulseek search failed: {error}"))?;
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
    let ranked = rank(candidates, &query, prefer, limit);
    if ranked.is_empty() {
        println!("No candidates found.");
    } else {
        println!("#\tScore\tFormat\tFiles\tUser\tPath");
        for (index, item) in ranked.iter().enumerate() {
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
            println!(
                "{}\t{:.1}\t{}\t{}\t{}\t{}",
                index + 1,
                item.score,
                format,
                item.candidate.files.len(),
                item.candidate.username,
                path
            );
        }
    }
    session.close();
    Ok(())
}
