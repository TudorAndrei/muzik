use muzik_core::{JobEvent, QualityPolicy};
use muzik_store::watchlist::jobs::{self, JobError, JobOptions, LoadedSource, Operations};
use muzik_store::watchlist::{
    ItemAction, ItemId, Playlist, ReconcileOptions, Repository, SourceKind, Stage, StageStatus,
    WatchItem,
};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn processed(item: &WatchItem) -> WatchItem {
    let mut updated = item.clone();
    for stage in [Stage::Download, Stage::Parse, Stage::Organize] {
        updated.set(stage, StageStatus::Complete);
    }
    for stage in [Stage::Quality, Stage::Split] {
        updated.set(stage, StageStatus::Skipped);
    }
    updated
}

struct Fake {
    processed: Vec<String>,
    cancel_after_first: bool,
}

impl Operations for Fake {
    fn load(&mut self, _: &Playlist) -> Result<LoadedSource, JobError> {
        Ok(LoadedSource {
            title: Some("Current title".into()),
            items: vec![card(1, "video_a"), card(2, "video_a"), card(3, "video_b")],
        })
    }

    fn process(
        &mut self,
        _: &Playlist,
        item: &WatchItem,
        _: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        let id = item
            .video_id
            .clone()
            .ok_or_else(|| JobError::Operation("video ID missing".into()))?;
        self.processed.push(id);
        if self.cancel_after_first && self.processed.len() == 2 {
            cancelled.store(true, Ordering::SeqCst);
            return Err(JobError::Cancelled);
        }
        Ok(processed(item))
    }
}

fn card(position: u64, id: &str) -> WatchItem {
    let mut item = WatchItem::new(position, id, SourceKind::Youtube);
    item.video_id = Some(id.into());
    item.video_url = Some(format!("https://www.youtube.com/watch?v={id}"));
    item
}

fn options(directory: &std::path::Path) -> JobOptions<'_> {
    JobOptions {
        reconcile: ReconcileOptions {
            output: directory,
            splits: directory,
            cache: directory,
            config: None,
            no_organize: false,
            no_split: false,
            quality_policy: QualityPolicy::Off,
        },
        output: directory,
        cache: directory,
        dry_run: false,
        playlist_id: None,
    }
}

fn repository_with(
    directory: &std::path::Path,
    items: Vec<WatchItem>,
) -> Result<Repository, Box<dyn std::error::Error>> {
    let repository = Repository::new(directory.join("muzik.db"));
    repository.add("https://www.youtube.com/playlist?list=PL123")?;
    repository.update(|document| {
        document
            .playlists
            .first_mut()
            .ok_or("the watchlist has no playlist")?
            .items = items;
        Ok(())
    })?;
    Ok(repository)
}

fn id(video_id: &str) -> ItemId {
    ItemId::new("PL123", 1, Some(video_id))
}

#[test]
fn refresh_keeps_prior_state_and_processes_each_video_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut saved = card(1, "video_a");
    saved.last_action = Some("retry".into());
    let repository = repository_with(directory.path(), vec![saved])?;
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
            .filter(|event| **event == JobEvent::WatchlistSaved)
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
        repository.load()?.playlists[0].processed_video_ids,
        ["video_a", "video_b"]
    );
    Ok(())
}

struct AsksOnFirst {
    processed: Vec<String>,
}

impl Operations for AsksOnFirst {
    fn load(&mut self, _: &Playlist) -> Result<LoadedSource, JobError> {
        Ok(LoadedSource {
            title: None,
            items: vec![card(1, "video_a"), card(2, "video_b")],
        })
    }

    fn process(
        &mut self,
        _: &Playlist,
        item: &WatchItem,
        _: ItemAction,
        _: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        let id = item.video_id.clone().unwrap_or_default();
        self.processed.push(id.clone());
        if id == "video_a" {
            return Err(JobError::Waiting {
                stage: Stage::Organize,
                question: json!({"kind": "import_match", "payload": {"task": {}}}),
            });
        }
        Ok(processed(item))
    }
}

