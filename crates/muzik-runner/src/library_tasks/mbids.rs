use super::{TaskContext, TaskReport};
use muzik_core::{BeetsConfig, DecisionKind, MatchAnswer};
use muzik_import::mbid_fill::{self, AlbumMatch, FillSettings, MissingAlbum};
use muzik_import::plan::{PlannedCandidate, ReleaseProvider};
use muzik_library::Library;
use serde_json::{Value, json};

pub fn run(context: &TaskContext<'_>) -> crate::Result<TaskReport> {
    run_with(context, &mbid_fill::metadata_client())
}

fn run_with<P: ReleaseProvider>(
    context: &TaskContext<'_>,
    provider: &P,
) -> crate::Result<TaskReport> {
    let mut report = TaskReport::default();
    if !context.library().exists() {
        context.message("The library has no albums yet.");
        return Ok(report);
    }
    let config =
        BeetsConfig::load(&context.paths.config, json!({})).map_err(|error| error.to_string())?;
    let settings = FillSettings::from_beets(&config)?;
    let albums = mbid_fill::albums_without_mbid(&Library::open_read_only(context.library())?)?;
    if albums.is_empty() {
        context.message("Every album has a MusicBrainz ID.");
        return Ok(report);
    }
    let mut library = if context.dry_run {
        None
    } else {
        Some(Library::open_read_write(context.library())?)
    };
    let total = albums.len();
    context.progress_started("Fill MusicBrainz IDs", total);
    for (index, album) in albums.iter().enumerate() {
        let outcome = context.check_cancelled().and_then(|()| {
            fill_album(
                context,
                provider,
                &settings,
                library.as_mut(),
                album,
                &mut report,
            )
        });
        if let Err(error) = outcome {
            context.progress_finished(false);
            return Err(error);
        }
        report.processed = report.processed.saturating_add(1);
        context.progress(index.saturating_add(1), total);
    }
    context.progress_finished(true);
    Ok(report)
}

fn fill_album<P: ReleaseProvider>(
    context: &TaskContext<'_>,
    provider: &P,
    settings: &FillSettings,
    library: Option<&mut Library>,
    album: &MissingAlbum,
    report: &mut TaskReport,
) -> crate::Result<()> {
    let label = format!("{} - {}", album.artist, album.album);
    let found = match mbid_fill::find_match(provider, settings, album, &|| context.is_cancelled()) {
        Ok(found) => found,
        Err(muzik_import::Error::Cancelled) => return Err(crate::Error::Cancelled),
        Err(error) => {
            fail(context, report, format!("Cannot search {label}: {error}"));
            return Ok(());
        }
    };
    let Some(best) = found.candidates.first() else {
        skip(
            context,
            report,
            format!("No MusicBrainz match for {label}."),
        );
        return Ok(());
    };
    let candidate = if found.strong {
        best
    } else if context.dry_run {
        skip(
            context,
            report,
            format!("{label} needs a choice between MusicBrainz matches."),
        );
        return Ok(());
    } else if let Some(chosen) = choose(context, album, &found)? {
        chosen
    } else {
        skip(context, report, format!("Skipped {label}."));
        return Ok(());
    };
    let release = &candidate.release;
    let Some(library) = library else {
        report.changed = report.changed.saturating_add(1);
        context.message(format!(
            "Would add MusicBrainz release {} ({} - {}) to {label}.",
            release.id.0, release.artist, release.title
        ));
        return Ok(());
    };
    match mbid_fill::write_mbids(library, provider, album, candidate) {
        Ok(_) => {
            report.changed = report.changed.saturating_add(1);
            context.message(format!(
                "Added MusicBrainz release {} to {label}.",
                release.id.0
            ));
        }
        Err(error) => fail(
            context,
            report,
            format!("Cannot write MusicBrainz IDs for {label}: {error}"),
        ),
    }
    Ok(())
}

