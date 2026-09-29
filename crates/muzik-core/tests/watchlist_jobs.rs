use muzik_core::watchlist::jobs::{
    self, ItemSelection, JobError, JobOptions, LoadedSource, Operations,
};
use muzik_core::watchlist::{ReconcileOptions, Repository};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

struct Fake {
    processed: Vec<String>,
    cancel_after_first: bool,
}

impl Operations for Fake {
    fn load(&mut self, _: &Value) -> Result<LoadedSource, JobError> {
        Ok(LoadedSource {
            title: Some("Current title".into()),
            items: vec![card(1, "video_a"), card(2, "video_a"), card(3, "video_b")],
        })
    }

    fn process(
        &mut self,
        _: &Value,
        item: &Value,
        _: &str,
        cancelled: &AtomicBool,
    ) -> Result<Value, JobError> {
        let id = item["video_id"]
            .as_str()
            .ok_or_else(|| JobError::Operation("video ID missing".into()))?;
        self.processed.push(id.to_owned());
        if self.cancel_after_first && self.processed.len() == 2 {
            cancelled.store(true, Ordering::SeqCst);
            return Err(JobError::Cancelled);
        }
        let mut updated = item.clone();
        for stage in ["download", "parse", "organize"] {
            updated["stages"][stage]["status"] = json!("complete");
        }
        for stage in ["quality", "split"] {
            updated["stages"][stage]["status"] = json!("skipped");
        }
        Ok(updated)
    }
}

fn card(position: usize, id: &str) -> Value {
    json!({"position":position,"title":id,"video_id":id,"video_url":format!("https://www.youtube.com/watch?v={id}"),"kind":"youtube"})
}

fn options<'a>(directory: &'a std::path::Path) -> JobOptions<'a> {
    JobOptions {
        reconcile: ReconcileOptions {
            output: directory,
            splits: directory,
            cache: directory,
            config: None,
            no_organize: false,
            no_split: false,
            quality_policy: "off",
        },
        output: directory,
        cache: directory,
        dry_run: false,
    }
}

#[test]
fn refresh_keeps_prior_state_and_processes_each_video_once(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("watchlist.json"));
    repository.add("https://www.youtube.com/playlist?list=PL123")?;
    let mut saved = repository.load()?;
    saved["playlists"][0]["items"] = json!([card(1, "video_a")]);
    saved["playlists"][0]["items"][0]["last_action"] = json!("retry");
    repository.save(saved)?;
    let mut fake = Fake {
        processed: Vec::new(),
        cancel_after_first: false,
    };
    let cancelled = AtomicBool::new(false);
    let mut events = Vec::new();
    let result = jobs::refresh(
        &repository,
        options(directory.path()),
        &mut fake,
        &cancelled,
        &mut |event| events.push(event),
    )?;
    assert_eq!(fake.processed, ["video_a", "video_b"]);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "watchlist_saved")
            .count(),
        3
    );
    assert_eq!(result["summary"]["completed_videos"], 2);
    assert_eq!(
        result["watchlist"]["playlists"][0]["title"],
        "Current title"
    );
    assert_eq!(
        result["watchlist"]["playlists"][0]["items"][1]["stages"]["download"]["status"],
        "complete"
    );
    assert_eq!(
        repository.load()?["playlists"][0]["processed_video_ids"],
        json!(["video_a", "video_b"])
    );
    Ok(())
}

struct CallLog(Vec<String>);

impl Operations for CallLog {
    fn load(&mut self, playlist: &Value) -> Result<LoadedSource, JobError> {
        let id = playlist["playlist_id"].as_str().unwrap_or("");
        self.0.push(format!("load {id}"));
        Ok(LoadedSource {
            title: None,
            items: vec![card(1, &format!("video_{id}"))],
        })
    }

    fn process(
        &mut self,
        _: &Value,
        item: &Value,
        _: &str,
        _: &AtomicBool,
    ) -> Result<Value, JobError> {
        self.0.push(format!(
            "process {}",
            item["video_id"].as_str().unwrap_or("")
        ));
        Ok(item.clone())
    }
}

#[test]
fn refresh_reads_every_playlist_before_it_processes_items() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("watchlist.json"));
    repository.add("https://www.youtube.com/playlist?list=PLone")?;
    repository.add("https://www.youtube.com/playlist?list=PLtwo")?;
    let mut log = CallLog(Vec::new());
    jobs::refresh(
        &repository,
        options(directory.path()),
        &mut log,
        &AtomicBool::new(false),
        &mut |_| {},
    )?;
    assert_eq!(
        log.0,
        [
            "load PLone",
            "load PLtwo",
            "process video_PLone",
            "process video_PLtwo"
        ]
    );
    Ok(())
}

struct ActionFailure;

impl Operations for ActionFailure {
    fn load(&mut self, _: &Value) -> Result<LoadedSource, JobError> {
        Err(JobError::Operation("unused".into()))
    }

    fn process(
        &mut self,
        _: &Value,
        _: &Value,
        _: &str,
        _: &AtomicBool,
    ) -> Result<Value, JobError> {
        Err(JobError::Operation("download failed".into()))
    }
}

