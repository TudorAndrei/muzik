use muzik_workflow::{
    AudioFallback, AudioSource, ChapterReview, Error, QualityCheckedAudio, SplitProgress,
    SplitTask, WorkflowEvent, WorkflowInput, WorkflowOperations, WorkflowOptions, WorkflowRequest,
    classify_input, plan_audio_processing, run_workflow, run_workflow_with_events,
};

#[test]
fn workflow_uses_selected_youtube_description_for_chapter_plan()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("album.opus");
    fs::write(&audio, b"audio")?;
    fs::write(
        dir.path().join("album.info.json"),
        r#"{"description":"0:00 Opening\n3:12 Closing"}"#,
    )?;
    let mut operations = RecordingOperations::default();
    let result = muzik_workflow::process_audio_plan(
        std::slice::from_ref(&audio),
        &[],
        &dir.path().join("splits"),
        &WorkflowOptions {
            metadata_source: muzik_workflow::MetadataSource::Youtube,
            no_organize: true,
            ..WorkflowOptions::default()
        },
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert_eq!(result.plan.albums.len(), 1);
    assert_eq!(result.plan.albums[0].chapters[1].title, "Closing");
    assert_eq!(operations.split_sources, [audio]);
    Ok(())
}
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct RecordingOperations {
    downloads: Vec<String>,
    split_sources: Vec<PathBuf>,
    organized: Vec<PathBuf>,
    downloaded_files: Vec<PathBuf>,
    soulseek_files: Vec<PathBuf>,
    soulseek_queries: Vec<String>,
    review_decision: Option<ChapterReview>,
    reviewed: Vec<PathBuf>,
    split_chapters: Vec<Vec<muzik_core::chapters::Chapter>>,
    quality_result: Option<QualityCheckedAudio>,
    quality_calls: usize,
}

impl WorkflowOperations for RecordingOperations {
    fn download_youtube(
        &mut self,
        url: &str,
        _output: &Path,
        _force: bool,
    ) -> Result<Vec<PathBuf>, String> {
        self.downloads.push(url.to_owned());
        Ok(self.downloaded_files.clone())
    }

    fn acquire_soulseek(&mut self, query: &str) -> Result<Vec<PathBuf>, String> {
        self.soulseek_queries.push(query.to_owned());
        Ok(self.soulseek_files.clone())
    }

    fn organize(&mut self, target: &Path, _options: &WorkflowOptions) -> Result<(), String> {
        self.organized.push(target.to_path_buf());
        Ok(())
    }

    fn review_chapters(
        &mut self,
        source: &Path,
        _: &[muzik_core::chapters::Chapter],
        _: &AtomicBool,
    ) -> Result<ChapterReview, String> {
        self.reviewed.push(source.to_path_buf());
        Ok(self
            .review_decision
            .clone()
            .unwrap_or(ChapterReview::Accept))
    }

    fn split(&mut self, task: &SplitTask, _options: &WorkflowOptions) -> Result<(), String> {
        self.split_sources.push(task.source.clone());
        self.split_chapters.push(task.chapters.clone());
        fs::create_dir_all(&task.output).map_err(|error| error.to_string())
    }

    fn split_with_cancel(
        &mut self,
        task: &SplitTask,
        options: &WorkflowOptions,
        _cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(SplitProgress),
    ) -> Result<(), String> {
        self.split(task, options)?;
        on_progress(SplitProgress {
            completed: 1,
            total: task.chapters.len(),
            chapter_index: task
                .chapters
                .first()
                .ok_or("the task has no chapters")?
                .index,
        });
        Ok(())
    }

    fn check_quality(
        &mut self,
        audio_files: &[PathBuf],
        _: &WorkflowOptions,
        _: &AtomicBool,
    ) -> Result<QualityCheckedAudio, String> {
        self.quality_calls = self.quality_calls.saturating_add(1);
        Ok(self
            .quality_result
            .clone()
            .unwrap_or_else(|| QualityCheckedAudio {
                audio_files: audio_files.to_vec(),
                pre_split_dirs: Vec::new(),
            }))
    }
}

#[test]
fn quality_replacement_album_is_organized_without_chapter_split()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("source.flac");
    fs::write(&audio, b"audio")?;
    let replacement = dir.path().join("replacement-album");
    fs::create_dir(&replacement)?;
    let mut operations = RecordingOperations {
        quality_result: Some(QualityCheckedAudio {
            audio_files: Vec::new(),
            pre_split_dirs: vec![replacement.clone()],
        }),
        ..Default::default()
    };
    let result = run_workflow(
        &request(audio.to_string_lossy().into_owned(), dir.path()),
        &WorkflowOptions::default(),
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert_eq!(operations.quality_calls, 1);
    assert!(operations.split_sources.is_empty());
    assert_eq!(
        operations.organized.as_slice(),
        std::slice::from_ref(&replacement)
    );
    assert_eq!(result.split_dirs, [replacement]);
    assert!(audio.exists());
    Ok(())
}

