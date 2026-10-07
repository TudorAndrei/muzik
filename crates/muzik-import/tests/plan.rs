use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use muzik_core::{BeetsConfig, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
use muzik_import::history::IncrementalHistory;
use muzik_import::plan::{ImportMode, ImportPlanner, PlanOptions, ReleaseProvider};
use muzik_library::Library;
use muzik_match::MatchConfig;
use muzik_metadata::{ReleaseSearch, ReleaseSearchHit};

struct FixtureProvider;

struct FailingProvider;

#[derive(Default)]
struct NoLookupProvider {
    requested: Cell<bool>,
}

struct CancellingProvider<'a>(&'a AtomicBool);

impl ReleaseProvider for CancellingProvider<'_> {
    fn search_releases(
        &self,
        _: &ReleaseSearch,
        _: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        self.0.store(true, Ordering::SeqCst);
        Ok(Vec::new())
    }

    fn lookup_release(&self, _: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        Err(muzik_metadata::Error::EmptyReleaseTitle)
    }

    fn lookup_recording(&self, _: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
        Err(muzik_metadata::Error::EmptyReleaseTitle)
    }
}

#[test]
fn cancellation_after_metadata_search_stops_planning() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("song.flac");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    fs::copy(crates.join("muzik-tags/tests/fixtures/blank.flac"), &source).unwrap();
    fs::write(
        source.with_extension("muzik.json"),
        r#"{"resolved":{"title":"Song","artist":"Mara Vale","album":"Night Lines"}}"#,
    )
    .unwrap();
    let library = Library::empty().unwrap();
    let config = BeetsConfig::from_layers("", serde_json::json!({})).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let cancelled = AtomicBool::new(false);
    let planner = ImportPlanner {
        provider: &CancellingProvider(&cancelled),
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let result = planner.plan_with_options_and_cancel(
        &[source],
        ImportMode::Album,
        PlanOptions::default(),
        &|| cancelled.load(Ordering::SeqCst),
    );
    assert!(matches!(result, Err(muzik_import::Error::Cancelled)));
}

impl NoLookupProvider {
    fn request<T>(&self) -> Result<T, muzik_metadata::Error> {
        self.requested.set(true);
        Err(muzik_metadata::Error::EmptyReleaseTitle)
    }
}

impl ReleaseProvider for NoLookupProvider {
    fn search_releases(
        &self,
        _: &ReleaseSearch,
        _: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        self.request()
    }

    fn lookup_release(&self, _: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        self.request()
    }

    fn lookup_recording(&self, _: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
        self.request()
    }
}

impl ReleaseProvider for FailingProvider {
    fn search_releases(
        &self,
        _: &ReleaseSearch,
        _: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        Err(muzik_metadata::Error::EmptyReleaseTitle)
    }

    fn lookup_release(&self, _: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        Err(muzik_metadata::Error::EmptyReleaseTitle)
    }

    fn lookup_recording(&self, _: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
        Err(muzik_metadata::Error::EmptyReleaseTitle)
    }
}

impl ReleaseProvider for FixtureProvider {
    fn search_releases(
        &self,
        criteria: &ReleaseSearch,
        _limit: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        if criteria.release != "Night Lines"
            || criteria.artist.as_deref() != Some("Mara Vale")
            || criteria.tracks != Some(1)
        {
            return Err(muzik_metadata::Error::EmptyReleaseTitle);
        }
        Ok(vec![ReleaseSearchHit {
            id: ReleaseId("release-1".to_owned()),
            title: "Night Lines".to_owned(),
            artist: "Mara Vale".to_owned(),
            score: Some(100),
        }])
    }

    fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        if id != "release-1" {
            return Err(muzik_metadata::Error::EmptyReleaseTitle);
        }
        Ok(ReleaseCandidate {
            id: ReleaseId(id.to_owned()),
            title: "Night Lines".to_owned(),
            artist: "Mara Vale".to_owned(),
            tracks: vec![TrackCandidate {
                recording_id: Some(RecordingId(
                    "11111111-1111-4111-8111-111111111111".to_owned(),
                )),
                release_track_id: Some("release-track-1".into()),
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

    fn lookup_recording(&self, _: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
        Err(muzik_metadata::Error::EmptyReleaseTitle)
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
    let duration = muzik_tags::probe(&source)
        .unwrap()
        .duration_seconds
        .unwrap();
    assert_eq!(
        album.items[0].match_item.length.to_bits(),
        duration.to_bits()
    );
    assert_eq!(album.candidates.len(), 1);
    assert_eq!(album.candidates[0].release.id.0, "release-1");
    assert_eq!(album.candidates[0].assignment.pairs, vec![(0, 0)]);
    assert_eq!(album.duplicates.len(), 1);
}

#[test]
fn metadata_failure_keeps_as_is_import_plan() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Night Lines.flac");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    fs::copy(crates.join("muzik-tags/tests/fixtures/blank.flac"), &source).unwrap();
    fs::write(
        temp.path().join("Night Lines.muzik.json"),
        r#"{"resolved":{"title":"Song","artist":"Mara Vale","album":"Night Lines"}}"#,
    )
    .unwrap();
    let library =
        Library::open_read_only(&crates.join("muzik-library/tests/fixtures/library.db")).unwrap();
    let config = BeetsConfig::from_layers("", serde_json::json!({})).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let planner = ImportPlanner {
        provider: &FailingProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };

    let plan = planner.plan(&[source]).unwrap();

    assert_eq!(plan.albums.len(), 1);
    assert!(plan.albums[0].candidates.is_empty());
    assert_eq!(plan.albums[0].items[0].match_item.artist, "Mara Vale");
}

#[test]
fn autotag_off_plans_as_is_without_metadata_requests() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Song.flac");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    fs::copy(crates.join("muzik-tags/tests/fixtures/blank.flac"), &source).unwrap();
    let library =
        Library::open_read_only(&crates.join("muzik-library/tests/fixtures/library.db")).unwrap();
    let beets = BeetsConfig::from_layers("", serde_json::json!({})).unwrap();
    let config = MatchConfig::from_beets(&beets).unwrap();
    let provider = NoLookupProvider::default();
    let planner = ImportPlanner {
        provider: &provider,
        library: &library,
        match_config: &config,
        search_limit: 5,
    };
    let plan = planner
        .plan_with_options(
            &[source],
            ImportMode::Album,
            PlanOptions {
                autotag: false,
                ..PlanOptions::default()
            },
        )
        .unwrap();
    assert!(!provider.requested.get());
    assert_eq!(plan.albums.len(), 1);
    assert!(plan.albums[0].candidates.is_empty());
}

#[test]
fn incremental_plan_skips_seeded_album_directory() {
    let temp = tempfile::tempdir().unwrap();
    let album_dir = temp.path().join("incoming");
    fs::create_dir(&album_dir).unwrap();
    let source = album_dir.join("Song.flac");
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    fs::copy(crates.join("muzik-tags/tests/fixtures/blank.flac"), &source).unwrap();
    let library =
        Library::open_read_only(&crates.join("muzik-library/tests/fixtures/library.db")).unwrap();
    let beets = BeetsConfig::from_layers("", serde_json::json!({})).unwrap();
    let config = MatchConfig::from_beets(&beets).unwrap();
    let history = IncrementalHistory::open_or_seed(
        &temp.path().join("state.pickle"),
        &[vec![album_dir.canonicalize().unwrap()]],
    )
    .unwrap();
    let provider = NoLookupProvider::default();
    let planner = ImportPlanner {
        provider: &provider,
        library: &library,
        match_config: &config,
        search_limit: 5,
    };
    let plan = planner
        .plan_with_options(
            &[source],
            ImportMode::Album,
            PlanOptions {
                history: Some(history),
                ..PlanOptions::default()
            },
        )
        .unwrap();
    assert!(!provider.requested.get());
    assert!(plan.albums.is_empty());
    assert_eq!(plan.skipped_incremental, 1);
    assert!(!IncrementalHistory::path_for_statefile(&temp.path().join("state.pickle")).exists());
}
