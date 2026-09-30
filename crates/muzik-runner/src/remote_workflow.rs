//! Remote acquisition for queued workflow jobs.

use crate::gates::{self, Gate};
use crate::local_workflow;
use muzik_core::process::background_command;
use muzik_core::watchlist::Stage;
use muzik_core::{app_config, chapters::Chapter, paths, DecisionKind};
use muzik_soulseek::job::{JobOutcome, JobState};
use muzik_soulseek::ranking::{rank, search_query};
use muzik_soulseek::session::{setting, Session, SessionSettings};
use muzik_soulseek::types::{Candidate, DownloadProgress};
use muzik_workflow::{
    classify_input, playlist, run_workflow_with_events, AudioFallback, AudioSource, ChapterReview,
    QualityCheckedAudio, SplitProgress, SplitTask, WorkflowEvent, WorkflowInput,
    WorkflowOperations, WorkflowOptions,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use yt_dlp::executor::Executor;

pub struct RemoteRequest {
    input: WorkflowInput,
    local: local_workflow::LocalRequest,
}

/// Return `None` for local audio.
pub fn supported(params: &Value) -> Option<Result<RemoteRequest, String>> {
    let raw = params.get("raw")?.as_str()?.trim();
    let input = classify_input(raw);
    if matches!(input, WorkflowInput::Local(_)) || raw.is_empty() {
        return None;
    }
    Some(parse(input, params))
}

fn parse(input: WorkflowInput, params: &Value) -> Result<RemoteRequest, String> {
    let mut merged = app_config::load_gui_defaults(&app_config::path())?;
    let supplied = params
        .as_object()
        .ok_or("workflow params must be a mapping")?;
    {
        let values = merged
            .as_object_mut()
            .ok_or("GUI defaults are not a mapping")?;
        for (key, value) in supplied {
            values.insert(key.clone(), value.clone());
        }
    }
    let raw = merged["raw"]
        .as_str()
        .ok_or("raw must be a string")?
        .to_owned();
    let local = local_workflow::parse(&raw, &merged)?;
    Ok(RemoteRequest { input, local })
}

pub fn run(
    remote: RemoteRequest,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
    on_import_event: &mut dyn FnMut(Value),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<Value, muzik_workflow::Error> {
    if cancelled.load(Ordering::SeqCst) {
        return Err(muzik_workflow::Error::Cancelled);
    }
    let mut operations = RemoteOperations {
        local: local_workflow::LocalOperations {
            decide,
            on_import_event,
            cancelled,
        },
        prefer: remote.local.options.prefer.clone(),
        interactive: remote.local.options.interactive,
        audio_source: remote.local.options.audio_source,
        fallback: remote.local.options.fallback,
        output: remote.local.request.output.clone(),
        youtube_acquired: false,
    };
    let mut report = |event| on_event(event_record(event));
    if let WorkflowInput::SpotifyExport(ref path) = remote.input {
        let result = playlist::run_spotify_export(
            &remote.local.request,
            &remote.local.options,
            &mut operations,
            cancelled,
            path,
            &mut report,
        );
        if cancelled.load(Ordering::SeqCst) {
            return Err(muzik_workflow::Error::Cancelled);
        }
        let result = result?;
        let processing = result.processing;
        return Ok(
            json!({"albums":processing.plan.albums.len(),"singles":processing.plan.singles.len(),"split_dirs":processing.split_dirs,"items":result.items.len()}),
        );
    }
    if let WorkflowInput::YoutubePlaylist { url, playlist_id } = remote.input {
        let result = playlist::run_youtube_playlist(
            &remote.local.request,
            &remote.local.options,
            &mut operations,
            cancelled,
            &playlist_id,
            &url,
            &mut report,
        )?;
        let failures = result
            .items
            .iter()
            .filter(|item| !item.completed)
            .map(|item| json!({"id":item.id,"error":item.error}))
            .collect::<Vec<_>>();
        let processing = result.processing;
        return Ok(
            json!({"albums":processing.plan.albums.len(),"singles":processing.plan.singles.len(),"split_dirs":processing.split_dirs,"failures":failures}),
        );
    }
    let result = run_workflow_with_events(
        &remote.local.request,
        &remote.local.options,
        &mut operations,
        cancelled,
        &mut report,
    )?;
    let audio_files = result
        .plan
        .singles
        .iter()
        .chain(result.plan.albums.iter().map(|album| &album.source))
        .collect::<Vec<_>>();
    Ok(
        json!({"albums":result.plan.albums.len(),"singles":result.plan.singles.len(),"split_dirs":result.split_dirs,"audio_files":audio_files}),
    )
}

struct RemoteOperations<'a> {
    local: local_workflow::LocalOperations<'a>,
    prefer: String,
    interactive: bool,
    audio_source: AudioSource,
    fallback: AudioFallback,
    output: PathBuf,
    youtube_acquired: bool,
}

impl WorkflowOperations for RemoteOperations<'_> {
    fn download_youtube(
        &mut self,
        url: &str,
        output: &Path,
        force: bool,
    ) -> Result<Vec<PathBuf>, String> {
        let files = download(url, output, force, self.local.cancelled)
            .map_err(|error| error.to_string())?;
        self.youtube_acquired = true;
        Ok(files)
    }

    fn acquire_soulseek(&mut self, query: &str) -> Result<Vec<PathBuf>, String> {
        self.youtube_acquired = false;
        let query = if matches!(classify_input(query), WorkflowInput::YoutubeVideo { .. }) {
            execute(
                vec!["--skip-download".into(), query.to_owned()],
                "title",
                Duration::from_secs(120),
                self.local.cancelled,
            )
            .map_err(|error| error.to_string())?
            .trim()
            .to_owned()
        } else {
            query.to_owned()
        };
        if query.is_empty() {
            return Err("YouTube video has no title for Soulseek search".into());
        }
        soulseek_download(
            &query,
            &self.prefer,
            self.interactive,
            self.local.cancelled,
            self.local.decide,
            false,
            None,
        )
    }

    fn acquire_spotify_track(
        &mut self,
        track: &playlist::SpotifyTrack,
    ) -> Result<Vec<PathBuf>, String> {
        let query = format!("{} - {}", track.artist, track.title);
        let ready = self.soulseek_ready();
        let source = self.audio_source;
        let fallback = self.fallback;
        let output = self.output.clone();
        let cancelled = self.local.cancelled;
        let prefer = self.prefer.clone();
        let interactive = self.interactive;
        let decide = &mut *self.local.decide;
        let (files, from_youtube) = acquire_spotify_audio(
            source,
            fallback,
            ready,
            || soulseek_download(&query, &prefer, interactive, cancelled, decide, false, None),
            || download(&query, &output, false, cancelled).map_err(|error| error.to_string()),
        )?;
        self.youtube_acquired = from_youtube;
        Ok(files)
    }

    fn soulseek_ready(&self) -> bool {
        soulseek_ready()
    }

    fn check_quality(
        &mut self,
        audio_files: &[PathBuf],
        options: &WorkflowOptions,
        cancelled: &AtomicBool,
    ) -> Result<QualityCheckedAudio, String> {
        let from_youtube = std::mem::take(&mut self.youtube_acquired)
            || audio_files
                .iter()
                .any(|path| muzik_core::chapters::sidecar_path(path, ".info.json").is_file());
        if !from_youtube {
            return Ok(QualityCheckedAudio {
                audio_files: audio_files.to_vec(),
                pre_split_dirs: Vec::new(),
            });
        }
        let _permit = gates::enter(Gate::Process, Stage::Quality, cancelled)?;
        let result = muzik_workflow::quality::check_youtube_quality(
            audio_files.to_vec(),
            options.quality_policy,
            options.min_bitrate,
            &self.prefer,
            cancelled,
            self.local.on_import_event,
            self.local.decide,
        )?;
        Ok(QualityCheckedAudio {
            audio_files: result.audio_files,
            pre_split_dirs: result.pre_split_dirs,
        })
    }

    fn youtube_playlist_video_ids(&mut self, url: &str) -> Result<Vec<String>, String> {
        playlist_ids(url, self.local.cancelled).map_err(|error| error.to_string())
    }

    fn organize(&mut self, target: &Path, options: &WorkflowOptions) -> Result<(), String> {
        self.local.organize(target, options)
    }

    fn review_chapters(
        &mut self,
        source: &Path,
        chapters: &[Chapter],
        cancelled: &AtomicBool,
    ) -> Result<ChapterReview, String> {
        self.local.review_chapters(source, chapters, cancelled)
    }

    fn split(&mut self, task: &SplitTask, options: &WorkflowOptions) -> Result<(), String> {
        self.local.split(task, options)
    }

    fn split_with_cancel(
        &mut self,
        task: &SplitTask,
        options: &WorkflowOptions,
        cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(SplitProgress),
    ) -> Result<(), String> {
        self.local
            .split_with_cancel(task, options, cancelled, on_progress)
    }
}

