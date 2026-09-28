//! Workflow planning and execution shared by command-line and desktop callers.
//!
//! The caller owns split and import decisions. This crate owns source selection,
//! audio discovery, the split/organize order, and safe cancellation points.

use muzik_core::chapters::{self, Chapter};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use url::Url;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowRequest {
    pub raw: String,
    pub output: PathBuf,
    pub splits: PathBuf,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AudioSource {
    #[default]
    Youtube,
    Soulseek,
    Auto,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AudioFallback {
    #[default]
    Youtube,
    None,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MetadataSource {
    None,
    Youtube,
    Musicbrainz,
    #[default]
    Auto,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QualityPolicy {
    #[default]
    Off,
    Ask,
    Auto,
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
    pub config: Option<PathBuf>,
    pub metadata_source: MetadataSource,
    pub audio_source: AudioSource,
    pub prefer: String,
    pub fallback: AudioFallback,
    pub interactive: bool,
    pub quality_policy: QualityPolicy,
    pub min_bitrate: u32,
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
            config: None,
            metadata_source: MetadataSource::Auto,
            audio_source: AudioSource::Youtube,
            prefer: "lossless".into(),
            fallback: AudioFallback::Youtube,
            interactive: true,
            quality_policy: QualityPolicy::Off,
            min_bitrate: 256,
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
    let expanded = expand_home(raw);
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

fn expand_home(raw: &str) -> PathBuf {
    if raw == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(raw)
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioProcessingResult {
    pub plan: AudioProcessingPlan,
    pub split_dirs: Vec<PathBuf>,
    pub organize_targets: Vec<PathBuf>,
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
    fn soulseek_ready(&self) -> bool {
        false
    }
    /// Keep Beets configuration, match decisions, and duplicate behavior in the import adapter.
    fn organize(&mut self, target: &Path, options: &WorkflowOptions) -> Result<(), String>;
    fn split(&mut self, task: &SplitTask, options: &WorkflowOptions) -> Result<(), String>;
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
        } else if path.is_file() && is_audio(&path) {
            let identity = fs::canonicalize(&path)?;
            if seen.insert(identity) {
                result.push(path);
            }
        }
    }
    result.sort();
    Ok(result)
}

fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "flac"
                    | "mp3"
                    | "m4a"
                    | "opus"
                    | "wav"
                    | "aac"
                    | "ogg"
                    | "aiff"
                    | "aif"
                    | "ape"
                    | "wv"
            )
        })
}

pub fn plan_audio_processing(
    audio_files: &[PathBuf],
    pre_split_dirs: &[PathBuf],
    no_split: bool,
) -> Result<AudioProcessingPlan, Error> {
    let mut albums = Vec::new();
    let mut singles = Vec::new();
    for source in audio_files {
        let chapters = if no_split {
            Vec::new()
        } else {
            chapters::find_chapters(source)?
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
    check_cancelled(cancelled)?;
    let plan = plan_audio_processing(audio_files, pre_split_dirs, options.no_split)?;
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
            operations.split(&task, options).map_err(Error::Operation)?;
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
                operations
                    .organize(target, options)
                    .map_err(Error::Operation)?;
            }
        }
        targets
    };
    check_cancelled(cancelled)?;
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
    check_cancelled(cancelled)?;
    let input = classify_input(&request.raw);
    let files = match input {
        WorkflowInput::Local(path) => find_audio_inputs(&[path])?,
        WorkflowInput::YoutubeVideo { url, video_id } => {
            if options.dry_run {
                Vec::new()
            } else if !options.force {
                let existing = find_audio_by_youtube_id(&request.output, &video_id)?;
                if existing.is_empty() {
                    operations
                        .download_youtube(&url, &request.output, false)
                        .map_err(Error::Operation)?
                } else {
                    existing
                }
            } else {
                operations
                    .download_youtube(&url, &request.output, true)
                    .map_err(Error::Operation)?
            }
        }
        WorkflowInput::Search(query) => {
            if options.dry_run {
                Vec::new()
            } else if matches!(options.audio_source, AudioSource::Soulseek)
                || matches!(options.audio_source, AudioSource::Auto) && operations.soulseek_ready()
            {
                operations
                    .acquire_soulseek(&query)
                    .map_err(Error::Operation)?
            } else {
                operations
                    .download_youtube(&query, &request.output, options.force)
                    .map_err(Error::Operation)?
            }
        }
        WorkflowInput::YoutubePlaylist { .. } => return Err(Error::PlaylistAdapterRequired),
        WorkflowInput::SpotifyExport(_) => return Err(Error::SpotifyAdapterRequired),
    };
    check_cancelled(cancelled)?;
    let files = find_audio_inputs(&files)?;
    if files.is_empty() && !options.dry_run {
        return Err(Error::NoAudio);
    }
    process_audio_plan(&files, &[], &request.splits, options, operations, cancelled)
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
