use super::{LibraryTask, TaskContext, TaskReport};
use muzik_core::BeetsConfig;
use muzik_import::singletons::{self, Converter, OneSongAlbum};
use muzik_library::Library;
use serde_json::json;

pub fn run(context: &TaskContext<'_>) -> crate::Result<TaskReport> {
    if !context.library().is_file() {
        context.message("There is no beets library yet.");
        return Ok(TaskReport::default());
    }
    let config = BeetsConfig::load(&context.paths.config, json!({}))
        .map_err(|error| crate::Error::Message(error.to_string()))?;
    let converter = Converter::from_beets(&config, context.directory().to_path_buf())?;
    let mut library = if context.dry_run {
        Library::open_read_only(context.library())?
    } else {
        Library::open_read_write(context.library())?
    };
    let albums = singletons::one_song_albums(&library, context.directory())?;
    let total = albums.len();
    let mut report = TaskReport::default();
    context.progress_started(LibraryTask::Singletons.title(), total);
    for (index, album) in albums.iter().enumerate() {
        if let Err(error) = context.check_cancelled() {
            context.progress_finished(false);
            return Err(error);
        }
        report.processed = report.processed.saturating_add(1);
        convert(context, &converter, &mut library, album, &mut report);
        context.progress(index.saturating_add(1), total);
    }
    context.progress_finished(true);
    Ok(report)
}

