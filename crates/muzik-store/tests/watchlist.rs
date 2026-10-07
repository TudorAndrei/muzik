use muzik_core::QualityPolicy;
use muzik_store::watchlist::{
    bandcamp_source, import_cache, reconcile, view, CheckedWrite, ReconcileOptions, Repository,
    SourceKind, Stage, StageStatus, WatchItem, Watchlist,
};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn reconciled(
    document: Value,
    options: ReconcileOptions<'_>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let mut document = Watchlist::from_value(document)?;
    reconcile(&mut document, options)?;
    Ok(document.to_value())
}

fn imported(
    repository: &Repository,
    document: Value,
    options: ReconcileOptions<'_>,
) -> Result<Value, Box<dyn std::error::Error>> {
    repository.save(&Watchlist::from_value(document)?)?;
    assert!(import_cache(repository, options)?);
    let mut document = repository.load()?;
    reconcile(&mut document, options)?;
    Ok(document.to_value())
}

fn options<'a>(output: &'a Path, splits: &'a Path, cache: &'a Path) -> ReconcileOptions<'a> {
    ReconcileOptions {
        output,
        splits,
        cache,
        config: None,
        no_organize: false,
        no_split: false,
        quality_policy: QualityPolicy::Off,
    }
}

#[test]
fn reconcile_fills_finished_stages_and_keeps_the_waiting_one() -> TestResult {
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("cache");
    let output = directory.path().join("downloads");
    let splits = directory.path().join("splits");
    fs::create_dir(&cache)?;
    fs::create_dir(&output)?;
    fs::create_dir(&splits)?;
    let split = splits.join("Song [abcdefghijk]");
    fs::create_dir(&split)?;
    let question = json!({"kind": "import_match", "payload": {"task": {}}});
    let document = json!({"version": 3, "playlists": [{
        "playlist_id": "PL1", "url": "https://www.youtube.com/playlist?list=PL1",
        "processed_video_ids": [],
        "items": [{"position": 1, "title": "Song", "video_id": "abcdefghijk",
            "video_url": "https://www.youtube.com/watch?v=abcdefghijk",
            "stages": {"organize": {"status": "waiting", "question": question}}}]
    }]});
    fs::write(
        cache.join("playlist_PL1.json"),
        serde_json::to_vec(&json!({"videos": {"abcdefghijk": {
            "status": "split", "audio_file": output.join("Song.flac"), "split_dir": split
        }}}))?,
    )?;
    let repository = Repository::new(directory.path().join("muzik.db"));
    let document = imported(&repository, document, options(&output, &splits, &cache))?;
    let stages = &document["playlists"][0]["items"][0]["stages"];
    assert_eq!(stages["download"]["status"], "complete");
    assert_eq!(stages["split"]["status"], "complete");
    assert_eq!(stages["organize"]["status"], "waiting");
    assert_eq!(stages["organize"]["question"], question);
    assert_eq!(document["playlists"][0]["processed_video_ids"], json!([]));
    Ok(())
}

#[test]
fn imports_old_watchlist_file_and_preserves_saved_item_state() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("watchlist.json");
    let backup = directory.path().join("watchlist.json.migrated");
    let old = json!({
        "version": 1,
        "playlists": [{
            "playlist_id": "PL1",
            "url": "https://www.youtube.com/playlist?list=PL1",
            "items": [{
                "position": 1,
                "title": "Song",
                "video_id": "abcdefghijk",
                "stages": {
                    "download": {"status": "complete", "path": "/music/song.flac"},
                    "parse": {"status": "complete"},
                    "split": {"status": "skipped"},
                    "organize": {"status": "complete"}
                }
            }],
            "processed_video_ids": ["abcdefghijk", "abcdefghijk"]
        }]
    });
    fs::write(&path, serde_json::to_vec(&old)?)?;
    let repository = Repository::new(directory.path().join("muzik.db")).with_legacy(path.clone());

    let loaded = repository.load()?;
    let value = loaded.to_value();
    let item = &value["playlists"][0]["items"][0];
    assert_eq!(value["version"], 3);
    assert_eq!(item["stages"]["quality"]["status"], "not_started");
    assert_eq!(item["stages"]["download"]["path"], "/music/song.flac");
    assert_eq!(item["stages"]["parse"]["status"], "complete");
    assert_eq!(item["stages"]["split"]["status"], "skipped");
    assert_eq!(
        value["playlists"][0]["processed_video_ids"],
        json!(["abcdefghijk"])
    );
    assert!(!path.exists());
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&backup)?)?, old);

    let revision = repository.revision()?;
    repository.save(&loaded)?;
    assert_eq!(repository.revision()?, revision);
    assert_eq!(repository.load()?, loaded);
    Ok(())
}

