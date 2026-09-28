use std::fs;
use std::path::PathBuf;

use muzik_core::{BeetsConfig, ReleaseCandidate, ReleaseId, TrackCandidate};
use muzik_import::apply::{self, AlbumDecision, ApplyOptions, MatchDecision};
use muzik_import::plan::{ImportPlanner, ReleaseProvider};
use muzik_library::{Library, SqlValue};
use muzik_match::MatchConfig;
use muzik_metadata::{ReleaseSearch, ReleaseSearchHit};

struct RecordedRelease {
    release: ReleaseCandidate,
}

impl ReleaseProvider for RecordedRelease {
    fn search_releases(
        &self,
        criteria: &ReleaseSearch,
        _: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        assert_eq!(criteria.release, "Dummy");
        assert_eq!(criteria.artist.as_deref(), Some("Portishead"));
        Ok(vec![ReleaseSearchHit {
            id: self.release.id.clone(),
            title: self.release.title.clone(),
            artist: self.release.artist.clone(),
            score: Some(100),
        }])
    }

    fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        assert_eq!(id, self.release.id.0);
        Ok(self.release.clone())
    }

    fn lookup_recording(&self, _: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
        unreachable!()
    }
}

#[test]
fn imports_matched_album_from_recorded_musicbrainz_release() {
    let recorded: serde_json::Value = serde_json::from_str(include_str!(
        "../../muzik-metadata/tests/fixtures/release.json"
    ))
    .unwrap();
    let release: ReleaseCandidate = serde_json::from_value(recorded["expected"].clone()).unwrap();
    assert_eq!(
        release.id,
        ReleaseId("76df3287-6cda-33eb-8e9a-044b5e15ffdd".into())
    );
    let provider = RecordedRelease { release };
    let temp = tempfile::tempdir().unwrap();
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let source_dir = temp.path().join("incoming/Dummy");
    fs::create_dir_all(&source_dir).unwrap();
    for track in &provider.release.tracks {
        let source = source_dir.join(format!("{:02} {}.flac", track.index, track.title));
        fs::copy(crates.join("muzik-tags/tests/fixtures/blank.flac"), &source).unwrap();
        fs::write(
            source.with_extension("muzik.json"),
            serde_json::json!({"resolved": {
                "title": track.title, "artist": track.artist, "album": "Dummy", "year": "1994"
            }})
            .to_string(),
        )
        .unwrap();
    }
    let database = temp.path().join("library.db");
    fs::copy(
        crates.join("muzik-library/tests/fixtures/library.db"),
        &database,
    )
    .unwrap();
    let root = temp.path().join("music");
    fs::create_dir(&root).unwrap();
    let config = BeetsConfig::from_layers("", serde_json::json!({})).unwrap();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let planner = ImportPlanner {
        provider: &provider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner.plan(&[source_dir]).unwrap();
    assert_eq!(plan.albums.len(), 1);
    assert_eq!(plan.albums[0].candidates.len(), 1);
    assert_eq!(plan.albums[0].candidates[0].assignment.pairs.len(), 11);

    let options = ApplyOptions::from_beets(&config, root.clone()).unwrap();
    let result = apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::Candidate(0),
            duplicate: None,
        }],
        &options,
    )
    .unwrap();

    assert_eq!(result.item_ids.len(), 11);
    assert!(
        result
            .destinations
            .iter()
            .all(|path| path.starts_with(root.join("Portishead/Dummy")))
    );
    let first = library.item(result.item_ids[0]).unwrap().unwrap();
    assert_eq!(
        first.field("mb_albumid"),
        Some(&SqlValue::Text(provider.release.id.0.clone()))
    );
    assert_eq!(
        first.field("mb_releasetrackid"),
        Some(&SqlValue::Text(
            provider.release.tracks[0].release_track_id.clone().unwrap()
        ))
    );
    let tags = muzik_tags::read(&result.destinations[0], &[]).unwrap();
    assert_eq!(
        tags.fields.get("mb_releasetrackid"),
        provider.release.tracks[0].release_track_id.as_ref()
    );
}
