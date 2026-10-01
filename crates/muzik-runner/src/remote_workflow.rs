//! Remote acquisition for queued workflow jobs.

use crate::gates::{self, Gate};
use crate::local_workflow;
use crate::settings::Settings;
use muzik_core::paths::Paths;
use muzik_core::watchlist::Stage;
use muzik_core::{app_config, chapters::Chapter, DecisionKind};
use muzik_soulseek::fetch::Timeouts;
use muzik_soulseek::session::{setting, Session, SessionSettings};
use muzik_soulseek::types::Candidate;
use muzik_workflow::ytdlp::{Download, YtDlp};
use muzik_workflow::{
    classify_input, playlist, run_workflow_with_events, AudioFallback, AudioSource, ChapterReview,
    QualityCheckedAudio, SplitProgress, SplitTask, WorkflowEvent, WorkflowInput,
    WorkflowOperations, WorkflowOptions,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub fn run(
    input: WorkflowInput,
    settings: &Settings,
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
        paths: settings.paths.clone(),
        prefer: settings.options.prefer.clone(),
        interactive: settings.options.interactive,
        audio_source: settings.options.audio_source,
        fallback: settings.options.fallback,
        output: settings.request.output.clone(),
        youtube_acquired: false,
    };
    let mut report = |event| on_event(event_record(event));
    if let WorkflowInput::SpotifyExport(ref path) = input {
        let result = playlist::run_spotify_export(
            &settings.request,
            &settings.options,
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
    if let WorkflowInput::YoutubePlaylist { url, playlist_id } = input {
        let result = playlist::run_youtube_playlist(
            &settings.request,
            &settings.options,
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
        &settings.request,
        &settings.options,
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
    paths: Paths,
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
            YtDlp::default()
                .field(query, "title", self.local.cancelled)
                .map_err(|error| error.to_string())?
        } else {
            query.to_owned()
        };
        if query.is_empty() {
            return Err("YouTube video has no title for Soulseek search".into());
        }
        soulseek_download(
            &self.paths,
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
        let paths = &self.paths;
        let decide = &mut *self.local.decide;
        let (files, from_youtube) = acquire_spotify_audio(
            source,
            fallback,
            ready,
            || {
                soulseek_download(
                    paths,
                    &query,
                    &prefer,
                    interactive,
                    cancelled,
                    decide,
                    false,
                    None,
                )
            },
            || download(&query, &output, false, cancelled).map_err(|error| error.to_string()),
        )?;
        self.youtube_acquired = from_youtube;
        Ok(files)
    }

    fn soulseek_ready(&self) -> bool {
        soulseek_ready(&self.paths)
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
        YtDlp::default()
            .playlist_ids(url, self.local.cancelled)
            .map_err(|error| error.to_string())
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

pub(crate) fn soulseek_ready(paths: &Paths) -> bool {
    app_config::load(&paths.config_file())
        .ok()
        .is_some_and(|config| SessionSettings::configured(&config).is_some())
}

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

#[allow(clippy::too_many_arguments)]
pub(crate) fn soulseek_download(
    paths: &Paths,
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
    let config = app_config::load(&paths.config_file())?;
    let settings = SessionSettings::configured(&config)
        .ok_or("Set Soulseek credentials in configuration first.")?;
    let session = Session::shared(settings).map_err(|error| error.to_string())?;
    let timeouts = Timeouts::configured(&config);
    let ranked = session.search(query, prefer, 10, timeouts.search, cancelled)?;
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
    let output = output_root
        .map(Path::to_path_buf)
        .or_else(|| {
            setting(&config, "MUZIK_SOULSEEK_DOWNLOAD_DIR", "download_dir").map(PathBuf::from)
        })
        .unwrap_or_else(|| paths.soulseek());
    std::fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let destination = tempfile::Builder::new()
        .prefix("soulseek-")
        .tempdir_in(&output)
        .map_err(|error| error.to_string())?;
    session.fetch(
        &selected
            .candidate
            .audio_only(if single_file { 1 } else { usize::MAX }),
        destination.path(),
        timeouts.download,
        cancelled,
    )?;
    let files = muzik_workflow::find_audio_inputs(&[destination.path().to_path_buf()])
        .map_err(|error| error.to_string())?;
    if files.is_empty() {
        return Err("Soulseek returned no audio files.".into());
    }
    let root = destination.keep();
    muzik_workflow::find_audio_inputs(&[root]).map_err(|error| error.to_string())
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

pub(crate) fn download(
    url: &str,
    output: &Path,
    force: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<PathBuf>, muzik_workflow::Error> {
    let _permit = gates::enter(Gate::Download, Stage::Download, cancelled)
        .map_err(|_| muzik_workflow::Error::Cancelled)?;
    YtDlp::default().download(&Download::audio(url, output, force), cancelled)
}

#[cfg(test)]
mod tests {
    use super::{acquire_spotify_audio, candidate_row};
    use muzik_core::{AudioFallback, AudioSource};
    use muzik_soulseek::types::{Candidate, FileEntry};

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
}
