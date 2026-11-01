use super::{TaskContext, TaskReport};
use muzik_core::BeetsConfig;
use muzik_library::{Fields, Library, SqlValue, path_from_sql, path_to_sql, scalar_text};
use muzik_metadata::{CoverArtArchive, FrontCoverSource};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const USER_AGENT: &str = "muzik/0.1 (https://github.com/TudorAndrei/muzik)";
const DEFAULT_NAME: &str = "cover";

#[derive(Debug, Default)]
struct AlbumFolder {
    dir: PathBuf,
    album_id: Option<i64>,
    release_id: Option<String>,
    tracks: Vec<PathBuf>,
}

enum Outcome {
    Skipped,
    Written(PathBuf),
    Planned(String),
}

pub fn run(context: &TaskContext<'_>) -> crate::Result<TaskReport> {
    let folders = library_folders(context)?.into_values().collect();
    add_covers(context, folders, &CoverArtArchive::new(USER_AGENT))
}

pub fn after_import(
    context: &TaskContext<'_>,
    album_dirs: &[PathBuf],
) -> crate::Result<TaskReport> {
    let mut known = library_folders(context)?;
    let folders = album_dirs
        .iter()
        .map(|dir| {
            known
                .remove(dir)
                .unwrap_or_else(|| loose_folder(dir.clone()))
        })
        .collect();
    add_covers(context, folders, &CoverArtArchive::new(USER_AGENT))
}

fn add_covers(
    context: &TaskContext<'_>,
    folders: Vec<AlbumFolder>,
    source: &dyn FrontCoverSource,
) -> crate::Result<TaskReport> {
    let name = cover_name(context)?;
    let total = folders.len();
    let mut report = TaskReport::default();
    let mut artpaths = BTreeMap::new();
    context.progress_started("Add cover files", total);
    for (index, folder) in folders.into_iter().enumerate() {
        if let Err(error) = context.check_cancelled() {
            context.progress_finished(false);
            return Err(error);
        }
        report.processed = report.processed.saturating_add(1);
        match add_cover(context, &folder, &name, source) {
            Ok(Outcome::Skipped) => report.skipped = report.skipped.saturating_add(1),
            Ok(Outcome::Planned(message)) => {
                report.changed = report.changed.saturating_add(1);
                context.message(message.clone());
                report.messages.push(message);
            }
            Ok(Outcome::Written(path)) => {
                report.changed = report.changed.saturating_add(1);
                let message = format!("Wrote {}.", path.display());
                context.message(message.clone());
                report.messages.push(message);
                if let Some(id) = folder.album_id {
                    artpaths.insert(id, path_to_sql(&path));
                }
            }
            Err(error) => {
                report.failed = report.failed.saturating_add(1);
                let message = format!(
                    "Cannot add a cover file to {}: {error}",
                    folder.dir.display()
                );
                context.warn(message.clone());
                report.messages.push(message);
            }
        }
        context.progress(index.saturating_add(1), total);
    }
    if let Err(error) = save_artpaths(context.library(), &artpaths) {
        report.failed = report.failed.saturating_add(artpaths.len());
        let message = format!("Cannot save the cover paths in the library: {error}");
        context.warn(message.clone());
        report.messages.push(message);
    }
    context.progress_finished(true);
    Ok(report)
}

fn add_cover(
    context: &TaskContext<'_>,
    folder: &AlbumFolder,
    name: &str,
    source: &dyn FrontCoverSource,
) -> crate::Result<Outcome> {
    if !folder.dir.is_dir() || muzik_tags::find_folder_cover(&folder.dir, name).is_some() {
        return Ok(Outcome::Skipped);
    }
    if let Some(bytes) = muzik_tags::first_front_cover(&folder.tracks) {
        return write(context, &folder.dir, name, &bytes);
    }
    let Some(release_id) = &folder.release_id else {
        return Ok(Outcome::Skipped);
    };
    if context.dry_run {
        return Ok(Outcome::Planned(format!(
            "Would fetch the Cover Art Archive front image for {} into {}.",
            release_id,
            folder.dir.display()
        )));
    }
    source
        .front_cover(release_id)
        .map_err(|error| crate::Error::Message(error.to_string()))?
        .map_or(Ok(Outcome::Skipped), |bytes| {
            write(context, &folder.dir, name, &bytes)
        })
}

