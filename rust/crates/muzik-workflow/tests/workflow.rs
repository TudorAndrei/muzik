use muzik_workflow::{
    AudioSource, Error, SplitTask, WorkflowInput, WorkflowOperations, WorkflowOptions,
    WorkflowRequest, classify_input, plan_audio_processing, run_workflow,
};
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

    fn acquire_soulseek(&mut self, _query: &str) -> Result<Vec<PathBuf>, String> {
        Ok(self.soulseek_files.clone())
    }

    fn organize(&mut self, target: &Path, _options: &WorkflowOptions) -> Result<(), String> {
        self.organized.push(target.to_path_buf());
        Ok(())
    }

    fn split(&mut self, task: &SplitTask, _options: &WorkflowOptions) -> Result<(), String> {
        self.split_sources.push(task.source.clone());
        fs::create_dir_all(&task.output).map_err(|error| error.to_string())
    }
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
