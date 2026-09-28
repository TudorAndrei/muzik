use muzik_library::Library;
use rusqlite::types::Value;
use serde_json::Value as JsonValue;
use std::path::PathBuf;

fn fixture() -> (Library, JsonValue) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library.db");
    let library = Library::open_read_only(&path).expect("open beets fixture");
    let expected = serde_json::from_str(include_str!("fixtures/library.json")).unwrap();
    (library, expected)
}

#[test]
fn reads_items_and_flexible_attributes() {
    let (library, expected) = fixture();
    let items = library.items().expect("read items");
    assert_eq!(items.len(), 2);
    let first = &items[0];
    assert_eq!(first.id, expected["first_id"].as_i64().unwrap());
    assert_eq!(first.field("title"), Some(&Value::Text("Track".into())));
    assert_eq!(first.field("length"), Some(&Value::Real(183.5)));
    assert_eq!(first.album_id(), expected["album_id"].as_i64());
    assert_eq!(
        first.attribute("muzik_source_id"),
        Some(&Value::Text("video-123".into()))
    );
    assert!(matches!(first.field("path"), Some(Value::Blob(_))));
    assert_eq!(library.item(first.id).unwrap(), Some(first.clone()));

    let second = &items[1];
    assert_eq!(second.id, expected["second_id"].as_i64().unwrap());
    assert_eq!(second.album_id(), None);
    assert_eq!(
        second.attribute("muzik_source_id"),
        Some(&Value::Text("video-456".into()))
    );
}

#[test]
fn reads_albums_and_album_items() {
    let (library, expected) = fixture();
    let albums = library.albums().expect("read albums");
    assert_eq!(albums.len(), 1);
    let album = &albums[0];
    assert_eq!(album.id, expected["album_id"].as_i64().unwrap());
    assert_eq!(album.field("album"), Some(&Value::Text("Album".into())));
    assert_eq!(
        album.attribute("fixture_note"),
        Some(&Value::Text("album-flex".into()))
    );
    assert_eq!(library.album(album.id).unwrap(), Some(album.clone()));
    let items = library.items_for_album(album.id).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, expected["first_id"].as_i64().unwrap());
}

#[test]
fn beets_sql_functions_accept_text_and_blobs() {
    let (library, _) = fixture();
    let connection = library.connection();
    let regex: bool = connection
        .query_row("SELECT regexp('foobar', 'foo(?=bar)')", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(regex);
    let transliterated: String = connection
        .query_row("SELECT unidecode('Björk')", [], |row| row.get(0))
        .unwrap();
    assert_eq!(transliterated, "Bjork");
    let lowered: Vec<u8> = connection
        .query_row("SELECT bytelower(X'414243')", [], |row| row.get(0))
        .unwrap();
    assert_eq!(lowered, b"abc");
}
