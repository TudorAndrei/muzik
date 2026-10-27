//! Remote acquisition for queued workflow jobs.

use crate::gates::{self, Gate};
use crate::local_workflow;
use crate::settings::Settings;
use crate::workflow_result::WorkflowResult;
use muzik_core::paths::Paths;
use muzik_core::{DecisionKind, JobEvent, PreferredAudio, Step, app_config, chapters::Chapter};
use muzik_soulseek::fetch::Timeouts;
use muzik_soulseek::session::{Session, SessionSettings, setting};
use muzik_soulseek::types::Candidate;
use muzik_store::watchlist::Stage;
use muzik_workflow::acquisition::{self, AcquiredAudio, AcquisitionSource};
use muzik_workflow::ytdlp::{Download, YtDlp};
use muzik_workflow::{
    AudioFallback, AudioSource, ChapterReview, QualityCheckedAudio, SplitProgress, SplitTask,
    WorkflowEvent, WorkflowInput, WorkflowOperations, WorkflowOptions, classify_input, playlist,
    run_workflow_with_events,
};
use serde_json::{Value, json};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub fn run(
    input: WorkflowInput,
    settings: &Settings,
    cancelled: &AtomicBool,
    stage: &Cell<Stage>,
    on_event: &mut dyn FnMut(JobEvent),
    on_import_event: &mut dyn FnMut(JobEvent),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<WorkflowResult, muzik_workflow::Error> {
    if cancelled.load(Ordering::SeqCst) {
        return Err(muzik_workflow::Error::Cancelled);
    }
    let mut operations = RemoteOperations {
        local: local_workflow::LocalOperations {
            decide,
            on_import_event,
            cancelled,
            stage,
        },
        paths: settings.paths.clone(),
        prefer: settings.options.prefer,
        interactive: settings.options.interactive,
        audio_source: settings.options.audio_source,
        fallback: settings.options.fallback,
        output: settings.request.output.clone(),
        acquired_from: None,
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
        let mut summary = WorkflowResult::from(result.processing);
        summary.items = Some(result.items.len());
        return Ok(summary);
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
        let mut summary = WorkflowResult::from(result.processing);
        summary.failures = Some(
            result
                .items
                .into_iter()
                .filter(|item| !item.completed)
                .map(Into::into)
                .collect(),
        );
        return Ok(summary);
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
        .cloned()
        .collect();
    let mut summary = WorkflowResult::from(result);
    summary.audio_files = Some(audio_files);
    Ok(summary)
}

struct RemoteOperations<'a> {
    local: local_workflow::LocalOperations<'a>,
    paths: Paths,
    prefer: PreferredAudio,
    interactive: bool,
    audio_source: AudioSource,
    fallback: AudioFallback,
    output: PathBuf,
    acquired_from: Option<AcquisitionSource>,
}

impl WorkflowOperations for RemoteOperations<'_> {
    fn download_youtube(
        &mut self,
        url: &str,
        output: &Path,
        force: bool,
    ) -> Result<Vec<PathBuf>, String> {
        self.local.stage.set(Stage::Download);
        let files = download(url, output, force, self.local.cancelled)?;
        self.acquired_from = Some(AcquisitionSource::Youtube);
        Ok(files)
    }

    fn acquire_soulseek(&mut self, query: &str) -> Result<Vec<PathBuf>, String> {
        self.local.stage.set(Stage::Download);
        self.acquired_from = Some(AcquisitionSource::Soulseek);
        let query = if matches!(classify_input(query), WorkflowInput::YoutubeVideo { .. }) {
            YtDlp::default().field(query, "title", self.local.cancelled)?
        } else {
            query.to_owned()
        };
        if query.is_empty() {
            return Err("YouTube video has no title for Soulseek search".into());
        }
        Ok(soulseek_download(
            &self.paths,
            &query,
            self.prefer,
            self.interactive,
            self.local.cancelled,
            self.local.decide,
            false,
            None,
        )?)
    }

    fn acquire_spotify_track(
        &mut self,
        track: &playlist::SpotifyTrack,
    ) -> Result<Vec<PathBuf>, String> {
        self.local.stage.set(Stage::Download);
        let query = format!("{} - {}", track.artist, track.title);
        let ready = self.soulseek_ready();
        let source = self.audio_source;
        let fallback = self.fallback;
        let output = self.output.clone();
        let cancelled = self.local.cancelled;
        let prefer = self.prefer;
        let interactive = self.interactive;
        let paths = &self.paths;
        let decide = &mut *self.local.decide;
        let acquired = acquire_spotify_audio(
            source,
            fallback,
            ready,
            cancelled,
            || {
                soulseek_download(
                    paths,
                    &query,
                    prefer,
                    interactive,
                    cancelled,
                    decide,
                    false,
                    None,
                )
            },
            || {
                let _permit = gates::enter(Gate::Download, cancelled)?;
                Ok(YtDlp::default()
                    .download_search(&Download::audio(&query, &output, false), cancelled)?)
            },
        )?;
        self.acquired_from = Some(acquired.source);
        Ok(acquired.files)
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
        let from_youtube = self.acquired_from.take() == Some(AcquisitionSource::Youtube)
            || audio_files
                .iter()
                .any(|path| muzik_core::chapters::Sidecar::Info.path(path).is_file());
        if !from_youtube {
            return Ok(QualityCheckedAudio {
                audio_files: audio_files.to_vec(),
                pre_split_dirs: Vec::new(),
            });
        }
        self.local.stage.set(Stage::Quality);
        let _permit = gates::enter(Gate::Process, cancelled)?;
        let result = muzik_workflow::quality::check_youtube_quality(
            &self.paths,
            audio_files.to_vec(),
            options.quality_policy,
            options.min_bitrate,
            self.prefer,
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
        Ok(YtDlp::default().playlist_ids(url, self.local.cancelled)?)
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

pub fn soulseek_ready(paths: &Paths) -> bool {
    app_config::load(&paths.config_file())
        .is_ok_and(|config| SessionSettings::configured(&config).is_some())
}

fn acquire_spotify_audio<S, Y>(
    source: AudioSource,
    fallback: AudioFallback,
    soulseek_ready: bool,
    cancelled: &AtomicBool,
    mut soulseek: S,
    mut youtube: Y,
) -> Result<AcquiredAudio, muzik_workflow::Error>
where
    S: FnMut() -> crate::Result<Vec<PathBuf>>,
    Y: FnMut() -> crate::Result<Vec<PathBuf>>,
{
    let first = if source == AudioSource::Youtube || source == AudioSource::Auto && !soulseek_ready
    {
        AcquisitionSource::Youtube
    } else {
        AcquisitionSource::Soulseek
    };
    acquisition::acquire(first, fallback, cancelled, |source| {
        match source {
            AcquisitionSource::Soulseek => soulseek(),
            AcquisitionSource::Youtube => youtube(),
        }
        .map_err(String::from)
    })
}

fn event_record(event: WorkflowEvent) -> JobEvent {
    match event {
        WorkflowEvent::InputClassified(_) => JobEvent::message("Reading remote input."),
        WorkflowEvent::AcquisitionStarted => JobEvent::StepStarted(Step::Download),
        WorkflowEvent::AcquisitionCompleted { .. } => JobEvent::StepFinished(Step::Download),
        WorkflowEvent::Completed => JobEvent::message("Remote workflow complete."),
        other => local_workflow::event_record(other),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "three callers each pass a different mix of search options and callbacks"
)]
pub fn soulseek_download(
    paths: &Paths,
    query: &str,
    prefer: PreferredAudio,
    interactive: bool,
    cancelled: &AtomicBool,
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    single_file: bool,
    output_root: Option<&Path>,
) -> crate::Result<Vec<PathBuf>> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("Soulseek search cancelled".into());
    }
    let _permit = gates::enter(Gate::Download, cancelled)?;
    let config = app_config::load(&paths.config_file())?;
    let settings = SessionSettings::configured(&config)
        .ok_or("Set Soulseek credentials in configuration first.")?;
    let session = Session::shared(settings)?;
    let timeouts = Timeouts::configured(&config);
    let ranked = session.search(query, prefer, 10, timeouts.search, cancelled)?;
    if ranked.is_empty() {
        return Err(format!("No Soulseek audio found for {query}.").into());
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
    std::fs::create_dir_all(&output)?;
    let destination = tempfile::Builder::new()
        .prefix("soulseek-")
        .tempdir_in(&output)?;
    session.fetch(
        &selected
            .candidate
            .audio_only(if single_file { 1 } else { usize::MAX }),
        destination.path(),
        timeouts.download,
        cancelled,
    )?;
    let files = muzik_workflow::find_audio_inputs(&[destination.path().to_path_buf()])?;
    if files.is_empty() {
        return Err("Soulseek returned no audio files.".into());
    }
    let root = destination.keep();
    Ok(muzik_workflow::find_audio_inputs(&[root])?)
}

fn candidate_row(candidate: &Candidate, score: f64) -> Value {
    let path = candidate
        .files
        .first()
        .map_or("", |file| file.name.as_str());
    let title = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let format = candidate
        .files
        .iter()
        .find_map(muzik_soulseek::ranking::format)
        .map_or_else(String::new, |format| format.to_string());
    json!({"title":title,"score":score,"user":candidate.username,"quality":{"format":format},"files":candidate.files,"path":path})
}

pub fn download(
    url: &str,
    output: &Path,
    force: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<PathBuf>, muzik_workflow::Error> {
    let _permit =
        gates::enter(Gate::Download, cancelled).map_err(|_| muzik_workflow::Error::Cancelled)?;
    YtDlp::default().download(&Download::audio(url, output, force), cancelled)
}

#[cfg(test)]
mod tests {
    use super::{acquire_spotify_audio, candidate_row};
    use muzik_core::{AudioFallback, AudioSource};
    use muzik_soulseek::types::{Candidate, FileEntry};

    #[test]
    fn spotify_youtube_source_does_not_call_soulseek() {
        let audio = std::path::PathBuf::from("youtube-audio.flac");
        let acquired = acquire_spotify_audio(
            AudioSource::Youtube,
            AudioFallback::None,
            true,
            &std::sync::atomic::AtomicBool::new(false),
            || Err("Soulseek must not run".into()),
            || Ok(vec![audio.clone()]),
        )
        .unwrap();
        assert_eq!(acquired.files, vec![audio]);
        assert_eq!(acquired.source, super::AcquisitionSource::Youtube);
    }

    #[test]
    fn spotify_soulseek_failure_uses_selected_fallback() {
        let audio = std::path::PathBuf::from("fallback-audio.flac");
        let acquired = acquire_spotify_audio(
            AudioSource::Soulseek,
            AudioFallback::Youtube,
            true,
            &std::sync::atomic::AtomicBool::new(false),
            || Err("Soulseek is unavailable".into()),
            || Ok(vec![audio.clone()]),
        )
        .unwrap();
        assert_eq!(acquired.files, vec![audio]);
        assert_eq!(acquired.source, super::AcquisitionSource::Youtube);
        let error = acquire_spotify_audio(
            AudioSource::Soulseek,
            AudioFallback::None,
            true,
            &std::sync::atomic::AtomicBool::new(false),
            || Err("Soulseek is unavailable".into()),
            || Err("YouTube must not run".into()),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "Soulseek is unavailable");
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
