use muzik_core::watchlist::{reconcile, view, ReconcileOptions, Repository};
use muzik_core::QualityPolicy;
use serde_json::{json, Value};
use std::fs;

#[test]
fn reconcile_fills_finished_stages_and_keeps_the_waiting_one(
) -> Result<(), Box<dyn std::error::Error>> {
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
    let mut document = json!({"version": 3, "playlists": [{
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
    reconcile(
        &mut document,
        ReconcileOptions {
            output: &output,
            splits: &splits,
            cache: &cache,
            config: None,
            no_organize: false,
            no_split: false,
            quality_policy: QualityPolicy::Off,
        },
    )?;
    let stages = &document["playlists"][0]["items"][0]["stages"];
    assert_eq!(stages["download"]["status"], "complete");
    assert_eq!(stages["split"]["status"], "complete");
    assert_eq!(stages["organize"]["status"], "waiting");
    assert_eq!(stages["organize"]["question"], question);
    assert_eq!(document["playlists"][0]["processed_video_ids"], json!([]));
    Ok(())
}

#[test]
fn imports_old_watchlist_file_and_preserves_saved_item_state(
) -> Result<(), Box<dyn std::error::Error>> {
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
    let item = &loaded["playlists"][0]["items"][0];
    assert_eq!(loaded["version"], 3);
    assert_eq!(item["stages"]["quality"]["status"], "not_started");
    assert_eq!(item["stages"]["download"]["path"], "/music/song.flac");
    assert_eq!(item["stages"]["parse"]["status"], "complete");
    assert_eq!(item["stages"]["split"]["status"], "skipped");
    assert_eq!(
        loaded["playlists"][0]["processed_video_ids"],
        json!(["abcdefghijk"])
    );
    assert!(!path.exists());
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&backup)?)?, old);

    let revision = repository.revision()?;
    repository.save(loaded.clone())?;
    assert_eq!(repository.revision()?, revision);
    assert_eq!(repository.load()?, loaded);
    Ok(())
}

#[test]
fn edits_saved_sources_without_losing_item_state() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("config/muzik.db"));
    let added = repository.add("https://www.youtube.com/watch?v=abcdefghijk&list=PL_ONE")?;
    assert_eq!(added["playlist_id"], "PL_ONE");
    assert_eq!(added["url"], "https://www.youtube.com/playlist?list=PL_ONE");
    assert!(repository
        .add("https://youtube.com/playlist?list=PL_ONE")
        .is_err());

    let mut saved = repository.load()?;
    saved["playlists"][0]["items"] = json!([{
        "position": 1,
        "title": "Song",
        "stages": {"download": {"status": "complete", "path": "/music/song.flac"}}
    }]);
    repository.save(saved)?;
    assert!(repository.rename("PL_ONE", " Jazz albums ")?);
    assert_eq!(repository.load()?["playlists"][0]["title"], "Jazz albums");
    assert_eq!(
        repository.load()?["playlists"][0]["items"][0]["stages"]["download"]["path"],
        "/music/song.flac"
    );

    let liked = repository.add("liked")?;
    assert_eq!(liked["playlist_id"], "spotify:liked");
    assert_eq!(liked["title"], "Liked Songs");
    let spotify =
        repository.add("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=1")?;
    assert_eq!(
        spotify["playlist_id"],
        "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M"
    );
    assert!(repository.remove("spotify:liked")?);
    assert_eq!(
        repository.load()?["playlists"].as_array().map(Vec::len),
        Some(2)
    );
    Ok(())
}

