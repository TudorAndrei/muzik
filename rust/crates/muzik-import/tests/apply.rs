use std::fs;
use std::path::PathBuf;

use muzik_core::{BeetsConfig, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
use muzik_import::apply::{
    self, AlbumDecision, ApplyError, ApplyOptions, DuplicateDecision, MatchDecision,
};
use muzik_import::files::Placement;
use muzik_import::history::IncrementalHistory;
use muzik_import::plan::{
    Duplicate, DuplicateReason, ImportMode, ImportPlanner, PlanOptions, ReleaseProvider,
};
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
                release_track_id: Some("release-track-1".into()),
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

    fn lookup_recording(&self, _: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
        unreachable!()
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
fn singleton_mode_uses_singleton_path_without_an_album_row() {
    let (_temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner.plan_singletons(&[source]).unwrap();
    assert_eq!(plan.albums.len(), 1);
    assert_eq!(plan.albums[0].kind, ImportMode::Singleton);
    assert!(plan.albums[0].candidates.is_empty());
    let mut options = ApplyOptions::from_beets(&config, root).unwrap();
    options.paths.singleton = "Singles/$artist/$title".into();
    let result = apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::AsIs,
            duplicate: None,
        }],
        &options,
    )
    .unwrap();
    assert!(result.album_ids.is_empty());
    assert!(result.destinations[0].ends_with("Singles/Mara Vale/Song.flac"));
    assert!(result.destinations[0].exists());
    let item = library.item(result.item_ids[0]).unwrap().unwrap();
    assert_eq!(item.album_id(), None);
}

#[test]
fn move_import_removes_source_after_the_database_write() {
    let (_temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner
        .plan_singletons(std::slice::from_ref(&source))
        .unwrap();
    let mut options = ApplyOptions::from_beets(&config, root).unwrap();
    options.placement = Placement::Move;
    let result = apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::AsIs,
            duplicate: None,
        }],
        &options,
    )
    .unwrap();
    assert!(result.source_cleanup_failed.is_empty());
    assert!(!source.exists());
    assert!(result.destinations[0].exists());
    assert!(library.item(result.item_ids[0]).unwrap().is_some());
}

#[test]
fn successful_import_records_incremental_history() {
    let (temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let history = IncrementalHistory::open_or_seed(&temp.path().join("state.pickle"), &[]).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner
        .plan_with_options(
            std::slice::from_ref(&source),
            ImportMode::Album,
            PlanOptions {
                autotag: false,
                history: Some(history.clone()),
                incremental_skip_later: false,
            },
        )
        .unwrap();
    let options = ApplyOptions::from_beets(&config, root).unwrap();
    let result = apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::AsIs,
            duplicate: Some(DuplicateDecision::Keep),
        }],
        &options,
    )
    .unwrap();
    assert!(result.history_failed.is_empty());
    assert!(
        history
            .contains(&[source.parent().unwrap().canonicalize().unwrap()])
            .unwrap()
    );
}

#[test]
fn skipped_import_respects_incremental_skip_later() {
    let (temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    for skip_later in [false, true] {
        let history = IncrementalHistory::open_or_seed(
            &temp.path().join(format!("state-{skip_later}.pickle")),
            &[],
        )
        .unwrap();
        let planner = ImportPlanner {
            provider: &FixtureProvider,
            library: &library,
            match_config: &match_config,
            search_limit: 5,
        };
        let plan = planner
            .plan_with_options(
                std::slice::from_ref(&source),
                ImportMode::Album,
                PlanOptions {
                    autotag: false,
                    history: Some(history.clone()),
                    incremental_skip_later: skip_later,
                },
            )
            .unwrap();
        let options = ApplyOptions::from_beets(&config, root.clone()).unwrap();
        let result = apply::apply(
            &mut library,
            &plan,
            &[AlbumDecision {
                choice: MatchDecision::Skip,
                duplicate: None,
            }],
            &options,
        )
        .unwrap();
        assert_eq!(result.skipped_albums, 1);
        assert_eq!(
            history
                .contains(&[source.parent().unwrap().canonicalize().unwrap()])
                .unwrap(),
            !skip_later
        );
    }
}

#[test]
fn dry_run_does_not_write_incremental_history() {
    let (temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let statefile = temp.path().join("state.pickle");
    let history = IncrementalHistory::open_or_seed(&statefile, &[]).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner
        .plan_with_options(
            &[source],
            ImportMode::Album,
            PlanOptions {
                autotag: false,
                history: Some(history),
                incremental_skip_later: false,
            },
        )
        .unwrap();
    let mut options = ApplyOptions::from_beets(&config, root).unwrap();
    options.dry_run = true;
    apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::AsIs,
            duplicate: Some(DuplicateDecision::Keep),
        }],
        &options,
    )
    .unwrap();
    assert!(!IncrementalHistory::path_for_statefile(&statefile).exists());
}