fn choose<'a>(
    context: &TaskContext<'_>,
    album: &MissingAlbum,
    found: &'a AlbumMatch,
) -> crate::Result<Option<&'a PlannedCandidate>> {
    let answer = context.decide(
        DecisionKind::ImportMatch,
        json!({"task":album_task(album, found)}),
    );
    context.check_cancelled()?;
    match serde_json::from_value(answer?) {
        Ok(MatchAnswer::Candidate(id)) => found
            .candidates
            .iter()
            .find(|candidate| candidate_id(candidate) == id)
            .map(Some)
            .ok_or_else(|| "Select a valid match ID.".into()),
        Ok(MatchAnswer::KeepCurrentTags | MatchAnswer::Skip) => Ok(None),
        Err(_) => Err("Select a valid match ID.".into()),
    }
}

fn album_task(album: &MissingAlbum, found: &AlbumMatch) -> Value {
    json!({
        "task_id":format!("mbids:{}", album.album_id),
        "paths":album.items.iter().map(|item| &item.path).collect::<Vec<_>>(),
        "is_album":true,
        "item_count":album.items.len(),
        "current_artist":album.artist,
        "current_album":album.album,
        "current_year":album.year,
        "matches":found.candidates.iter().map(|item| json!({
            "candidate_id":candidate_id(item),
            "artist":item.release.artist,
            "album":item.release.title,
            "year":item.release.year,
            "country":item.release.country,
            "media":item.release.media,
            "label":item.release.label,
            "track_count":item.release.tracks.len(),
            "distance":item.distance,
            "score":crate::choices::percent(1.0 - item.distance.clamp(0.0, 1.0)),
        })).collect::<Vec<_>>(),
    })
}

fn candidate_id(candidate: &PlannedCandidate) -> String {
    format!("release:{}", candidate.release.id.0)
}

fn skip(context: &TaskContext<'_>, report: &mut TaskReport, message: String) {
    report.skipped = report.skipped.saturating_add(1);
    context.message(message.clone());
    report.messages.push(message);
}

fn fail(context: &TaskContext<'_>, report: &mut TaskReport, message: String) {
    report.failed = report.failed.saturating_add(1);
    context.warn(message.clone());
    report.messages.push(message);
}

#[cfg(test)]
mod tests {
    use super::run_with;
    use crate::library_tasks::{TaskContext, TaskReport};
    use muzik_core::{
        DecisionKind, MatchAnswer, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate,
    };
    use muzik_import::beets::BeetsPaths;
    use muzik_import::plan::ReleaseProvider;
    use muzik_library::{Fields, Library, SqlValue, path_to_sql};
    use muzik_metadata::{ReleaseSearch, ReleaseSearchHit};
    use serde_json::{Value, json};
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    const TITLES: [&str; 2] = ["Morning Light", "Evening Rain"];

    struct Provider {
        release: ReleaseCandidate,
        offline: bool,
        searches: Cell<usize>,
    }

    impl Provider {
        fn new(release: ReleaseCandidate) -> Self {
            Self {
                release,
                offline: false,
                searches: Cell::new(0),
            }
        }
    }

    impl ReleaseProvider for Provider {
        fn search_releases(
            &self,
            _: &ReleaseSearch,
            _: u8,
        ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
            self.searches.set(self.searches.get().saturating_add(1));
            if self.offline {
                return Err(muzik_metadata::Error::EmptyReleaseTitle);
            }
            Ok(vec![ReleaseSearchHit {
                id: self.release.id.clone(),
                title: self.release.title.clone(),
                artist: self.release.artist.clone(),
                score: Some(100),
            }])
        }

        fn lookup_release(&self, _: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
            Ok(self.release.clone())
        }

        fn lookup_recording(&self, _: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
            Err(muzik_metadata::Error::EmptyReleaseTitle)
        }
    }