#[test]
fn rejected_chapters_make_the_source_a_single() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("album.flac");
    fs::write(&audio, b"audio")?;
    fs::write(
        dir.path().join("album.chapters.txt"),
        "0:00 First\n1:00 Second\n",
    )?;
    let mut operations = RecordingOperations {
        review_decision: Some(ChapterReview::Reject),
        ..Default::default()
    };
    let options = WorkflowOptions {
        review: true,
        ..WorkflowOptions::default()
    };
    let result = run_workflow(
        &request(audio.to_string_lossy().into_owned(), dir.path()),
        &options,
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert!(result.plan.albums.is_empty());
    assert_eq!(result.plan.singles.as_slice(), std::slice::from_ref(&audio));
    assert_eq!(operations.reviewed.as_slice(), std::slice::from_ref(&audio));
    assert!(operations.split_sources.is_empty());
    assert_eq!(operations.organized, [audio]);
    Ok(())
}

#[test]
fn edited_chapters_reach_the_split_task() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("album.flac");
    fs::write(&audio, b"audio")?;
    fs::write(
        dir.path().join("album.chapters.txt"),
        "0:00 First\n1:00 Second\n",
    )?;
    let edited = vec![muzik_core::chapters::Chapter {
        index: 1,
        start: 0,
        end: None,
        title: "Edited".into(),
    }];
    let mut operations = RecordingOperations {
        review_decision: Some(ChapterReview::Edit(edited.clone())),
        ..Default::default()
    };
    let options = WorkflowOptions {
        review: true,
        ..WorkflowOptions::default()
    };
    let result = run_workflow(
        &request(audio.to_string_lossy().into_owned(), dir.path()),
        &options,
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert_eq!(result.plan.albums[0].chapters, edited);
    assert_eq!(operations.split_chapters, [edited]);
    Ok(())
}

fn request(raw: String, root: &Path) -> WorkflowRequest {
    WorkflowRequest {
        raw,
        output: root.join("downloads"),
        splits: root.join("splits"),
    }
}

#[test]
fn classifies_playlist_before_video_and_reads_local_exports()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let export = dir.path().join("spotify.csv");
    fs::write(&export, "Track Name,Artist Name\n")?;
    assert_eq!(
        classify_input(&export.to_string_lossy()),
        WorkflowInput::SpotifyExport(export)
    );
    assert!(matches!(
        classify_input("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL123"),
        WorkflowInput::YoutubePlaylist { playlist_id, .. } if playlist_id == "PL123"
    ));
    assert!(matches!(
        classify_input("https://youtu.be/dQw4w9WgXcQ"),
        WorkflowInput::YoutubeVideo { video_id, .. } if video_id == "dQw4w9WgXcQ"
    ));
    Ok(())
}

#[test]
fn local_album_splits_then_organizes_split_dir() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("album.flac");
    fs::write(&audio, b"audio")?;
    fs::write(
        dir.path().join("album.chapters.txt"),
        "0:00 First\n1:00 Second\n",
    )?;
    let mut operations = RecordingOperations::default();
    let result = run_workflow(
        &request(audio.to_string_lossy().into_owned(), dir.path()),
        &WorkflowOptions::default(),
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert_eq!(result.plan.albums.len(), 1);
    assert_eq!(operations.split_sources, vec![audio]);
    assert_eq!(operations.organized, vec![dir.path().join("splits/album")]);
    Ok(())
}

#[test]
fn dry_run_plans_without_download_or_file_changes() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut operations = RecordingOperations::default();
    let options = WorkflowOptions {
        dry_run: true,
        ..WorkflowOptions::default()
    };
    let result = run_workflow(
        &request("https://youtu.be/dQw4w9WgXcQ".into(), dir.path()),
        &options,
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert!(operations.downloads.is_empty());
    assert!(operations.organized.is_empty());
    assert!(result.organize_targets.is_empty());
    Ok(())
}

#[test]
fn video_reuses_existing_file_and_groups_singles() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let downloads = dir.path().join("downloads");
    fs::create_dir_all(&downloads)?;
    let audio = downloads.join("Track [dQw4w9WgXcQ].mp3");
    fs::write(&audio, b"audio")?;
    let mut operations = RecordingOperations::default();
    let result = run_workflow(
        &request("https://youtu.be/dQw4w9WgXcQ".into(), dir.path()),
        &WorkflowOptions::default(),
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert!(operations.downloads.is_empty());
    assert_eq!(result.plan.singles, vec![audio.clone()]);
    assert_eq!(operations.organized, vec![audio]);
    Ok(())
}

