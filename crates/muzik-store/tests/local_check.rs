use muzik_core::QualityPolicy;
use muzik_store::watchlist::{
    LocalChecks, ReconcileOptions, Repository, Stage, StageStatus, Watchlist,
};
use serde_json::json;
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture {
    root: tempfile::TempDir,
    generation: Arc<AtomicU64>,
    checks: LocalChecks,
}
impl Fixture {
    fn new() -> muzik_store::Result<Self> {
        let root = tempfile::tempdir()?;
        let generation = Arc::new(AtomicU64::new(0));
        let checks = LocalChecks::new(Arc::clone(&generation));
        let fixture = Self {
            root,
            generation,
            checks,
        };
        fixture.repository().save(&Watchlist::from_value(json!({"version":3,"playlists":[{
            "playlist_id":"PL1", "url":"https://www.youtube.com/playlist?list=PL1", "title":"Original",
            "items":[{"position":1,"title":"Song","video_id":"abcdefghijk", "stages":{"download":{"status":"running"}}}]
        }]}))?)?;
        Ok(fixture)
    }
    fn repository(&self) -> Repository {
        Repository::new(self.root.path().join("state.db"))
    }
    fn options(&self) -> ReconcileOptions<'_> {
        ReconcileOptions {
            output: self.root.path(),
            splits: self.root.path(),
            cache: self.root.path(),
            config: None,
            no_organize: false,
            no_split: false,
            quality_policy: QualityPolicy::Off,
        }
    }
}

#[test]
fn a_local_check_returns_saved_state_then_saves_and_publishes_checked_state() {
    let fixture = Fixture::new().unwrap();
    let (saved, check) = fixture
        .checks
        .prepare::<muzik_store::Error>(fixture.repository(), || Ok(fixture.options().into()))
        .unwrap();
    assert_eq!(
        saved["playlists"][0]["items"][0]["stages"]["download"]["status"],
        "running"
    );
    let mut published = None;
    check.run(|| false, |result| published = Some(result.unwrap()));
    let visible = published.unwrap();
    assert_eq!(
        visible["playlists"][0]["items"][0]["stages"]["download"]["status"],
        "not_started"
    );
    let saved = fixture.repository().load().unwrap();
    assert_eq!(
        saved.playlists[0].items[0].status(Stage::Download),
        StageStatus::NotStarted
    );
}

#[test]
fn busy_or_superseded_checks_do_not_change_or_publish_saved_state() {
    for reason in [
        "busy",
        "new_load",
        "during_settings",
        "failed_load",
        "edit",
        "runner",
    ] {
        let fixture = Fixture::new().unwrap();
        let (_, check) = fixture
            .checks
            .prepare::<muzik_store::Error>(fixture.repository(), || {
                if reason == "during_settings" {
                    let _ = fixture
                        .checks
                        .prepare::<muzik_store::Error>(fixture.repository(), || {
                            Ok(fixture.options().into())
                        })?;
                }
                Ok(fixture.options().into())
            })
            .unwrap();
        match reason {
            "new_load" => {
                let _ = fixture
                    .checks
                    .prepare::<muzik_store::Error>(fixture.repository(), || {
                        Ok(fixture.options().into())
                    })
                    .unwrap();
            }
            "failed_load" => {
                assert!(
                    fixture
                        .checks
                        .prepare::<muzik_store::Error>(fixture.repository(), || Err(
                            "settings unavailable".into()
                        ))
                        .is_err()
                );
            }
            "edit" => {
                fixture
                    .checks
                    .edit(|| fixture.repository().rename("PL1", "Edited"))
                    .unwrap();
            }
            "runner" => {
                fixture.generation.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        let before = fixture.repository().load().unwrap();
        let revision = fixture.repository().revision().unwrap();
        let mut published = false;
        check.run(|| reason == "busy", |_| published = true);
        assert!(!published, "{reason}");
        assert_eq!(fixture.repository().load().unwrap(), before, "{reason}");
        assert_eq!(
            fixture.repository().revision().unwrap(),
            revision,
            "{reason}"
        );
    }
}

#[test]
fn a_revision_conflict_reloads_the_source_edit_before_saving() {
    let fixture = Fixture::new().unwrap();
    let (_, check) = fixture
        .checks
        .prepare::<muzik_store::Error>(fixture.repository(), || Ok(fixture.options().into()))
        .unwrap();
    let polls = Cell::new(0_u32);
    let mut published = None;
    check.run(
        || {
            polls.set(polls.get().saturating_add(1));
            // Simulate another process editing after reconciliation, before the checked save.
            if polls.get() == 3 {
                fixture
                    .repository()
                    .rename("PL1", "Edited by another process")
                    .unwrap();
            }
            false
        },
        |result| published = Some(result.unwrap()),
    );
    assert_eq!(
        published.unwrap()["playlists"][0]["title"],
        "Edited by another process"
    );
    let saved = fixture.repository().load().unwrap();
    assert_eq!(
        saved.playlists[0].title.as_deref(),
        Some("Edited by another process")
    );
    assert_eq!(
        saved.playlists[0].items[0].status(Stage::Download),
        StageStatus::NotStarted
    );
}

#[test]
fn repeated_revision_conflicts_report_an_error_and_preserve_the_external_edit() {
    let fixture = Fixture::new().unwrap();
    let (_, check) = fixture
        .checks
        .prepare::<muzik_store::Error>(fixture.repository(), || Ok(fixture.options().into()))
        .unwrap();
    let edits = Cell::new(0_u32);
    let mut published = None;
    check.run(
        || {
            edits.set(edits.get().saturating_add(1));
            fixture
                .repository()
                .rename("PL1", &format!("Edit {}", edits.get()))
                .unwrap();
            false
        },
        |result| published = Some(result),
    );
    assert!(
        published
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("changed during the local check")
    );
    let saved = fixture.repository().load().unwrap();
    assert_eq!(
        saved.playlists[0].title,
        Some(format!("Edit {}", edits.get()))
    );
    assert_eq!(
        saved.playlists[0].items[0].status(Stage::Download),
        StageStatus::Running
    );
}