#[test]
fn a_waiting_item_does_not_block_the_refresh() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = repository_with(directory.path(), Vec::new())?;
    let mut fake = AsksOnFirst {
        processed: Vec::new(),
    };
    let cancelled = AtomicBool::new(false);
    let result = jobs::refresh(
        &repository,
        options(directory.path()),
        &mut fake,
        &cancelled,
        &mut |_| {},
    )?;
    assert_eq!(fake.processed, ["video_a", "video_b"]);
    assert_eq!(result["summary"]["waiting_videos"], 1);
    assert_eq!(result["summary"]["failed_videos"], 0);
    let saved = repository.load()?;
    let waiting = saved.playlists[0].items[0].stage(Stage::Organize);
    assert_eq!(waiting.status, StageStatus::Waiting);
    assert_eq!(
        waiting.question.as_ref().map(|question| &question["kind"]),
        Some(&json!("import_match"))
    );
    assert_eq!(
        result["watchlist"]["playlists"][0]["items"][0]["summary"],
        "Waiting"
    );
    fake.processed.clear();
    jobs::refresh(
        &repository,
        options(directory.path()),
        &mut fake,
        &cancelled,
        &mut |_| {},
    )?;
    assert!(fake.processed.is_empty());
    assert_eq!(
        repository.load()?.playlists[0].items[0].status(Stage::Organize),
        StageStatus::Waiting
    );
    Ok(())
}

struct CallLog(Vec<String>);

impl Operations for CallLog {
    fn load(&mut self, playlist: &Playlist) -> Result<LoadedSource, JobError> {
        let id = &playlist.playlist_id;
        self.0.push(format!("load {id}"));
        Ok(LoadedSource {
            title: None,
            items: vec![card(1, &format!("video_{id}"))],
        })
    }

    fn process(
        &mut self,
        _: &Playlist,
        item: &WatchItem,
        _: ItemAction,
        _: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        self.0.push(format!(
            "process {}",
            item.video_id.as_deref().unwrap_or("")
        ));
        Ok(item.clone())
    }
}

#[test]
fn refresh_reads_every_playlist_before_it_processes_items() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("muzik.db"));
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

#[test]
fn refresh_of_one_source_reads_and_processes_only_that_source() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("muzik.db"));
    repository.add("https://www.youtube.com/playlist?list=PLone")?;
    repository.add("https://www.youtube.com/playlist?list=PLtwo")?;
    let mut log = CallLog(Vec::new());
    let mut only = options(directory.path());
    only.playlist_id = Some("PLtwo");
    jobs::refresh(
        &repository,
        only,
        &mut log,
        &AtomicBool::new(false),
        &mut |_| {},
    )?;
    assert_eq!(log.0, ["load PLtwo", "process video_PLtwo"]);
    let mut missing = options(directory.path());
    missing.playlist_id = Some("PLgone");
    assert!(jobs::refresh(
        &repository,
        missing,
        &mut log,
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .is_err());
    Ok(())
}

struct ActionFailure;

impl Operations for ActionFailure {
    fn load(&mut self, _: &Playlist) -> Result<LoadedSource, JobError> {
        Err(JobError::Operation("unused".into()))
    }

    fn process(
        &mut self,
        _: &Playlist,
        _: &WatchItem,
        _: ItemAction,
        _: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        Err(JobError::Operation("download failed".into()))
    }
}

#[test]
fn an_action_that_needs_a_choice_parks_the_item() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = repository_with(directory.path(), vec![card(1, "video_a")])?;
    let result = jobs::action(
        &repository,
        options(directory.path()),
        &id("video_a"),
        ItemAction::Run,
        &mut AsksOnFirst {
            processed: Vec::new(),
        },
        &AtomicBool::new(false),
    )?;
    assert_eq!(result["action"]["waiting_stage"], "organize");
    let saved = repository.load()?;
    let stage = saved.playlists[0].items[0].stage(Stage::Organize);
    assert_eq!(stage.status, StageStatus::Waiting);
    assert_eq!(
        stage.question.as_ref().map(|question| &question["kind"]),
        Some(&json!("import_match"))
    );
    Ok(())
}

