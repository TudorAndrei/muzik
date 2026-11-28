use super::{LibraryTask, TaskContext, TaskReport};
use muzik_library::{Fields, Library, SqlValue, path_from_sql, scalar_text};
use muzik_media::loudness::{self, Gain, LoudnessError, Measurement};
use muzik_tags::TagData;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const TAGS: [&str; 4] = [
    "rg_track_gain",
    "rg_track_peak",
    "rg_album_gain",
    "rg_album_peak",
];
const TAGGABLE: [&str; 6] = ["flac", "mp3", "m4a", "ogg", "oga", "opus"];

struct Track {
    id: Option<i64>,
    path: PathBuf,
}

struct Album {
    id: Option<i64>,
    name: String,
    tracks: Vec<Track>,
    singleton: bool,
}

pub fn run(context: &TaskContext<'_>) -> crate::Result<TaskReport> {
    if !context.library().exists() {
        context.message("There is no beets library yet.");
        return Ok(TaskReport::default());
    }
    let mut library = open(context.library(), context.dry_run)?;
    let mut albums = Vec::new();
    for album in library.albums()? {
        albums.push(load(&library, &album)?);
    }
    albums.extend(
        library
            .items()?
            .iter()
            .filter(|item| item.album_id().is_none())
            .filter_map(singleton),
    );
    process(context, Some(&mut library), &albums)
}

pub fn after_import(
    context: &TaskContext<'_>,
    album_dirs: &[PathBuf],
    songs: &[PathBuf],
) -> crate::Result<TaskReport> {
    let dirs: BTreeSet<&Path> = album_dirs.iter().map(PathBuf::as_path).collect();
    let mut library = if context.library().exists() {
        Some(open(context.library(), context.dry_run)?)
    } else {
        None
    };
    let mut albums = Vec::new();
    if let Some(library) = &library {
        let ids: BTreeSet<i64> = library
            .items()?
            .iter()
            .filter(|item| {
                item.field("path")
                    .and_then(path_from_sql)
                    .is_some_and(|path| path.parent().is_some_and(|dir| dirs.contains(dir)))
            })
            .filter_map(muzik_library::Item::album_id)
            .collect();
        for id in ids {
            if let Some(album) = library.album(id)? {
                albums.push(load(library, &album)?);
            }
        }
        albums.extend(
            library
                .items()?
                .iter()
                .filter(|item| item.album_id().is_none())
                .filter_map(singleton)
                .filter(|song| song.tracks.iter().any(|track| songs.contains(&track.path))),
        );
    }
    let covered: BTreeSet<&Path> = albums
        .iter()
        .flat_map(|album| album.tracks.iter())
        .filter_map(|track| track.path.parent())
        .collect();
    let mut folders = Vec::new();
    for dir in album_dirs {
        if !covered.contains(dir.as_path())
            && let Some(album) = folder_album(dir)?
        {
            folders.push(album);
        }
    }
    albums.extend(folders);
    process(context, library.as_mut(), &albums)
}

fn open(path: &Path, dry_run: bool) -> crate::Result<Library> {
    Ok(if dry_run {
        Library::open_read_only(path)?
    } else {
        Library::open_read_write(path)?
    })
}

fn load(library: &Library, album: &muzik_library::Album) -> crate::Result<Album> {
    let tracks = library
        .items_for_album(album.id)?
        .into_iter()
        .filter_map(|item| {
            let path = item.field("path").and_then(path_from_sql)?;
            Some(Track {
                id: Some(item.id),
                path,
            })
        })
        .collect();
    let text = |name: &str| album.field(name).and_then(scalar_text).unwrap_or_default();
    Ok(Album {
        id: Some(album.id),
        name: format!("{} - {}", text("albumartist"), text("album")),
        tracks,
        singleton: false,
    })
}

fn singleton(item: &muzik_library::Item) -> Option<Album> {
    let path = item.field("path").and_then(path_from_sql)?;
    let text = |name: &str| item.field(name).and_then(scalar_text).unwrap_or_default();
    Some(Album {
        id: None,
        name: format!("{} - {}", text("artist"), text("title")),
        tracks: vec![Track {
            id: Some(item.id),
            path,
        }],
        singleton: true,
    })
}