#[test]
fn rejects_an_invalid_old_file_without_moving_it() -> Result<(), Box<dyn std::error::Error>> {
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
fn view_adds_card_actions_and_cached_thumbnail_without_saving(
) -> Result<(), Box<dyn std::error::Error>> {
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
    repository.save(document)?;
    let saved = (repository.revision()?, repository.load()?);
    let cards = view(repository.load()?, directory.path(), &cache)?;
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
fn reconcile_reads_playlist_cache_and_marks_remaining_import_failed(
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("cache");
    let output = directory.path().join("downloads");
    let splits = directory.path().join("splits");
    fs::create_dir(&cache)?;
    fs::create_dir(&output)?;
    fs::create_dir(&splits)?;
    let audio = output.join("Song [abcdefghijk].flac");
    fs::write(&audio, b"audio")?;
    let mut document = json!({"version": 3, "playlists": [{
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
    reconcile(
        &mut document,
        ReconcileOptions {
            output: &output,
            splits: &splits,
            cache: &cache,
            config: None,
            no_organize: false,
            no_split: false,
            quality_policy: QualityPolicy::Off,
        },
    )?;
    let playlist = &document["playlists"][0];
    assert_eq!(playlist["processed_video_ids"], json!([]));
    assert_eq!(
        playlist["items"][0]["stages"]["organize"]["status"],
        "failed"
    );
    assert_eq!(playlist["items"][0]["last_action"], "refresh");
    fs::remove_file(audio)?;
    reconcile(
        &mut document,
        ReconcileOptions {
            output: &output,
            splits: &splits,
            cache: &cache,
            config: None,
            no_organize: false,
            no_split: false,
            quality_policy: QualityPolicy::Off,
        },
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
fn retained_source_with_empty_split_dir_keeps_processed_state(
) -> Result<(), Box<dyn std::error::Error>> {
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
    let mut document = json!({"version":3,"playlists":[{
        "playlist_id":"PL1","url":"https://www.youtube.com/playlist?list=PL1",
        "processed_video_ids":["abcdefghijk"],
        "items":[{"position":1,"title":"Song","video_id":"abcdefghijk"}]
    }]});
    reconcile(
        &mut document,
        ReconcileOptions {
            output: directory.path(),
            splits: &splits,
            cache: &cache,
            config: None,
            no_organize: false,
            no_split: false,
            quality_policy: QualityPolicy::Off,
        },
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
fn reconcile_finds_existing_source_id_in_beets_library() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../muzik-library/tests/fixtures/library.db");
    fs::copy(fixture, directory.path().join("library.db"))?;
    let config = directory.path().join("config.yaml");
    fs::write(&config, "library: library.db\ndirectory: .\n")?;
    let mut document = json!({"version": 3, "playlists": [{
        "playlist_id": "PL1", "url": "https://www.youtube.com/playlist?list=PL1",
        "items": [
            {"position": 1, "title": "Unrelated title", "video_id": "video-123",
                "video_url": "https://www.youtube.com/watch?v=video-123"},
            {"position": 2, "title": "Artist - Album (Full Album)", "video_id": "other-video",
                "video_url": "https://www.youtube.com/watch?v=other-video"}
        ]
    }]});
    reconcile(
        &mut document,
        ReconcileOptions {
            output: &directory.path().join("downloads"),
            splits: &directory.path().join("splits"),
            cache: &directory.path().join("cache"),
            config: Some(&config),
            no_organize: false,
            no_split: false,
            quality_policy: QualityPolicy::Off,
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
fn reconcile_reads_legacy_audio_and_spotify_track_cache() -> Result<(), Box<dyn std::error::Error>>
{
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
    let mut document = json!({"version": 3, "playlists": [
        {"playlist_id": "PL1", "url": "https://www.youtube.com/playlist?list=PL1", "items": [
            {"position": 1, "title": "Legacy", "video_id": "abcdefghijk", "video_url": "https://www.youtube.com/watch?v=abcdefghijk"}
        ]},
        {"playlist_id": "spotify:playlist:TEST", "url": "https://open.spotify.com/playlist/TEST", "kind": "spotify", "items": [
            {"position": 1, "title": "Track", "video_id": "track-1", "entry_id": "track-1", "kind": "spotify", "video_url": "https://open.spotify.com/track/TEST"}
        ]}
    ]});
    reconcile(
        &mut document,
        ReconcileOptions {
            output: &output,
            splits: &splits,
            cache: &cache,
            config: None,
            no_organize: true,
            no_split: false,
            quality_policy: QualityPolicy::Off,
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
