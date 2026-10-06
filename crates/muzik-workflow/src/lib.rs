//! Workflow planning and execution shared by command-line and desktop callers.
//!
//! The caller owns split and import decisions. This crate owns source selection,
//! audio discovery, the split/organize order, and safe cancellation points.

use muzik_core::chapters::{self, Chapter};
use muzik_core::config_choices::DEFAULT_AUDIO_PREFERENCE;
pub use muzik_core::splitter::SplitProgress;
pub use muzik_core::{AudioFallback, AudioSource, DuplicatePolicy, MetadataSource, QualityPolicy};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use url::Url;

pub mod discovery;
pub mod playlist;
pub mod quality;
pub mod ytdlp;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowRequest {
    pub raw: String,
    pub output: PathBuf,
    pub splits: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowOptions {
    pub review: bool,
    pub no_split: bool,
    pub no_organize: bool,
    pub import: bool,
    pub tag_only: bool,
    pub dry_run: bool,
    pub jobs: usize,
    pub keep_source: bool,
    pub force: bool,
    pub compilation: bool,
    pub config: Option<PathBuf>,
    pub metadata_source: MetadataSource,
    pub audio_source: AudioSource,
    pub prefer: String,
    pub fallback: AudioFallback,
    pub interactive: bool,
    pub quality_policy: QualityPolicy,
    pub min_bitrate: u32,
    pub duplicates: DuplicatePolicy,
}

impl Default for WorkflowOptions {
    fn default() -> Self {
        Self {
            review: false,
            no_split: false,
            no_organize: false,
            import: false,
            tag_only: false,
            dry_run: false,
            jobs: 0,
            keep_source: false,
            force: false,
            compilation: false,
            config: None,
            metadata_source: MetadataSource::default(),
            audio_source: AudioSource::default(),
            prefer: DEFAULT_AUDIO_PREFERENCE.into(),
            fallback: AudioFallback::default(),
            interactive: true,
            quality_policy: QualityPolicy::default(),
            min_bitrate: 256,
            duplicates: DuplicatePolicy::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowInput {
    Local(PathBuf),
    YoutubeVideo { url: String, video_id: String },
    YoutubePlaylist { url: String, playlist_id: String },
    SpotifyExport(PathBuf),
    Search(String),
}

pub fn classify_input(raw: &str) -> WorkflowInput {
    let expanded = muzik_core::paths::expand_home(Path::new(raw));
    if expanded.exists() {
        if expanded.is_file()
            && expanded
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "csv" | "json"))
        {
            return WorkflowInput::SpotifyExport(expanded);
        }
        return WorkflowInput::Local(expanded);
    }
    if let Ok(url) = Url::parse(raw)
        && matches!(url.scheme(), "http" | "https")
    {
        let host = url.host_str().unwrap_or("").trim_start_matches("www.");
        if matches!(
            host,
            "youtube.com" | "m.youtube.com" | "music.youtube.com" | "youtu.be"
        ) {
            if let Some(playlist_id) = url.query_pairs().find_map(|(key, value)| {
                (key == "list" && !value.is_empty()).then(|| value.into_owned())
            }) {
                return WorkflowInput::YoutubePlaylist {
                    url: raw.to_owned(),
                    playlist_id,
                };
            }
            let id = if host == "youtu.be" {
                url.path_segments()
                    .and_then(|mut parts| parts.next())
                    .unwrap_or("")
                    .to_owned()
            } else {
                url.query_pairs()
                    .find_map(|(key, value)| (key == "v").then(|| value.into_owned()))
                    .unwrap_or_default()
            };
            if id.len() == 11
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            {
                return WorkflowInput::YoutubeVideo {
                    url: raw.to_owned(),
                    video_id: id,
                };
            }
        }
    }
    WorkflowInput::Search(raw.to_owned())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumInput {
    pub source: PathBuf,
    pub chapters: Vec<Chapter>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioProcessingPlan {
    pub albums: Vec<AlbumInput>,
    pub singles: Vec<PathBuf>,
    pub pre_split_dirs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitTask {
    pub source: PathBuf,
    pub chapters: Vec<Chapter>,
    pub output: PathBuf,
}

/// The result of a review before the audio plan is fixed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChapterReview {
    Accept,
    Edit(Vec<Chapter>),
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioProcessingResult {
    pub plan: AudioProcessingPlan,
    pub split_dirs: Vec<PathBuf>,
    pub organize_targets: Vec<PathBuf>,
}

/// Audio files and ready-to-import directories after a source quality check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualityCheckedAudio {
    pub audio_files: Vec<PathBuf>,
    pub pre_split_dirs: Vec<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkflowEvent {
    InputClassified(WorkflowInput),
    AcquisitionStarted,
    AcquisitionCompleted {
        files: Vec<PathBuf>,
    },
    PlanReady {
        albums: usize,
        singles: usize,
    },
    SplitStarted(SplitTask),
    SplitProgress {
        source: PathBuf,
        progress: SplitProgress,
    },
    SplitCompleted {
        output: PathBuf,
    },
    OrganizeStarted {
        target: PathBuf,
    },
    OrganizeCompleted {
        target: PathBuf,
    },
    Completed,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("workflow cancelled")]
    Cancelled,
    #[error("file operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("chapter lookup failed: {0}")]
    Chapters(#[from] chapters::Error),
    #[error("{0}")]
    Operation(String),
    #[error("playlist workflow needs a playlist adapter")]
    PlaylistAdapterRequired,
    #[error("Spotify export workflow needs a playlist adapter")]
    SpotifyAdapterRequired,
    #[error("no audio files found in output directory")]
    NoAudio,
}

pub trait WorkflowOperations {
    /// Return acquired files. The implementation may use the existing Rust yt-dlp path.
    fn download_youtube(
        &mut self,
        url: &str,
        output: &Path,
        force: bool,
    ) -> Result<Vec<PathBuf>, String>;
    fn acquire_soulseek(&mut self, query: &str) -> Result<Vec<PathBuf>, String>;
    /// Return video IDs in playlist order.
    fn youtube_playlist_video_ids(&mut self, _url: &str) -> Result<Vec<String>, String> {
        Err("YouTube playlist discovery is not configured".into())
    }
    /// Acquire audio using the metadata of one Spotify track.
    fn acquire_spotify_track(
        &mut self,
        track: &playlist::SpotifyTrack,
    ) -> Result<Vec<PathBuf>, String> {
        let query = [
            Some(track.artist.as_str()),
            Some(track.title.as_str()),
            track.album.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" - ");
        self.acquire_soulseek(&query)
    }
    fn soulseek_ready(&self) -> bool {
        false
    }
    /// Check newly acquired audio before chapter planning. The default keeps it.
    fn check_quality(
        &mut self,
        audio_files: &[PathBuf],
        _options: &WorkflowOptions,
        _cancelled: &AtomicBool,
    ) -> Result<QualityCheckedAudio, String> {
        Ok(QualityCheckedAudio {
            audio_files: audio_files.to_vec(),
            pre_split_dirs: Vec::new(),
        })
    }
    /// Find chapters from the selected metadata service when no local chapters exist.
    fn discover_chapters(
        &mut self,
        source: &Path,
        selected: MetadataSource,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Chapter>, String> {
        discovery::discover(source, selected, cancelled)
    }
    /// Ask for a decision when `WorkflowOptions::review` is set.
    /// Returning `Reject` treats the source as one track.
    fn review_chapters(
        &mut self,
        _source: &Path,
        _chapters: &[Chapter],
        _cancelled: &AtomicBool,
    ) -> Result<ChapterReview, String> {
        Ok(ChapterReview::Accept)
    }
    /// Keep Beets configuration, match decisions, and duplicate behavior in the import adapter.
    fn organize(&mut self, target: &Path, options: &WorkflowOptions) -> Result<(), String>;
    fn split(&mut self, task: &SplitTask, options: &WorkflowOptions) -> Result<(), String>;
    /// Override this to pass cancellation and per-track progress to the splitter.
    fn split_with_cancel(
        &mut self,
        task: &SplitTask,
        options: &WorkflowOptions,
        _cancelled: &AtomicBool,
        _on_progress: &mut dyn FnMut(SplitProgress),
    ) -> Result<(), String> {
        self.split(task, options)
    }
}

/// Find supported audio below files and directories, with one result per real path.
pub fn find_audio_inputs(paths: &[PathBuf]) -> Result<Vec<PathBuf>, Error> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut pending = paths.to_vec();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            let mut children = fs::read_dir(&path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()?;
            children.sort();
            pending.extend(children.into_iter().rev());
        } else if path.is_file() && muzik_core::audio::is_audio(&path) {
            let identity = fs::canonicalize(&path)?;
            if seen.insert(identity) {
                result.push(path);
            }
        }
    }
    result.sort();
    Ok(result)
}

pub fn plan_audio_processing(
    audio_files: &[PathBuf],
    pre_split_dirs: &[PathBuf],
    no_split: bool,
) -> Result<AudioProcessingPlan, Error> {
    plan_audio_processing_with_source(audio_files, pre_split_dirs, no_split, MetadataSource::Auto)
}

fn plan_audio_processing_with_source(
    audio_files: &[PathBuf],
    pre_split_dirs: &[PathBuf],
    no_split: bool,
    metadata_source: MetadataSource,
) -> Result<AudioProcessingPlan, Error> {
    let mut albums = Vec::new();
    let mut singles = Vec::new();
    for source in audio_files {
        let chapters = if no_split {
            Vec::new()
        } else {
            chapters::find_chapters_with_info(
                source,
                matches!(
                    metadata_source,
                    MetadataSource::Youtube | MetadataSource::Auto
                ),
            )?
        };
        if chapters.is_empty() {
            singles.push(source.clone());
        } else {
            albums.push(AlbumInput {
                source: source.clone(),
                chapters,
            });
        }
    }
    Ok(AudioProcessingPlan {
        albums,
        singles,
        pre_split_dirs: pre_split_dirs.to_vec(),
    })
}

pub fn process_audio_plan<O: WorkflowOperations>(
    audio_files: &[PathBuf],
    pre_split_dirs: &[PathBuf],
    splits: &Path,
    options: &WorkflowOptions,
    operations: &mut O,
    cancelled: &AtomicBool,
) -> Result<AudioProcessingResult, Error> {
    process_audio_plan_with_events(
        audio_files,
        pre_split_dirs,
        splits,
        options,
        operations,
        cancelled,
        &mut |_| {},
    )
}

pub fn process_audio_plan_with_events<O: WorkflowOperations>(
    audio_files: &[PathBuf],
    pre_split_dirs: &[PathBuf],
    splits: &Path,
    options: &WorkflowOptions,
    operations: &mut O,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(WorkflowEvent),
) -> Result<AudioProcessingResult, Error> {
    check_cancelled(cancelled)?;
    let checked = operations
        .check_quality(audio_files, options, cancelled)
        .map_err(|error| operation_error(error, cancelled))?;
    check_cancelled(cancelled)?;
    let mut ready_dirs = pre_split_dirs.to_vec();
    ready_dirs.extend(checked.pre_split_dirs);
    let mut plan = plan_audio_processing_with_source(
        &checked.audio_files,
        &ready_dirs,
        options.no_split,
        options.metadata_source,
    )?;
    if !options.no_split && options.metadata_source != MetadataSource::None {
        let mut without_chapters = Vec::new();
        for source in plan.singles {
            check_cancelled(cancelled)?;
            let found = operations
                .discover_chapters(&source, options.metadata_source, cancelled)
                .map_err(|error| operation_error(error, cancelled))?;
            if found.is_empty() {
                without_chapters.push(source);
            } else {
                plan.albums.push(AlbumInput {
                    source,
                    chapters: found,
                });
            }
        }
        plan.singles = without_chapters;
    }
    if options.review {
        let mut reviewed = Vec::with_capacity(plan.albums.len());
        for mut album in plan.albums {
            check_cancelled(cancelled)?;
            match operations
                .review_chapters(&album.source, &album.chapters, cancelled)
                .map_err(|error| operation_error(error, cancelled))?
            {
                ChapterReview::Accept => reviewed.push(album),
                ChapterReview::Edit(chapters) if chapters.is_empty() => {
                    return Err(Error::Operation("edited chapters must not be empty".into()));
                }
                ChapterReview::Edit(chapters) => {
                    album.chapters = chapters;
                    reviewed.push(album);
                }
                ChapterReview::Reject => plan.singles.push(album.source),
            }
            check_cancelled(cancelled)?;
        }
        plan.albums = reviewed;
    }
    on_event(WorkflowEvent::PlanReady {
        albums: plan.albums.len(),
        singles: plan.singles.len(),
    });
    let mut split_dirs = plan.pre_split_dirs.clone();
    for album in &plan.albums {
        check_cancelled(cancelled)?;
        let stem = album
            .source
            .file_stem()
            .ok_or_else(|| Error::Operation("audio file has no name".into()))?;
        let task = SplitTask {
            source: album.source.clone(),
            chapters: album.chapters.clone(),
            output: splits.join(stem),
        };
        if !options.dry_run {
            on_event(WorkflowEvent::SplitStarted(task.clone()));
            operations
                .split_with_cancel(&task, options, cancelled, &mut |progress| {
                    on_event(WorkflowEvent::SplitProgress {
                        source: task.source.clone(),
                        progress,
                    });
                })
                .map_err(|error| operation_error(error, cancelled))?;
            check_cancelled(cancelled)?;
            on_event(WorkflowEvent::SplitCompleted {
                output: task.output.clone(),
            });
            split_dirs.push(task.output);
        }
    }
    let organize_targets = if options.no_organize {
        Vec::new()
    } else {
        let mut targets = split_dirs.clone();
        targets.extend(organize_targets_for_singles(&plan.singles));
        for target in &targets {
            check_cancelled(cancelled)?;
            if !options.dry_run {
                on_event(WorkflowEvent::OrganizeStarted {
                    target: target.clone(),
                });
                operations
                    .organize(target, options)
                    .map_err(|error| operation_error(error, cancelled))?;
                check_cancelled(cancelled)?;
                on_event(WorkflowEvent::OrganizeCompleted {
                    target: target.clone(),
                });
            }
        }
        targets
    };
    check_cancelled(cancelled)?;
    on_event(WorkflowEvent::Completed);
    Ok(AudioProcessingResult {
        plan,
        split_dirs,
        organize_targets,
    })
}

pub fn organize_targets_for_singles(singles: &[PathBuf]) -> Vec<PathBuf> {
    if singles.len() > 1 {
        let mut common = singles[0].parent().map(Path::to_path_buf);
        for path in singles.iter().skip(1) {
            while let Some(root) = common.as_ref() {
                if path.starts_with(root) {
                    break;
                }
                common = root.parent().map(Path::to_path_buf);
            }
        }
        if let Some(root) = common.filter(|root| root.is_dir()) {
            return vec![root];
        }
    }
    singles.to_vec()
}

/// Run one local file, one YouTube video, or one Soulseek search through import.
pub fn run_workflow<O: WorkflowOperations>(
    request: &WorkflowRequest,
    options: &WorkflowOptions,
    operations: &mut O,
    cancelled: &AtomicBool,
) -> Result<AudioProcessingResult, Error> {
    run_workflow_with_events(request, options, operations, cancelled, &mut |_| {})
}

pub fn run_workflow_with_events<O: WorkflowOperations>(
    request: &WorkflowRequest,
    options: &WorkflowOptions,
    operations: &mut O,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(WorkflowEvent),
) -> Result<AudioProcessingResult, Error> {
    check_cancelled(cancelled)?;
    let input = classify_input(&request.raw);
    on_event(WorkflowEvent::InputClassified(input.clone()));
    if matches!(
        input,
        WorkflowInput::YoutubeVideo { .. } | WorkflowInput::Search(_)
    ) && !options.dry_run
    {
        on_event(WorkflowEvent::AcquisitionStarted);
    }
    let files = match input {
        WorkflowInput::Local(path) => find_audio_inputs(&[path])?,
        WorkflowInput::YoutubeVideo { url, video_id } => {
            if options.dry_run {
                Vec::new()
            } else if !options.force && options.audio_source != AudioSource::Soulseek {
                let existing = find_audio_by_youtube_id(&request.output, &video_id)?;
                if existing.is_empty() {
                    operations
                        .download_youtube(&url, &request.output, false)
                        .map_err(|error| operation_error(error, cancelled))?
                } else {
                    existing
                }
            } else if options.audio_source == AudioSource::Soulseek {
                match operations.acquire_soulseek(&url) {
                    Ok(files) if !files.is_empty() => files,
                    Ok(_) | Err(_) if options.fallback == AudioFallback::Youtube => operations
                        .download_youtube(&url, &request.output, options.force)
                        .map_err(|error| operation_error(error, cancelled))?,
                    Ok(files) => files,
                    Err(error) => return Err(operation_error(error, cancelled)),
                }
            } else {
                operations
                    .download_youtube(&url, &request.output, true)
                    .map_err(|error| operation_error(error, cancelled))?
            }
        }
        WorkflowInput::Search(query) => {
            if options.dry_run {
                Vec::new()
            } else if matches!(options.audio_source, AudioSource::Soulseek)
                || matches!(options.audio_source, AudioSource::Auto) && operations.soulseek_ready()
            {
                match operations.acquire_soulseek(&query) {
                    Ok(files) if !files.is_empty() => files,
                    Ok(_) | Err(_) if options.fallback == AudioFallback::Youtube => operations
                        .download_youtube(&query, &request.output, options.force)
                        .map_err(|error| operation_error(error, cancelled))?,
                    Ok(files) => files,
                    Err(error) => return Err(operation_error(error, cancelled)),
                }
            } else {
                operations
                    .download_youtube(&query, &request.output, options.force)
                    .map_err(|error| operation_error(error, cancelled))?
            }
        }
        WorkflowInput::YoutubePlaylist { url, playlist_id } => {
            return playlist::run_youtube_playlist(
                request,
                options,
                operations,
                cancelled,
                &playlist_id,
                &url,
                on_event,
            )
            .map(|result| result.processing);
        }
        WorkflowInput::SpotifyExport(path) => {
            return playlist::run_spotify_export(
                request, options, operations, cancelled, &path, on_event,
            )
            .map(|result| result.processing);
        }
    };
    check_cancelled(cancelled)?;
    if !options.dry_run {
        on_event(WorkflowEvent::AcquisitionCompleted {
            files: files.clone(),
        });
    }
    let files = find_audio_inputs(&files)?;
    if files.is_empty() && !options.dry_run {
        return Err(Error::NoAudio);
    }
    process_audio_plan_with_events(
        &files,
        &[],
        &request.splits,
        options,
        operations,
        cancelled,
        on_event,
    )
}

fn operation_error(message: String, cancelled: &AtomicBool) -> Error {
    if cancelled.load(Ordering::SeqCst) {
        Error::Cancelled
    } else {
        Error::Operation(message)
    }
}

fn find_audio_by_youtube_id(directory: &Path, video_id: &str) -> Result<Vec<PathBuf>, Error> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    Ok(find_audio_inputs(&[directory.to_path_buf()])?
        .into_iter()
        .filter(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.contains(&format!("[{video_id}]")))
        })
        .collect())
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), Error> {
    if cancelled.load(Ordering::SeqCst) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