#[test]
fn the_bandcamp_collection_is_added_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("muzik.db"));
    let source = bandcamp_source("listener");
    assert!(repository.ensure(&source)?);
    let revision = repository.revision()?;
    assert!(!repository.ensure(&source)?);
    assert_eq!(repository.revision()?, revision);
    let saved = repository.load()?.to_value();
    assert_eq!(saved["playlists"].as_array().map(Vec::len), Some(1));
    assert_eq!(saved["playlists"][0]["kind"], "bandcamp");
    assert_eq!(
        saved["playlists"][0]["url"],
        "https://bandcamp.com/listener"
    );
    Ok(())
}

#[test]
fn edits_saved_sources_without_losing_item_state() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("config/muzik.db"));
    let added = repository.add("https://www.youtube.com/watch?v=abcdefghijk&list=PL_ONE")?;
    assert_eq!(added.playlist_id, "PL_ONE");
    assert_eq!(added.url, "https://www.youtube.com/playlist?list=PL_ONE");
    assert!(repository
        .add("https://youtube.com/playlist?list=PL_ONE")
        .is_err());

    let mut saved = repository.load()?.to_value();
    saved["playlists"][0]["items"] = json!([{
        "position": 1,
        "title": "Song",
        "stages": {"download": {"status": "complete", "path": "/music/song.flac"}}
    }]);
    repository.save(&Watchlist::from_value(saved)?)?;
    assert!(repository.rename("PL_ONE", " Jazz albums ")?);
    let saved = repository.load()?.to_value();
    assert_eq!(saved["playlists"][0]["title"], "Jazz albums");
    assert_eq!(
        saved["playlists"][0]["items"][0]["stages"]["download"]["path"],
        "/music/song.flac"
    );

    let liked = repository.add("liked")?;
    assert_eq!(liked.playlist_id, "spotify:liked");
    assert_eq!(liked.title.as_deref(), Some("Liked Songs"));
    let spotify =
        repository.add("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=1")?;
    assert_eq!(
        spotify.playlist_id,
        "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"
    );
    assert!(repository.remove("spotify:liked")?);
    assert_eq!(repository.load()?.playlists.len(), 2);
    Ok(())
}

#[test]
fn rejects_an_invalid_old_file_without_moving_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("watchlist.json");
    let invalid = r#"{"version":99,"playlists":[]}"#;
    fs::write(&path, invalid)?;
    let repository = Repository::new(directory.path().join("muzik.db")).with_legacy(path.clone());

    assert!(repository
        .add("https://youtube.com/playlist?list=PL_NEW")
        .is_err());
    assert_eq!(fs::read_to_string(path)?, invalid);
    Ok(())
}