fn folder_album(dir: &Path) -> crate::Result<Option<Album>> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let taggable = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| TAGGABLE.contains(&extension.to_ascii_lowercase().as_str()));
        if taggable && path.is_file() {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Ok(None);
    }
    paths.sort();
    Ok(Some(Album {
        id: None,
        name: dir.display().to_string(),
        tracks: paths
            .into_iter()
            .map(|path| Track { id: None, path })
            .collect(),
        singleton: false,
    }))
}

fn process(
    context: &TaskContext<'_>,
    mut library: Option<&mut Library>,
    albums: &[Album],
) -> crate::Result<TaskReport> {
    let mut report = TaskReport::default();
    let total = albums.len();
    context.progress_started(LibraryTask::ReplayGain.title(), total);
    for (index, album) in albums.iter().enumerate() {
        if let Err(error) = context.check_cancelled() {
            context.progress_finished(false);
            return Err(error);
        }
        report.processed = report.processed.saturating_add(1);
        process_album(context, library.as_deref_mut(), album, &mut report);
        context.progress(index.saturating_add(1), total);
    }
    context.progress_finished(true);
    Ok(report)
}

fn process_album(
    context: &TaskContext<'_>,
    library: Option<&mut Library>,
    album: &Album,
    report: &mut TaskReport,
) {
    if album.tracks.is_empty()
        || album
            .tracks
            .iter()
            .all(|track| tagged(&track.path, album.singleton))
    {
        report.skipped = report.skipped.saturating_add(1);
        return;
    }
    let paths: Vec<PathBuf> = album
        .tracks
        .iter()
        .map(|track| track.path.clone())
        .collect();
    let mut measured = Vec::new();
    let mut broken = false;
    let mut silent = false;
    for (path, outcome) in paths.iter().zip(loudness::measure_all(&paths)) {
        match outcome {
            Ok(measurement) => measured.push(measurement),
            Err(LoudnessError::Silent) => silent = true,
            Err(error) => {
                warn(
                    context,
                    report,
                    format!("Cannot measure {}: {error}", path.display()),
                );
                broken = true;
            }
        }
    }
    if broken {
        report.failed = report.failed.saturating_add(1);
        return;
    }
    if silent {
        context.message(format!(
            "{}: a track is too short or too quiet to measure.",
            album.name
        ));
        report.skipped = report.skipped.saturating_add(1);
        return;
    }
    let album_gain = if album.singleton {
        None
    } else {
        match loudness::album(&measured) {
            Ok(gain) => Some(gain),
            Err(error) => {
                warn(
                    context,
                    report,
                    format!("Cannot measure {}: {error}", album.name),
                );
                report.failed = report.failed.saturating_add(1);
                return;
            }
        }
    };
    let summary = match (album_gain, measured.first()) {
        (Some(gain), _) => format!("album gain {}, peak {}", gain.gain_text(), gain.peak_text()),
        (None, Some(measurement)) => format!(
            "track gain {}, peak {}",
            measurement.track().gain_text(),
            measurement.track().peak_text()
        ),
        (None, None) => String::new(),
    };
    if context.dry_run {
        context.message(format!("{}: {summary} (not written).", album.name));
        report.changed = report.changed.saturating_add(1);
        return;
    }
    match write(library, album, &measured, album_gain) {
        Ok(()) => {
            context.message(format!("{}: {summary}.", album.name));
            report.changed = report.changed.saturating_add(1);
        }
        Err(error) => {
            warn(
                context,
                report,
                format!("Cannot write ReplayGain for {}: {error}", album.name),
            );
            report.failed = report.failed.saturating_add(1);
        }
    }
}

fn warn(context: &TaskContext<'_>, report: &mut TaskReport, message: String) {
    context.warn(message.clone());
    report.messages.push(message);
}

