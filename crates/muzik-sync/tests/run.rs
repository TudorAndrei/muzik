use muzik_core::SyncPreset;
use muzik_core::audio::Codec;
use muzik_library::{Fields, Library, SqlValue};
use muzik_media::quality::MeasuredQuality;
use muzik_store::Connection;
use muzik_sync::{Error, Options, Prepared, Selection, Target};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Layout {
    _dir: tempfile::TempDir,
    library: PathBuf,
    card: PathBuf,
    target: Target,
}

fn layout() -> Result<Layout, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let library = dir.path().join("library");
    let card = dir.path().join("card");
    fs::create_dir_all(library.join("Artist"))?;
    fs::create_dir_all(&card)?;
    let target = Target {
        path: card.clone(),
        preset: SyncPreset::EchoMini,
        bitrate: None,
        covers: false,
    };
    Ok(Layout {
        _dir: dir,
        library,
        card,
        target,
    })
}

fn probe(path: &Path) -> Option<MeasuredQuality> {
    if path.file_name().is_some_and(|name| name == "broken.mp3") {
        return None;
    }
    Some(MeasuredQuality {
        format: Codec::Mp3,
        lossless: false,
        bitrate_kbps: Some(128),
        sample_rate: Some(44_100),
        bit_depth: None,
        channels: Some(2),
        size: Some(5),
    })
}

fn add_tracks(layout: &Layout, names: &[&str]) -> Result<Selection, Box<dyn std::error::Error>> {
    let mut tracks = Vec::new();
    for name in names {
        let path = layout.library.join("Artist").join(name);
        fs::write(&path, b"audio")?;
        tracks.push(path);
    }
    Ok(Selection {
        tracks,
        covers: Vec::new(),
    })
}

fn stray(layout: &Layout) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let folder = layout.card.join("Old");
    fs::create_dir_all(&folder)?;
    let path = folder.join("old.mp3");
    fs::write(&path, b"1234567")?;
    Ok(path)
}

fn prepare(
    layout: &Layout,
    selection: &Selection,
    connection: &Connection,
    delete: bool,
) -> muzik_sync::Result<Prepared> {
    muzik_sync::prepare(
        &layout.target,
        &layout.library,
        selection,
        connection,
        Options { delete, jobs: 1 },
        &|path| Ok(probe(path)),
    )
}

fn destinations(prepared: &Prepared) -> Vec<PathBuf> {
    prepared
        .plan()
        .pending
        .iter()
        .map(|transfer| transfer.destination.clone())
        .collect()
}

#[test]
fn prepare_blocks_delete_when_a_track_cannot_be_read() {
    let layout = layout().unwrap();
    let selection = add_tracks(&layout, &["good.mp3", "broken.mp3"]).unwrap();
    stray(&layout).unwrap();
    let connection = muzik_store::db::open_in_memory().unwrap();
    let prepared = prepare(&layout, &selection, &connection, true).unwrap();
    assert!(!prepared.delete());
    assert!(prepared.delete_blocked());
    assert!(prepared.stale().is_empty());
    assert_eq!(prepared.freed(), 0);
}

#[test]
fn prepare_lists_stale_files_when_delete_is_safe() {
    let layout = layout().unwrap();
    let selection = add_tracks(&layout, &["good.mp3"]).unwrap();
    let old = stray(&layout).unwrap();
    let connection = muzik_store::db::open_in_memory().unwrap();
    let prepared = prepare(&layout, &selection, &connection, true).unwrap();
    assert!(prepared.delete());
    assert!(!prepared.delete_blocked());
    assert_eq!(prepared.stale(), [old]);
    assert_eq!(prepared.freed(), 7);
}

#[test]
fn apply_refuses_a_target_that_is_gone_before_it_changes_files() {
    let layout = layout().unwrap();
    let selection = add_tracks(&layout, &["good.mp3"]).unwrap();
    stray(&layout).unwrap();
    let connection = muzik_store::db::open_in_memory().unwrap();
    let prepared = prepare(&layout, &selection, &connection, true).unwrap();
    let unplugged = layout.card.with_file_name("unplugged");
    fs::rename(&layout.card, &unplugged).unwrap();
    let result = muzik_sync::apply(prepared, connection, &|_| {});
    assert!(matches!(result, Err(Error::TargetMissing(path)) if path == layout.card));
    assert!(unplugged.join("Old/old.mp3").is_file());
    assert!(!layout.card.exists());
}

