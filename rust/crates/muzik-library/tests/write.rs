use muzik_library::{Error, Fields, Library, SqlValue as Value};
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn fixture() -> (TempDir, Library) {
    let directory = tempfile::tempdir().unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library.db");
    let database = directory.path().join("library.db");
    fs::copy(source, &database).unwrap();
    let library = Library::open_read_write(&database).unwrap();
    (directory, library)
}

fn fields(pairs: &[(&str, Value)]) -> Fields {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect()
}

#[test]
fn inserts_and_updates_fixed_and_flexible_fields() {
    let (directory, mut library) = fixture();
    let album_id = library
        .transaction(|writer| {
            let album_id = writer.insert_album(
                &fields(&[("album", Value::Text("New Album".into()))]),
                &fields(&[("review", Value::Text("pending".into()))]),
            )?;
            let item_id = writer.insert_item(
                &fields(&[
                    ("album_id", Value::Integer(album_id)),
                    ("title", Value::Text("New Track".into())),
                    ("path", Value::Blob(b"/fixture/new.mp3".to_vec())),
                ]),
                &fields(&[("muzik_source_id", Value::Text("source-123".into()))]),
            )?;
            writer.update_item(
                item_id,
                &fields(&[("title", Value::Text("Changed".into()))]),
                &Fields::new(),
            )?;
            Ok(album_id)
        })
        .unwrap();
    let item = library.items_for_album(album_id).unwrap().pop().unwrap();
    assert_eq!(item.field("title"), Some(&Value::Text("Changed".into())));
    assert_eq!(
        item.attribute("muzik_source_id"),
        Some(&Value::Text("source-123".into()))
    );
    assert_eq!(
        library
            .album(album_id)
            .unwrap()
            .unwrap()
            .attribute("review"),
        Some(&Value::Text("pending".into()))
    );
    assert!(directory.path().join("library.db.native-backup").exists());
    library.remove_item(item.id).unwrap();
    assert!(library.album(album_id).unwrap().is_none());
}

#[test]
fn failed_group_write_rolls_back_all_rows() {
    let (_directory, mut library) = fixture();
    let before = library.albums().unwrap().len();
    let result = library.transaction(|writer| {
        writer.insert_album(
            &fields(&[("album", Value::Text("Rollback".into()))]),
            &Fields::new(),
        )?;
        writer.insert_item(
            &fields(&[("not_a_beets_field", Value::Text("bad".into()))]),
            &Fields::new(),
        )?;
        Ok(())
    });
    assert!(
        matches!(result, Err(Error::InvalidField { .. })),
        "{result:?}"
    );
    assert_eq!(library.albums().unwrap().len(), before);
}

#[test]
fn prune_checks_fraction_before_removing_any_rows() {
    let (directory, mut library) = fixture();
    let music = directory.path().join("music");
    fs::create_dir(&music).unwrap();
    let present = music.join("present.mp3");
    fs::write(&present, b"music").unwrap();
    let first = library
        .insert_item(
            &fields(&[(
                "path",
                Value::Blob(present.as_os_str().as_encoded_bytes().to_vec()),
            )]),
            &Fields::new(),
        )
        .unwrap();
    let total = library.items().unwrap().len();
    let result = library.prune_missing_items(&music, 0.5);
    assert!(matches!(
        result,
        Err(Error::PruneAborted {
            missing: 2,
            total: 3
        })
    ));
    assert_eq!(library.items().unwrap().len(), total);
    assert_eq!(library.prune_missing_items(&music, 1.0).unwrap(), 2);
    assert_eq!(library.items().unwrap().len(), 1);
    assert_eq!(
        library.item(first).unwrap().unwrap().field("path"),
        Some(&Value::Blob(
            present.as_os_str().as_encoded_bytes().to_vec()
        ))
    );
}
