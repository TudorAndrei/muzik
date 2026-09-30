//! Rust source and audio operations for saved watchlist jobs.

use crate::gates::{self, Gate};
use crate::{local_workflow, remote_workflow};
use muzik_core::watchlist::jobs::{
    self, ItemSelection, JobError, JobOptions, LoadedSource, Operations, PendingItem,
};
use muzik_core::watchlist::{
    set_stage_status, stage_status, ItemAction, SourceKind, Stage, StageStatus,
};
use muzik_core::{
    app_config, chapters, paths, spotify, watchlist, AudioSource, ChapterAnswer, DecisionKind,
    QualityPolicy,
};
use muzik_workflow::playlist::{write_spotify_tags, SpotifyTags};
use muzik_workflow::quality::{check_youtube_quality, QualityUpgradeResult};
use muzik_workflow::{process_audio_plan_with_events, WorkflowOperations, WorkflowOptions};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use yt_dlp::executor::Executor;

pub fn sync(
    params: &Value,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
) -> Result<Vec<PendingItem>, JobError> {
    let prepared = Prepared::new(params)?;
    let events = RefCell::new(on_event);
    let parked = RefCell::new(None);
    let mut adapter = Adapter {
        prepared: &prepared,
        events: &events,
        on_import_event: &mut |_| {},
        decide: &mut |_, _| Err("A playlist check does not ask for choices.".into()),
        parked: &parked,
        cancelled,
    };
    let synced = jobs::sync(
        &prepared.repository,
        prepared.job_options(),
        &mut adapter,
        cancelled,
        &mut |record| {
            (events.borrow_mut())(record);
        },
    )?;
    (events.borrow_mut())(
        json!({"event":"progress_finished","data":{"task_id":"watchlist-refresh","success":synced.errors == 0}}),
    );
    Ok(synced.pending)
}

pub fn action(
    params: &Value,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
    on_import_event: &mut dyn FnMut(Value),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    parked: &RefCell<Option<Parked>>,
) -> Result<Value, JobError> {
    let prepared = Prepared::new(params)?;
    let playlist_id = required(params, "playlist_id")?;
    let position = params["position"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| JobError::Operation("position must be a positive integer".into()))?;
    let video_id = params["video_id"].as_str();
    let name = required(params, "action")?;
    let name: ItemAction = name
        .parse()
        .map_err(|_| JobError::Operation(format!("Unknown item action: {name}")))?;
    let events = RefCell::new(on_event);
    let mut adapter = Adapter {
        prepared: &prepared,
        events: &events,
        on_import_event,
        decide,
        parked,
        cancelled,
    };
    jobs::action(
        &prepared.repository,
        prepared.job_options(),
        ItemSelection {
            playlist_id,
            position,
            video_id,
            action: name,
        },
        &mut adapter,
        cancelled,
    )
}

fn required<'a>(params: &'a Value, key: &str) -> Result<&'a str, JobError> {
    params[key]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| JobError::Operation(format!("{key} must be a non-empty string")))
}

struct Prepared {
    params: Value,
    local: local_workflow::LocalRequest,
    cache: PathBuf,
    quality_policy: QualityPolicy,
    repository: watchlist::Repository,
}

pub struct Parked {
    pub kind: DecisionKind,
    pub payload: Value,
}

impl Parked {
    fn question(&self) -> Value {
        json!({"kind":self.kind,"payload":self.payload})
    }
}

impl Prepared {
    fn new(params: &Value) -> Result<Self, JobError> {
        let mut merged = app_config::load_gui_defaults(&app_config::path())?;
        let saved = merged
            .as_object_mut()
            .ok_or_else(|| JobError::Operation("GUI defaults are not a mapping".into()))?;
        let supplied = params
            .as_object()
            .ok_or_else(|| JobError::Operation("watchlist params must be a mapping".into()))?;
        saved.extend(supplied.clone());
        let quality_policy = merged["quality_policy"]
            .as_str()
            .and_then(|policy| policy.parse().ok())
            .unwrap_or_default();
        let raw = merged["raw"].as_str().unwrap_or("").to_owned();
        let local = local_workflow::parse(&raw, &merged)?;
        let cache = paths::cache_dir();
        Ok(Self {
            params: merged,
            local,
            cache,
            quality_policy,
            repository: watchlist::Repository::new(watchlist::Repository::default_path()),
        })
    }

    fn job_options(&self) -> JobOptions<'_> {
        JobOptions {
            reconcile: watchlist::ReconcileOptions {
                output: &self.local.request.output,
                splits: &self.local.request.splits,
                cache: &self.cache,
                config: self.local.options.config.as_deref(),
                no_organize: self.local.options.no_organize,
                no_split: self.local.options.no_split,
                quality_policy: self.quality_policy,
            },
            output: &self.local.request.output,
            cache: &self.cache,
            dry_run: self.local.options.dry_run,
        }
    }
}

struct Adapter<'a, 'b> {
    prepared: &'a Prepared,
    events: &'a RefCell<&'b mut dyn FnMut(Value)>,
    on_import_event: &'a mut dyn FnMut(Value),
    decide: &'a mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    parked: &'a RefCell<Option<Parked>>,
    cancelled: &'a AtomicBool,
}