#[test]
fn view_adds_card_actions_and_cached_thumbnail_without_saving() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("muzik.db"));
    let cache = directory.path().join("cache");
    fs::create_dir(&cache)?;
    fs::write(cache.join("yt_thumbnail_abcdefghijk.jpg"), b"image")?;
    let document = json!({"version": 3, "playlists": [{
        "playlist_id": "PL1", "url": "https://www.youtube.com/playlist?list=PL1",
        "items": [{"position": 1, "title": "Song", "video_id": "abcdefghijk",
            "video_url": "https://www.youtube.com/watch?v=abcdefghijk"}]
    }]});
    repository.save(&Watchlist::from_value(document)?)?;
    let saved = (repository.revision()?, repository.load()?);
    let cards = view(&repository.load()?, directory.path(), &cache)?;
    let item = &cards["playlists"][0]["items"][0];
    assert_eq!(item["summary"], "Pending");
    assert_eq!(
        item["primary_action"],
        json!({"action": "run", "label": "Run"})
    );
    assert_eq!(
        item["actions"]["run"],
        json!({"enabled": true, "reason": null})
    );
    assert_eq!(item["actions"]["split_again"]["enabled"], false);
    assert_eq!(
        item["thumbnail_path"],
        cache
            .join("yt_thumbnail_abcdefghijk.jpg")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!((repository.revision()?, repository.load()?), saved);
    Ok(())
}

#[test]
fn the_cache_import_marks_a_remaining_import_failed_once() -> TestResult {
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("cache");
    let output = directory.path().join("downloads");
    let splits = directory.path().join("splits");
    fs::create_dir(&cache)?;
    fs::create_dir(&output)?;
    fs::create_dir(&splits)?;
    let audio = output.join("Song [abcdefghijk].flac");
    fs::write(&audio, b"audio")?;
    let document = json!({"version": 3, "playlists": [{
        "playlist_id": "PL1", "url": "https://www.youtube.com/playlist?list=PL1",
        "processed_video_ids": ["abcdefghijk"],
        "items": [{"position": 1, "title": "Song", "video_id": "abcdefghijk",
            "video_url": "https://www.youtube.com/watch?v=abcdefghijk"}]
    }]});
    fs::write(
        cache.join("playlist_PL1.json"),
        serde_json::to_vec(&json!({
            "videos": {"abcdefghijk": {"status": "organized", "audio_file": audio}}
        }))?,
    )?;
    let repository = Repository::new(directory.path().join("muzik.db"));
    let document = imported(&repository, document, options(&output, &splits, &cache))?;
    let playlist = &document["playlists"][0];
    assert_eq!(playlist["processed_video_ids"], json!([]));
    assert_eq!(
        playlist["items"][0]["stages"]["organize"]["status"],
        "failed"
    );
    assert_eq!(playlist["items"][0]["last_action"], "refresh");
    fs::remove_file(audio)?;
    assert!(!import_cache(
        &repository,
        options(&output, &splits, &cache)
    )?);
    let saved = repository.load()?.to_value();
    assert_eq!(
        saved["playlists"][0]["items"][0]["stages"]["organize"]["status"],
        "failed"
    );
    Ok(())
}

#[test]
fn a_second_reconcile_changes_no_rows() -> TestResult {
    let directory = tempfile::tempdir()?;
    let output = directory.path().join("downloads");
    fs::create_dir(&output)?;
    fs::write(output.join("Song [bcdefghijkl].flac"), b"audio")?;
    let repository = Repository::new(directory.path().join("muzik.db"));
    repository.save(&Watchlist::from_value(json!({"version": 3, "playlists": [
        {"playlist_id": "PL1", "url": "https://www.youtube.com/playlist?list=PL1",
         "processed_video_ids": ["abcdefghijk"],
         "items": [
            {"position": 1, "title": "Done", "video_id": "abcdefghijk", "video_url": "u",
             "stages": {"download": {"status": "running"}}},
            {"position": 2, "title": "Downloaded", "video_id": "bcdefghijkl", "video_url": "u"}
         ]},
        {"playlist_id": "spotify:liked", "url": "https://open.spotify.com/collection/tracks",
         "kind": "spotify", "items": [
            {"position": 1, "title": "Track", "video_id": "t1", "entry_id": "t1#0", "kind": "spotify"}
         ]}
    ]}))?)?;
    let options = options(&output, directory.path(), directory.path());
    for _ in 0..2 {
        let mut document = repository.load()?;
        reconcile(&mut document, options)?;
        repository.save(&document)?;
    }
    let revision = repository.revision()?;
    let mut document = repository.load()?;
    reconcile(&mut document, options)?;
    repository.save(&document)?;
    assert_eq!(repository.revision()?, revision);
    let saved = repository.load()?.to_value();
    assert_eq!(
        saved["playlists"][0]["items"][1]["stages"]["download"]["status"],
        "complete"
    );
    assert_eq!(
        saved["playlists"][1]["items"][0]["stages"]["quality"]["status"],
        "skipped"
    );
    Ok(())
}