fn convert(
    context: &TaskContext<'_>,
    converter: &Converter,
    library: &mut Library,
    album: &OneSongAlbum,
    report: &mut TaskReport,
) {
    let outcome = if context.dry_run {
        converter
            .destination(library, album)
            .map(|destination| format!("Would move {} to {}.", album.name, destination.display()))
    } else {
        converter.convert(library, album).map(|converted| {
            for path in &converted.cleanup_failed {
                let message = format!("The old album file {} remains.", path.display());
                context.warn(message.clone());
                report.messages.push(message);
            }
            format!(
                "Moved {} to {}.",
                album.name,
                converted.destination.display()
            )
        })
    };
    match outcome {
        Ok(message) => {
            report.changed = report.changed.saturating_add(1);
            context.message(message.clone());
            report.messages.push(message);
        }
        Err(error) => {
            report.failed = report.failed.saturating_add(1);
            let message = format!("Cannot make {} a singleton: {error}", album.name);
            context.warn(message.clone());
            report.messages.push(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::library_tasks::{TaskContext, TaskReport};
    use muzik_core::DecisionKind;
    use muzik_import::beets::BeetsPaths;
    use muzik_library::{Fields, Library, SqlValue, path_from_sql, path_to_sql};
    use muzik_tags::TagData;
    use serde_json::Value;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    fn paths(root: &Path) -> BeetsPaths {
        BeetsPaths {
            config: root.join("config.yaml"),
            library: root.join("library.db"),
            directory: root.join("Music"),
            statefile: root.join("state.pickle"),
        }
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../muzik-tags/tests/fixtures")
            .join(name)
    }

    fn text(value: &str) -> SqlValue {
        SqlValue::Text(value.to_owned())
    }

    fn add_album(
        paths: &BeetsPaths,
        folder: &str,
        titles: &[&str],
        tracktotal: i64,
        cover: bool,
    ) -> (i64, Vec<PathBuf>) {
        let dir = paths.directory.join(folder);
        fs::create_dir_all(&dir).unwrap();
        let mut library = Library::open_or_create(&paths.library).unwrap();
        let mut album_fields = Fields::from([
            ("album".to_owned(), text("Night Lines")),
            ("albumartist".to_owned(), text("Mara Vale")),
            ("mb_albumid".to_owned(), text("old-release")),
        ]);
        if cover {
            let art = dir.join("cover.png");
            fs::copy(fixture("cover.png"), &art).unwrap();
            album_fields.insert("artpath".into(), path_to_sql(&art));
        }
        let album = library.insert_album(&album_fields, &Fields::new()).unwrap();
        let mut files = Vec::new();
        for (track, title) in (1_i64..).zip(titles) {
            let file = dir.join(format!("0{track} {title}.flac"));
            fs::copy(fixture("blank.flac"), &file).unwrap();
            let mut tags = TagData::default();
            for (name, value) in [
                ("title", (*title).to_owned()),
                ("artist", "Mara Vale".to_owned()),
                ("album", "Night Lines".to_owned()),
                ("albumartist", "Mara Vale".to_owned()),
                ("track", track.to_string()),
                ("tracktotal", tracktotal.to_string()),
                ("mb_albumid", "old-release".to_owned()),
            ] {
                tags.fields.insert(name.into(), value);
            }
            muzik_tags::write(&file, &tags).unwrap();
            library
                .insert_item(
                    &Fields::from([
                        ("album_id".to_owned(), SqlValue::Integer(album)),
                        ("path".to_owned(), path_to_sql(&file)),
                        ("title".to_owned(), text(title)),
                        ("artist".to_owned(), text("Mara Vale")),
                        ("album".to_owned(), text("Night Lines")),
                        ("albumartist".to_owned(), text("Mara Vale")),
                        ("track".to_owned(), SqlValue::Integer(track)),
                        ("tracktotal".to_owned(), SqlValue::Integer(tracktotal)),
                        ("mb_albumid".to_owned(), text("old-release")),
                    ]),
                    &Fields::new(),
                )
                .unwrap();
            files.push(file);
        }
        (album, files)
    }

    fn report(paths: &BeetsPaths, dry_run: bool) -> TaskReport {
        let cancelled = AtomicBool::new(false);
        let mut on_event = |_| {};
        let mut decide = |_: DecisionKind, _: Value| Err("no".to_owned());
        run(&TaskContext::new(
            paths,
            dry_run,
            &cancelled,
            &mut on_event,
            &mut decide,
        ))
        .unwrap()
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
    fn a_one_song_album_becomes_a_singleton_with_the_album_cover() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let (lonely, files) = add_album(&paths, "Mara Vale/Night Lines", &["Song"], 12, true);
        let (full, _) = add_album(&paths, "Mara Vale/Full", &["One", "Two"], 2, false);
        let (single, _) = add_album(&paths, "Mara Vale/Single", &["Hit"], 1, false);
        let source = files[0].clone();
        let before = fs::read(&source).unwrap();

        let preview = report(&paths, true);
        assert_eq!(counts(&preview), (1, 1, 0, 0));
        assert!(preview.messages[0].contains("Non-Album/Mara Vale/Song.flac"));
        assert_eq!(fs::read(&source).unwrap(), before);

        let done = report(&paths, false);
        assert_eq!(counts(&done), (1, 1, 0, 0));
        let destination = paths.directory.join("Non-Album/Mara Vale/Song.flac");
        assert!(!source.exists());
        let tags = muzik_tags::read(&destination, &[]).unwrap();
        assert_eq!(tags.fields.get("title").map(String::as_str), Some("Song"));
        assert_eq!(
            tags.fields.get("artist").map(String::as_str),
            Some("Mara Vale")
        );
        for name in ["album", "albumartist", "track", "tracktotal", "mb_albumid"] {
            assert_eq!(tags.fields.get(name), None, "{name}");
        }
        assert!(muzik_tags::has_front_cover(&destination).unwrap());

        let library = Library::open_read_only(&paths.library).unwrap();
        assert!(library.album(lonely).unwrap().is_none());
        assert!(library.album(full).unwrap().is_some());
        assert!(library.album(single).unwrap().is_some());
        let item = library
            .items()
            .unwrap()
            .into_iter()
            .find(|item| item.field("title") == Some(&text("Song")))
            .unwrap();
        assert_eq!(item.album_id(), None);
        assert_eq!(
            item.field("path").and_then(path_from_sql),
            Some(destination)
        );
        assert_eq!(item.field("album"), Some(&text("")));
        assert_eq!(item.field("track"), Some(&SqlValue::Integer(0)));
        assert_eq!(
            item.attribute("muzik_original_album"),
            Some(&text("Night Lines"))
        );
        assert_eq!(
            item.attribute("muzik_original_mb_albumid"),
            Some(&text("old-release"))
        );
        drop(library);

        assert_eq!(counts(&report(&paths, false)), (0, 0, 0, 0));
    }

    #[test]
    fn a_taken_singleton_path_leaves_the_album_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let (album, files) = add_album(&paths, "Mara Vale/Night Lines", &["Song"], 0, false);
        let taken = paths.directory.join("Non-Album/Mara Vale/Song.flac");
        fs::create_dir_all(taken.parent().unwrap()).unwrap();
        fs::write(&taken, b"other").unwrap();
        let before = fs::read(&files[0]).unwrap();
        let done = report(&paths, false);
        assert_eq!(counts(&done), (1, 0, 0, 1));
        assert_eq!(fs::read(&files[0]).unwrap(), before);
        assert_eq!(fs::read(&taken).unwrap(), b"other");
        let library = Library::open_read_only(&paths.library).unwrap();
        assert!(library.album(album).unwrap().is_some());
    }
}