#[test]
fn failed_action_saves_its_target_stage() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = repository_with(directory.path(), vec![card(1, "video_a")])?;
    let error = jobs::action(
        &repository,
        options(directory.path()),
        &id("video_a"),
        ItemAction::Run,
        &mut ActionFailure,
        &AtomicBool::new(false),
    );
    assert!(matches!(error, Err(JobError::Operation(_))));
    let item = &repository.load()?.playlists[0].items[0];
    assert_eq!(item.status(Stage::Download), StageStatus::Failed);
    assert_eq!(item.last_error.as_deref(), Some("download failed"));
    Ok(())
}

#[test]
fn cancellation_keeps_items_saved_before_the_stop() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = repository_with(directory.path(), Vec::new())?;
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
        repository.load()?.playlists[0].processed_video_ids,
        ["video_a"]
    );
    Ok(())
}

#[test]
fn action_checks_the_saved_item_identity() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = repository_with(directory.path(), vec![card(1, "video_a")])?;
    let mut fake = Fake {
        processed: Vec::new(),
        cancel_after_first: false,
    };
    let result = jobs::action(
        &repository,
        options(directory.path()),
        &id("other"),
        ItemAction::Run,
        &mut fake,
        &AtomicBool::new(false),
    );
    assert!(matches!(result, Err(JobError::Operation(_))));
    assert!(fake.processed.is_empty());
    Ok(())
}

#[test]
fn dry_run_preserves_saved_state_and_does_not_process_audio() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = repository_with(directory.path(), vec![card(1, "video_a")])?;
    let before = (repository.revision()?, repository.load()?);
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
    assert_eq!((repository.revision()?, repository.load()?), before);
    let result = jobs::action(
        &repository,
        options,
        &id("video_a"),
        ItemAction::DownloadAgain,
        &mut fake,
        &AtomicBool::new(false),
    )?;
    assert_eq!(result["action"]["dry_run"], true);
    assert_eq!((repository.revision()?, repository.load()?), before);
    assert!(fake.processed.is_empty());
    Ok(())
}

struct SplitFailure;

impl Operations for SplitFailure {
    fn load(&mut self, _: &Playlist) -> Result<LoadedSource, JobError> {
        Err(JobError::Operation("unused".into()))
    }

    fn process(
        &mut self,
        _: &Playlist,
        _: &WatchItem,
        _: ItemAction,
        _: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        Err(JobError::Failed {
            stage: Stage::Split,
            message: "split failed".into(),
        })
    }
}

#[test]
fn a_failure_marks_the_stage_that_failed() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = repository_with(directory.path(), vec![card(1, "video_a")])?;
    let error = jobs::run_item(
        &repository,
        options(directory.path()),
        &id("video_a"),
        ItemAction::Run,
        &mut SplitFailure,
        &AtomicBool::new(false),
    );
    assert!(matches!(error, Err(JobError::Failed { .. })));
    let item = &repository.load()?.playlists[0].items[0];
    assert_eq!(item.status(Stage::Split), StageStatus::Failed);
    assert_eq!(
        item.stage(Stage::Split).error.as_deref(),
        Some("split failed")
    );
    assert_eq!(item.status(Stage::Download), StageStatus::NotStarted);
    Ok(())
}

#[test]
fn sync_lists_pending_items_and_keeps_running_stages() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut running = card(1, "video_a");
    running.set(Stage::Download, StageStatus::Running);
    let repository = repository_with(directory.path(), vec![running])?;
    let mut fake = Fake {
        processed: Vec::new(),
        cancel_after_first: false,
    };
    let synced = jobs::sync(
        &repository,
        options(directory.path()),
        &mut fake,
        &AtomicBool::new(false),
        &mut |_| {},
    )?;
    assert!(fake.processed.is_empty());
    assert_eq!(
        synced.pending,
        [
            jobs::PendingItem {
                id: id("video_a"),
                title: "video_a".into(),
            },
            jobs::PendingItem {
                id: ItemId::new("PL123", 3, Some("video_b")),
                title: "video_b".into(),
            },
        ]
    );
    assert_eq!(
        repository.load()?.playlists[0].items[0].status(Stage::Download),
        StageStatus::Running
    );
    Ok(())
}

