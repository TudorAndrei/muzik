//! Rust source and audio operations for saved watchlist jobs.

use crate::gates::{self, Gate};
use crate::settings::Settings;
use crate::{local_workflow, remote_workflow};
use muzik_core::watchlist::jobs::{
    self, JobError, JobOptions, LoadedSource, Operations, PendingItem,
};
use muzik_core::watchlist::{
    AudioIndex, ItemAction, ItemId, Playlist, SourceKind, Stage, StageStatus, WatchItem,
};
use muzik_core::{
    bandcamp, chapters, spotify, watchlist, AudioSource, ChapterAnswer, DecisionKind, QualityPolicy,
};
use muzik_workflow::playlist::{write_spotify_tags, SpotifyTags};
use muzik_workflow::quality::{check_youtube_quality, QualityUpgradeResult};
use muzik_workflow::ytdlp::{is_video_id, YtDlp};
use muzik_workflow::{
    classify_input, process_audio_plan_with_events, WorkflowInput, WorkflowOperations,
    WorkflowOptions,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub fn sync(
    settings: &Settings,
    playlist_id: Option<&str>,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
) -> Result<Vec<PendingItem>, JobError> {
    let prepared = Prepared::new(settings);
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
    let mut options = prepared.job_options();
    options.playlist_id = playlist_id.filter(|id| !id.is_empty());
    let synced = jobs::sync(
        &prepared.repository,
        options,
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
    settings: &Settings,
    params: &Value,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
    on_import_event: &mut dyn FnMut(Value),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    parked: &RefCell<Option<Parked>>,
) -> Result<Value, JobError> {
    let prepared = Prepared::new(settings);
    let id = ItemId::from_params(params)?;
    let name = required(params["action"].as_str(), "action")?;
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
        &id,
        name,
        &mut adapter,
        cancelled,
    )
}

fn required<'a>(value: Option<&'a str>, key: &str) -> Result<&'a str, JobError> {
    value
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| JobError::Operation(format!("{key} must be a non-empty string")))
}

struct Prepared<'a> {
    settings: &'a Settings,
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

impl<'a> Prepared<'a> {
    fn new(settings: &'a Settings) -> Self {
        Self {
            settings,
            repository: watchlist::Repository::open(&settings.paths),
        }
    }

    fn job_options(&self) -> JobOptions<'_> {
        JobOptions {
            reconcile: self.settings.reconcile(),
            output: &self.settings.request.output,
            cache: &self.settings.paths.cache,
            dry_run: self.settings.options.dry_run,
            playlist_id: None,
        }
    }

    fn audio(&self, item: &WatchItem) -> Option<PathBuf> {
        item.downloaded_audio(&AudioIndex::scan(&self.settings.request.output))
    }
}

struct Adapter<'a, 'b> {
    prepared: &'a Prepared<'a>,
    events: &'a RefCell<&'b mut dyn FnMut(Value)>,
    on_import_event: &'a mut dyn FnMut(Value),
    decide: &'a mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    parked: &'a RefCell<Option<Parked>>,
    cancelled: &'a AtomicBool,
}

impl Operations for Adapter<'_, '_> {
    fn load(&mut self, playlist: &Playlist) -> Result<LoadedSource, JobError> {
        check_cancelled(self.cancelled)?;
        match playlist.kind {
            SourceKind::Bandcamp => {
                let login = bandcamp::Login::load()
                    .ok_or_else(|| JobError::Operation(BANDCAMP_LOGIN.into()))?;
                let purchases = bandcamp::collection(&login)?;
                check_cancelled(self.cancelled)?;
                Ok(bandcamp_items(&purchases))
            }
            SourceKind::Spotify => {
                let document = spotify::load_playlist_document(
                    &self.prepared.settings.paths.config_file(),
                    &spotify::token_path(),
                    &playlist.playlist_id,
                )?;
                check_cancelled(self.cancelled)?;
                spotify_items(&document)
            }
            SourceKind::Youtube => {
                let source = YtDlp::default()
                    .playlist(&playlist.url, self.cancelled)
                    .map_err(workflow_error)?;
                Ok(youtube_items(playlist, &source))
            }
        }
    }

