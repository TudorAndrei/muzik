use muzik_workflow::playlist::{
    SpotifyTrack, load_spotify_export, run_spotify_export, run_youtube_playlist,
};
use muzik_workflow::{
    SplitTask, WorkflowOperations, WorkflowOptions, WorkflowRequest, run_workflow,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Default)]
struct Operations {
    playlist_ids: Vec<String>,
    calls: Vec<String>,
    fail_video: Option<String>,
    spotify_file: Option<PathBuf>,
}

impl WorkflowOperations for Operations {
    fn download_youtube(
        &mut self,
        url: &str,
        output: &Path,
        _force: bool,
    ) -> Result<Vec<PathBuf>, String> {
        let id = url.split("v=").last().unwrap_or_default();
        self.calls.push(format!("download:{id}"));
        if self.fail_video.as_deref() == Some(id) {
            return Err("video unavailable".into());
        }
        fs::create_dir_all(output).map_err(|error| error.to_string())?;
        let file = output.join(format!("track [{id}].mp3"));
        fs::write(&file, b"audio").map_err(|error| error.to_string())?;
        Ok(vec![file])
    }

    fn acquire_soulseek(&mut self, query: &str) -> Result<Vec<PathBuf>, String> {
        self.calls.push(format!("soulseek:{query}"));
        Ok(self.spotify_file.iter().cloned().collect())
    }

    fn youtube_playlist_video_ids(&mut self, _url: &str) -> Result<Vec<String>, String> {
        self.calls.push("list".into());
        Ok(self.playlist_ids.clone())
    }

    fn acquire_spotify_track(&mut self, track: &SpotifyTrack) -> Result<Vec<PathBuf>, String> {
        self.calls.push(format!("spotify:{}", track.title));
        Ok(self.spotify_file.iter().cloned().collect())
    }

    fn soulseek_ready(&self) -> bool {
        true
    }

    fn organize(&mut self, target: &Path, _options: &WorkflowOptions) -> Result<(), String> {
        self.calls.push(format!("organize:{}", target.display()));
        Ok(())
    }

    fn split(&mut self, _task: &SplitTask, _options: &WorkflowOptions) -> Result<(), String> {
        Ok(())
    }
}

fn request(root: &Path) -> WorkflowRequest {
    WorkflowRequest {
        raw: String::new(),
        output: root.join("downloads"),
        splits: root.join("splits"),
    }
}

#[test]
fn youtube_playlist_keeps_order_continues_after_failure_and_resumes()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut operations = Operations {
        playlist_ids: vec!["first".into(), "second".into(), "third".into()],
        fail_video: Some("second".into()),
        ..Operations::default()
    };
    let options = WorkflowOptions::default();
    let cancelled = AtomicBool::new(false);
    let run = || request(dir.path());
    let first = run_youtube_playlist(
        &run(),
        &options,
        &mut operations,
        &cancelled,
        "PL1",
        "https://youtube.com/playlist?list=PL1",
        &mut |_| {},
    )?;
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.completed)
            .collect::<Vec<_>>(),
        vec![true, false, true]
    );
    assert_eq!(first.processing.plan.singles.len(), 2);
    assert_eq!(
        operations
            .calls
            .iter()
            .filter(|call| call.starts_with("download:"))
            .cloned()
            .collect::<Vec<_>>(),
        vec!["download:first", "download:second", "download:third"]
    );

    operations.calls.clear();
    operations.fail_video = None;
    let second = run_youtube_playlist(
        &run(),
        &options,
        &mut operations,
        &cancelled,
        "PL1",
        "https://youtube.com/playlist?list=PL1",
        &mut |_| {},
    )?;
    assert!(second.items.iter().all(|item| item.completed));
    assert_eq!(second.processing.plan.singles.len(), 1);
    assert_eq!(
        operations
            .calls
            .iter()
            .filter(|call| call.starts_with("download:"))
            .cloned()
            .collect::<Vec<_>>(),
        vec!["download:second"]
    );
    Ok(())
}

#[test]
fn spotify_json_processes_repeated_tracks_in_order_and_resumes()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let export = dir.path().join("spotify.json");
    fs::write(
        &export,
        r#"{"version":1,"source":"spotify","type":"playlist","id":"p1","title":"List","entries":[{"title":"One","artist":"A","source_id":"spotify:track:same","index":1},{"title":"Two","artist":"B","source_id":"spotify:track:same","index":2}]}"#,
    )?;
    let audio = dir.path().join("song.mp3");
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../muzik-tags/tests/fixtures/blank.mp3"),
        &audio,
    )?;
    let mut operations = Operations {
        spotify_file: Some(audio.clone()),
        ..Operations::default()
    };
    let options = WorkflowOptions {
        audio_source: muzik_workflow::AudioSource::Soulseek,
        ..WorkflowOptions::default()
    };
    let cancelled = AtomicBool::new(false);
    let first = run_spotify_export(
        &request(dir.path()),
        &options,
        &mut operations,
        &cancelled,
        &export,
        &mut |_| {},
    )?;
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["spotify:track:same#0", "spotify:track:same#1"]
    );
    assert_eq!(first.processing.plan.singles.len(), 2);
    let tags = muzik_tags::read(&audio, &[])?;
    assert_eq!(tags.fields.get("title").map(String::as_str), Some("Two"));
    assert_eq!(tags.fields.get("artist").map(String::as_str), Some("B"));
    operations.calls.clear();
    let second = run_spotify_export(
        &request(dir.path()),
        &options,
        &mut operations,
        &cancelled,
        &export,
        &mut |_| {},
    )?;
    assert!(second.processing.plan.singles.is_empty());
    assert!(
        !operations
            .calls
            .iter()
            .any(|call| call.starts_with("spotify:"))
    );
    Ok(())
}

#[test]
fn spotify_csv_reads_quoted_names_and_rejects_duplicate_positions()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let export = dir.path().join("spotify.csv");
    fs::write(
        &export,
        "track_name,artist_name,spotify_track_id,position\n\"One, Two\",Artist,abc,1\n",
    )?;
    let playlist = load_spotify_export(&export)?;
    assert_eq!(playlist.tracks[0].title, "One, Two");
    fs::write(
        &export,
        "track_name,artist_name,spotify_track_id,position\nOne,Artist,abc,1\nTwo,Artist,def,1\n",
    )?;
    assert!(load_spotify_export(&export).is_err());
    Ok(())
}

#[test]
fn spotify_json_rejects_an_invalid_track_position() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let export = dir.path().join("spotify.json");
    fs::write(
        &export,
        r#"{"version":1,"source":"spotify","type":"playlist","id":"p1","title":"List","entries":[{"title":"One","artist":"A","index":"wrong"}]}"#,
    )?;
    assert!(load_spotify_export(&export).is_err());
    Ok(())
}

#[test]
fn main_workflow_routes_a_youtube_playlist() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let mut request = request(dir.path());
    request.raw = "https://www.youtube.com/playlist?list=PL1".into();
    let mut operations = Operations {
        playlist_ids: vec!["first".into()],
        ..Operations::default()
    };
    let result = run_workflow(
        &request,
        &WorkflowOptions::default(),
        &mut operations,
        &AtomicBool::new(false),
    )?;
    assert_eq!(result.plan.singles.len(), 1);
    assert!(operations.calls.contains(&"list".into()));
    Ok(())
}