pub(crate) fn soulseek_ready() -> bool {
    app_config::load(&app_config::path())
        .ok()
        .is_some_and(|config| SessionSettings::configured(&config).is_some())
}

/// Apply the same source and fallback policy to Spotify exports and saved items.
pub(crate) fn acquire_spotify_audio<S, Y>(
    source: AudioSource,
    fallback: AudioFallback,
    soulseek_ready: bool,
    mut soulseek: S,
    mut youtube: Y,
) -> Result<(Vec<PathBuf>, bool), String>
where
    S: FnMut() -> Result<Vec<PathBuf>, String>,
    Y: FnMut() -> Result<Vec<PathBuf>, String>,
{
    if source == AudioSource::Youtube || source == AudioSource::Auto && !soulseek_ready {
        return youtube().map(|files| (files, true));
    }
    match soulseek() {
        Ok(files) if !files.is_empty() => Ok((files, false)),
        Ok(_) | Err(_) if fallback == AudioFallback::Youtube => {
            youtube().map(|files| (files, true))
        }
        Ok(files) => Ok((files, false)),
        Err(error) => Err(error),
    }
}

fn event_record(event: WorkflowEvent) -> Value {
    match event {
        WorkflowEvent::InputClassified(_) => {
            json!({"event":"message","data":{"message":"Reading remote input."}})
        }
        WorkflowEvent::AcquisitionStarted => {
            json!({"event":"step_started","data":{"name":"download"}})
        }
        WorkflowEvent::AcquisitionCompleted { files } => {
            json!({"event":"step_finished","data":{"name":"download","files":files}})
        }
        WorkflowEvent::Completed => {
            json!({"event":"message","data":{"message":"Remote workflow complete."}})
        }
        other => local_workflow::event_record(other),
    }
}

