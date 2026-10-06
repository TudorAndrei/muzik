use muzik_core::audio::Codec;
use muzik_core::SyncPreset;
use muzik_library::{Fields, Library, SqlValue};
use muzik_media::quality::MeasuredQuality;
use muzik_store::Connection;
use muzik_sync::{Options, Plan, Prepared, Selection, Target};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

type Outcome = Result<(), Box<dyn std::error::Error>>;

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

fn probe(path: &Path) -> Result<Option<MeasuredQuality>, String> {
    if path.file_name().is_some_and(|name| name == "broken.mp3") {
        return Ok(None);
    }
    Ok(Some(MeasuredQuality {
        format: Codec::Mp3,
        lossless: false,
        bitrate_kbps: Some(128),
        sample_rate: Some(44_100),
        bit_depth: None,
        channels: Some(2),
        size: Some(5),
    }))
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
        &probe,
    )
}

#[test]
fn prepare_blocks_delete_when_a_track_cannot_be_read() -> Outcome {
    let layout = layout()?;
    let selection = add_tracks(&layout, &["good.mp3", "broken.mp3"])?;
    stray(&layout)?;
    let connection = muzik_store::db::open_in_memory()?;
    let prepared = prepare(&layout, &selection, &connection, true)?;
    assert!(!prepared.delete);
    assert!(prepared.delete_blocked);
    assert!(prepared.stale.is_empty());
    assert_eq!(prepared.freed, 0);
    Ok(())
}

#[test]
fn prepare_lists_stale_files_when_delete_is_safe() -> Outcome {
    let layout = layout()?;
    let selection = add_tracks(&layout, &["good.mp3"])?;
    let old = stray(&layout)?;
    let connection = muzik_store::db::open_in_memory()?;
    let prepared = prepare(&layout, &selection, &connection, true)?;
    assert!(prepared.delete);
    assert!(!prepared.delete_blocked);
    assert_eq!(prepared.stale, vec![old]);
    assert_eq!(prepared.freed, 7);
    Ok(())
}

#[test]
fn prepared_fits_uses_freed_space() {
    let prepared = |available, freed| Prepared {
        plan: Plan::default(),
        delete: true,
        delete_blocked: false,
        stale: Vec::new(),
        freed,
        needed: 10,
        available,
    };
    assert!(prepared(Some(5), 5).fits());
    assert!(!prepared(Some(5), 4).fits());
    assert!(prepared(None, 0).fits());
}

#[test]
fn apply_deletes_stale_files_and_copies_pending() -> Outcome {
    let layout = layout()?;
    let selection = add_tracks(&layout, &["good.mp3", "other.mp3"])?;
    let old = stray(&layout)?;
    let connection = muzik_store::db::open_in_memory()?;
    let prepared = prepare(&layout, &selection, &connection, true)?;
    let calls = AtomicUsize::new(0);
    let report = muzik_sync::apply(&prepared, &layout.target, connection, 1, &|done| {
        assert!(done.result.is_ok());
        assert!(done.record_error.is_none());
        calls.fetch_add(1, Ordering::Relaxed);
    })?;
    let pending = prepared.plan.pending.len();
    assert_eq!(pending, 2);
    assert!(!old.exists());
    assert!(!layout.card.join("Old").exists());
    for transfer in &prepared.plan.pending {
        assert!(transfer.destination.is_file());
    }
    assert_eq!(report.written, pending);
    assert_eq!(report.failed, 0);
    assert_eq!(calls.load(Ordering::Relaxed), pending);
    Ok(())
}

#[test]
fn apply_skips_stale_files_that_are_already_gone() -> Outcome {
    let layout = layout()?;
    let selection = add_tracks(&layout, &["good.mp3", "other.mp3"])?;
    let old = stray(&layout)?;
    let connection = muzik_store::db::open_in_memory()?;
    let prepared = prepare(&layout, &selection, &connection, true)?;
    assert_eq!(prepared.stale, vec![old.clone()]);
    fs::remove_file(&old)?;
    let report = muzik_sync::apply(&prepared, &layout.target, connection, 1, &|_| {})?;
    let pending = prepared.plan.pending.len();
    assert_eq!(pending, 2);
    for transfer in &prepared.plan.pending {
        assert!(transfer.destination.is_file());
    }
    assert_eq!(report.written, pending);
    assert_eq!(report.failed, 0);
    Ok(())
}

#[test]
fn apply_counts_transfers_whose_encoding_was_not_saved() -> Outcome {
    let layout = layout()?;
    let selection = add_tracks(&layout, &["good.mp3", "other.mp3"])?;
    let migrated = muzik_store::db::open_in_memory()?;
    let prepared = prepare(&layout, &selection, &migrated, false)?;
    let bare = Connection::open_in_memory()?;
    let unsaved = AtomicUsize::new(0);
    let report = muzik_sync::apply(&prepared, &layout.target, bare, 1, &|done| {
        assert!(done.result.is_ok());
        if done.record_error.is_some() {
            unsaved.fetch_add(1, Ordering::Relaxed);
        }
    })?;
    let pending = prepared.plan.pending.len();
    assert_eq!(pending, 2);
    assert_eq!(report.written, pending);
    assert_eq!(report.failed, 0);
    assert_eq!(report.unrecorded, pending);
    assert_eq!(unsaved.load(Ordering::Relaxed), pending);
    Ok(())
}

#[test]
fn select_reads_track_and_cover_paths() -> Outcome {
    let temp = tempfile::tempdir()?;
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("no crates folder")?
        .to_owned();
    let database = temp.path().join("library.db");
    fs::copy(
        crates.join("muzik-library/tests/fixtures/library.db"),
        &database,
    )?;
    let directory = temp.path().join("music");
    fs::create_dir_all(&directory)?;
    let art = directory.join("cover.jpg");
    fs::write(&art, b"art")?;
    let bytes = |path: &Path| SqlValue::Blob(path.as_os_str().as_encoded_bytes().to_vec());
    let mut library = Library::open_read_write(&database)?;
    let mut album_fields = Fields::new();
    album_fields.insert("album".into(), SqlValue::Text("SelectRun".into()));
    album_fields.insert("artpath".into(), bytes(&art));
    let album_id = library.insert_album(&album_fields, &Fields::new())?;
    let mut item_fields = Fields::new();
    item_fields.insert("album_id".into(), SqlValue::Integer(album_id));
    item_fields.insert("album".into(), SqlValue::Text("SelectRun".into()));
    item_fields.insert("title".into(), SqlValue::Text("Song".into()));
    item_fields.insert("path".into(), bytes(Path::new("Artist/song.mp3")));
    library.insert_item(&item_fields, &Fields::new())?;
    drop(library);

    let library = Library::open_read_only(&database)?;
    let selection = muzik_sync::select(&library, &directory, "album:SelectRun", true)?;
    assert_eq!(selection.tracks, vec![directory.join("Artist/song.mp3")]);
    assert_eq!(selection.covers, vec![art]);
    let without = muzik_sync::select(&library, &directory, "album:SelectRun", false)?;
    assert!(without.covers.is_empty());
    Ok(())
}