    fn release(album: &str, titles: [&str; 2]) -> ReleaseCandidate {
        ReleaseCandidate {
            id: ReleaseId("release-1".into()),
            title: album.into(),
            artist: "Mara Vale".into(),
            tracks: titles
                .iter()
                .zip(1_u32..)
                .map(|(title, index)| TrackCandidate {
                    recording_id: Some(RecordingId(format!("rec-{index}"))),
                    release_track_id: Some(format!("rt-{index}")),
                    title: (*title).into(),
                    artist: "Mara Vale".into(),
                    length_seconds: Some(200.0),
                    index,
                    medium: 1,
                    medium_index: index,
                })
                .collect(),
            release_group_id: Some("group-1".into()),
            year: Some(2020),
            country: None,
            media: Some("Digital Media".into()),
            label: None,
            catalog_number: None,
            disambiguation: None,
            is_various_artists: false,
        }
    }

    fn text(value: &str) -> SqlValue {
        SqlValue::Text(value.into())
    }

    fn library(root: &Path, mbid: Option<&str>) -> (BeetsPaths, Vec<PathBuf>) {
        let paths = BeetsPaths {
            config: root.join("config.yaml"),
            library: root.join("library.db"),
            directory: root.join("Music"),
            statefile: root.join("state.pickle"),
        };
        let blank = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../muzik-tags/tests/fixtures/blank.flac");
        let folder = paths.directory.join("Mara Vale/Quiet Hours");
        fs::create_dir_all(&folder).unwrap();
        let mut library = Library::open_or_create(&paths.library).unwrap();
        let mut album = Fields::from([
            ("album".into(), text("Quiet Hours")),
            ("albumartist".into(), text("Mara Vale")),
        ]);
        if let Some(id) = mbid {
            album.insert("mb_albumid".into(), text(id));
        }
        let album_id = library.insert_album(&album, &Fields::new()).unwrap();
        let mut files = Vec::new();
        for (title, track) in TITLES.iter().zip(1_i64..) {
            let path = folder.join(format!("{track:02} {title}.flac"));
            fs::copy(&blank, &path).unwrap();
            library
                .insert_item(
                    &Fields::from([
                        ("album_id".into(), SqlValue::Integer(album_id)),
                        ("title".into(), text(title)),
                        ("artist".into(), text("Mara Vale")),
                        ("album".into(), text("Quiet Hours")),
                        ("albumartist".into(), text("Mara Vale")),
                        ("track".into(), SqlValue::Integer(track)),
                        ("disc".into(), SqlValue::Integer(1)),
                        ("length".into(), SqlValue::Real(200.0)),
                        ("path".into(), path_to_sql(&path)),
                    ]),
                    &Fields::new(),
                )
                .unwrap();
            files.push(path);
        }
        (paths, files)
    }

    fn fill(
        paths: &BeetsPaths,
        provider: &Provider,
        dry_run: bool,
        answer: &Value,
    ) -> (TaskReport, Vec<(DecisionKind, Value)>) {
        let cancelled = AtomicBool::new(false);
        let mut asked = Vec::new();
        let report = {
            let mut events = |_| {};
            let mut decide = |kind, payload| {
                asked.push((kind, payload));
                Ok(answer.clone())
            };
            let context = TaskContext::new(paths, dry_run, &cancelled, &mut events, &mut decide);
            run_with(&context, provider).unwrap()
        };
        (report, asked)
    }

    fn album_field(paths: &BeetsPaths, name: &str) -> SqlValue {
        Library::open_read_only(&paths.library)
            .unwrap()
            .albums()
            .unwrap()
            .remove(0)
            .fields
            .remove(name)
            .unwrap()
    }