    fn process(
        &mut self,
        playlist: &Playlist,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        check_cancelled(cancelled)?;
        self.parked.replace(None);
        gates::take_stage();
        let result = match item.kind {
            SourceKind::Spotify => self.process_spotify(item, action, cancelled),
            SourceKind::Youtube => self.process_youtube(item, action, cancelled),
            SourceKind::Bandcamp => self.process_bandcamp(item, action, cancelled),
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
                "playlist_id":playlist.playlist_id,
                "position":item.position,
                "video_id":item.video_id.as_deref().or(item.entry_id.as_deref()),
                "title":item.title,
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
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        if action.stage() != Stage::Download {
            return self.process_local_stage(item, action, cancelled);
        }
        if matches!(action, ItemAction::Run | ItemAction::Retry)
            && ready_quality_directory(item).is_some()
        {
            if !self.prepared.settings.options.no_organize {
                return self.process_local_stage(item, ItemAction::OrganizeAgain, cancelled);
            }
            let mut updated = item.clone();
            updated.set(Stage::Organize, StageStatus::Skipped);
            return Ok(updated);
        }
        let url = required(item.video_url.as_deref(), "video_url")?;
        let mut settings = self.prepared.settings.clone();
        url.clone_into(&mut settings.request.raw);
        match action {
            ItemAction::DownloadAgain => {
                settings.options.force = true;
                settings.options.no_split = true;
                settings.options.no_organize = true;
            }
            ItemAction::RunAllAgain => settings.options.force = true,
            _ => {}
        }
        let input = classify_input(url);
        if matches!(input, WorkflowInput::Local(_)) {
            return Err(JobError::Operation(
                "The saved YouTube item is not a video URL.".into(),
            ));
        }
        let result = remote_workflow::run(
            input,
            &settings,
            cancelled,
            &mut |event| (self.events.borrow_mut())(event),
            self.on_import_event,
            self.decide,
        )
        .map_err(workflow_error)?;
        let mut updated = item.clone();
        self.save_output_paths(&mut updated, &result);
        if action == ItemAction::DownloadAgain {
            updated.set(Stage::Download, StageStatus::Complete);
            updated.invalidate(&[Stage::Parse, Stage::Split, Stage::Organize]);
        } else {
            let split = result["split_dirs"]
                .as_array()
                .is_some_and(|dirs| !dirs.is_empty());
            mark_full(&mut updated, &self.prepared.settings.options, split);
        }
        Ok(updated)
    }

    fn save_output_paths(&self, item: &mut WatchItem, result: &Value) {
        let paths = |key: &str| {
            result[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        };
        let audio = paths("audio_files")
            .into_iter()
            .find(|path| path.is_file())
            .or_else(|| self.prepared.audio(item));
        item.set_path(Stage::Download, audio);
        let split = paths("split_dirs").into_iter().find(|path| path.is_dir());
        item.set_path(Stage::Split, split);
    }

    fn process_local_stage(
        &mut self,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        let audio = self.prepared.audio(item);
        let mut updated = item.clone();
        if action == ItemAction::OrganizeAgain {
            let target = item
                .path(Stage::Split)
                .filter(|path| path.is_dir())
                .map(Path::to_path_buf)
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
            let mut options = self.prepared.settings.options.clone();
            options.force = true;
            options.no_organize = false;
            local.organize(&target, &options)?;
            check_cancelled(cancelled)?;
            updated.set(Stage::Organize, StageStatus::Complete);
            return Ok(updated);
        }
        let audio = audio
            .ok_or_else(|| JobError::Operation("Downloaded audio is not available.".into()))?;
        if action == ItemAction::CheckQualityAgain {
            updated.set_path(Stage::Download, Some(audio.clone()));
            let _permit = gates::enter(Gate::Process, Stage::Quality, cancelled)
                .map_err(|_| JobError::Cancelled)?;
            let result = check_youtube_quality(
                vec![audio],
                self.prepared.settings.options.quality_policy,
                self.prepared.settings.options.min_bitrate,
                &self.prepared.settings.options.prefer,
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
            let video_url = required(item.video_url.as_deref(), "video_url")?;
            let chapter_path = refresh_chapters(&audio, video_url, cancelled, self.decide)?;
            updated.complete(Stage::Parse, Some(chapter_path));
            updated.invalidate(&[Stage::Split, Stage::Organize]);
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
        let output = self.prepared.settings.request.splits.join(stem);
        let task = muzik_workflow::SplitTask {
            source: audio,
            chapters: found,
            output: output.clone(),
        };
        let mut options = self.prepared.settings.options.clone();
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
        updated.complete(Stage::Split, Some(output));
        updated.invalidate(&[Stage::Organize]);
        Ok(updated)
    }

    fn process_spotify(
        &mut self,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        if action == ItemAction::OrganizeAgain {
            return match self.prepared.audio(item) {
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
        let track = item
            .track
            .as_ref()
            .and_then(Value::as_object)
            .ok_or_else(|| {
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
        let preference = self.prepared.settings.options.prefer.as_str();
        if !fresh {
            if let Some(file) = item.path(Stage::Download).filter(|path| path.is_file()) {
                return self.process_spotify_file(item, file.to_path_buf(), cancelled);
            }
        }
        let entry_id = required(item.entry_id.as_deref(), "entry_id")?;
        let root = self
            .prepared
            .settings
            .request
            .output
            .join("spotify-watchlist")
            .join(safe_name(entry_id));
        let files = remote_workflow::soulseek_download(
            &self.prepared.settings.paths,
            &query,
            preference,
            false,
            cancelled,
            self.decide,
            true,
            Some(&root),
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

    fn process_bandcamp(
        &mut self,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        let entry_id = required(item.entry_id.as_deref(), "entry_id")?;
        let directory = self
            .prepared
            .settings
            .request
            .output
            .join("bandcamp-watchlist")
            .join(safe_name(entry_id));
        let fresh = matches!(action, ItemAction::DownloadAgain | ItemAction::RunAllAgain);
        let saved = !fresh && !bandcamp::audio_files(&directory).is_empty();
        if action == ItemAction::OrganizeAgain && !saved {
            return Err(JobError::Operation(
                "Download this purchase before you organize it again.".into(),
            ));
        }
        if action != ItemAction::OrganizeAgain && action.stage() != Stage::Download {
            return Err(JobError::Operation(format!(
                "A Bandcamp purchase does not support {action}."
            )));
        }
        if !saved {
            let page = item
                .track
                .as_ref()
                .and_then(|track| track["download_page"].as_str())
                .ok_or_else(|| {
                    JobError::Operation(
                        "The Bandcamp purchase has no download page. Refresh the collection."
                            .into(),
                    )
                })?;
            let login = bandcamp::Login::load()
                .ok_or_else(|| JobError::Operation(BANDCAMP_LOGIN.into()))?;
            if directory.exists() {
                std::fs::remove_dir_all(&directory).map_err(|error| error.to_string())?;
            }
            let _permit = gates::enter(Gate::Download, Stage::Download, cancelled)
                .map_err(|_| JobError::Cancelled)?;
            let on_import_event = &mut *self.on_import_event;
            let mut reported = None;
            bandcamp::download(
                &login,
                page,
                bandcamp::DEFAULT_FORMAT,
                &directory,
                cancelled,
                &mut |received, total| {
                    let completed = received / MEGABYTE;
                    let total = total.map(|total| total.div_ceil(MEGABYTE));
                    if reported.is_none() {
                        on_import_event(json!({"event":"progress_started","data":{"task_id":"bandcamp-download","description":"Downloading from Bandcamp (MB)","total":total}}));
                    } else if reported == Some(completed) {
                        return;
                    }
                    reported = Some(completed);
                    on_import_event(json!({"event":"progress_advanced","data":{"task_id":"bandcamp-download","completed":completed}}));
                },
            )
            .map_err(|error| {
                if cancelled.load(Ordering::SeqCst) {
                    JobError::Cancelled
                } else {
                    JobError::Failed {
                        stage: Stage::Download,
                        message: error,
                    }
                }
            })?;
            (self.on_import_event)(
                json!({"event":"progress_finished","data":{"task_id":"bandcamp-download","success":true}}),
            );
        }
        let mut options = self.prepared.settings.options.clone();
        options.no_split = true;
        options.interactive = false;
        if action == ItemAction::OrganizeAgain {
            options.no_organize = false;
            options.force = true;
        }
        if !options.no_organize {
            let mut local = local_workflow::LocalOperations {
                decide: self.decide,
                on_import_event: self.on_import_event,
                cancelled,
            };
            local.organize(&directory, &options)?;
            check_cancelled(cancelled)?;
        }
        let mut updated = item.clone();
        mark_full(&mut updated, &options, false);
        updated.set_path(Stage::Download, Some(directory));
        Ok(updated)
    }

    fn process_spotify_file(
        &mut self,
        item: &WatchItem,
        file: PathBuf,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        let track = item.track.clone().unwrap_or(Value::Null);
        write_spotify_tags(&file, &spotify_tags(&track)).map_err(JobError::Operation)?;
        let mut options = self.prepared.settings.options.clone();
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
            &self.prepared.settings.request.splits,
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
            updated.set_path(Stage::Download, Some(file));
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

fn ready_quality_directory(item: &WatchItem) -> Option<PathBuf> {
    if item.status(Stage::Split) != StageStatus::Complete
        || item.path(Stage::Quality) != item.path(Stage::Split)
    {
        return None;
    }
    item.path(Stage::Split)
        .filter(|path| path.is_dir())
        .map(Path::to_path_buf)
}

fn apply_quality_result(item: &mut WatchItem, result: &QualityUpgradeResult) {
    item.set(Stage::Quality, StageStatus::Complete);
    if let Some(directory) = result.pre_split_dirs.first() {
        item.set_path(Stage::Quality, Some(directory.clone()));
        item.set_path(Stage::Split, Some(directory.clone()));
        item.set(Stage::Parse, StageStatus::Skipped);
        item.set(Stage::Split, StageStatus::Complete);
        item.invalidate(&[Stage::Organize]);
    } else if let Some(replacement) = result.audio_files.first() {
        if item.path(Stage::Download) != Some(replacement.as_path()) {
            item.set_path(Stage::Download, Some(replacement.clone()));
            item.set_path(Stage::Quality, Some(replacement.clone()));
            item.invalidate(&[Stage::Parse, Stage::Split, Stage::Organize]);
        }
    }
}

fn refresh_chapters(
    audio: &Path,
    video_url: &str,
    cancelled: &AtomicBool,
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<PathBuf, JobError> {
    refresh_chapters_with(audio, cancelled, decide, |comments| {
        YtDlp::default()
            .video(video_url, comments, cancelled)
            .map_err(workflow_error)
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

fn youtube_items(playlist: &Playlist, source: &Value) -> LoadedSource {
    let old: HashMap<&str, &WatchItem> = playlist
        .items
        .iter()
        .filter_map(|item| Some((item.video_id.as_deref()?, item)))
        .collect();
    let items = source["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, entry)| {
            let id = entry["id"].as_str().or_else(|| entry["url"].as_str())?;
            if !is_video_id(id) {
                return None;
            }
            let saved = old.get(id).copied();
            let listed_title = entry["title"].as_str().filter(|title| {
                !title.is_empty() && !(title.starts_with('[') && title.ends_with(" video]"))
            });
            let unavailable = listed_title.is_none() && entry["duration"].is_null();
            let title = listed_title
                .or_else(|| saved.map(|item| item.title.as_str()))
                .unwrap_or(id);
            let thumbnail = entry["thumbnail"]
                .as_str()
                .or_else(|| {
                    entry["thumbnails"]
                        .as_array()
                        .and_then(|images| images.last())
                        .and_then(|image| image["url"].as_str())
                })
                .map(str::to_owned)
                .or_else(|| saved.and_then(|item| item.thumbnail_url.clone()));
            let mut item = WatchItem::new(index as u64 + 1, title, SourceKind::Youtube);
            item.video_id = Some(id.to_owned());
            item.video_url = Some(format!("https://www.youtube.com/watch?v={id}"));
            item.thumbnail_url = thumbnail;
            item.unavailable = Some(unavailable);
            Some(item)
        })
        .collect();
    LoadedSource {
        title: source["title"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| playlist.title.clone()),
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
        let mut item = WatchItem::new(index as u64 + 1, &label, SourceKind::Spotify);
        item.video_id = Some(source.rsplit(':').next().unwrap_or("").to_owned());
        item.video_url = track["source_url"].as_str().map(str::to_owned);
        item.thumbnail_url = track["source_metadata"]["image"]
            .as_str()
            .map(str::to_owned);
        item.entry_id = Some(entry_id);
        item.track = Some(track.clone());
        items.push(item);
    }
    Ok(LoadedSource {
        title: document["title"].as_str().map(str::to_owned),
        items,
    })
}

const BANDCAMP_LOGIN: &str = "Set your Bandcamp login in Settings first.";
const MEGABYTE: u64 = 1024 * 1024;

fn safe_name(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn bandcamp_items(purchases: &[bandcamp::Purchase]) -> LoadedSource {
    let items = purchases
        .iter()
        .enumerate()
        .map(|(index, purchase)| {
            let mut item =
                WatchItem::new(index as u64 + 1, &purchase.label(), SourceKind::Bandcamp);
            item.video_id = Some(purchase.key.clone());
            item.entry_id = Some(purchase.key.clone());
            item.video_url = Some(
                purchase
                    .item_url
                    .clone()
                    .unwrap_or_else(|| purchase.download_page.clone()),
            );
            item.thumbnail_url = purchase.art_url.clone();
            item.track = Some(json!({
                "artist": purchase.artist,
                "title": purchase.title,
                "single": purchase.single,
                "download_page": purchase.download_page,
            }));
            item
        })
        .collect();
    LoadedSource {
        title: Some("Bandcamp collection".into()),
        items,
    }
}

fn mark_full(item: &mut WatchItem, options: &WorkflowOptions, split: bool) {
    let single_file = !item.kind.is_youtube();
    for stage in Stage::ALL.iter().copied() {
        let skipped = match stage {
            Stage::Download => false,
            Stage::Quality => {
                single_file
                    || options.quality_policy == QualityPolicy::Off
                    || options.audio_source == AudioSource::Soulseek
            }
            Stage::Parse | Stage::Split => single_file || !split,
            Stage::Organize => options.no_organize,
        };
        item.set(
            stage,
            if skipped {
                StageStatus::Skipped
            } else {
                StageStatus::Complete
            },
        );
    }
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
    use super::{bandcamp_items, refresh_chapters_with, spotify_items, youtube_items};
    use crate::settings::Settings;
    use muzik_core::paths::Paths;
    use muzik_core::watchlist::{ItemAction, Playlist, SourceKind, Stage, StageStatus, WatchItem};
    use muzik_core::{ChapterAnswer, DecisionKind};
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    fn library_config(directory: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let config = directory.join("config.yaml");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                directory.join("music").display(),
                directory.join("library.db").display(),
                directory.join("state").display()
            ),
        )?;
        Ok(config)
    }

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/muzik-tags/tests/fixtures/blank.flac")
    }

    #[test]
    fn saved_output_paths_resolve_new_and_existing_audio() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let audio = directory.path().join("Song [abcdefghijk].flac");
        let split = directory.path().join("split");
        fs::write(&audio, [])?;
        fs::create_dir(&split)?;
        let settings = Settings::parse(
            &Paths::under(directory.path()),
            &json!({"output":directory.path()}),
        )?;
        let prepared = super::Prepared::new(&settings);
        let mut event = |_| {};
        let events = std::cell::RefCell::new(&mut event as &mut dyn FnMut(serde_json::Value));
        let mut imported = |_| {};
        let mut decide = |_: DecisionKind, _: serde_json::Value| Err("unexpected decision".into());
        let cancelled = AtomicBool::new(false);
        let parked = std::cell::RefCell::new(None);
        let adapter = super::Adapter {
            prepared: &prepared,
            events: &events,
            on_import_event: &mut imported,
            decide: &mut decide,
            parked: &parked,
            cancelled: &cancelled,
        };
        let mut item = WatchItem::new(1, "Song", SourceKind::Youtube);
        item.video_id = Some("abcdefghijk".into());
        adapter.save_output_paths(&mut item, &json!({"split_dirs":[split]}));
        assert_eq!(item.path(Stage::Download), Some(audio.as_path()));
        assert_eq!(item.path(Stage::Split), Some(split.as_path()));
        let replacement = directory.path().join("replacement.flac");
        fs::write(&replacement, [])?;
        adapter.save_output_paths(
            &mut item,
            &json!({"audio_files":[replacement],"split_dirs":[]}),
        );
        assert_eq!(prepared.audio(&item), Some(replacement));
        Ok(())
    }

    #[test]
    fn spotify_organize_again_imports_saved_audio() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let audio = directory.path().join("track.flac");
        fs::copy(fixture(), &audio)?;
        let config = library_config(directory.path())?;
        let settings = Settings::parse(
            &Paths::under(directory.path()),
            &json!({"output":directory.path(),"config":config,"interactive":true,"quality_policy":"off"}),
        )?;
        let prepared = super::Prepared::new(&settings);
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
        let mut item = WatchItem::new(1, "Warhaus - Love's a Stranger", SourceKind::Spotify);
        item.set_path(Stage::Download, Some(audio));
        item.track = Some(json!({
            "title":"Love's a Stranger","artists":["Warhaus"],"album":"Warhaus",
            "track_number":2,"disc_number":1,"release_date":"2017-10-13"
        }));
        let result = adapter.process_spotify(&item, ItemAction::OrganizeAgain, &cancelled)?;
        assert_eq!(result.status(Stage::Organize), StageStatus::Complete);
        let imported =
            muzik_library::Library::open_read_only(&directory.path().join("library.db"))?
                .items()?;
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
        for path in [&original, &album.join("one.flac"), &album.join("two.flac")] {
            fs::copy(fixture(), path)?;
        }
        let mut item = WatchItem::new(1, "Album", SourceKind::Youtube);
        item.video_id = Some("abcdefghijk".into());
        item.video_url = Some("https://www.youtube.com/watch?v=abcdefghijk".into());
        item.complete(Stage::Download, Some(original.clone()));
        super::apply_quality_result(
            &mut item,
            &super::QualityUpgradeResult {
                audio_files: Vec::new(),
                pre_split_dirs: vec![album.clone()],
            },
        );
        assert_eq!(item.path(Stage::Download), Some(original.as_path()));
        assert_eq!(super::ready_quality_directory(&item), Some(album.clone()));
        assert_eq!(item.status(Stage::Parse), StageStatus::Skipped);
        assert_eq!(item.status(Stage::Split), StageStatus::Complete);
        assert_eq!(item.status(Stage::Organize), StageStatus::Stale);
        let config = library_config(directory.path())?;
        let settings = Settings::parse(
            &Paths::under(directory.path()),
            &json!({"output":directory.path(),"config":config,"interactive":false,"quality_policy":"auto"}),
        )?;
        let prepared = super::Prepared::new(&settings);
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
        assert_eq!(result.status(Stage::Organize), StageStatus::Complete);
        assert_eq!(
            muzik_library::Library::open_read_only(&directory.path().join("library.db"))?
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
        assert_eq!(
            loaded.items[0].entry_id.as_deref(),
            Some("spotify:track:t1#0")
        );
        assert_eq!(
            loaded.items[1].entry_id.as_deref(),
            Some("spotify:track:t1#1")
        );
        assert_eq!(loaded.items[0].title, "Alex - One");
        Ok(())
    }

    #[test]
    fn bandcamp_purchases_become_items_with_their_download_page() {
        let loaded = bandcamp_items(&[muzik_core::bandcamp::Purchase {
            key: "p12".into(),
            artist: "Band".into(),
            title: "Album".into(),
            single: false,
            download_page: "https://bandcamp.com/download?id=12".into(),
            item_url: Some("https://band.bandcamp.com/album/album".into()),
            art_url: None,
        }]);
        let item = &loaded.items[0];
        assert_eq!(item.kind, SourceKind::Bandcamp);
        assert_eq!(item.entry_id.as_deref(), Some("p12"));
        assert_eq!(item.video_id.as_deref(), Some("p12"));
        assert_eq!(item.title, "Band - Album");
        assert_eq!(
            item.video_url.as_deref(),
            Some("https://band.bandcamp.com/album/album")
        );
        assert_eq!(
            item.track.as_ref().map(|track| &track["download_page"]),
            Some(&json!("https://bandcamp.com/download?id=12"))
        );
    }

    #[test]
    fn youtube_reload_preserves_saved_card_metadata() {
        let mut playlist = Playlist::new("PL1", "u", SourceKind::Youtube, Some("My list"));
        let mut saved = WatchItem::new(1, "Saved title", SourceKind::Youtube);
        saved.video_id = Some("abcdefghijk".into());
        saved.thumbnail_url = Some("https://example.test/image.jpg".into());
        playlist.items.push(saved);
        let loaded = youtube_items(
            &playlist,
            &json!({"title":"Current list","entries":[{"id":"abcdefghijk"}]}),
        );
        assert_eq!(loaded.items[0].title, "Saved title");
        assert_eq!(
            loaded.items[0].thumbnail_url.as_deref(),
            Some("https://example.test/image.jpg")
        );
    }

    #[test]
    fn a_private_video_without_title_and_duration_is_unavailable() {
        let loaded = youtube_items(
            &Playlist::new("PL1", "u", SourceKind::Youtube, None),
            &json!({"entries":[
                {"id":"abcdefghijk","title":null,"duration":null},
                {"id":"bcdefghijkl","title":"[Private video]","duration":null},
                {"id":"cdefghijklm","title":"Song","duration":245.0}
            ]}),
        );
        let flags: Vec<_> = loaded.items.iter().map(|item| item.unavailable).collect();
        assert_eq!(flags, [Some(true), Some(true), Some(false)]);
        assert_eq!(loaded.items[1].title, "bcdefghijkl");
    }

    #[test]
    fn youtube_new_card_uses_source_title_and_thumbnail() {
        let loaded = youtube_items(
            &Playlist::new("PL1", "u", SourceKind::Youtube, None),
            &json!({"title":"Playlist", "entries":[{"id":"abcdefghijk", "title":"Song", "thumbnail":"https://example.test/new.jpg"}]}),
        );
        assert_eq!(loaded.title.as_deref(), Some("Playlist"));
        assert_eq!(loaded.items[0].title, "Song");
        assert_eq!(
            loaded.items[0].thumbnail_url.as_deref(),
            Some("https://example.test/new.jpg")
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
