use std::fs;
use std::path::PathBuf;

use muzik_core::{BeetsConfig, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
use muzik_import::plan::{ImportPlanner, ReleaseProvider};
use muzik_library::Library;
use muzik_match::MatchConfig;
use muzik_metadata::{ReleaseSearch, ReleaseSearchHit};

struct FixtureProvider;

impl ReleaseProvider for FixtureProvider {
    fn search_releases(
        &self,
        criteria: &ReleaseSearch,
        _limit: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        assert_eq!(criteria.release, "Night Lines");
        assert_eq!(criteria.artist.as_deref(), Some("Mara Vale"));
        assert_eq!(criteria.tracks, Some(1));
        Ok(vec![ReleaseSearchHit {
            id: ReleaseId("release-1".to_owned()),
            title: "Night Lines".to_owned(),
            artist: "Mara Vale".to_owned(),
            score: Some(100),
        }])
    }

    fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        assert_eq!(id, "release-1");
        Ok(ReleaseCandidate {
            id: ReleaseId(id.to_owned()),
            title: "Night Lines".to_owned(),
            artist: "Mara Vale".to_owned(),
            tracks: vec![TrackCandidate {
                recording_id: Some(RecordingId(
                    "11111111-1111-4111-8111-111111111111".to_owned(),
                )),
                title: "Tide & Stone".to_owned(),
                artist: "Mara Vale".to_owned(),
                length_seconds: None,
                index: 2,
                medium: 1,
                medium_index: 2,
            }],
            release_group_id: None,
            year: Some(2021),
            country: None,
            media: None,
            label: None,
            catalog_number: None,
            disambiguation: None,
            is_various_artists: false,
        })
    }
}

#[test]
fn groups_audio_and_ranks_release_with_source_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let album_dir = temp.path().join("album");
    fs::create_dir(&album_dir).unwrap();
    let source = album_dir.join("02 Tide & Stone.flac");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    fs::copy(
        crates.join("muzik-tags/tests/fixtures/mediafile.flac"),
        &source,
    )
    .unwrap();
    fs::write(
        album_dir.join("02 Tide & Stone.muzik.json"),
        r#"{"source_id":"video-123"}"#,
    )
    .unwrap();
    let library =
        Library::open_read_only(&crates.join("muzik-library/tests/fixtures/library.db")).unwrap();
    let beets = BeetsConfig::from_layers("", serde_json::json!({})).unwrap();
    let config = MatchConfig::from_beets(&beets).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &config,
        search_limit: 5,
    };

    let plan = planner.plan(&[album_dir.clone(), source.clone()]).unwrap();

    assert_eq!(plan.albums.len(), 1);
    let album = &plan.albums[0];
    assert_eq!(album.source_dir, album_dir.canonicalize().unwrap());
    assert_eq!(album.items.len(), 1);
    assert_eq!(album.items[0].source, source.canonicalize().unwrap());
    assert_eq!(album.items[0].source_id.as_deref(), Some("video-123"));
    assert_eq!(album.candidates.len(), 1);
    assert_eq!(album.candidates[0].release.id.0, "release-1");
    assert_eq!(album.candidates[0].assignment.pairs, vec![(0, 0)]);
    assert_eq!(album.duplicates.len(), 1);
}