#[test]
fn retained_source_with_empty_split_dir_keeps_processed_state() -> TestResult {
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("cache");
    let splits = directory.path().join("splits");
    let empty_split = splits.join("Song [abcdefghijk]");
    fs::create_dir(&cache)?;
    fs::create_dir_all(&empty_split)?;
    let audio = directory.path().join("Song [abcdefghijk].flac");
    fs::write(&audio, b"retained source")?;
    fs::write(
        cache.join("playlist_PL1.json"),
        serde_json::to_vec(&json!({"videos": {"abcdefghijk": {
            "status": "organized", "audio_file": audio, "split_dir": empty_split
        }}}))?,
    )?;
    let document = json!({"version":3,"playlists":[{
        "playlist_id":"PL1","url":"https://www.youtube.com/playlist?list=PL1",
        "processed_video_ids":["abcdefghijk"],
        "items":[{"position":1,"title":"Song","video_id":"abcdefghijk"}]
    }]});
    let repository = Repository::new(directory.path().join("muzik.db"));
    let document = imported(
        &repository,
        document,
        options(directory.path(), &splits, &cache),
    )?;
    assert_eq!(
        document["playlists"][0]["processed_video_ids"],
        json!(["abcdefghijk"])
    );
    assert_eq!(
        document["playlists"][0]["items"][0]["stages"]["organize"]["status"],
        "complete"
    );
    Ok(())
}

#[test]
fn reconcile_finds_existing_source_id_in_beets_library() -> TestResult {
    let directory = tempfile::tempdir()?;
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../muzik-library/tests/fixtures/library.db");
    fs::copy(fixture, directory.path().join("library.db"))?;
    let config = directory.path().join("config.yaml");
    fs::write(&config, "library: library.db\ndirectory: .\n")?;
    let document = json!({"version": 3, "playlists": [{
        "playlist_id": "PL1", "url": "https://www.youtube.com/playlist?list=PL1",
        "items": [
            {"position": 1, "title": "Unrelated title", "video_id": "video-123",
                "video_url": "https://www.youtube.com/watch?v=video-123"},
            {"position": 2, "title": "Artist - Album (Full Album)", "video_id": "other-video",
                "video_url": "https://www.youtube.com/watch?v=other-video"}
        ]
    }]});
    let downloads = directory.path().join("downloads");
    let splits = directory.path().join("splits");
    let cache = directory.path().join("cache");
    let document = reconciled(
        document,
        ReconcileOptions {
            config: Some(&config),
            ..options(&downloads, &splits, &cache)
        },
    )?;
    assert_eq!(
        document["playlists"][0]["processed_video_ids"],
        json!(["video-123", "other-video"])
    );
    assert_eq!(
        document["playlists"][0]["items"][0]["stages"]["download"]["status"],
        "complete"
    );
    assert_eq!(
        document["playlists"][0]["items"][0]["stages"]["organize"]["status"],
        "complete"
    );
    Ok(())
}

