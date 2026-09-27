use muzik_core::watchlist::Repository;
use serde_json::{json, Value};
use std::fs;

#[test]
fn reads_old_watchlist_and_preserves_saved_item_state() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("watchlist.json");
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
    let repository = Repository::new(path.clone());

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
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&path)?)?, old);

    repository.save(loaded.clone())?;
    assert_eq!(repository.load()?, loaded);
    Ok(())
}

#[test]
fn edits_saved_sources_without_losing_item_state() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let repository = Repository::new(directory.path().join("config/watchlist.json"));
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
fn rejects_invalid_data_without_replacing_the_file() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("watchlist.json");
    let invalid = r#"{"version":99,"playlists":[]}"#;
    fs::write(&path, invalid)?;
    let repository = Repository::new(path.clone());

    assert!(repository
        .add("https://youtube.com/playlist?list=PL_NEW")
        .is_err());
    assert_eq!(fs::read_to_string(path)?, invalid);
    Ok(())
}