#[test]
fn apply_deletes_stale_files_and_copies_pending() {
    let layout = layout().unwrap();
    let selection = add_tracks(&layout, &["good.mp3", "other.mp3"]).unwrap();
    let old = stray(&layout).unwrap();
    let connection = muzik_store::db::open_in_memory().unwrap();
    let prepared = prepare(&layout, &selection, &connection, true).unwrap();
    let destinations = destinations(&prepared);
    let calls = AtomicUsize::new(0);
    let report = muzik_sync::apply(prepared, connection, &|done| {
        assert!(done.result.is_ok());
        assert!(done.record_error.is_none());
        calls.fetch_add(1, Ordering::Relaxed);
    })
    .unwrap();
    let pending = destinations.len();
    assert_eq!(pending, 2);
    assert!(!old.exists());
    assert!(!layout.card.join("Old").exists());
    for destination in &destinations {
        assert!(destination.is_file());
    }
    assert_eq!(report.written, pending);
    assert_eq!(report.failed, 0);
    assert_eq!(calls.load(Ordering::Relaxed), pending);
}

#[test]
fn apply_skips_stale_files_that_are_already_gone() {
    let layout = layout().unwrap();
    let selection = add_tracks(&layout, &["good.mp3", "other.mp3"]).unwrap();
    let old = stray(&layout).unwrap();
    let connection = muzik_store::db::open_in_memory().unwrap();
    let prepared = prepare(&layout, &selection, &connection, true).unwrap();
    assert_eq!(prepared.stale(), std::slice::from_ref(&old));
    fs::remove_file(&old).unwrap();
    let destinations = destinations(&prepared);
    let report = muzik_sync::apply(prepared, connection, &|_| {}).unwrap();
    let pending = destinations.len();
    assert_eq!(pending, 2);
    for destination in &destinations {
        assert!(destination.is_file());
    }
    assert_eq!(report.written, pending);
    assert_eq!(report.failed, 0);
}

#[test]
fn apply_counts_transfers_whose_encoding_was_not_saved() {
    let layout = layout().unwrap();
    let selection = add_tracks(&layout, &["good.mp3", "other.mp3"]).unwrap();
    let migrated = muzik_store::db::open_in_memory().unwrap();
    let prepared = prepare(&layout, &selection, &migrated, false).unwrap();
    let bare = Connection::open_in_memory().unwrap();
    let unsaved = AtomicUsize::new(0);
    let pending = prepared.plan().pending.len();
    let report = muzik_sync::apply(prepared, bare, &|done| {
        assert!(done.result.is_ok());
        if done.record_error.is_some() {
            unsaved.fetch_add(1, Ordering::Relaxed);
        }
    })
    .unwrap();
    assert_eq!(pending, 2);
    assert_eq!(report.written, pending);
    assert_eq!(report.failed, 0);
    assert_eq!(report.unrecorded, pending);
    assert_eq!(unsaved.load(Ordering::Relaxed), pending);
}

#[test]
fn select_reads_track_and_cover_paths() {
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
    let directory = temp.path().join("music");
    fs::create_dir_all(&directory).unwrap();
    let art = directory.join("cover.jpg");
    fs::write(&art, b"art").unwrap();
    let bytes = |path: &Path| SqlValue::Blob(path.as_os_str().as_encoded_bytes().to_vec());
    let mut library = Library::open_read_write(&database).unwrap();
    let mut album_fields = Fields::new();
    album_fields.insert("album".into(), SqlValue::Text("SelectRun".into()));
    album_fields.insert("artpath".into(), bytes(&art));
    let album_id = library.insert_album(&album_fields, &Fields::new()).unwrap();
    let mut item_fields = Fields::new();
    item_fields.insert("album_id".into(), SqlValue::Integer(album_id));
    item_fields.insert("album".into(), SqlValue::Text("SelectRun".into()));
    item_fields.insert("title".into(), SqlValue::Text("Song".into()));
    item_fields.insert("path".into(), bytes(Path::new("Artist/song.mp3")));
    library.insert_item(&item_fields, &Fields::new()).unwrap();
    drop(library);

    let library = Library::open_read_only(&database).unwrap();
    let selection = muzik_sync::select(&library, &directory, "album:SelectRun", true).unwrap();
    assert_eq!(selection.tracks, vec![directory.join("Artist/song.mp3")]);
    assert_eq!(selection.covers, vec![art]);
    let without = muzik_sync::select(&library, &directory, "album:SelectRun", false).unwrap();
    assert!(without.covers.is_empty());
}