fn tagged(path: &Path, singleton: bool) -> bool {
    let names = if singleton { &TAGS[..2] } else { &TAGS[..] };
    muzik_tags::read(path, &[])
        .is_ok_and(|tags| names.iter().all(|name| tags.fields.contains_key(*name)))
}

fn write(
    library: Option<&mut Library>,
    album: &Album,
    measured: &[Measurement],
    album_gain: Option<Gain>,
) -> Result<(), String> {
    let backup = tempfile::tempdir().map_err(|error| error.to_string())?;
    for (index, track) in album.tracks.iter().enumerate() {
        fs::copy(&track.path, backup.path().join(index.to_string()))
            .map_err(|error| error.to_string())?;
    }
    apply(library, album, measured, album_gain).map_err(|message| {
        let mut parts = vec![message];
        for (index, track) in album.tracks.iter().enumerate() {
            if let Err(error) = fs::copy(backup.path().join(index.to_string()), &track.path) {
                parts.push(format!("cannot restore {}: {error}", track.path.display()));
            }
        }
        parts.join("; ")
    })
}

fn apply(
    library: Option<&mut Library>,
    album: &Album,
    measured: &[Measurement],
    album_gain: Option<Gain>,
) -> Result<(), String> {
    for (track, measurement) in album.tracks.iter().zip(measured) {
        muzik_tags::write(&track.path, &tags(measurement.track(), album_gain))
            .map_err(|error| error.to_string())?;
    }
    let Some(library) = library else {
        return Ok(());
    };
    let album_fields = album_gain.map(|gain| {
        Fields::from([
            ("rg_album_gain".into(), gain_value(gain)),
            ("rg_album_peak".into(), peak_value(gain)),
        ])
    });
    library
        .transaction(|writer| {
            if let (Some(id), Some(fields)) = (album.id, &album_fields) {
                writer.update_album(id, fields, &Fields::new())?;
            }
            for (track, measurement) in album.tracks.iter().zip(measured) {
                if let Some(id) = track.id {
                    let mut fields = Fields::from([
                        ("rg_track_gain".into(), gain_value(measurement.track())),
                        ("rg_track_peak".into(), peak_value(measurement.track())),
                    ]);
                    fields.extend(album_fields.clone().unwrap_or_default());
                    writer.update_item(id, &fields, &Fields::new())?;
                }
            }
            Ok(())
        })
        .map_err(|error| error.to_string())
}

fn tags(track: Gain, album: Option<Gain>) -> TagData {
    let mut data = TagData::default();
    data.fields.extend([
        ("rg_track_gain".into(), track.gain_text()),
        ("rg_track_peak".into(), track.peak_text()),
    ]);
    if let Some(album) = album {
        data.fields.extend([
            ("rg_album_gain".into(), album.gain_text()),
            ("rg_album_peak".into(), album.peak_text()),
        ]);
    }
    data
}

fn gain_value(gain: Gain) -> SqlValue {
    SqlValue::Real(format!("{:.2}", gain.gain).parse().unwrap_or(gain.gain))
}

fn peak_value(gain: Gain) -> SqlValue {
    SqlValue::Real(format!("{:.6}", gain.peak).parse().unwrap_or(gain.peak))
}

#[cfg(test)]
mod tests {
    use super::{after_import, run};
    use crate::library_tasks::{TaskContext, TaskReport};
    use muzik_core::{JobEvent, Severity};
    use muzik_import::beets::BeetsPaths;
    use muzik_library::{Fields, Library, SqlValue, path_to_sql};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::AtomicBool;

    fn paths(root: &Path) -> BeetsPaths {
        BeetsPaths {
            config: root.join("config.yaml"),
            library: root.join("library.db"),
            directory: root.join("Music"),
            statefile: root.join("state.pickle"),
        }
    }