#[test]
fn real_run_saves_seed_when_all_groups_were_already_imported() {
    let (temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let statefile = temp.path().join("state.pickle");
    let source_dir = source.parent().unwrap().canonicalize().unwrap();
    let history =
        IncrementalHistory::open_or_seed(&statefile, &[vec![source_dir.clone()]]).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner
        .plan_with_options(
            &[source],
            ImportMode::Album,
            PlanOptions {
                autotag: true,
                history: Some(history.clone()),
                incremental_skip_later: false,
            },
        )
        .unwrap();
    assert!(plan.albums.is_empty());
    let options = ApplyOptions::from_beets(&config, root).unwrap();
    let result = apply::apply(&mut library, &plan, &[], &options).unwrap();
    assert_eq!(result.skipped_incremental, 1);
    assert!(IncrementalHistory::path_for_statefile(&statefile).exists());
    assert!(history.contains(&[source_dir]).unwrap());
}

#[test]
fn custom_replace_rule_changes_import_destination() {
    let (_temp, source, database, root, _) = fixture();
    let config = BeetsConfig::from_layers(
        "",
        serde_json::json!({
            "replace": {"Song": "Tune"}
        }),
    )
    .unwrap();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner.plan(&[source]).unwrap();
    assert_eq!(plan.albums[0].kind, ImportMode::Album);
    let mut options = ApplyOptions::from_beets(&config, root).unwrap();
    options.dry_run = true;
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
    assert!(result.destinations[0].ends_with("02 Tune.flac"));
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
    let old_item_id = library
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
    let mut options = ApplyOptions::from_beets(&config, root).unwrap();

    options.dry_run = true;
    let preview = apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::Candidate(0),
            duplicate: Some(DuplicateDecision::Replace),
        }],
        &options,
    )
    .unwrap();
    options.dry_run = false;
    let mut old_path = muzik_library::Fields::new();
    old_path.insert(
        "path".into(),
        SqlValue::Blob(
            preview.destinations[0]
                .as_os_str()
                .as_encoded_bytes()
                .to_vec(),
        ),
    );
    library
        .update_item(old_item_id, &old_path, &Default::default())
        .unwrap();

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
    assert!(result.destinations[0].exists());
    assert!(result.cleanup_failed.is_empty());
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

#[test]
fn replace_keeps_album_from_unselected_release() {
    let (_temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let mut fields = muzik_library::Fields::new();
    fields.insert("album".into(), SqlValue::Text("Night Lines".into()));
    fields.insert("albumartist".into(), SqlValue::Text("Mara Vale".into()));
    fields.insert("mb_albumid".into(), SqlValue::Text("other-release".into()));
    let other_id = library.insert_album(&fields, &Default::default()).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner.plan(&[source]).unwrap();
    assert!(
        plan.albums[0]
            .duplicates
            .iter()
            .any(|duplicate| duplicate.album_id == other_id)
    );
    let mut options = ApplyOptions::from_beets(&config, root).unwrap();
    options.placement = Placement::Copy;
    apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::Candidate(0),
            duplicate: Some(DuplicateDecision::Replace),
        }],
        &options,
    )
    .unwrap();
    assert!(library.album(other_id).unwrap().is_some());
}