struct PrivateSecond;

impl Operations for PrivateSecond {
    fn load(&mut self, _: &Playlist) -> Result<LoadedSource, JobError> {
        let mut private = card(2, "video_b");
        private.unavailable = Some(true);
        Ok(LoadedSource {
            title: None,
            items: vec![card(1, "video_a"), private],
        })
    }

    fn process(
        &mut self,
        _: &Playlist,
        _: &WatchItem,
        _: ItemAction,
        _: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        Err(JobError::Operation("unused".into()))
    }
}

#[test]
fn a_private_video_is_not_queued_and_shows_as_unavailable() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = repository_with(directory.path(), Vec::new())?;
    let synced = jobs::sync(
        &repository,
        options(directory.path()),
        &mut PrivateSecond,
        &AtomicBool::new(false),
        &mut |_| {},
    )?;
    assert_eq!(
        synced
            .pending
            .iter()
            .map(|item| item.id.position)
            .collect::<Vec<_>>(),
        [1]
    );
    let visible =
        muzik_store::watchlist::view(&repository.load()?, directory.path(), directory.path())?;
    let private = &visible["playlists"][0]["items"][1];
    assert_eq!(private["summary"], "Unavailable");
    assert_eq!(private["primary_action"], Value::Null);
    assert_eq!(private["actions"]["retry"]["enabled"], false);
    Ok(())
}

#[test]
fn parallel_updates_keep_every_change() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = std::sync::Arc::new(repository_with(directory.path(), Vec::new())?);
    let workers: Vec<_> = (0..8)
        .map(|index| {
            let repository = std::sync::Arc::clone(&repository);
            std::thread::spawn(move || {
                repository.update(|document| {
                    document.playlists[0].mark_processed(&format!("video_{index}"), true);
                    Ok(())
                })
            })
        })
        .collect();
    for worker in workers {
        worker.join().map_err(|_| "worker panicked")??;
    }
    assert_eq!(repository.load()?.playlists[0].processed_video_ids.len(), 8);
    Ok(())
}

struct RepeatDownload;

impl Operations for RepeatDownload {
    fn load(&mut self, _: &Playlist) -> Result<LoadedSource, JobError> {
        Err(JobError::Operation("unused".into()))
    }

    fn process(
        &mut self,
        _: &Playlist,
        item: &WatchItem,
        _: ItemAction,
        _: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        let mut item = item.clone();
        item.set(Stage::Download, StageStatus::Complete);
        item.invalidate(&[Stage::Parse, Stage::Split, Stage::Organize]);
        Ok(item)
    }
}

#[test]
fn repeat_action_preserves_stale_stages_across_cached_reconciliation() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = repository_with(
        directory.path(),
        vec![card(1, "video_a"), card(2, "video_a")],
    )?;
    repository.update(|document| {
        document.playlists[0].mark_processed("video_a", true);
        Ok(())
    })?;
    std::fs::write(
        directory.path().join("playlist_PL123.json"),
        serde_json::to_vec(&json!({
            "videos":{"video_a":{"status":"organized","audio_file":"old.flac"}}
        }))?,
    )?;
    jobs::action(
        &repository,
        options(directory.path()),
        &id("video_a"),
        ItemAction::DownloadAgain,
        &mut RepeatDownload,
        &AtomicBool::new(false),
    )?;
    let mut saved = repository.load()?;
    muzik_store::watchlist::reconcile(&mut saved, options(directory.path()).reconcile)?;
    assert!(saved.playlists[0].processed_video_ids.is_empty());
    for (index, position) in [(0, 1), (1, 2)] {
        let item = &saved.playlists[0].items[index];
        assert_eq!(item.position, position);
        assert_eq!(item.status(Stage::Download), StageStatus::Complete);
        assert_eq!(item.status(Stage::Organize), StageStatus::Stale);
    }
    Ok(())
}
