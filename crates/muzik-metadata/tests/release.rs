use musicbrainz_rs::entity::release::Release;
use muzik_core::ReleaseCandidate;
use muzik_metadata::{
    ReleaseOptions, recording_candidate, release_artist_ids, release_candidate,
    release_candidate_with_options,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    beets_version: String,
    raw: serde_json::Value,
    expected: ReleaseCandidate,
}

#[test]
fn mapping_uses_preferred_event_and_ignored_media() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/release.json")).unwrap();
    let mut release: Release = serde_json::from_value(fixture.raw).unwrap();
    release.country = Some("US".to_string());
    release.date = Some("2000-01-01".into());
    let options = ReleaseOptions {
        preferred_countries: vec!["XE".to_string()],
        ignored_media: vec!["CD".to_string()],
        ..ReleaseOptions::default()
    };
    let mapped = release_candidate_with_options(&release, &options);
    assert_eq!(mapped.country.as_deref(), Some("XE"));
    assert_eq!(mapped.year, Some(1994));
    assert!(mapped.tracks.is_empty());
}

#[test]
fn recorded_release_matches_beets_album_info() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/release.json")).unwrap();
    assert_eq!(fixture.beets_version, "2.13.1");
    let release: Release = serde_json::from_value(fixture.raw).unwrap();
    assert_eq!(release_candidate(&release), fixture.expected);
}

#[test]
fn recorded_release_gives_artist_ids_by_release_track() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/release.json")).unwrap();
    let release: Release = serde_json::from_value(fixture.raw).unwrap();
    let ids = release_artist_ids(&release);
    let portishead = "8f6bd1e4-fbe1-4f50-aa9b-94c450ec0f11";
    assert_eq!(ids.album_artist.as_deref(), Some(portishead));
    assert_eq!(ids.tracks.len(), fixture.expected.tracks.len());
    assert_eq!(
        ids.tracks
            .get("17f6c0af-4e48-3810-9d13-17ac5de492df")
            .map(String::as_str),
        Some(portishead)
    );
}

#[test]
fn recorded_recording_maps_for_singleton_sync() {
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/release.json")).unwrap();
    let recording: musicbrainz_rs::entity::recording::Recording =
        serde_json::from_value(fixture.raw["media"][0]["tracks"][0]["recording"].clone()).unwrap();
    let mapped = recording_candidate(&recording);
    assert_eq!(
        mapped.recording_id.unwrap().0,
        "b5d7d380-f43a-4c1f-a5de-694150b093ac"
    );
    assert_eq!(mapped.title, "Mysterons");
    assert_eq!(mapped.artist, "Portishead");
    assert!(mapped.release_track_id.is_none());
}