    fn sine_flac(path: &Path, volume: f64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let made = Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("sine=frequency=1000:duration=2,volume={volume}"))
            .args(["-metadata", "title=Song", "-y"])
            .arg(path)
            .output()
            .unwrap();
        assert!(made.status.success());
    }

    fn library_album(root: &Path, files: &[PathBuf]) -> Vec<i64> {
        let mut library = Library::open_or_create(&paths(root).library).unwrap();
        let album = library
            .insert_album(
                &Fields::from([
                    ("album".into(), SqlValue::Text("Record".into())),
                    ("albumartist".into(), SqlValue::Text("Artist".into())),
                ]),
                &Fields::new(),
            )
            .unwrap();
        files
            .iter()
            .map(|file| {
                library
                    .insert_item(
                        &Fields::from([
                            ("path".into(), path_to_sql(file)),
                            ("album_id".into(), SqlValue::Integer(album)),
                        ]),
                        &Fields::new(),
                    )
                    .unwrap()
            })
            .collect()
    }

    fn with_context<T>(
        root: &Path,
        dry_run: bool,
        task: impl FnOnce(&TaskContext<'_>) -> T,
    ) -> (T, Vec<JobEvent>) {
        let paths = paths(root);
        let cancelled = AtomicBool::new(false);
        let mut events = Vec::new();
        let result = {
            let mut on_event = |event| events.push(event);
            let mut decide = |_, _| Err("no questions".to_owned());
            let context = TaskContext::new(&paths, dry_run, &cancelled, &mut on_event, &mut decide);
            task(&context)
        };
        (result, events)
    }

    fn gain(path: &Path, name: &str) -> f64 {
        let tags = muzik_tags::read(path, &[]).unwrap();
        let text = tags.fields.get(name).unwrap();
        text.trim_end_matches(" dB").parse().unwrap()
    }

    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 0.15,
            "{actual} is not near {expected}"
        );
    }

    fn warnings(events: &[JobEvent]) -> usize {
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    JobEvent::Message {
                        severity: Severity::Warning,
                        ..
                    }
                )
            })
            .count()
    }

    #[test]
    fn the_backfill_writes_track_and_album_gain_to_files_and_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let loud = dir.path().join("Music/Artist/Record/01.flac");
        let quiet = dir.path().join("Music/Artist/Record/02.flac");
        sine_flac(&loud, 1.0);
        sine_flac(&quiet, 0.5);
        let ids = library_album(dir.path(), &[loud.clone(), quiet.clone()]);
        let (report, _) = with_context(dir.path(), false, |context| run(context).unwrap());
        assert_eq!(
            (
                report.processed,
                report.changed,
                report.skipped,
                report.failed
            ),
            (1, 1, 0, 0)
        );
        close(gain(&loud, "rg_track_gain"), 3.07);
        close(gain(&quiet, "rg_track_gain"), 9.09);
        let album = gain(&loud, "rg_album_gain");
        assert!(album > 3.07 && album < 9.09);
        close(gain(&quiet, "rg_album_gain"), album);
        let tags = muzik_tags::read(&quiet, &[]).unwrap();
        assert_eq!(tags.fields.get("title").map(String::as_str), Some("Song"));
        let peak: f64 = tags.fields.get("rg_album_peak").unwrap().parse().unwrap();
        assert!((peak - 0.125).abs() < 0.01);
        let library = Library::open_read_only(&paths(dir.path()).library).unwrap();
        let item = library.item(ids[1]).unwrap().unwrap();
        assert_eq!(
            item.field("rg_track_gain"),
            Some(&SqlValue::Real(gain(&quiet, "rg_track_gain")))
        );
        let row = library.album(item.album_id().unwrap()).unwrap().unwrap();
        assert_eq!(row.field("rg_album_gain"), Some(&SqlValue::Real(album)));
        drop(library);
        let before = fs::read(&loud).unwrap();
        let (again, _) = with_context(dir.path(), false, |context| run(context).unwrap());
        assert_eq!((again.processed, again.changed, again.skipped), (1, 0, 1));
        assert_eq!(fs::read(&loud).unwrap(), before);
    }

    #[test]
    fn a_dry_run_measures_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let track = dir.path().join("Music/Artist/Record/01.flac");
        sine_flac(&track, 1.0);
        let ids = library_album(dir.path(), std::slice::from_ref(&track));
        let before = fs::read(&track).unwrap();
        let (report, _) = with_context(dir.path(), true, |context| run(context).unwrap());
        assert_eq!((report.processed, report.changed), (1, 1));
        assert_eq!(fs::read(&track).unwrap(), before);
        let library = Library::open_read_only(&paths(dir.path()).library).unwrap();
        let item = library.item(ids[0]).unwrap().unwrap();
        assert_eq!(item.field("rg_track_gain"), Some(&SqlValue::Null));
    }

    #[test]
    fn after_an_import_folders_are_measured_and_broken_files_fail() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("Music/A/Good/01.flac");
        let broken = dir.path().join("Music/B/Broken/01.flac");
        sine_flac(&good, 1.0);
        fs::create_dir_all(broken.parent().unwrap()).unwrap();
        fs::write(&broken, b"not audio").unwrap();
        let dirs = [
            good.parent().unwrap().to_path_buf(),
            broken.parent().unwrap().to_path_buf(),
        ];
        let (report, events) = with_context(dir.path(), false, |context| {
            after_import(context, &dirs, &[]).unwrap()
        });
        assert_eq!(
            (
                report.processed,
                report.changed,
                report.skipped,
                report.failed
            ),
            (2, 1, 0, 1)
        );
        assert_eq!(report.messages.len(), 1);
        assert_eq!(warnings(&events), 1);
        close(gain(&good, "rg_track_gain"), 3.07);
        assert_eq!(fs::read(&broken).unwrap(), b"not audio");
    }

    #[test]
    fn an_imported_song_without_an_album_gets_only_track_gain() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("Music/Non-Album/Artist/Song.flac");
        let other = dir.path().join("Music/Non-Album/Artist/Other.flac");
        sine_flac(&song, 1.0);
        sine_flac(&other, 0.5);
        let mut library = Library::open_or_create(&paths(dir.path()).library).unwrap();
        let id = library
            .insert_item(
                &Fields::from([("path".into(), path_to_sql(&song))]),
                &Fields::new(),
            )
            .unwrap();
        library
            .insert_item(
                &Fields::from([("path".into(), path_to_sql(&other))]),
                &Fields::new(),
            )
            .unwrap();
        drop(library);
        let (report, _) = with_context(dir.path(), false, |context| {
            after_import(context, &[], std::slice::from_ref(&song)).unwrap()
        });
        assert_eq!((report.processed, report.changed), (1, 1));
        close(gain(&song, "rg_track_gain"), 3.07);
        let tags = muzik_tags::read(&song, &[]).unwrap();
        assert!(!tags.fields.contains_key("rg_album_gain"));
        assert!(
            !muzik_tags::read(&other, &[])
                .unwrap()
                .fields
                .contains_key("rg_track_gain")
        );
        let library = Library::open_read_only(&paths(dir.path()).library).unwrap();
        let item = library.item(id).unwrap().unwrap();
        assert_eq!(
            item.field("rg_track_gain"),
            Some(&SqlValue::Real(gain(&song, "rg_track_gain")))
        );
        assert_eq!(item.field("rg_album_gain"), Some(&SqlValue::Null));
    }

    #[test]
    fn short_fixture_tracks_are_skipped_without_changes() {
        let dir = tempfile::tempdir().unwrap();
        let track = dir.path().join("Music/A/One/01.flac");
        fs::create_dir_all(track.parent().unwrap()).unwrap();
        fs::copy(crate::sources::testing::fixture(), &track).unwrap();
        library_album(dir.path(), std::slice::from_ref(&track));
        let before = fs::read(&track).unwrap();
        let (report, events) = with_context(dir.path(), false, |context| {
            after_import(context, &[track.parent().unwrap().to_path_buf()], &[]).unwrap()
        });
        assert_eq!(
            report,
            TaskReport {
                processed: 1,
                skipped: 1,
                ..TaskReport::default()
            }
        );
        assert_eq!(warnings(&events), 0);
        assert_eq!(fs::read(&track).unwrap(), before);
    }
}
