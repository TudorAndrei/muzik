use std::fs;
use std::path::PathBuf;

use muzik_core::{RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};
use muzik_import::plan::ReleaseProvider;
use muzik_import::sync;
use muzik_library::{Fields, Library, SqlValue};
use muzik_metadata::{ReleaseSearch, ReleaseSearchHit};

struct FixtureProvider;

impl ReleaseProvider for FixtureProvider {
    fn search_releases(
        &self,
        _: &ReleaseSearch,
        _: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        unreachable!()
    }

    fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        assert_eq!(id, "release-1");
        Ok(ReleaseCandidate {
            id: ReleaseId(id.into()),
            title: "New Album Title".into(),
            artist: "New Album Artist".into(),
            tracks: vec![TrackCandidate {
                recording_id: Some(RecordingId("recording-1".into())),
                title: "New Song (feat. Guest)".into(),
                artist: "New Artist".into(),
                length_seconds: None,
                index: 1,
                medium: 1,
                medium_index: 1,
            }],
            release_group_id: Some("group-1".into()),
            year: Some(2022),
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
fn sync_query_updates_one_album_and_its_audio_tags() {
    let temp = tempfile::tempdir().unwrap();
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned();
    let database = temp.path().join("library.db");
    fs::copy(
        crates.join("muzik-library/tests/fixtures/library.db"),
        &database,
    )
    .unwrap();
    let audio = temp.path().join("song.flac");
    fs::copy(crates.join("muzik-tags/tests/fixtures/blank.flac"), &audio).unwrap();
    let mut library = Library::open_read_write(&database).unwrap();
    let mut album_fields = Fields::new();
    album_fields.insert("album".into(), SqlValue::Text("SyncTest".into()));
    album_fields.insert("mb_albumid".into(), SqlValue::Text("release-1".into()));
    let album_id = library.insert_album(&album_fields, &Fields::new()).unwrap();
    let mut item_fields = Fields::new();
    item_fields.insert("album_id".into(), SqlValue::Integer(album_id));
    item_fields.insert("title".into(), SqlValue::Text("Old Song".into()));
    item_fields.insert("track".into(), SqlValue::Integer(1));
    item_fields.insert("disc".into(), SqlValue::Integer(1));
    item_fields.insert(
        "path".into(),
        SqlValue::Blob(audio.as_os_str().as_encoded_bytes().to_vec()),
    );
    let item_id = library.insert_item(&item_fields, &Fields::new()).unwrap();

    let result = sync::sync(&mut library, &FixtureProvider, "album:SyncTest", true).unwrap();

    assert_eq!(result.albums_updated, 1);
    assert_eq!(result.items_updated, 1);
    let item = library.item(item_id).unwrap().unwrap();
    assert_eq!(
        item.field("title"),
        Some(&SqlValue::Text("New Song".into()))
    );
    assert_eq!(
        item.field("artist"),
        Some(&SqlValue::Text("New Artist feat. Guest".into()))
    );
    let tags = muzik_tags::read(&audio, &[]).unwrap();
    assert_eq!(
        tags.fields.get("title").map(String::as_str),
        Some("New Song")
    );
    assert_eq!(
        tags.fields.get("artist").map(String::as_str),
        Some("New Artist feat. Guest")
    );
}