    #[test]
    fn a_strong_match_fills_the_ids_without_a_question() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, files) = library(dir.path(), None);
        let provider = Provider::new(release("Quiet Hours", TITLES));
        let (report, asked) = fill(&paths, &provider, false, &json!(MatchAnswer::Skip));
        assert!(asked.is_empty());
        assert_eq!((report.processed, report.changed), (1, 1));
        assert_eq!(album_field(&paths, "mb_albumid"), text("release-1"));
        assert_eq!(album_field(&paths, "album"), text("Quiet Hours"));
        let items = Library::open_read_only(&paths.library)
            .unwrap()
            .items()
            .unwrap();
        assert_eq!(items[1].field("mb_trackid"), Some(&text("rec-2")));
        assert_eq!(items[1].field("title"), Some(&text(TITLES[1])));
        assert_eq!(items[1].field("path"), Some(&path_to_sql(&files[1])));
    }

    #[test]
    fn a_weak_match_asks_and_a_skip_keeps_the_album_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, files) = library(dir.path(), None);
        let before = fs::read(&files[0]).unwrap();
        let provider = Provider::new(release("Loud Days", ["Static", "Noise Floor"]));
        let (report, asked) = fill(&paths, &provider, false, &json!(MatchAnswer::Skip));
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, DecisionKind::ImportMatch);
        let task = &asked[0].1["task"];
        assert_eq!(task["current_album"], "Quiet Hours");
        assert_eq!(task["item_count"], 2);
        assert_eq!(task["matches"][0]["candidate_id"], "release:release-1");
        assert_eq!(
            (report.processed, report.skipped, report.changed),
            (1, 1, 0)
        );
        assert_eq!(album_field(&paths, "mb_albumid"), SqlValue::Null);
        assert_eq!(fs::read(&files[0]).unwrap(), before);
    }

    #[test]
    fn a_chosen_weak_match_fills_the_ids() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, _) = library(dir.path(), None);
        let provider = Provider::new(release("Loud Days", ["Static", "Noise Floor"]));
        let answer = json!(MatchAnswer::Candidate("release:release-1".into()));
        let (report, asked) = fill(&paths, &provider, false, &answer);
        assert_eq!(asked.len(), 1);
        assert_eq!(report.changed, 1);
        assert_eq!(album_field(&paths, "mb_releasegroupid"), text("group-1"));
        assert_eq!(album_field(&paths, "album"), text("Quiet Hours"));
    }

    #[test]
    fn an_album_with_a_release_id_is_not_searched() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, _) = library(dir.path(), Some("known"));
        let provider = Provider::new(release("Quiet Hours", TITLES));
        let (report, asked) = fill(&paths, &provider, false, &json!(MatchAnswer::Skip));
        assert_eq!(report, TaskReport::default());
        assert!(asked.is_empty());
        assert_eq!(provider.searches.get(), 0);
        assert_eq!(album_field(&paths, "mb_albumid"), text("known"));
    }

    #[test]
    fn a_dry_run_reports_matches_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, files) = library(dir.path(), None);
        let before = fs::read(&files[0]).unwrap();
        let database = fs::read(&paths.library).unwrap();
        let strong = Provider::new(release("Quiet Hours", TITLES));
        let (report, asked) = fill(&paths, &strong, true, &json!(MatchAnswer::Skip));
        assert!(asked.is_empty());
        assert_eq!(report.changed, 1);
        let weak = Provider::new(release("Loud Days", ["Static", "Noise Floor"]));
        let (report, asked) = fill(&paths, &weak, true, &json!(MatchAnswer::Skip));
        assert!(asked.is_empty());
        assert_eq!((report.skipped, report.changed), (1, 0));
        assert_eq!(fs::read(&files[0]).unwrap(), before);
        assert_eq!(fs::read(&paths.library).unwrap(), database);
    }

    #[test]
    fn a_search_failure_counts_as_failed_and_the_task_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let (paths, _) = library(dir.path(), None);
        let mut provider = Provider::new(release("Quiet Hours", TITLES));
        provider.offline = true;
        let (report, _) = fill(&paths, &provider, false, &json!(MatchAnswer::Skip));
        assert_eq!((report.processed, report.failed), (1, 1));
        assert_eq!(album_field(&paths, "mb_albumid"), SqlValue::Null);
    }
}