#[test]
fn reconcile_reads_legacy_audio_and_spotify_track_cache() -> TestResult {
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("cache");
    let output = directory.path().join("downloads");
    let splits = directory.path().join("splits");
    fs::create_dir(&cache)?;
    fs::create_dir(&output)?;
    let audio = output.join("legacy.flac");
    fs::write(&audio, b"audio")?;
    fs::write(
        cache.join("yt_abcdefghijk.txt"),
        audio.to_string_lossy().as_bytes(),
    )?;
    fs::write(
        cache.join("playlist_spotify_TEST.json"),
        serde_json::to_vec(&json!({
            "videos": {"track-1": {"status": "downloaded", "files": [audio]}}
        }))?,
    )?;
    let document = json!({"version": 3, "playlists": [
        {"playlist_id": "PL1", "url": "https://www.youtube.com/playlist?list=PL1", "items": [
            {"position": 1, "title": "Legacy", "video_id": "abcdefghijk", "video_url": "https://www.youtube.com/watch?v=abcdefghijk"}
        ]},
        {"playlist_id": "spotify:playlist:TEST", "url": "https://open.spotify.com/playlist/TEST", "kind": "spotify", "items": [
            {"position": 1, "title": "Track", "video_id": "track-1", "entry_id": "track-1", "kind": "spotify", "video_url": "https://open.spotify.com/track/TEST"}
        ]}
    ]});
    let repository = Repository::new(directory.path().join("muzik.db"));
    let document = imported(
        &repository,
        document,
        ReconcileOptions {
            no_organize: true,
            ..options(&output, &splits, &cache)
        },
    )?;
    assert_eq!(
        document["playlists"][0]["items"][0]["stages"]["download"]["status"],
        "complete"
    );
    assert_eq!(
        document["playlists"][1]["items"][0]["stages"]["download"]["status"],
        "complete"
    );
    assert_eq!(
        document["playlists"][1]["items"][0]["stages"]["quality"]["status"],
        "skipped"
    );
    assert_eq!(
        document["playlists"][1]["items"][0]["stages"]["organize"]["status"],
        "skipped"
    );
    Ok(())
}

#[test]
fn removed_and_private_videos_leave_the_failed_list() -> TestResult {
    let directory = tempfile::tempdir()?;
    let failed = |id: &str, error: &str| {
        json!({"position": 1, "title": id, "video_id": id,
            "video_url": format!("https://www.youtube.com/watch?v={id}"),
            "last_error": error,
            "stages": {"download": {"status": "failed", "error": error}}})
    };
    let document = Watchlist::from_value(json!({"version": 3, "playlists": [{
        "playlist_id": "PL1", "url": "https://www.youtube.com/playlist?list=PL1",
        "items": [
            failed("aaaaaaaaaaa", "yt-dlp failed: ERROR: [youtube] aaaaaaaaaaa: Private video"),
            failed("bbbbbbbbbbb", "yt-dlp failed: WARNING: [youtube] unable to extract yt initial data\nERROR: [youtube] bbbbbbbbbbb: Video unavailable"),
            failed("ccccccccccc", "yt-dlp failed: ERROR: unable to download video data: HTTP Error 403: Forbidden")
        ]
    }]}))?;
    let cards = view(&document, directory.path(), directory.path())?;
    let items = &cards["playlists"][0]["items"];
    for gone in [&items[0], &items[1]] {
        assert_eq!(gone["summary"], "Unavailable");
        assert!(gone["primary_action"].is_null());
        assert_eq!(gone["actions"]["retry"]["enabled"], false);
    }
    assert_eq!(items[2]["summary"], "Failed");
    assert_eq!(items[2]["primary_action"]["action"], "retry");
    Ok(())
}

#[test]
fn each_decision_kind_maps_to_its_stage() {
    use muzik_core::DecisionKind;
    use muzik_store::watchlist::Stage;
    assert_eq!(
        Stage::of_decision(DecisionKind::ImportMatch),
        Stage::Organize
    );
    assert_eq!(
        Stage::of_decision(DecisionKind::ImportDuplicate),
        Stage::Organize
    );
    assert_eq!(
        Stage::of_decision(DecisionKind::ChapterReview),
        Stage::Parse
    );
    assert_eq!(Stage::of_decision(DecisionKind::ChapterEdit), Stage::Parse);
    assert_eq!(
        Stage::of_decision(DecisionKind::QualityReplacement),
        Stage::Quality
    );
    assert_eq!(
        Stage::of_decision(DecisionKind::SoulseekCandidate),
        Stage::Download
    );
}

