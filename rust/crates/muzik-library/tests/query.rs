use muzik_library::query::Query;
use muzik_library::{Album, Fields, Item, Library, SqlValue};
use serde_json::Value;
use std::path::PathBuf;

#[test]
fn parsed_queries_and_item_ids_match_beets() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/query.json")).unwrap();
    assert_eq!(fixture["beets_version"], "2.13.1");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library.db");
    let library = Library::open_read_only(&path).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let query = Query::parse(text).unwrap();
        let expected_groups = case["groups"].as_array().unwrap();
        assert_eq!(query.groups.len(), expected_groups.len(), "{text}");
        for (group, expected_terms) in query.groups.iter().zip(expected_groups) {
            let expected_terms = expected_terms.as_array().unwrap();
            assert_eq!(group.len(), expected_terms.len(), "{text}");
            for (term, expected) in group.iter().zip(expected_terms) {
                assert_eq!(term.field.as_deref(), expected["field"].as_str(), "{text}");
                assert_eq!(
                    term.pattern,
                    expected["pattern"].as_str().unwrap(),
                    "{text}"
                );
                assert_eq!(format!("{:?}", term.kind), expected["kind"], "{text}");
                assert_eq!(
                    term.negated,
                    expected["negated"].as_bool().unwrap(),
                    "{text}"
                );
            }
        }
        let expected_sorts = case["sorts"].as_array().unwrap();
        assert_eq!(query.sorts.len(), expected_sorts.len(), "{text}");
        for (sort, expected) in query.sorts.iter().zip(expected_sorts) {
            assert_eq!(sort.field, expected["field"].as_str().unwrap(), "{text}");
            assert_eq!(
                sort.ascending,
                expected["ascending"].as_bool().unwrap(),
                "{text}"
            );
        }
        let ids: Vec<i64> = library
            .query_items(text)
            .unwrap()
            .iter()
            .map(|item| item.id)
            .collect();
        let expected_ids: Vec<i64> = case["item_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|id| id.as_i64().unwrap())
            .collect();
        assert_eq!(ids, expected_ids, "{text}");
    }
}

#[test]
fn queries_album_fields() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library.db");
    let library = Library::open_read_only(&path).unwrap();
    assert_eq!(library.query_albums("album::^Alb").unwrap()[0].id, 1);
    assert!(library.query_albums("album:Other").unwrap().is_empty());
}

#[test]
fn default_item_sort_uses_beets_artist_sort_field() {
    let make_item = |id: i64, artist: &str, artist_sort: &str| Item {
        id,
        fields: Fields::from([
            ("artist".into(), SqlValue::Text(artist.into())),
            ("artist_sort".into(), SqlValue::Text(artist_sort.into())),
        ]),
        attributes: Fields::new(),
    };
    let mut items = vec![make_item(1, "Alpha", "Zulu"), make_item(2, "Zulu", "Alpha")];
    Query::parse("").unwrap().sort_items(&mut items);
    assert_eq!(items.iter().map(|item| item.id).collect::<Vec<_>>(), [2, 1]);

    let make_album = |id: i64, artist: &str, artist_sort: &str| Album {
        id,
        fields: Fields::from([
            ("albumartist".into(), SqlValue::Text(artist.into())),
            (
                "albumartist_sort".into(),
                SqlValue::Text(artist_sort.into()),
            ),
        ]),
        attributes: Fields::new(),
    };
    let mut albums = vec![
        make_album(1, "Alpha", "Zulu"),
        make_album(2, "Zulu", "Alpha"),
    ];
    Query::parse("").unwrap().sort_albums(&mut albums);
    assert_eq!(
        albums.iter().map(|album| album.id).collect::<Vec<_>>(),
        [2, 1]
    );
}