#[test]
fn failed_action_saves_its_target_stage() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("watchlist.json"));
    repository.add("https://www.youtube.com/playlist?list=PL123")?;
    let mut saved = repository.load()?;
    saved["playlists"][0]["items"] = json!([card(1, "video_a")]);
    repository.save(saved)?;
    let error = jobs::action(
        &repository,
        options(directory.path()),
        ItemSelection {
            playlist_id: "PL123",
            position: 1,
            video_id: Some("video_a"),
            action: "run",
        },
        &mut ActionFailure,
        &AtomicBool::new(false),
    );
    assert!(matches!(error, Err(JobError::Operation(_))));
    let saved = repository.load()?;
    assert_eq!(
        saved["playlists"][0]["items"][0]["stages"]["download"]["status"],
        "failed"
    );
    assert_eq!(
        saved["playlists"][0]["items"][0]["last_error"],
        "download failed"
    );
    Ok(())
}

#[test]
fn cancellation_keeps_items_saved_before_the_stop() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("watchlist.json"));
    repository.add("https://www.youtube.com/playlist?list=PL123")?;
    let mut fake = Fake {
        processed: Vec::new(),
        cancel_after_first: true,
    };
    let cancelled = AtomicBool::new(false);
    let result = jobs::refresh(
        &repository,
        options(directory.path()),
        &mut fake,
        &cancelled,
        &mut |_| {},
    );
    assert!(matches!(result, Err(JobError::Cancelled)));
    assert_eq!(
        repository.load()?["playlists"][0]["processed_video_ids"],
        json!(["video_a"])
    );
    Ok(())
}

#[test]
fn action_checks_the_saved_item_identity() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("watchlist.json"));
    repository.add("https://www.youtube.com/playlist?list=PL123")?;
    let mut saved = repository.load()?;
    saved["playlists"][0]["items"] = json!([card(1, "video_a")]);
    repository.save(saved)?;
    let mut fake = Fake {
        processed: Vec::new(),
        cancel_after_first: false,
    };
    let result = jobs::action(
        &repository,
        options(directory.path()),
        ItemSelection {
            playlist_id: "PL123",
            position: 1,
            video_id: Some("other"),
            action: "run",
        },
        &mut fake,
        &AtomicBool::new(false),
    );
    assert!(matches!(result, Err(JobError::Operation(_))));
    assert!(fake.processed.is_empty());
    Ok(())
}

#[test]
fn dry_run_preserves_saved_state_and_does_not_process_audio(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("watchlist.json"));
    repository.add("https://www.youtube.com/playlist?list=PL123")?;
    let mut saved = repository.load()?;
    saved["playlists"][0]["items"] = json!([card(1, "video_a")]);
    repository.save(saved)?;
    let before = std::fs::read(repository.path())?;
    let mut fake = Fake {
        processed: Vec::new(),
        cancel_after_first: false,
    };
    let mut options = options(directory.path());
    options.dry_run = true;
    let result = jobs::refresh(
        &repository,
        options,
        &mut fake,
        &AtomicBool::new(false),
        &mut |_| {},
    )?;
    assert_eq!(result["summary"]["pending_videos"], 2);
    assert_eq!(result["summary"]["completed_videos"], 0);
    assert_eq!(std::fs::read(repository.path())?, before);
    let result = jobs::action(
        &repository,
        options,
        ItemSelection {
            playlist_id: "PL123",
            position: 1,
            video_id: Some("video_a"),
            action: "download_again",
        },
        &mut fake,
        &AtomicBool::new(false),
    )?;
    assert_eq!(result["action"]["dry_run"], true);
    assert_eq!(std::fs::read(repository.path())?, before);
    assert!(fake.processed.is_empty());
    Ok(())
}

struct RepeatDownload;

impl Operations for RepeatDownload {
    fn load(&mut self, _: &Value) -> Result<LoadedSource, JobError> {
        Err(JobError::Operation("unused".into()))
    }
    fn process(
        &mut self,
        _: &Value,
        item: &Value,
        _: &str,
        _: &AtomicBool,
    ) -> Result<Value, JobError> {
        let mut item = item.clone();
        item["stages"]["download"]["status"] = json!("complete");
        for stage in ["parse", "split", "organize"] {
            item["stages"][stage]["status"] = json!("stale");
        }
        Ok(item)
    }
}

#[test]
fn repeat_action_preserves_stale_stages_across_cached_reconciliation(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("watchlist.json"));
    repository.add("https://www.youtube.com/playlist?list=PL123")?;
    let mut saved = repository.load()?;
    saved["playlists"][0]["items"] = json!([card(1, "video_a"), card(2, "video_a")]);
    saved["playlists"][0]["processed_video_ids"] = json!(["video_a"]);
    repository.save(saved)?;
    std::fs::write(
        directory.path().join("playlist_PL123.json"),
        serde_json::to_vec(&json!({
            "videos":{"video_a":{"status":"organized","audio_file":"old.flac"}}
        }))?,
    )?;
    jobs::action(
        &repository,
        options(directory.path()),
        ItemSelection {
            playlist_id: "PL123",
            position: 1,
            video_id: Some("video_a"),
            action: "download_again",
        },
        &mut RepeatDownload,
        &AtomicBool::new(false),
    )?;
    let mut saved = repository.load()?;
    muzik_core::watchlist::reconcile(&mut saved, options(directory.path()).reconcile)?;
    assert_eq!(saved["playlists"][0]["processed_video_ids"], json!([]));
    for (index, position) in [(0, 1), (1, 2)] {
        let item = &saved["playlists"][0]["items"][index];
        assert_eq!(item["position"], position);
        assert_eq!(item["stages"]["download"]["status"], "complete");
        assert_eq!(item["stages"]["organize"]["status"], "stale");
    }
    Ok(())
}