fn source_with_item(path: &Path) -> Result<Repository, Box<dyn std::error::Error>> {
    let repository = Repository::new(path.to_path_buf());
    repository.add("https://www.youtube.com/playlist?list=PL1")?;
    repository.update(|document| {
        document
            .playlists
            .first_mut()
            .ok_or("the watchlist has no playlist")?
            .items = vec![WatchItem::new(1, "Song", SourceKind::Youtube)];
        Ok(())
    })?;
    Ok(repository)
}

#[test]
fn a_checked_write_keeps_a_concurrent_source_edit_and_succeeds_on_retry() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("muzik.db");
    let check = source_with_item(&path)?;
    let editor = Repository::new(path);
    let (mut stale, revision) = check.load_revision()?;
    assert_eq!(revision, check.revision()?);
    editor.rename("PL1", "Edited")?;
    stale.playlists[0].items[0].title = "Checked".into();
    assert_eq!(check.save_at(revision, &stale)?, CheckedWrite::Conflict);
    let saved = editor.load()?;
    assert_eq!(saved.playlists[0].title.as_deref(), Some("Edited"));
    assert_eq!(saved.playlists[0].items[0].title, "Song");
    let (mut fresh, revision) = check.load_revision()?;
    fresh.playlists[0].items[0].title = "Checked".into();
    assert_eq!(check.save_at(revision, &fresh)?, CheckedWrite::Written);
    let saved = editor.load()?;
    assert_eq!(saved.playlists[0].title.as_deref(), Some("Edited"));
    assert_eq!(saved.playlists[0].items[0].title, "Checked");
    assert_eq!(editor.revision()?, revision + 1);
    Ok(())
}

#[test]
fn a_checked_write_keeps_a_concurrent_stage_change() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("muzik.db");
    let check = source_with_item(&path)?;
    let runner = Repository::new(path);
    let (stale, revision) = check.load_revision()?;
    runner.update(|document| {
        document.playlists[0].items[0].complete(Stage::Download, None);
        Ok(())
    })?;
    assert_eq!(check.save_at(revision, &stale)?, CheckedWrite::Conflict);
    assert_eq!(
        runner.load()?.playlists[0].items[0].status(Stage::Download),
        StageStatus::Complete
    );
    Ok(())
}

#[test]
fn a_checked_write_of_an_unchanged_document_keeps_the_revision() -> TestResult {
    let directory = tempfile::tempdir()?;
    let repository = source_with_item(&directory.path().join("muzik.db"))?;
    let (document, revision) = repository.load_revision()?;
    assert_eq!(
        repository.save_at(revision, &document)?,
        CheckedWrite::Written
    );
    assert_eq!(repository.revision()?, revision);
    assert_eq!(repository.load()?, document);
    Ok(())
}

#[test]
fn a_failed_checked_write_changes_nothing() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("muzik.db");
    let repository = source_with_item(&path)?;
    let (mut document, revision) = repository.load_revision()?;
    rusqlite::Connection::open(&path)?.execute_batch(
        "CREATE TRIGGER refuse BEFORE INSERT ON watchlist_items
         BEGIN SELECT RAISE(ABORT, 'refused'); END;",
    )?;
    document.playlists[0].title = Some("Renamed".into());
    document.playlists[0]
        .items
        .push(WatchItem::new(2, "New", SourceKind::Youtube));
    assert!(repository.save_at(revision, &document).is_err());
    let saved = repository.load()?;
    assert_eq!(saved.playlists[0].title, None);
    assert_eq!(saved.playlists[0].items.len(), 1);
    assert_eq!(repository.revision()?, revision);
    Ok(())
}