#[test]
fn soulseek_source_and_cancellation_stop_before_import() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("song.flac");
    fs::write(&audio, b"audio")?;
    let mut operations = RecordingOperations {
        soulseek_files: vec![audio.clone()],
        ..RecordingOperations::default()
    };
    let options = WorkflowOptions {
        audio_source: AudioSource::Soulseek,
        ..WorkflowOptions::default()
    };
    let result = run_workflow(
        &request("Artist - Song".into(), dir.path()),
        &options,
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert_eq!(result.organize_targets, vec![audio.clone()]);
    assert!(operations.downloads.is_empty());

    let cancelled = AtomicBool::new(true);
    operations.organized.clear();
    let failure = run_workflow(
        &request(audio.to_string_lossy().into_owned(), dir.path()),
        &options,
        &mut operations,
        &cancelled,
    );
    assert!(matches!(failure, Err(Error::Cancelled)));
    assert!(operations.organized.is_empty());
    cancelled.store(false, Ordering::SeqCst);
    Ok(())
}

#[test]
fn video_honors_soulseek_then_youtube_fallback() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("song.flac");
    fs::write(&audio, b"audio")?;
    let url = "https://youtu.be/dQw4w9WgXcQ";
    let mut operations = RecordingOperations {
        soulseek_files: vec![audio.clone()],
        downloaded_files: vec![audio],
        ..RecordingOperations::default()
    };
    let options = WorkflowOptions {
        audio_source: AudioSource::Soulseek,
        fallback: AudioFallback::Youtube,
        no_organize: true,
        ..WorkflowOptions::default()
    };
    run_workflow(
        &request(url.into(), dir.path()),
        &options,
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert_eq!(operations.soulseek_queries, [url]);
    assert!(operations.downloads.is_empty());

    operations.soulseek_files.clear();
    run_workflow(
        &request(url.into(), dir.path()),
        &options,
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert_eq!(operations.soulseek_queries, [url, url]);
    assert_eq!(operations.downloads, [url]);
    Ok(())
}

#[test]
fn search_falls_back_to_youtube_when_soulseek_is_empty() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("song.flac");
    fs::write(&audio, b"audio")?;
    let mut operations = RecordingOperations {
        downloaded_files: vec![audio],
        ..RecordingOperations::default()
    };
    let options = WorkflowOptions {
        audio_source: AudioSource::Soulseek,
        fallback: AudioFallback::Youtube,
        no_organize: true,
        ..WorkflowOptions::default()
    };
    run_workflow(
        &request("Artist - Song".into(), dir.path()),
        &options,
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert_eq!(operations.soulseek_queries, ["Artist - Song"]);
    assert_eq!(operations.downloads, ["Artist - Song"]);
    Ok(())
}

#[test]
fn no_split_keeps_chaptered_audio_as_one_import_target() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("album.flac");
    fs::write(&audio, b"audio")?;
    fs::write(dir.path().join("album.chapters.txt"), "0:00 First\n")?;
    let plan = plan_audio_processing(std::slice::from_ref(&audio), &[], true)?;
    assert!(plan.albums.is_empty());
    assert_eq!(plan.singles, vec![audio]);
    Ok(())
}

#[test]
fn events_report_work_and_callback_can_cancel_before_split()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("album.flac");
    fs::write(&audio, b"audio")?;
    fs::write(dir.path().join("album.chapters.txt"), "0:00 First\n")?;
    let cancelled = AtomicBool::new(false);
    let mut operations = RecordingOperations::default();
    let mut events = Vec::new();
    let result = run_workflow_with_events(
        &request(audio.to_string_lossy().into_owned(), dir.path()),
        &WorkflowOptions::default(),
        &mut operations,
        &cancelled,
        &mut |event| {
            if matches!(event, WorkflowEvent::PlanReady { .. }) {
                cancelled.store(true, Ordering::SeqCst);
            }
            events.push(event);
        },
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert!(matches!(
        events.first(),
        Some(WorkflowEvent::InputClassified(_))
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        WorkflowEvent::PlanReady {
            albums: 1,
            singles: 0
        }
    )));
    assert!(operations.split_sources.is_empty());
    Ok(())
}

#[test]
fn events_include_track_progress_and_completion() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let audio = dir.path().join("album.flac");
    fs::write(&audio, b"audio")?;
    fs::write(dir.path().join("album.chapters.txt"), "0:00 First\n")?;
    let mut events = Vec::new();
    run_workflow_with_events(
        &request(audio.to_string_lossy().into_owned(), dir.path()),
        &WorkflowOptions::default(),
        &mut RecordingOperations::default(),
        &AtomicBool::new(false),
        &mut |event| events.push(event),
    )?;
    assert!(events.iter().any(|event| matches!(
        event,
        WorkflowEvent::SplitProgress {
            progress: SplitProgress {
                completed: 1,
                total: 1,
                chapter_index: 1
            },
            ..
        }
    )));
    assert!(matches!(events.last(), Some(WorkflowEvent::Completed)));
    Ok(())
}