#[test]
fn replace_can_use_an_occupied_old_destination() {
    let (_temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let mut options = ApplyOptions::from_beets(&config, root).unwrap();
    options.placement = Placement::Copy;
    options.paths.default = "$albumartist/$album/$track $title".into();

    let planner = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let first_plan = planner.plan(std::slice::from_ref(&source)).unwrap();
    let first = apply::apply(
        &mut library,
        &first_plan,
        &[AlbumDecision {
            choice: MatchDecision::AsIs,
            duplicate: Some(DuplicateDecision::Keep),
        }],
        &options,
    )
    .unwrap();
    let old_album_id = first.album_ids[0];
    let old_destination = first.destinations[0].clone();
    assert!(old_destination.exists());

    let second_plan = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    }
    .plan(&[source])
    .unwrap();
    assert!(
        second_plan.albums[0]
            .duplicates
            .iter()
            .any(|duplicate| duplicate.album_id == old_album_id)
    );
    let second = apply::apply(
        &mut library,
        &second_plan,
        &[AlbumDecision {
            choice: MatchDecision::AsIs,
            duplicate: Some(DuplicateDecision::Replace),
        }],
        &options,
    )
    .unwrap();
    assert_eq!(second.destinations, vec![old_destination.clone()]);
    assert!(old_destination.exists());
    assert!(library.album(old_album_id).unwrap().is_none());
    assert_eq!(
        library.items_for_album(second.album_ids[0]).unwrap().len(),
        1
    );
}

#[test]
fn failed_replace_restores_occupied_old_destination() {
    let (_temp, source, database, root, config) = fixture();
    let mut library = Library::open_read_write(&database).unwrap();
    let match_config = MatchConfig::from_beets(&config).unwrap();
    let mut options = ApplyOptions::from_beets(&config, root).unwrap();
    options.placement = Placement::Copy;
    let first_plan = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    }
    .plan(std::slice::from_ref(&source))
    .unwrap();
    let first = apply::apply(
        &mut library,
        &first_plan,
        &[AlbumDecision {
            choice: MatchDecision::AsIs,
            duplicate: Some(DuplicateDecision::Keep),
        }],
        &options,
    )
    .unwrap();
    let old_destination = &first.destinations[0];
    let old_bytes = fs::read(old_destination).unwrap();
    let second_plan = ImportPlanner {
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    }
    .plan(std::slice::from_ref(&source))
    .unwrap();
    fs::remove_file(source).unwrap();

    assert!(
        apply::apply(
            &mut library,
            &second_plan,
            &[AlbumDecision {
                choice: MatchDecision::AsIs,
                duplicate: Some(DuplicateDecision::Replace),
            }],
            &options,
        )
        .is_err()
    );
    assert_eq!(fs::read(old_destination).unwrap(), old_bytes);
    assert!(library.album(first.album_ids[0]).unwrap().is_some());
}

#[test]
fn as_is_flac_matches_beets_path_and_library_fields() {
    let temp = tempfile::tempdir().unwrap();
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let source_dir = temp.path().join("incoming");
    fs::create_dir(&source_dir).unwrap();
    let source = source_dir.join("02 Tide & Stone.flac");
    fs::copy(
        crates.join("muzik-tags/tests/fixtures/mediafile.flac"),
        &source,
    )
    .unwrap();
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
        provider: &FixtureProvider,
        library: &library,
        match_config: &match_config,
        search_limit: 5,
    };
    let plan = planner.plan(&[source]).unwrap();
    let options = ApplyOptions::from_beets(&config, root.clone()).unwrap();

    let result = apply::apply(
        &mut library,
        &plan,
        &[AlbumDecision {
            choice: MatchDecision::AsIs,
            duplicate: None,
        }],
        &options,
    )
    .unwrap();

    assert_eq!(
        result.destinations[0],
        root.join("Mara Vale/Night Lines/02 Tide & Stone.flac")
    );
    let item = library.item(result.item_ids[0]).unwrap().unwrap();
    let album = library.album(result.album_ids[0]).unwrap().unwrap();
    assert_eq!(item.field("comp"), Some(&SqlValue::Integer(0)));
    assert_eq!(album.field("comp"), Some(&SqlValue::Integer(0)));
    assert_eq!(item.field("month"), Some(&SqlValue::Integer(4)));
    assert_eq!(item.field("day"), Some(&SqlValue::Integer(7)));
    assert_eq!(item.field("original_year"), Some(&SqlValue::Integer(2019)));
    assert_eq!(item.field("rg_track_gain"), Some(&SqlValue::Real(-5.25)));
    let tags = muzik_tags::read(&result.destinations[0], &[]).unwrap();
    assert_eq!(tags.fields.get("comp").map(String::as_str), Some("1"));
}
