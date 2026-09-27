use std::fs;
use std::path::PathBuf;

use muzik_core::{BeetsConfig, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
use muzik_import::apply::{
    self, AlbumDecision, ApplyError, ApplyOptions, DuplicateDecision, MatchDecision,
};
use muzik_import::plan::{Duplicate, DuplicateReason, ImportPlanner, ReleaseProvider};
use muzik_library::{Library, SqlValue};
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
        Ok(vec![ReleaseSearchHit {
            id: ReleaseId("new-release".into()),
            title: "Night Lines".into(),
            artist: "Mara Vale".into(),
            score: Some(100),
        }])
    }

    fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        Ok(ReleaseCandidate {
            id: ReleaseId(id.into()),
            title: "Night Lines".into(),
            artist: "Mara Vale".into(),
            tracks: vec![TrackCandidate {
                recording_id: Some(RecordingId("recording-1".into())),
                title: "Song (feat. Guest)".into(),
                artist: "Mara Vale".into(),
                length_seconds: None,
                index: 2,
                medium: 1,
                medium_index: 2,
            }],
            release_group_id: Some("group-1".into()),
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

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, BeetsConfig) {
    let temp = tempfile::tempdir().unwrap();
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let source_dir = temp.path().join("incoming");
    fs::create_dir(&source_dir).unwrap();
    let source = source_dir.join("02 Song.flac");
    fs::copy(crates.join("muzik-tags/tests/fixtures/blank.flac"), &source).unwrap();
    fs::copy(
        crates.join("muzik-tags/tests/fixtures/cover.png"),
        source_dir.join("cover.png"),
    )
    .unwrap();
    fs::write(source_dir.join("02 Song.muzik.json"), r#"{"source_id":"video-123","resolved":{"title":"Song","artist":"Mara Vale","album":"Night Lines","year":"2021"}}"#).unwrap();
    let database = temp.path().join("library.db");
    fs::copy(
        crates.join("muzik-library/tests/fixtures/library.db"),
        &database,
    )
    .unwrap();
    let root = temp.path().join("music");
    fs::create_dir(&root).unwrap();
    let config = BeetsConfig::from_layers("", serde_json::json!({})).unwrap();
    (temp, source, database, root, config)
}

#[test]
fn applies_candidate_with_tags_art_source_id_and_ftclean() {
    let (_temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner.plan(std::slice::from_ref(&source)).unwrap();
    assert_eq!(plan.albums[0].candidates.len(), 1);
    let options = ApplyOptions::from_beets(&config, root).unwrap();
    let result = apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::Candidate(0),
            duplicate: Some(DuplicateDecision::Keep),
        }],
        &options,
    )
    .unwrap();
    assert_eq!(result.item_ids.len(), 1);
    assert!(source.exists());
    let destination = &result.destinations[0];
    assert_eq!(destination.file_name().unwrap(), "02 Song.flac");
    let tags = muzik_tags::read(destination, &[]).unwrap();
    assert_eq!(tags.fields.get("title").map(String::as_str), Some("Song"));
    assert_eq!(
        tags.fields.get("artist").map(String::as_str),
        Some("Mara Vale feat. Guest")
    );
    assert_eq!(
        tags.fields.get("mb_albumid").map(String::as_str),
        Some("new-release")
    );
    assert!(muzik_tags::has_front_cover(destination).unwrap());
    let item = library.item(result.item_ids[0]).unwrap().unwrap();
    assert_eq!(
        item.attribute("muzik_source_id"),
        Some(&SqlValue::Text("video-123".into()))
    );
    assert_eq!(item.field("title"), Some(&SqlValue::Text("Song".into())));
    let album = library.album(result.album_ids[0]).unwrap().unwrap();
    assert_eq!(
        album.field("mb_albumid"),
        Some(&SqlValue::Text("new-release".into()))
    );
}

#[test]
fn dry_run_and_duplicate_skip_do_not_write() {
    let (_temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let mut plan = planner.plan(&[source]).unwrap();
    let original_count = library.albums().unwrap().len();
    let mut options = ApplyOptions::from_beets(&config, root).unwrap();
    options.dry_run = true;
    let decision = AlbumDecision {
        choice: MatchDecision::AsIs,
        duplicate: Some(DuplicateDecision::Keep),
    };
    let result = apply::apply(&mut library, &plan, &[decision], &options).unwrap();
    assert_eq!(result.destinations.len(), 1);
    assert!(!result.destinations[0].exists());
    assert_eq!(library.albums().unwrap().len(), original_count);

    plan.albums[0].duplicates.push(Duplicate {
        album_id: 1,
        reason: DuplicateReason::ArtistAndAlbum,
    });
    assert!(matches!(
        apply::apply(
            &mut library,
            &plan,
            &[AlbumDecision {
                choice: MatchDecision::AsIs,
                duplicate: None
            }],
            &options
        ),
        Err(ApplyError::DuplicateDecision)
    ));
    let result = apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::AsIs,
            duplicate: Some(DuplicateDecision::Skip),
        }],
        &options,
    )
    .unwrap();
    assert_eq!(result.skipped_albums, 1);
}

#[test]
fn replace_removes_selected_duplicate_rows() {
    let (_temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let mut fields = muzik_library::Fields::new();
    fields.insert("album".into(), SqlValue::Text("Night Lines".into()));
    fields.insert("albumartist".into(), SqlValue::Text("Mara Vale".into()));
    fields.insert("mb_albumid".into(), SqlValue::Text("new-release".into()));
    let old_id = library.insert_album(&fields, &Default::default()).unwrap();
    let mut item_fields = muzik_library::Fields::new();
    item_fields.insert("album_id".into(), SqlValue::Integer(old_id));
    item_fields.insert(
        "path".into(),
        SqlValue::Blob(
            root.join("old-missing.flac")
                .as_os_str()
                .as_encoded_bytes()
                .to_vec(),
        ),
    );
    library
        .insert_item(&item_fields, &Default::default())
        .unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let mut plan = planner.plan(&[source]).unwrap();
    plan.albums[0]
        .duplicates
        .retain(|duplicate| duplicate.album_id == old_id);
    assert_eq!(plan.albums[0].duplicates.len(), 1);
    let options = ApplyOptions::from_beets(&config, root).unwrap();

    let result = apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::Candidate(0),
            duplicate: Some(DuplicateDecision::Replace),
        }],
        &options,
    )
    .unwrap();

    assert_eq!(result.album_ids.len(), 1);
    assert!(library.album(result.album_ids[0]).unwrap().is_some());
    let items = library.items_for_album(result.album_ids[0]).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].field("path"),
        Some(&SqlValue::Blob(
            result.destinations[0]
                .as_os_str()
                .as_encoded_bytes()
                .to_vec()
        ))
    );
}