fn write(
    context: &TaskContext<'_>,
    dir: &Path,
    name: &str,
    bytes: &[u8],
) -> crate::Result<Outcome> {
    let tags_error = |error: muzik_tags::TagsError| crate::Error::Message(error.to_string());
    if context.dry_run {
        let path = muzik_tags::cover_file_path(dir, name, bytes).map_err(tags_error)?;
        return Ok(Outcome::Planned(format!("Would write {}.", path.display())));
    }
    muzik_tags::write_cover_file(dir, name, bytes)
        .map(Outcome::Written)
        .map_err(tags_error)
}

fn cover_name(context: &TaskContext<'_>) -> crate::Result<String> {
    let config = BeetsConfig::load(&context.paths.config, json!({}))
        .map_err(|error| crate::Error::Message(error.to_string()))?;
    Ok(config
        .get(&["art_filename"])
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_NAME)
        .to_owned())
}

fn library_folders(context: &TaskContext<'_>) -> crate::Result<BTreeMap<PathBuf, AlbumFolder>> {
    if !context.library().is_file() {
        return Ok(BTreeMap::new());
    }
    let library = Library::open_read_only(context.library())?;
    let mut tracks: BTreeMap<i64, Vec<PathBuf>> = BTreeMap::new();
    for item in library.items()? {
        let (Some(album_id), Some(path)) =
            (item.album_id(), item.field("path").and_then(path_from_sql))
        else {
            continue;
        };
        tracks
            .entry(album_id)
            .or_default()
            .push(context.directory().join(path));
    }
    let mut folders = BTreeMap::new();
    for album in library.albums()? {
        let Some(tracks) = tracks.remove(&album.id) else {
            continue;
        };
        let Some(dir) = tracks
            .first()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf)
        else {
            continue;
        };
        let release_id = album
            .field("mb_albumid")
            .and_then(scalar_text)
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty());
        folders.entry(dir.clone()).or_insert(AlbumFolder {
            dir,
            album_id: Some(album.id),
            release_id,
            tracks,
        });
    }
    Ok(folders)
}

fn loose_folder(dir: PathBuf) -> AlbumFolder {
    let mut tracks: Vec<PathBuf> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .collect();
    tracks.sort();
    AlbumFolder {
        dir,
        tracks,
        ..AlbumFolder::default()
    }
}