pub(crate) fn soulseek_download(
    query: &str,
    prefer: &str,
    interactive: bool,
    cancelled: &AtomicBool,
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    single_file: bool,
    output_root: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("Soulseek search cancelled".into());
    }
    let _permit = gates::enter(Gate::Download, Stage::Download, cancelled)?;
    let config = app_config::load(&app_config::path())?;
    let settings = SessionSettings::configured(&config)
        .ok_or("Set Soulseek credentials in configuration first.")?;
    let session = Session::shared(settings).map_err(|error| error.to_string())?;
    let search_timeout = configured_timeout(
        &config,
        "MUZIK_SOULSEEK_SEARCH_TIMEOUT",
        "search_timeout",
        15.0,
        120.0,
    );
    let search = session.start_track_search(search_query(query, prefer), search_timeout);
    let candidates = loop {
        if cancelled.load(Ordering::SeqCst) {
            search.cancel();
            return Err("Soulseek search cancelled".into());
        }
        match search.snapshot() {
            JobState::Running => std::thread::sleep(Duration::from_millis(100)),
            JobState::Completed(JobOutcome::Search(candidates)) => break candidates,
            JobState::Completed(JobOutcome::Download(_)) => {
                return Err("Soulseek returned a download for a search.".into())
            }
            JobState::Failed(error) => {
                Session::forget_shared();
                return Err(error);
            }
            JobState::Cancelled => return Err("Soulseek search cancelled".into()),
        }
    };
    let ranked = rank(candidates, query, prefer, 10);
    if ranked.is_empty() {
        return Err(format!("No Soulseek audio found for {query}."));
    }
    let selected = if interactive {
        let rows = ranked
            .iter()
            .map(|item| candidate_row(&item.candidate, item.score))
            .collect::<Vec<_>>();
        let answer = decide(
            DecisionKind::SoulseekCandidate,
            json!({"query":query,"candidates":rows}),
        )?;
        let index = answer
            .as_u64()
            .or_else(|| answer.get("index").and_then(Value::as_u64))
            .and_then(|index| usize::try_from(index).ok())
            .ok_or("Select a Soulseek candidate index.")?;
        ranked
            .get(index)
            .ok_or("Select a Soulseek candidate index in range.")?
    } else {
        ranked.first().ok_or("No Soulseek audio found.")?
    };
    if selected.candidate.username.trim().is_empty()
        || selected.candidate.username.chars().any(char::is_control)
    {
        return Err("Soulseek result has an invalid username.".into());
    }
    let output = output_root
        .map(Path::to_path_buf)
        .or_else(|| {
            setting(&config, "MUZIK_SOULSEEK_DOWNLOAD_DIR", "download_dir").map(PathBuf::from)
        })
        .unwrap_or_else(|| paths::data_dir().join("soulseek"));
    std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let destination = tempfile::Builder::new()
        .prefix("soulseek-")
        .tempdir_in(&output)
        .map_err(|error| error.to_string())?;
    let timeout = configured_timeout(
        &config,
        "MUZIK_SOULSEEK_DOWNLOAD_TIMEOUT",
        "download_timeout",
        600.0,
        3600.0,
    );
    let mut names = std::collections::HashSet::new();
    for file in selected
        .candidate
        .files
        .iter()
        .filter(|file| !muzik_soulseek::ranking::format(file).is_empty())
        .take(if single_file { 1 } else { usize::MAX })
    {
        if cancelled.load(Ordering::SeqCst) {
            return Err("Soulseek download cancelled".into());
        }
        let name = file.name.rsplit(['/', '\\']).next().unwrap_or("");
        if name.is_empty()
            || name.chars().any(char::is_control)
            || !names.insert(name.to_ascii_lowercase())
        {
            return Err("Soulseek result has missing or duplicate file names.".into());
        }
        let job = session
            .start_download(
                selected.candidate.username.clone(),
                file.name.clone(),
                file.size,
                destination.path().to_string_lossy().into_owned(),
            )
            .map_err(|error| error.to_string())?;
        let deadline = Instant::now() + Duration::from_secs_f64(timeout + 5.0);
        loop {
            if cancelled.load(Ordering::SeqCst) {
                job.cancel();
                return Err("Soulseek download cancelled".into());
            }
            match job.snapshot() {
                JobState::Running if Instant::now() >= deadline => {
                    job.cancel();
                    return Err("Soulseek download timed out".into());
                }
                JobState::Running => std::thread::sleep(Duration::from_millis(100)),
                JobState::Completed(JobOutcome::Download(DownloadProgress::Completed)) => break,
                JobState::Completed(JobOutcome::Download(_)) => {
                    return Err("Soulseek download did not complete".into())
                }
                JobState::Completed(JobOutcome::Search(_)) => {
                    return Err("Soulseek returned a search for a download".into())
                }
                JobState::Failed(error) => return Err(error),
                JobState::Cancelled => return Err("Soulseek download cancelled".into()),
            }
        }
    }
    let files = muzik_workflow::find_audio_inputs(&[destination.path().to_path_buf()])
        .map_err(|error| error.to_string())?;
    if files.is_empty() {
        return Err("Soulseek returned no audio files.".into());
    }
    let root = destination.keep();
    muzik_workflow::find_audio_inputs(&[root]).map_err(|error| error.to_string())
}