impl Operations for Adapter<'_, '_> {
    fn load(&mut self, playlist: &Value) -> Result<LoadedSource, JobError> {
        check_cancelled(self.cancelled)?;
        let id = required(playlist, "playlist_id")?;
        if SourceKind::of(playlist) == SourceKind::Spotify {
            let document =
                spotify::load_playlist_document(&app_config::path(), &spotify::token_path(), id)?;
            check_cancelled(self.cancelled)?;
            return spotify_items(&document);
        }
        let url = required(playlist, "url")?;
        let source = youtube_source(url, self.cancelled)?;
        Ok(youtube_items(playlist, &source))
    }

    fn process(
        &mut self,
        playlist: &Value,
        item: &Value,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<Value, JobError> {
        check_cancelled(cancelled)?;
        self.parked.replace(None);
        gates::take_stage();
        let result = match SourceKind::of(item) {
            SourceKind::Spotify => self.process_spotify(playlist, item, action, cancelled),
            SourceKind::Youtube => self.process_youtube(item, action, cancelled),
        };
        let stage = gates::take_stage();
        result.map_err(|error| {
            let Some(parked) = self.parked.replace(None) else {
                return match (error, stage) {
                    (JobError::Operation(message), Some(stage)) => {
                        JobError::Failed { stage, message }
                    }
                    (error, _) => error,
                };
            };
            let stage = parked.kind.stage();
            let question = parked.question();
            (self.events.borrow_mut())(json!({"event":"item_waiting","data":{
                "playlist_id":playlist["playlist_id"],
                "position":item["position"],
                "video_id":item["video_id"].as_str().or_else(|| item["entry_id"].as_str()),
                "title":item["title"],
                "stage":stage,
                "question":question,
            }}));
            JobError::Waiting { stage, question }
        })
    }
}

impl Adapter<'_, '_> {
    fn process_youtube(
        &mut self,
        item: &Value,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<Value, JobError> {
        if action.stage() != Stage::Download {
            return self.process_local_stage(item, action, cancelled);
        }
        if matches!(action, ItemAction::Run | ItemAction::Retry)
            && ready_quality_directory(item).is_some()
        {
            if !self.prepared.local.options.no_organize {
                return self.process_local_stage(item, ItemAction::OrganizeAgain, cancelled);
            }
            let mut updated = item.clone();
            set_stage(&mut updated, Stage::Organize, StageStatus::Skipped);
            return Ok(updated);
        }
        let url = required(item, "video_url")?;
        let mut params = self.prepared.params.clone();
        params["raw"] = json!(url);
        match action {
            ItemAction::DownloadAgain => {
                params["force"] = json!(true);
                params["no_split"] = json!(true);
                params["no_organize"] = json!(true);
            }
            ItemAction::RunAllAgain => params["force"] = json!(true),
            _ => {}
        }
        let remote = remote_workflow::supported(&params).ok_or_else(|| {
            JobError::Operation("The saved YouTube item is not a video URL.".into())
        })??;
        let result = remote_workflow::run(
            remote,
            cancelled,
            &mut |event| (self.events.borrow_mut())(event),
            self.on_import_event,
            self.decide,
        )
        .map_err(workflow_error)?;
        let mut updated = item.clone();
        save_output_paths(&mut updated, &result, &self.prepared.local.request.output)?;
        if action == ItemAction::DownloadAgain {
            set_stage(&mut updated, Stage::Download, StageStatus::Complete);
            for stage in [Stage::Parse, Stage::Split, Stage::Organize] {
                set_stage(&mut updated, stage, StageStatus::Stale);
            }
        } else {
            let split = result["split_dirs"]
                .as_array()
                .is_some_and(|dirs| !dirs.is_empty());
            mark_full(&mut updated, &self.prepared.local.options, split);
        }
        Ok(updated)
    }

    fn process_local_stage(
        &mut self,
        item: &Value,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<Value, JobError> {
        let audio = downloaded_audio(item, &self.prepared.local.request.output)?;
        let mut updated = item.clone();
        if action == ItemAction::OrganizeAgain {
            let target = stage_path(item, Stage::Split)
                .as_str()
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
                .or(audio)
                .ok_or_else(|| {
                    JobError::Operation(
                        "No downloaded audio or split directory is available.".into(),
                    )
                })?;
            let mut local = local_workflow::LocalOperations {
                decide: self.decide,
                on_import_event: self.on_import_event,
                cancelled,
            };
            let mut options = self.prepared.local.options.clone();
            options.force = true;
            options.no_organize = false;
            local.organize(&target, &options)?;
            check_cancelled(cancelled)?;
            set_stage(&mut updated, Stage::Organize, StageStatus::Complete);
            return Ok(updated);
        }
        let audio = audio
            .ok_or_else(|| JobError::Operation("Downloaded audio is not available.".into()))?;
        if action == ItemAction::CheckQualityAgain {
            set_path(&mut updated, Stage::Download, json!(audio));
            let _permit = gates::enter(Gate::Process, Stage::Quality, cancelled)
                .map_err(|_| JobError::Cancelled)?;
            let result = check_youtube_quality(
                vec![audio],
                self.prepared.local.options.quality_policy,
                self.prepared.local.options.min_bitrate,
                &self.prepared.local.options.prefer,
                cancelled,
                &mut |event| (self.events.borrow_mut())(event),
                self.decide,
            )
            .map_err(|error| {
                if cancelled.load(Ordering::SeqCst) {
                    JobError::Cancelled
                } else {
                    JobError::Operation(error)
                }
            })?;
            apply_quality_result(&mut updated, &result);
            return Ok(updated);
        }
        if action == ItemAction::ParseAgain {
            gates::mark_stage(Stage::Parse);
            let video_url = required(item, "video_url")?;
            let chapter_path = refresh_chapters(&audio, video_url, cancelled, self.decide)?;
            set_stage(&mut updated, Stage::Parse, StageStatus::Complete);
            set_path(&mut updated, Stage::Parse, json!(chapter_path));
            for stage in [Stage::Split, Stage::Organize] {
                set_stage(&mut updated, stage, StageStatus::Stale);
            }
            return Ok(updated);
        }
        let found = chapters::find_chapters(&audio)
            .map_err(|error| JobError::Operation(error.to_string()))?;
        if found.is_empty() {
            return Err(JobError::Operation(
                "No chapters were found for this audio.".into(),
            ));
        }
        let stem = audio
            .file_stem()
            .ok_or_else(|| JobError::Operation("Audio file has no name.".into()))?;
        let output = self.prepared.local.request.splits.join(stem);
        let task = muzik_workflow::SplitTask {
            source: audio,
            chapters: found,
            output: output.clone(),
        };
        let mut options = self.prepared.local.options.clone();
        options.force = true;
        options.keep_source = true;
        let mut local = local_workflow::LocalOperations {
            decide: self.decide,
            on_import_event: self.on_import_event,
            cancelled,
        };
        local
            .split_with_cancel(&task, &options, cancelled, &mut |_| {})
            .map_err(JobError::Operation)?;
        check_cancelled(cancelled)?;
        set_stage(&mut updated, Stage::Split, StageStatus::Complete);
        set_path(&mut updated, Stage::Split, json!(output));
        set_stage(&mut updated, Stage::Organize, StageStatus::Stale);
        Ok(updated)
    }

    fn process_spotify(
        &mut self,
        _playlist: &Value,
        item: &Value,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<Value, JobError> {
        if action == ItemAction::OrganizeAgain {
            let saved = downloaded_audio(item, &self.prepared.local.request.output)?;
            return match saved {
                Some(file) => self.process_spotify_file(item, file, cancelled),
                None => self.process_local_stage(item, action, cancelled),
            };
        }
        if action.stage() != Stage::Download {
            return Err(JobError::Operation(format!(
                "The Rust watchlist does not support {action} yet."
            )));
        }
        let fresh = matches!(action, ItemAction::DownloadAgain | ItemAction::RunAllAgain);
        let track = item["track"].as_object().ok_or_else(|| {
            JobError::Operation("The Spotify track has no saved metadata.".into())
        })?;
        let title = track
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| JobError::Operation("Spotify track title is missing".into()))?;
        let artists = track
            .get("artists")
            .and_then(Value::as_array)
            .map(|artists| {
                artists
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let query = format!("{artists} - {title}");
        let preference = self.prepared.local.options.prefer.as_str();
        if !fresh {
            let saved = stage_path(item, Stage::Download)
                .as_str()
                .map(PathBuf::from)
                .filter(|path| path.is_file());
            if let Some(file) = saved {
                return self.process_spotify_file(item, file, cancelled);
            }
        }
        let entry_id = required(item, "entry_id")?;
        let safe_id: String = entry_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        let root = self
            .prepared
            .local
            .request
            .output
            .join("spotify-watchlist")
            .join(safe_id);
        let options = &self.prepared.local.options;
        let (files, _) = remote_workflow::acquire_spotify_audio(
            options.audio_source,
            options.fallback,
            remote_workflow::soulseek_ready(),
            || {
                remote_workflow::soulseek_download(
                    &query,
                    preference,
                    false,
                    cancelled,
                    self.decide,
                    true,
                    Some(&root),
                )
            },
            || {
                remote_workflow::download(&query, &root, fresh, cancelled)
                    .map_err(|error| error.to_string())
            },
        )
        .map_err(|error| {
            if cancelled.load(Ordering::SeqCst) {
                JobError::Cancelled
            } else {
                JobError::Operation(error)
            }
        })?;
        let file = files
            .into_iter()
            .next()
            .ok_or_else(|| JobError::Operation("Soulseek returned no audio file.".into()))?;
        self.process_spotify_file(item, file, cancelled)
    }

    fn process_spotify_file(
        &mut self,
        item: &Value,
        file: PathBuf,
        cancelled: &AtomicBool,
    ) -> Result<Value, JobError> {
        write_spotify_tags(&file, &spotify_tags(&item["track"])).map_err(JobError::Operation)?;
        let mut options = self.prepared.local.options.clone();
        options.no_split = true;
        options.interactive = false;
        let mut local = local_workflow::LocalOperations {
            decide: self.decide,
            on_import_event: self.on_import_event,
            cancelled,
        };
        process_audio_plan_with_events(
            std::slice::from_ref(&file),
            &[],
            &self.prepared.local.request.splits,
            &options,
            &mut local,
            cancelled,
            &mut |event| {
                (self.events.borrow_mut())(local_workflow::event_record(event));
            },
        )
        .map_err(workflow_error)?;
        let mut updated = item.clone();
        mark_full(&mut updated, &options, false);
        if file.is_file() {
            set_path(&mut updated, Stage::Download, json!(file));
        }
        Ok(updated)
    }
}

fn spotify_tags(track: &Value) -> SpotifyTags {
    SpotifyTags {
        title: track["title"].as_str().unwrap_or("").to_owned(),
        artists: track["artists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        album: track["album"].as_str().map(str::to_owned),
        track: track["track_number"].as_u64(),
        disc: track["disc_number"].as_u64(),
        date: track["release_date"].as_str().map(str::to_owned),
    }
}

fn stage_path(item: &Value, stage: Stage) -> &Value {
    &item["stages"][stage.as_ref()]["path"]
}

fn set_path(item: &mut Value, stage: Stage, path: Value) {
    item["stages"][stage.as_ref()]["path"] = path;
}

fn ready_quality_directory(item: &Value) -> Option<PathBuf> {
    if stage_status(item, Stage::Split) != Some(StageStatus::Complete)
        || stage_path(item, Stage::Quality) != stage_path(item, Stage::Split)
    {
        return None;
    }
    stage_path(item, Stage::Split)
        .as_str()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
}

fn apply_quality_result(item: &mut Value, result: &QualityUpgradeResult) {
    set_stage(item, Stage::Quality, StageStatus::Complete);
    if let Some(directory) = result.pre_split_dirs.first() {
        set_path(item, Stage::Quality, json!(directory));
        set_path(item, Stage::Split, json!(directory));
        set_stage(item, Stage::Parse, StageStatus::Skipped);
        set_stage(item, Stage::Split, StageStatus::Complete);
        set_stage(item, Stage::Organize, StageStatus::Stale);
    } else if let Some(replacement) = result.audio_files.first() {
        if *stage_path(item, Stage::Download) != json!(replacement) {
            set_path(item, Stage::Download, json!(replacement));
            set_path(item, Stage::Quality, json!(replacement));
            for stage in [Stage::Parse, Stage::Split, Stage::Organize] {
                set_stage(item, stage, StageStatus::Stale);
            }
        }
    }
}

fn downloaded_audio(item: &Value, output: &Path) -> Result<Option<PathBuf>, JobError> {
    if let Some(path) = stage_path(item, Stage::Download)
        .as_str()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Ok(Some(path));
    }
    let Some(id) = item["video_id"].as_str().filter(|id| !id.is_empty()) else {
        return Ok(None);
    };
    if !output.is_dir() {
        return Ok(None);
    }
    let files =
        muzik_workflow::find_audio_inputs(&[output.to_path_buf()]).map_err(workflow_error)?;
    Ok(files.into_iter().find(|path| {
        path.file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(&format!("[{id}]")))
    }))
}

fn save_output_paths(item: &mut Value, result: &Value, output: &Path) -> Result<(), JobError> {
    let audio = result["audio_files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .find(|path| path.is_file());
    let audio = match audio {
        Some(path) => Some(path),
        None => downloaded_audio(item, output)?,
    };
    set_path(item, Stage::Download, json!(audio));
    let split = result["split_dirs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(PathBuf::from)
        .find(|path| path.is_dir());
    set_path(item, Stage::Split, json!(split));
    Ok(())
}

fn youtube_source(url: &str, cancelled: &AtomicBool) -> Result<Value, JobError> {
    check_cancelled(cancelled)?;
    let mut args = remote_workflow::yt_dlp_environment_args();
    args.extend([
        "--flat-playlist".to_owned(),
        "--dump-single-json".to_owned(),
        "--quiet".to_owned(),
        url.to_owned(),
    ]);
    let executor = Executor::new("yt-dlp", args, Duration::from_secs(600));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| JobError::Operation(error.to_string()))?;
    let output = runtime.block_on(async {
        let mut command = tokio::process::Command::new(executor.executable_path());
        command.args(executor.args()).kill_on_drop(true);
        tokio::select! {
            result = command.output() => result.map_err(|error| JobError::Operation(error.to_string())),
            () = async { while !cancelled.load(Ordering::SeqCst) { tokio::time::sleep(Duration::from_millis(100)).await; } } => Err(JobError::Cancelled),
            () = tokio::time::sleep(Duration::from_secs(600)) => Err(JobError::Operation("YouTube playlist lookup timed out.".into())),
        }
    })?;
    check_cancelled(cancelled)?;
    if !output.status.success() {
        return Err(JobError::Operation(format!(
            "YouTube playlist lookup failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| JobError::Operation(format!("Invalid YouTube playlist metadata: {error}")))
}

fn refresh_chapters(
    audio: &Path,
    video_url: &str,
    cancelled: &AtomicBool,
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<PathBuf, JobError> {
    refresh_chapters_with(audio, cancelled, decide, |comments| {
        youtube_video_metadata(video_url, comments, cancelled)
    })
}

fn refresh_chapters_with(
    audio: &Path,
    cancelled: &AtomicBool,
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    mut fetch: impl FnMut(bool) -> Result<Value, JobError>,
) -> Result<PathBuf, JobError> {
    let metadata = fetch(false)?;
    let info = serde_json::to_string_pretty(&metadata)
        .map_err(|error| JobError::Operation(error.to_string()))?;
    atomic_write(&chapters::sidecar_path(audio, ".info.json"), &(info + "\n"))?;
    check_cancelled(cancelled)?;
    let mut found = chapters::parse_info_json(&metadata.to_string())
        .map_err(|error| JobError::Operation(error.to_string()))?;
    if found.is_empty() {
        found = metadata["description"]
            .as_str()
            .map(chapters::parse_tracklist)
            .unwrap_or_default();
    }
    if found.is_empty() {
        let comments = fetch(true)?;
        found = chapters::best_comment_tracklist(&comments);
    }
    if found.is_empty() {
        return Err(JobError::Operation(
            "No YouTube chapters were found.".into(),
        ));
    }
    let records = found.iter().map(chapter_record).collect::<Vec<_>>();
    let answer = decide(
        DecisionKind::ChapterReview,
        json!({"source":audio,"chapters":records}),
    )
    .map_err(JobError::Operation)?;
    match answer.as_str().and_then(|answer| answer.parse().ok()) {
        Some(ChapterAnswer::Accept) => {}
        Some(ChapterAnswer::Edit) => {
            let answer = decide(DecisionKind::ChapterEdit, json!({"chapters":records}))
                .map_err(JobError::Operation)?;
            found = answer
                .as_array()
                .ok_or_else(|| JobError::Operation("Edited chapters must be a list.".into()))?
                .iter()
                .map(parse_chapter_record)
                .collect::<Result<Vec<_>, _>>()?;
        }
        Some(ChapterAnswer::Reject) => {
            return Err(JobError::Operation(
                "YouTube chapters were not accepted.".into(),
            ))
        }
        None => return Err(JobError::Operation("Select a chapter action.".into())),
    }
    if found.is_empty() {
        return Err(JobError::Operation(
            "YouTube chapters were not accepted.".into(),
        ));
    }
    check_cancelled(cancelled)?;
    let text = found
        .iter()
        .map(|chapter| format!("{} {}", format_time(chapter.start), chapter.title))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let path = chapters::sidecar_path(audio, ".chapters.txt");
    atomic_write(&path, &text)?;
    Ok(path)
}

fn youtube_video_metadata(
    url: &str,
    comments: bool,
    cancelled: &AtomicBool,
) -> Result<Value, JobError> {
    let mut args = remote_workflow::yt_dlp_environment_args();
    args.extend([
        "--no-playlist".to_owned(),
        "--dump-single-json".to_owned(),
        "--skip-download".to_owned(),
        "--quiet".to_owned(),
    ]);
    if comments {
        args.extend([
            "--write-comments".to_owned(),
            "--extractor-args".to_owned(),
            "youtube:max_comments=50,all,0,0;comment_sort=top".to_owned(),
        ]);
    }
    args.push(url.to_owned());
    let timeout = if comments {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(600)
    };
    let executor = Executor::new("yt-dlp", args, timeout);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| JobError::Operation(error.to_string()))?;
    let output = runtime.block_on(async {
        let mut command = tokio::process::Command::new(executor.executable_path());
        command.args(executor.args()).kill_on_drop(true);
        tokio::select! {
            result = command.output() => result.map_err(|error| JobError::Operation(error.to_string())),
            () = async { while !cancelled.load(Ordering::SeqCst) { tokio::time::sleep(Duration::from_millis(100)).await; } } => Err(JobError::Cancelled),
            () = tokio::time::sleep(timeout) => Err(JobError::Operation("YouTube metadata lookup timed out.".into())),
        }
    })?;
    check_cancelled(cancelled)?;
    if !output.status.success() {
        return Err(JobError::Operation(format!(
            "YouTube metadata lookup failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| JobError::Operation(format!("Invalid YouTube metadata: {error}")))
}

fn atomic_write(path: &Path, text: &str) -> Result<(), JobError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| JobError::Operation(error.to_string()))?;
    use std::io::Write;
    file.write_all(text.as_bytes())
        .map_err(|error| JobError::Operation(error.to_string()))?;
    file.persist(path)
        .map_err(|error| JobError::Operation(error.to_string()))?;
    Ok(())
}

fn chapter_record(chapter: &chapters::Chapter) -> Value {
    json!({"index":chapter.index,"start":chapter.start,"end":chapter.end,"title":chapter.title})
}

fn parse_chapter_record(value: &Value) -> Result<chapters::Chapter, JobError> {
    let index = value["index"]
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| JobError::Operation("Edited chapter index is invalid.".into()))?;
    let start = value["start"]
        .as_i64()
        .ok_or_else(|| JobError::Operation("Edited chapter start is invalid.".into()))?;
    let end = value["end"].as_i64();
    let title = value["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())
        .ok_or_else(|| JobError::Operation("Edited chapter title is missing.".into()))?;
    Ok(chapters::Chapter {
        index,
        start,
        end,
        title: title.to_owned(),
    })
}

fn format_time(seconds: i64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn youtube_items(playlist: &Value, source: &Value) -> LoadedSource {
    let old: HashMap<&str, &Value> = playlist["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| Some((item["video_id"].as_str()?, item)))
        .collect();
    let items = source["entries"].as_array().into_iter().flatten().enumerate().filter_map(|(index, entry)| {
        let id = entry["id"].as_str().or_else(|| entry["url"].as_str())?;
        if id.len() != 11 || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) { return None; }
        let saved = old.get(id).copied();
        let listed_title = entry["title"].as_str().filter(|title| !title.is_empty() && !(title.starts_with('[') && title.ends_with(" video]")));
        let unavailable = listed_title.is_none() && entry["duration"].is_null();
        let title = listed_title.or_else(|| saved.and_then(|item| item["title"].as_str())).unwrap_or(id);
        let thumbnail = entry["thumbnail"].as_str().or_else(|| entry["thumbnails"].as_array().and_then(|images| images.last()).and_then(|image| image["url"].as_str())).map(str::to_owned).or_else(|| saved.and_then(|item| item["thumbnail_url"].as_str()).map(str::to_owned));
        Some(json!({"position":index + 1,"title":title,"video_id":id,"video_url":format!("https://www.youtube.com/watch?v={id}"),"thumbnail_url":thumbnail,"kind":SourceKind::Youtube,"unavailable":unavailable}))
    }).collect();
    LoadedSource {
        title: source["title"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| playlist["title"].as_str().map(str::to_owned)),
        items,
    }
}

fn spotify_items(document: &Value) -> Result<LoadedSource, JobError> {
    let entries = document["entries"]
        .as_array()
        .ok_or_else(|| JobError::Operation("Spotify source has no entries.".into()))?;
    let mut occurrences = HashMap::<String, usize>::new();
    let mut items = Vec::new();
    for (index, track) in entries.iter().enumerate() {
        let source = track["source_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("spotify:{}", index + 1));
        let occurrence = occurrences.entry(source.clone()).or_default();
        let entry_id = format!("{source}#{occurrence}");
        *occurrence += 1;
        let artists = track["artists"]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let title = track["title"].as_str().unwrap_or("Unknown track");
        let label = if artists.is_empty() {
            title.to_owned()
        } else {
            format!("{artists} - {title}")
        };
        let video_id = source.rsplit(':').next().unwrap_or("");
        let image = track["source_metadata"]["image"].clone();
        items.push(json!({"position":index + 1,"title":label,"video_id":video_id,"video_url":track["source_url"],"thumbnail_url":image,"kind":SourceKind::Spotify,"entry_id":entry_id,"track":track}));
    }
    Ok(LoadedSource {
        title: document["title"].as_str().map(str::to_owned),
        items,
    })
}

fn mark_full(item: &mut Value, options: &WorkflowOptions, split: bool) {
    let spotify = SourceKind::of(item) == SourceKind::Spotify;
    for stage in Stage::ALL.iter().copied() {
        let skipped = match stage {
            Stage::Download => false,
            Stage::Quality => {
                spotify
                    || options.quality_policy == QualityPolicy::Off
                    || options.audio_source == AudioSource::Soulseek
            }
            Stage::Parse | Stage::Split => spotify || !split,
            Stage::Organize => options.no_organize,
        };
        let status = if skipped {
            StageStatus::Skipped
        } else {
            StageStatus::Complete
        };
        set_stage(item, stage, status);
    }
}

fn set_stage(item: &mut Value, stage: Stage, status: StageStatus) {
    set_stage_status(item, stage, status);
}

fn workflow_error(error: muzik_workflow::Error) -> JobError {
    match error {
        muzik_workflow::Error::Cancelled => JobError::Cancelled,
        other => JobError::Operation(other.to_string()),
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), JobError> {
    if cancelled.load(Ordering::SeqCst) {
        Err(JobError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{refresh_chapters_with, spotify_items, youtube_items};
    use muzik_core::watchlist::ItemAction;
    use muzik_core::{ChapterAnswer, DecisionKind, QualityPolicy};
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn saved_output_paths_resolve_new_and_existing_audio() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let audio = directory.path().join("Song [abcdefghijk].flac");
        let split = directory.path().join("split");
        fs::write(&audio, [])?;
        fs::create_dir(&split)?;
        let mut item = json!({"video_id":"abcdefghijk","stages":{"download":{},"split":{}}});
        super::save_output_paths(&mut item, &json!({"split_dirs":[split]}), directory.path())?;
        assert_eq!(item["stages"]["download"]["path"], json!(audio));
        assert_eq!(item["stages"]["split"]["path"], json!(split));
        let replacement = directory.path().join("replacement.flac");
        fs::write(&replacement, [])?;
        super::save_output_paths(
            &mut item,
            &json!({"audio_files":[replacement],"split_dirs":[]}),
            directory.path(),
        )?;
        assert_eq!(
            super::downloaded_audio(&item, directory.path())?,
            Some(replacement)
        );
        Ok(())
    }

    #[test]
    fn spotify_organize_again_imports_saved_audio() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let audio = directory.path().join("track.flac");
        fs::copy(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/muzik-tags/tests/fixtures/blank.flac"),
            &audio,
        )?;
        let config = directory.path().join("config.yaml");
        let library = directory.path().join("library.db");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                directory.path().join("music").display(),
                library.display(),
                directory.path().join("state").display()
            ),
        )?;
        let params = json!({"output":directory.path(),"config":config,"interactive":true});
        let prepared = super::Prepared {
            local: crate::local_workflow::parse("", &params)?,
            params,
            cache: directory.path().to_path_buf(),
            quality_policy: QualityPolicy::Off,
            repository: muzik_core::watchlist::Repository::new(
                directory.path().join("watchlist.json"),
            ),
        };
        let mut event = |_| {};
        let events = std::cell::RefCell::new(&mut event as &mut dyn FnMut(serde_json::Value));
        let mut imported = |_| {};
        let mut decide = |_: DecisionKind, _: serde_json::Value| Err("unexpected decision".into());
        let cancelled = AtomicBool::new(false);
        let parked = std::cell::RefCell::new(None);
        let mut adapter = super::Adapter {
            prepared: &prepared,
            events: &events,
            on_import_event: &mut imported,
            decide: &mut decide,
            parked: &parked,
            cancelled: &cancelled,
        };
        let result = adapter.process_spotify(
            &json!({}),
            &json!({"kind":"spotify","stages":{"download":{"path":audio},"organize":{}},"track":{
                "title":"Love's a Stranger","artists":["Warhaus"],"album":"Warhaus",
                "track_number":2,"disc_number":1,"release_date":"2017-10-13"
            }}),
            ItemAction::OrganizeAgain,
            &cancelled,
        )?;
        assert_eq!(result["stages"]["organize"]["status"], "complete");
        let imported = muzik_library::Library::open_read_only(&library)?.items()?;
        assert_eq!(imported.len(), 1);
        let text = |name: &str| match imported[0].field(name) {
            Some(muzik_library::SqlValue::Text(text)) => text.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(text("album"), "Warhaus");
        assert_eq!(text("title"), "Love's a Stranger");
        assert_eq!(text("albumartist"), "Warhaus");
        Ok(())
    }

    #[test]
    fn multi_file_quality_replacement_imports_as_a_ready_album(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let original = directory.path().join("Album [abcdefghijk].flac");
        let album = directory.path().join("replacement");
        fs::create_dir(&album)?;
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/muzik-tags/tests/fixtures/blank.flac");
        for path in [&original, &album.join("one.flac"), &album.join("two.flac")] {
            fs::copy(&fixture, path)?;
        }
        let mut item = json!({"position":1,"video_id":"abcdefghijk","video_url":"https://www.youtube.com/watch?v=abcdefghijk","title":"Album","kind":"youtube","stages":{
            "download":{"status":"complete","path":original},"quality":{},"parse":{},"split":{},"organize":{}
        }});
        super::apply_quality_result(
            &mut item,
            &super::QualityUpgradeResult {
                audio_files: Vec::new(),
                pre_split_dirs: vec![album.clone()],
            },
        );
        assert_eq!(
            super::downloaded_audio(&item, directory.path())?,
            Some(original.clone())
        );
        assert_eq!(super::ready_quality_directory(&item), Some(album.clone()));
        assert_eq!(item["stages"]["parse"]["status"], "skipped");
        assert_eq!(item["stages"]["split"]["status"], "complete");
        assert_eq!(item["stages"]["organize"]["status"], "stale");
        let config = directory.path().join("config.yaml");
        let library = directory.path().join("library.db");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                directory.path().join("music").display(),
                library.display(),
                directory.path().join("state").display()
            ),
        )?;
        let params = json!({"output":directory.path(),"config":config,"interactive":false});
        let prepared = super::Prepared {
            local: crate::local_workflow::parse("", &params)?,
            params,
            cache: directory.path().to_path_buf(),
            quality_policy: QualityPolicy::Auto,
            repository: muzik_core::watchlist::Repository::new(
                directory.path().join("watchlist.json"),
            ),
        };
        let mut event = |_| {};
        let events = std::cell::RefCell::new(&mut event as &mut dyn FnMut(serde_json::Value));
        let mut imported = |_| {};
        let mut decide = |_: DecisionKind, _: serde_json::Value| Err("unexpected decision".into());
        let cancelled = AtomicBool::new(false);
        let parked = std::cell::RefCell::new(None);
        let mut adapter = super::Adapter {
            prepared: &prepared,
            events: &events,
            on_import_event: &mut imported,
            decide: &mut decide,
            parked: &parked,
            cancelled: &cancelled,
        };
        let result = adapter.process_youtube(&item, ItemAction::Retry, &cancelled)?;
        assert_eq!(result["stages"]["organize"]["status"], "complete");
        assert_eq!(
            muzik_library::Library::open_read_only(&library)?
                .items()?
                .len(),
            2
        );
        assert!(original.is_file());
        Ok(())
    }

    #[test]
    fn spotify_repeated_tracks_keep_separate_state_keys() -> Result<(), Box<dyn std::error::Error>>
    {
        let loaded = spotify_items(&json!({"title":"Album","entries":[
            {"title":"One","artists":["Alex"],"source_id":"spotify:track:t1","source_url":"https://open.spotify.com/track/t1"},
            {"title":"One","artists":["Alex"],"source_id":"spotify:track:t1","source_url":"https://open.spotify.com/track/t1"}
        ]}))?;
        assert_eq!(loaded.items[0]["entry_id"], "spotify:track:t1#0");
        assert_eq!(loaded.items[1]["entry_id"], "spotify:track:t1#1");
        assert_eq!(loaded.items[0]["title"], "Alex - One");
        Ok(())
    }

    #[test]
    fn youtube_reload_preserves_saved_card_metadata() {
        let playlist = json!({"title":"My list", "items":[{"video_id":"abcdefghijk", "title":"Saved title", "thumbnail_url":"https://example.test/image.jpg"}]});
        let loaded = youtube_items(
            &playlist,
            &json!({"title":"Current list","entries":[{"id":"abcdefghijk"}]}),
        );
        assert_eq!(loaded.items[0]["title"], "Saved title");
        assert_eq!(
            loaded.items[0]["thumbnail_url"],
            "https://example.test/image.jpg"
        );
    }

    #[test]
    fn a_private_video_without_title_and_duration_is_unavailable() {
        let loaded = youtube_items(
            &json!({"items":[]}),
            &json!({"entries":[
                {"id":"abcdefghijk","title":null,"duration":null},
                {"id":"bcdefghijkl","title":"[Private video]","duration":null},
                {"id":"cdefghijklm","title":"Song","duration":245.0}
            ]}),
        );
        let flags: Vec<_> = loaded
            .items
            .iter()
            .map(|item| item["unavailable"].clone())
            .collect();
        assert_eq!(flags, [json!(true), json!(true), json!(false)]);
        assert_eq!(loaded.items[1]["title"], "bcdefghijkl");
    }

    #[test]
    fn youtube_new_card_uses_source_title_and_thumbnail() {
        let loaded = youtube_items(
            &json!({"items":[]}),
            &json!({"title":"Playlist", "entries":[{"id":"abcdefghijk", "title":"Song", "thumbnail":"https://example.test/new.jpg"}]}),
        );
        assert_eq!(loaded.title.as_deref(), Some("Playlist"));
        assert_eq!(loaded.items[0]["title"], "Song");
        assert_eq!(
            loaded.items[0]["thumbnail_url"],
            "https://example.test/new.jpg"
        );
    }

    #[test]
    fn description_tracklist_orders_times_and_keeps_titles() {
        let chapters = muzik_core::chapters::parse_tracklist("Track list:\n2. Song Two (04:10 - 08:00)\n[0:00] First Song\n4:10 Duplicate\n8:00 - Final Song\n");
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].title, "First Song");
        assert_eq!(chapters[0].end, Some(250));
        assert_eq!(chapters[1].title, "Song Two");
        assert_eq!(chapters[2].start, 480);
    }

    #[test]
    fn comment_tracklist_prefers_pinned_then_uploader() {
        let metadata = json!({"comments":[
            {"text":"0:00 Other\n2:00 End"},
            {"text":"0:00 Uploader\n2:00 End", "author_is_uploader":true},
            {"text":"0:00 Pinned\n2:00 End", "is_pinned":true}
        ]});
        assert_eq!(
            muzik_core::chapters::best_comment_tracklist(&metadata)[0].title,
            "Pinned"
        );
    }

    #[test]
    fn refreshed_chapters_replace_sidecar_after_review() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let audio = directory.path().join("Album.flac");
        fs::write(&audio, [])?;
        fs::write(directory.path().join("Album.chapters.txt"), "00:00 Old\n")?;
        let mut asked = false;
        let path = refresh_chapters_with(
            &audio,
            &AtomicBool::new(false),
            &mut |kind, value| {
                assert_eq!(kind, DecisionKind::ChapterReview);
                assert_eq!(value["chapters"][1]["title"], "Second");
                asked = true;
                Ok(json!(ChapterAnswer::Accept))
            },
            |_| {
                Ok(json!({"chapters":[
                    {"start_time":0,"title":"First"},
                    {"start_time":125,"title":"Second"}
                ]}))
            },
        )?;
        assert!(asked);
        assert_eq!(fs::read_to_string(path)?, "00:00 First\n02:05 Second\n");
        assert!(fs::read_to_string(directory.path().join("Album.info.json"))?.contains("Second"));
        Ok(())
    }
}