fn save_artpaths(library: &Path, artpaths: &BTreeMap<i64, SqlValue>) -> crate::Result<()> {
    if artpaths.is_empty() {
        return Ok(());
    }
    let mut library = Library::open_read_write(library)?;
    library.transaction(|writer| {
        for (id, path) in artpaths {
            writer.update_album(
                *id,
                &Fields::from([("artpath".to_owned(), path.clone())]),
                &Fields::new(),
            )?;
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AlbumFolder, add_covers, after_import, run};
    use crate::library_tasks::{TaskContext, TaskReport};
    use muzik_core::{DecisionKind, JobEvent};
    use muzik_import::beets::BeetsPaths;
    use muzik_library::{Fields, Library, SqlValue, path_from_sql, path_to_sql};
    use muzik_metadata::{CoverArtError, FrontCoverSource};
    use serde_json::Value;
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F', b'I', b'F', 0];
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n', 0, 0];
    const RELEASE: &str = "76df3287-6cda-33eb-8e9a-044b5e15ffdd";

    struct FakeArchive {
        image: Option<&'static [u8]>,
        calls: RefCell<Vec<String>>,
    }

    impl FakeArchive {
        const fn new(image: Option<&'static [u8]>) -> Self {
            Self {
                image,
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl FrontCoverSource for FakeArchive {
        fn front_cover(&self, release_id: &str) -> Result<Option<Vec<u8>>, CoverArtError> {
            self.calls.borrow_mut().push(release_id.to_owned());
            Ok(self.image.map(<[u8]>::to_vec))
        }
    }

    fn paths(root: &Path) -> BeetsPaths {
        BeetsPaths {
            config: root.join("config.yaml"),
            library: root.join("library.db"),
            directory: root.join("Music"),
            statefile: root.join("state.pickle"),
        }
    }

    fn blank_flac(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/muzik-tags/tests/fixtures/blank.flac"),
            path,
        )
        .unwrap();
    }

    fn add_album(paths: &BeetsPaths, dir: &str, release: Option<&str>) -> (i64, PathBuf) {
        let track = paths.directory.join(dir).join("01.flac");
        blank_flac(&track);
        let mut library = Library::open_or_create(&paths.library).unwrap();
        let mut fields = Fields::from([("album".to_owned(), SqlValue::Text(dir.to_owned()))]);
        if let Some(release) = release {
            fields.insert("mb_albumid".into(), SqlValue::Text(release.to_owned()));
        }
        let album = library.insert_album(&fields, &Fields::new()).unwrap();
        library
            .insert_item(
                &Fields::from([
                    ("album_id".to_owned(), SqlValue::Integer(album)),
                    ("path".to_owned(), path_to_sql(&track)),
                ]),
                &Fields::new(),
            )
            .unwrap();
        (album, track)
    }

    fn artpath(paths: &BeetsPaths, album: i64) -> Option<PathBuf> {
        Library::open_read_only(&paths.library)
            .unwrap()
            .album(album)
            .unwrap()
            .unwrap()
            .field("artpath")
            .and_then(path_from_sql)
    }

    fn with_context<T>(
        paths: &BeetsPaths,
        dry_run: bool,
        task: impl FnOnce(&TaskContext<'_>) -> T,
    ) -> (T, Vec<JobEvent>) {
        let cancelled = AtomicBool::new(false);
        let mut events = Vec::new();
        let mut on_event = |event| events.push(event);
        let mut decide = |_: DecisionKind, _: Value| Err("no".to_owned());
        let value = task(&TaskContext::new(
            paths,
            dry_run,
            &cancelled,
            &mut on_event,
            &mut decide,
        ));
        (value, events)
    }

    fn report(
        paths: &BeetsPaths,
        dry_run: bool,
        archive: &FakeArchive,
        folders: Vec<AlbumFolder>,
    ) -> TaskReport {
        with_context(paths, dry_run, |context| {
            add_covers(context, folders, archive).unwrap()
        })
        .0
    }

    fn library_folders(paths: &BeetsPaths) -> Vec<AlbumFolder> {
        with_context(paths, false, |context| {
            super::library_folders(context).unwrap()
        })
        .0
        .into_values()
        .collect()
    }

    fn counts(report: &TaskReport) -> (usize, usize, usize, usize) {
        (
            report.processed,
            report.changed,
            report.skipped,
            report.failed,
        )
    }

    #[test]
    fn embedded_art_becomes_the_cover_file_and_the_album_artpath() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let (album, track) = add_album(&paths, "A/One", Some(RELEASE));
        muzik_tags::embed_cover(&track, JPEG, &muzik_tags::MimeType::Jpeg).unwrap();
        let archive = FakeArchive::new(Some(PNG));
        let done = report(&paths, false, &archive, library_folders(&paths));
        let cover = paths.directory.join("A/One/cover.jpg");
        assert_eq!(counts(&done), (1, 1, 0, 0));
        assert_eq!(fs::read(&cover).unwrap(), JPEG);
        assert_eq!(artpath(&paths, album), Some(cover));
        assert!(archive.calls.borrow().is_empty());
    }

    #[test]
    fn a_folder_with_a_cover_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let (album, track) = add_album(&paths, "A/One", Some(RELEASE));
        muzik_tags::embed_cover(&track, JPEG, &muzik_tags::MimeType::Jpeg).unwrap();
        let existing = paths.directory.join("A/One/folder.png");
        fs::write(&existing, b"old").unwrap();
        let archive = FakeArchive::new(Some(PNG));
        let done = report(&paths, false, &archive, library_folders(&paths));
        assert_eq!(counts(&done), (1, 0, 1, 0));
        assert_eq!(fs::read(&existing).unwrap(), b"old");
        assert!(!paths.directory.join("A/One/cover.jpg").exists());
        assert_eq!(artpath(&paths, album), None);
        assert!(archive.calls.borrow().is_empty());
    }

    #[test]
    fn the_cover_art_archive_fills_albums_without_embedded_art() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let (album, _) = add_album(&paths, "A/One", Some(RELEASE));
        let archive = FakeArchive::new(Some(PNG));
        let done = report(&paths, false, &archive, library_folders(&paths));
        let cover = paths.directory.join("A/One/cover.png");
        assert_eq!(counts(&done), (1, 1, 0, 0));
        assert_eq!(fs::read(&cover).unwrap(), PNG);
        assert_eq!(artpath(&paths, album), Some(cover));
        assert_eq!(*archive.calls.borrow(), [RELEASE]);
    }

    #[test]
    fn a_release_without_archive_art_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        add_album(&paths, "A/One", Some(RELEASE));
        let archive = FakeArchive::new(None);
        let done = report(&paths, false, &archive, library_folders(&paths));
        assert_eq!(counts(&done), (1, 0, 1, 0));
        assert_eq!(*archive.calls.borrow(), [RELEASE]);
        assert_eq!(
            fs::read_dir(paths.directory.join("A/One")).unwrap().count(),
            1
        );
    }

    #[test]
    fn an_album_without_a_release_id_makes_no_request() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        add_album(&paths, "A/One", None);
        let archive = FakeArchive::new(Some(PNG));
        let done = report(&paths, false, &archive, library_folders(&paths));
        assert_eq!(counts(&done), (1, 0, 1, 0));
        assert!(archive.calls.borrow().is_empty());
    }

    #[test]
    fn a_dry_run_reports_the_covers_and_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let (embedded, track) = add_album(&paths, "A/One", None);
        muzik_tags::embed_cover(&track, JPEG, &muzik_tags::MimeType::Jpeg).unwrap();
        let (fetched, _) = add_album(&paths, "B/Two", Some(RELEASE));
        let archive = FakeArchive::new(Some(PNG));
        let done = report(&paths, true, &archive, library_folders(&paths));
        assert_eq!(counts(&done), (2, 2, 0, 0));
        assert!(done.messages[0].contains("A/One/cover.jpg"));
        assert!(done.messages[1].contains(RELEASE));
        assert!(archive.calls.borrow().is_empty());
        assert_eq!(
            fs::read_dir(paths.directory.join("A/One")).unwrap().count(),
            1
        );
        assert_eq!(
            fs::read_dir(paths.directory.join("B/Two")).unwrap().count(),
            1
        );
        assert_eq!(artpath(&paths, embedded), None);
        assert_eq!(artpath(&paths, fetched), None);
    }

    #[test]
    fn a_failed_album_is_counted_and_the_rest_continue() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        add_album(&paths, "A/One", Some(RELEASE));
        add_album(&paths, "B/Two", Some(RELEASE));
        let archive = FakeArchive::new(Some(b"<html>not an image</html>"));
        let (done, events) = with_context(&paths, false, |context| {
            add_covers(context, library_folders(&paths), &archive).unwrap()
        });
        assert_eq!(counts(&done), (2, 0, 0, 2));
        assert_eq!(archive.calls.borrow().len(), 2);
        let progress = events
            .iter()
            .filter(|event| matches!(event, JobEvent::ProgressAdvanced { .. }))
            .count();
        assert_eq!(progress, 2);
    }

    #[test]
    fn imported_folders_get_covers_without_a_library_entry() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let track = paths.directory.join("A/One/01.flac");
        blank_flac(&track);
        muzik_tags::embed_cover(&track, JPEG, &muzik_tags::MimeType::Jpeg).unwrap();
        let (done, _) = with_context(&paths, false, |context| {
            after_import(context, &[track.parent().unwrap().to_path_buf()]).unwrap()
        });
        assert_eq!(counts(&done), (1, 1, 0, 0));
        assert!(paths.directory.join("A/One/cover.jpg").is_file());
        let (again, _) = with_context(&paths, false, |context| run(context).unwrap());
        assert_eq!(counts(&again), (0, 0, 0, 0));
    }

    #[test]
    fn the_configured_art_filename_names_the_file() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        fs::write(&paths.config, "art_filename: albumart\n").unwrap();
        let (_, track) = add_album(&paths, "A/One", None);
        muzik_tags::embed_cover(&track, JPEG, &muzik_tags::MimeType::Jpeg).unwrap();
        let archive = FakeArchive::new(None);
        report(&paths, false, &archive, library_folders(&paths));
        assert!(paths.directory.join("A/One/albumart.jpg").is_file());
        let again = report(&paths, false, &archive, library_folders(&paths));
        assert_eq!(counts(&again), (1, 0, 1, 0));
    }
}