fn configured_timeout(
    config: &Value,
    environment: &str,
    key: &str,
    default: f64,
    maximum: f64,
) -> f64 {
    setting(config, environment, key)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && (1.0..=maximum).contains(value))
        .unwrap_or(default)
}

fn candidate_row(candidate: &Candidate, score: f64) -> Value {
    let path = candidate
        .files
        .first()
        .map(|file| file.name.as_str())
        .unwrap_or("");
    let title = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let format = candidate
        .files
        .iter()
        .map(muzik_soulseek::ranking::format)
        .find(|format| !format.is_empty())
        .unwrap_or("");
    json!({"title":title,"score":score,"user":candidate.username,"quality":{"format":format},"files":candidate.files,"path":path})
}

pub(crate) fn playlist_ids(
    url: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<String>, muzik_workflow::Error> {
    let args = vec!["--flat-playlist".to_owned(), url.to_owned()];
    let output = execute(args, "id", Duration::from_secs(600), cancelled)?;
    let ids = output
        .lines()
        .map(str::trim)
        .filter(|id| {
            id.len() == 11
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Err(muzik_workflow::Error::Operation(
            "Playlist contains no available videos.".into(),
        ));
    }
    Ok(ids)
}

pub(crate) fn download(
    url: &str,
    output: &Path,
    force: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<PathBuf>, muzik_workflow::Error> {
    let _permit = gates::enter(Gate::Download, Stage::Download, cancelled)
        .map_err(|_| muzik_workflow::Error::Cancelled)?;
    std::fs::create_dir_all(output)?;
    let output = std::fs::canonicalize(output)?;
    let target = if matches!(classify_input(url), WorkflowInput::Search(_)) {
        format!("ytsearch1:{url}")
    } else {
        url.to_owned()
    };
    let mut args = vec![
        "--no-playlist".to_owned(),
        "--paths".to_owned(),
        output.to_string_lossy().into_owned(),
        "--format".to_owned(),
        "bestaudio".to_owned(),
        "--extract-audio".to_owned(),
        "--audio-quality".to_owned(),
        "0".to_owned(),
        "--embed-metadata".to_owned(),
        "--add-metadata".to_owned(),
        "--write-thumbnail".to_owned(),
        "--convert-thumbnails".to_owned(),
        "jpg".to_owned(),
        "--write-info-json".to_owned(),
        "--embed-chapters".to_owned(),
        "--output".to_owned(),
        "%(title)s [%(id)s].%(ext)s".to_owned(),
        target,
    ];
    if force {
        args.insert(0, "--force-overwrites".into());
    }
    let stdout = execute(
        args,
        "after_move:filepath",
        Duration::from_secs(24 * 60 * 60),
        cancelled,
    )?;
    let files = stdout
        .lines()
        .map(str::trim)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    let files = muzik_workflow::find_audio_inputs(&files)?;
    if files.is_empty() {
        return Err(muzik_workflow::Error::NoAudio);
    }
    Ok(files)
}

fn execute(
    mut args: Vec<String>,
    template: &str,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<String, muzik_workflow::Error> {
    execute_with_path(Path::new("yt-dlp"), &mut args, template, timeout, cancelled)
}

fn execute_with_path(
    executable: &Path,
    args: &mut Vec<String>,
    template: &str,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> Result<String, muzik_workflow::Error> {
    if cancelled.load(Ordering::SeqCst) {
        return Err(muzik_workflow::Error::Cancelled);
    }
    args.splice(0..0, yt_dlp_environment_args());
    let output = tempfile::NamedTempFile::new()?;
    let target = args
        .pop()
        .ok_or_else(|| muzik_workflow::Error::Operation("yt-dlp target is missing".into()))?;
    args.extend([
        "--quiet".to_owned(),
        "--print-to-file".to_owned(),
        template.to_owned(),
        output.path().to_string_lossy().into_owned(),
        target,
    ]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| muzik_workflow::Error::Operation(error.to_string()))?;
    runtime.block_on(async {
        let executor = Executor::new(executable, args.iter().cloned(), timeout);
        let mut command =
            tokio::process::Command::from(background_command(executor.executable_path()));
        command.args(executor.args()).kill_on_drop(true);
        let result = tokio::select! {
            result = command.output() => result.map_err(muzik_workflow::Error::Io),
            () = async {
                while !cancelled.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            } => Err(muzik_workflow::Error::Cancelled),
            () = tokio::time::sleep(timeout) => Err(muzik_workflow::Error::Operation("yt-dlp timed out".into())),
        };
        let result = result?;
        if !result.status.success() {
            return Err(muzik_workflow::Error::Operation(format!(
                "yt-dlp failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            )));
        }
        Ok(())
    })?;
    if cancelled.load(Ordering::SeqCst) {
        return Err(muzik_workflow::Error::Cancelled);
    }
    std::fs::read_to_string(output.path()).map_err(muzik_workflow::Error::Io)
}

pub(crate) fn yt_dlp_environment_args() -> Vec<String> {
    let mut args = Vec::new();
    let browser = std::env::var("MUZIK_YTDLP_COOKIES_FROM_BROWSER").unwrap_or_default();
    if !browser.trim().is_empty() {
        args.extend(["--cookies-from-browser".to_owned(), browser]);
    } else {
        let cookies = std::env::var("MUZIK_YTDLP_COOKIES").unwrap_or_default();
        if !cookies.trim().is_empty() {
            args.extend(["--cookies".to_owned(), cookies]);
        }
    }
    for runtime in ["node", "bun"] {
        if std::env::var_os("PATH")
            .into_iter()
            .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
            .any(|directory| directory.join(runtime).is_file())
        {
            args.extend(["--js-runtimes".to_owned(), runtime.to_owned()]);
            break;
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::{acquire_spotify_audio, candidate_row, execute_with_path, supported};
    use muzik_core::{AudioFallback, AudioSource};
    use muzik_soulseek::types::{Candidate, FileEntry};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn spotify_youtube_source_does_not_call_soulseek() -> Result<(), String> {
        let audio = std::path::PathBuf::from("youtube-audio.flac");
        let (files, from_youtube) = acquire_spotify_audio(
            AudioSource::Youtube,
            AudioFallback::None,
            true,
            || Err("Soulseek must not run".into()),
            || Ok(vec![audio.clone()]),
        )?;
        assert_eq!(files, vec![audio]);
        assert!(from_youtube);
        Ok(())
    }

    #[test]
    fn spotify_soulseek_failure_uses_selected_fallback() -> Result<(), String> {
        let audio = std::path::PathBuf::from("fallback-audio.flac");
        let (files, from_youtube) = acquire_spotify_audio(
            AudioSource::Soulseek,
            AudioFallback::Youtube,
            true,
            || Err("Soulseek is unavailable".into()),
            || Ok(vec![audio.clone()]),
        )?;
        assert_eq!(files, vec![audio]);
        assert!(from_youtube);
        let error = acquire_spotify_audio(
            AudioSource::Soulseek,
            AudioFallback::None,
            true,
            || Err("Soulseek is unavailable".into()),
            || Err("YouTube must not run".into()),
        )
        .err()
        .ok_or("Soulseek failure must be returned")?;
        assert_eq!(error, "Soulseek is unavailable");
        Ok(())
    }

    #[test]
    fn selects_youtube_video_and_playlist() -> Result<(), Box<dyn std::error::Error>> {
        for raw in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/playlist?list=PLnative123",
        ] {
            let selected = supported(&json!({"raw":raw,"audio_source":"youtube"}))
                .ok_or("YouTube request was not selected")??;
            assert_eq!(
                selected.local.request.output,
                muzik_core::paths::download_dir()
            );
        }
        Ok(())
    }

    #[test]
    fn selects_search_and_spotify_export() -> Result<(), Box<dyn std::error::Error>> {
        let search =
            supported(&json!({"raw":"artist - track","audio_source":"soulseek","review":true}))
                .ok_or("search was not selected")??;
        assert!(matches!(
            search.input,
            muzik_workflow::WorkflowInput::Search(_)
        ));
        assert!(search.local.options.review);
        assert_eq!(
            search.local.options.audio_source,
            muzik_workflow::AudioSource::Soulseek
        );

        let dir = tempfile::tempdir()?;
        let export = dir.path().join("spotify.json");
        std::fs::write(&export, b"{}")?;
        let spotify = supported(&json!({"raw":export,"audio_source":"soulseek"}))
            .ok_or("Spotify export was not selected")??;
        assert!(matches!(
            spotify.input,
            muzik_workflow::WorkflowInput::SpotifyExport(_)
        ));
        Ok(())
    }

    #[test]
    fn soulseek_choice_has_the_fields_used_by_the_desktop_view() {
        let candidate = Candidate {
            username: "peer".into(),
            slots: 1,
            speed: 100,
            files: vec![FileEntry {
                name: "Artist\\Album\\01 Song.flac".into(),
                size: 42,
                bitrate_kbps: None,
                duration_seconds: None,
                vbr: None,
                sample_rate_hz: None,
                bit_depth: None,
            }],
        };
        let row = candidate_row(&candidate, 70.0);
        assert_eq!(row["title"], "01 Song.flac");
        assert_eq!(row["user"], "peer");
        assert_eq!(row["quality"]["format"], "flac");
        assert_eq!(row["files"].as_array().map(Vec::len), Some(1));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_stops_an_active_youtube_process() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let script = dir.path().join("yt-dlp");
        let started = dir.path().join("started");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\nsleep 30\n", started.display()),
        )?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let script_for_job = script.clone();
        let start = Instant::now();
        let job = std::thread::spawn(move || {
            execute_with_path(
                &script_for_job,
                &mut vec!["https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()],
                "after_move:filepath",
                Duration::from_secs(40),
                &flag,
            )
        });
        while !started.is_file() {
            if start.elapsed() > Duration::from_secs(3) {
                cancelled.store(true, Ordering::SeqCst);
                return Err("yt-dlp test process did not start".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        cancelled.store(true, Ordering::SeqCst);
        let result = job.join().map_err(|_| "yt-dlp test thread stopped")?;
        assert!(matches!(result, Err(muzik_workflow::Error::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(5));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn youtube_executor_reads_printed_file_path() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let script = dir.path().join("yt-dlp");
        std::fs::write(
            &script,
            "#!/bin/sh\nwhile [ \"$1\" != \"--print-to-file\" ]; do shift; done\nshift\nshift\nprintf '%s\\n' '/tmp/audio.flac' >> \"$1\"\n",
        )?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
        let result = execute_with_path(
            &script,
            &mut vec!["https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()],
            "after_move:filepath",
            Duration::from_secs(2),
            &AtomicBool::new(false),
        )?;
        assert_eq!(result.trim(), "/tmp/audio.flac");
        Ok(())
    }
}
