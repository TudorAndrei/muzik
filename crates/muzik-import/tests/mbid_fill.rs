extern crate bolero as _;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use muzik_core::{BeetsConfig, RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
use muzik_import::mbid_fill::{FillSettings, albums_without_mbid, find_match, write_mbids};
use muzik_import::plan::ReleaseProvider;
use muzik_library::{Fields, Library, SqlValue, path_to_sql};
use muzik_metadata::{ReleaseArtistIds, ReleaseSearch, ReleaseSearchHit};

const TITLES: [&str; 2] = ["Morning Light", "Evening Rain"];

struct Release(ReleaseCandidate);

impl ReleaseProvider for Release {
    fn search_releases(
        &self,
        _: &ReleaseSearch,
        _: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        Ok(vec![ReleaseSearchHit {
            id: self.0.id.clone(),
            title: self.0.title.clone(),
            artist: self.0.artist.clone(),
            score: Some(100),
        }])
    }

    fn lookup_release(&self, _: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        Ok(self.0.clone())
    }

    fn lookup_recording(&self, _: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
        Err(muzik_metadata::Error::EmptyReleaseTitle)
    }

    fn lookup_release_artist_ids(
        &self,
        _: &str,
    ) -> Result<ReleaseArtistIds, muzik_metadata::Error> {
        Ok(ReleaseArtistIds {
            album_artist: Some("artist-1".into()),
            tracks: BTreeMap::from([("rt-1".into(), "artist-1".into())]),
        })
    }
}

fn release() -> ReleaseCandidate {
    ReleaseCandidate {
        id: ReleaseId("release-1".into()),
        title: "Quiet Hours".into(),
        artist: "Mara Vale".into(),
        tracks: TITLES
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

fn library(root: &Path) -> Result<(PathBuf, Vec<PathBuf>), Box<dyn std::error::Error>> {
    let blank =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../muzik-tags/tests/fixtures/blank.flac");
    let folder = root.join("Music/Mara Vale/Quiet Hours");
    fs::create_dir_all(&folder)?;
    let database = root.join("library.db");
    let mut library = Library::open_or_create(&database)?;
    let album_id = library.insert_album(
        &Fields::from([
            ("album".into(), text("Quiet Hours")),
            ("albumartist".into(), text("Mara Vale")),
        ]),
        &Fields::new(),
    )?;
    let mut paths = Vec::new();
    for (title, track) in TITLES.iter().zip(1_i64..) {
        let path = folder.join(format!("{track:02} {title}.flac"));
        fs::copy(&blank, &path)?;
        let mut tags = muzik_tags::TagData::default();
        tags.fields.insert("title".into(), (*title).into());
        tags.fields.insert("artist".into(), "Mara Vale".into());
        tags.fields.insert("album".into(), "Quiet Hours".into());
        muzik_tags::write(&path, &tags)?;
        library.insert_item(
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
        )?;
        paths.push(path);
    }
    Ok((database, paths))
}

#[test]
fn a_strong_match_writes_only_musicbrainz_ids_to_files_and_library() {
    let temp = tempfile::tempdir().unwrap();
    let (database, paths) = library(temp.path()).unwrap();
    let mut library = Library::open_read_write(&database).unwrap();
    let provider = Release(release());
    let settings =
        FillSettings::from_beets(&BeetsConfig::from_layers("", serde_json::json!({})).unwrap())
            .unwrap();
    let albums = albums_without_mbid(&library).unwrap();
    assert_eq!(albums.len(), 1);
    let found = find_match(&provider, &settings, &albums[0], &|| false).unwrap();
    assert!(found.strong);
    write_mbids(&mut library, &provider, &albums[0], &found.candidates[0]).unwrap();

    assert!(albums_without_mbid(&library).unwrap().is_empty());
    let album = library.albums().unwrap().remove(0);
    assert_eq!(album.field("mb_albumid"), Some(&text("release-1")));
    assert_eq!(album.field("mb_releasegroupid"), Some(&text("group-1")));
    assert_eq!(album.field("mb_albumartistid"), Some(&text("artist-1")));
    assert_eq!(album.field("album"), Some(&text("Quiet Hours")));
    let items = library.items().unwrap();
    assert_eq!(items[0].field("mb_trackid"), Some(&text("rec-1")));
    assert_eq!(items[0].field("mb_releasetrackid"), Some(&text("rt-1")));
    assert_eq!(items[0].field("mb_artistid"), Some(&text("artist-1")));
    assert_eq!(items[1].field("mb_trackid"), Some(&text("rec-2")));
    assert_eq!(items[1].field("mb_artistid"), Some(&SqlValue::Null));
    for (item, (path, title)) in items.iter().zip(paths.iter().zip(TITLES)) {
        assert_eq!(item.field("path"), Some(&path_to_sql(path)));
        assert_eq!(item.field("title"), Some(&text(title)));
        assert_eq!(item.field("year"), Some(&SqlValue::Null));
        let tags = muzik_tags::read(path, &[]).unwrap();
        assert_eq!(tags.fields.get("title").map(String::as_str), Some(title));
        assert_eq!(
            tags.fields.get("artist").map(String::as_str),
            Some("Mara Vale")
        );
        assert_eq!(
            tags.fields.get("album").map(String::as_str),
            Some("Quiet Hours")
        );
        assert_eq!(
            tags.fields.get("mb_albumid").map(String::as_str),
            Some("release-1")
        );
        assert!(!tags.fields.contains_key("date"));
        assert!(!tags.fields.contains_key("track"));
    }
    assert_eq!(
        muzik_tags::read(&paths[0], &[])
            .unwrap()
            .fields
            .get("mb_releasetrackid"),
        Some(&"rt-1".to_owned())
    );
}
