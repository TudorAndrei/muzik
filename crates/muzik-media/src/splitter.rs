//! Split a chaptered audio file into tagged tracks with ffmpeg.

use crate::ffmpeg::{self, Cut, Ffmpeg};
use muzik_core::chapters::{Chapter, sidecar_path};
use parking_lot::Mutex;
use rayon::prelude::*;
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use unicode_normalization::UnicodeNormalization;

const THUMB_EXTS: &[&str] = &[".jpg", ".jpeg", ".png", ".webp"];

#[derive(Clone, Debug, Default)]
pub struct SplitOptions {
    /// Zero selects a worker count based on the available processors.
    pub jobs: usize,
    pub keep_source: bool,
    pub force: bool,
    pub compilation: bool,
    /// Defaults to the application cache directory.
    pub cache_dir: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SplitProgress {
    pub completed: usize,
    pub total: usize,
    pub chapter_index: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum SplitError {
    #[error("split cancelled")]
    Cancelled,
    #[error("file not found: {0}")]
    SourceMissing(PathBuf),
    #[error("no chapters found")]
    NoChapters,
    #[error("invalid chapter: {0}")]
    InvalidChapter(String),
    #[error("output path is not a directory: {0}")]
    OutputNotDirectory(PathBuf),
    #[error("The split folder {} already has other files.", .0.display())]
    OutputNotEmpty(PathBuf),
    #[error("output directory contains the source audio file")]
    OutputContainsSource,
    #[error("failed to split {0} track(s): {1}")]
    TracksFailed(usize, String),
    #[error(transparent)]
    Ffmpeg(ffmpeg::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Clone)]
struct Metadata {
    artist: String,
    album: String,
    year: String,
    source_id: Option<String>,
}

/// Split `source` at the supplied chapters and return the album directory.
///
/// Every track uses audio stream copy. If one track fails, source files stay in
/// place. A successful split removes the source and its download sidecars unless
/// `keep_source` is true.
pub fn split_audio(
    source: &Path,
    chapters: &[Chapter],
    output: &Path,
    options: &SplitOptions,
) -> Result<PathBuf, SplitError> {
    split_audio_with_cancel(
        source,
        chapters,
        output,
        options,
        &AtomicBool::new(false),
        &mut |_| {},
    )
}

/// Split tracks and stop active ffmpeg processes when `cancelled` becomes true.
/// Cancellation keeps the source and its sidecars in place.
pub fn split_audio_with_cancel(
    source: &Path,
    chapters: &[Chapter],
    output: &Path,
    options: &SplitOptions,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(SplitProgress),
) -> Result<PathBuf, SplitError> {
    split_audio_with_binary(
        source,
        chapters,
        output,
        options,
        cancelled,
        on_progress,
        &Ffmpeg::default(),
    )
}

fn split_audio_with_binary(
    source: &Path,
    chapters: &[Chapter],
    output: &Path,
    options: &SplitOptions,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(SplitProgress),
    ffmpeg: &Ffmpeg,
) -> Result<PathBuf, SplitError> {
    if cancelled.load(Ordering::SeqCst) {
        return Err(SplitError::Cancelled);
    }
    if !source.is_file() {
        return Err(SplitError::SourceMissing(source.to_path_buf()));
    }
    if chapters.is_empty() {
        return Err(SplitError::NoChapters);
    }
    validate_chapters(chapters, source, options.compilation)?;
    let source = source.canonicalize()?;
    // A forced split must never remove its input with the output directory.
    if output.exists() && source.starts_with(output.canonicalize()?) {
        return Err(SplitError::OutputContainsSource);
    }

    let metadata = extract_metadata(&source);
    let chapter_sidecar = sidecar_path(&source, ".chapters.txt");
    let cache_file = if chapter_sidecar.exists() {
        let key = format!(
            "split_{}_{}",
            file_hash(&source)?,
            file_hash(&chapter_sidecar)?
        );
        Some(
            options
                .cache_dir
                .clone()
                .unwrap_or_else(muzik_core::paths::cache_dir)
                .join(format!("{key}.txt")),
        )
    } else {
        None
    };
    if !options.force
        && let Some(ref cache_file) = cache_file
        && let Ok(cached) = fs::read_to_string(cache_file)
    {
        let cached = PathBuf::from(cached.trim());
        if cached.exists() {
            return Ok(cached);
        }
    }

    if cancelled.load(Ordering::SeqCst) {
        return Err(SplitError::Cancelled);
    }
    if output.exists() {
        if !output.is_dir() {
            return Err(SplitError::OutputNotDirectory(output.to_path_buf()));
        }
        if fs::read_dir(output)?.next().is_some() {
            if !options.force {
                let complete = chapters.iter().all(|chapter| {
                    fs::metadata(output.join(expected_track_name(
                        &source,
                        chapter,
                        options.compilation,
                    )))
                    .is_ok_and(|file| file.is_file() && file.len() > 0)
                });
                if complete {
                    return finish(&source, output, cache_file.as_deref(), options.keep_source);
                }
                return Err(SplitError::OutputNotEmpty(output.to_path_buf()));
            }
            fs::remove_dir_all(output)?;
        }
    }
    fs::create_dir_all(output)?;

    let workers = if options.jobs == 0 {
        std::thread::available_parallelism()
            .map_or(4, |count| count.get())
            .div_ceil(2)
            .clamp(2, 8)
    } else {
        options.jobs
    }
    .min(chapters.len());
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build()
        .map_err(io::Error::other)?;
    let failures = Mutex::new(Vec::new());
    let completed = AtomicUsize::new(0);
    let track_context = SplitTrackContext {
        source: &source,
        output,
        count: chapters.len(),
        metadata: &metadata,
        compilation: options.compilation,
        cancelled,
        ffmpeg,
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    let split = std::thread::scope(|scope| {
        let work = scope.spawn(|| {
            pool.install(|| {
                chapters
                    .par_iter()
                    .try_for_each_with(sender, |sender, chapter| {
                        if cancelled.load(Ordering::SeqCst) {
                            return Err(SplitError::Cancelled);
                        }
                        if split_track(&track_context, chapter)? {
                            let count = completed.fetch_add(1, Ordering::SeqCst).saturating_add(1);
                            let _ = sender.send(SplitProgress {
                                completed: count,
                                total: chapters.len(),
                                chapter_index: chapter.index,
                            });
                        } else {
                            failures.lock().push(chapter.title.clone());
                        }
                        Ok(())
                    })
            })
        });
        for progress in receiver {
            on_progress(progress);
        }
        work.join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    });
    if cancelled.load(Ordering::SeqCst) {
        return Err(SplitError::Cancelled);
    }
    split?;
    let failed = failures.into_inner();
    if !failed.is_empty() {
        return Err(SplitError::TracksFailed(failed.len(), failed.join(", ")));
    }

    if cancelled.load(Ordering::SeqCst) {
        return Err(SplitError::Cancelled);
    }
    // The following file updates form the final commit step. Cancellation is
    // observed before this step, so it cannot leave a removed source behind.
    finish(&source, output, cache_file.as_deref(), options.keep_source)
}

fn finish(
    source: &Path,
    output: &Path,
    cache_file: Option<&Path>,
    keep_source: bool,
) -> Result<PathBuf, SplitError> {
    place_cover(source, output);
    if let Some(cache_file) = cache_file {
        if let Some(parent) = cache_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(cache_file, output.to_string_lossy().as_bytes())?;
    }
    if !keep_source {
        fs::remove_file(source)?;
        for extension in [
            ".chapters.txt",
            ".info.json",
            ".metadata.txt",
            ".jpg",
            ".jpeg",
            ".png",
            ".webp",
        ] {
            let sidecar = sidecar_path(source, extension);
            if sidecar.exists() {
                fs::remove_file(sidecar)?;
            }
        }
    }
    Ok(output.to_path_buf())
}

/// Return `<source parent parent>/splits/<album slug>`.
pub fn default_output(source: &Path) -> Result<PathBuf, SplitError> {
    if !source.is_file() {
        return Err(SplitError::SourceMissing(source.to_path_buf()));
    }
    let parent = source.parent().unwrap_or(Path::new("."));
    let root = parent.parent().unwrap_or(parent);
    Ok(root
        .join("splits")
        .join(safe_filename(&extract_metadata(source).album)))
}

fn validate_chapters(
    chapters: &[Chapter],
    source: &Path,
    compilation: bool,
) -> Result<(), SplitError> {
    let mut names = HashSet::new();
    let extension = source
        .extension()
        .and_then(|part| part.to_str())
        .unwrap_or("");
    for chapter in chapters {
        if chapter.index == 0
            || chapter.start < 0
            || chapter.end.is_some_and(|end| end <= chapter.start)
        {
            return Err(SplitError::InvalidChapter(chapter.title.clone()));
        }
        let title = if compilation {
            parse_artist_title(&chapter.title).map_or(chapter.title.as_str(), |(_, song)| song)
        } else {
            chapter.title.as_str()
        };
        let (title, _) = strip_featured(title);
        let name = format!(
            "{:02}-{}.{}",
            chapter.index,
            safe_filename(&title),
            extension
        );
        if !names.insert(name) {
            return Err(SplitError::InvalidChapter(format!(
                "duplicate output for {}",
                chapter.title
            )));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SplitTrackContext<'a> {
    source: &'a Path,
    output: &'a Path,
    count: usize,
    metadata: &'a Metadata,
    compilation: bool,
    cancelled: &'a AtomicBool,
    ffmpeg: &'a Ffmpeg,
}

fn track_file_name(source: &Path, index: u32, title: &str) -> String {
    let mut name = format!("{index:02}-{}", safe_filename(title));
    if let Some(extension) = source.extension().and_then(|ext| ext.to_str()) {
        name.push('.');
        name.push_str(extension);
    }
    name
}

fn expected_track_name(source: &Path, chapter: &Chapter, compilation: bool) -> String {
    let title = if compilation {
        parse_artist_title(&chapter.title).map_or(chapter.title.as_str(), |(_, title)| title)
    } else {
        chapter.title.as_str()
    };
    let (title, _) = strip_featured(title);
    track_file_name(source, chapter.index, &title)
}

fn split_track(context: &SplitTrackContext<'_>, chapter: &Chapter) -> Result<bool, SplitError> {
    let SplitTrackContext {
        source,
        output,
        count,
        metadata,
        compilation,
        cancelled,
        ffmpeg,
    } = *context;
    let (mut artist, title) = if compilation {
        let (artist, title) = parse_artist_title(&chapter.title).map_or_else(
            || (metadata.artist.clone(), chapter.title.as_str()),
            |(artist, title)| (artist.to_owned(), title),
        );
        (artist, title)
    } else {
        (metadata.artist.clone(), chapter.title.as_str())
    };
    let albumartist = if compilation {
        "Various Artists"
    } else {
        &metadata.artist
    };
    let (title, featured) = strip_featured(title);
    if !featured.is_empty() {
        artist.push_str(" feat. ");
        artist.push_str(&featured.join(", "));
    }
    let destination = output.join(track_file_name(source, chapter.index, &title));
    let cut = Cut {
        source,
        destination: &destination,
        start: chapter.start,
        end: chapter.end,
        tags: &[
            ("title", title.clone()),
            ("artist", artist),
            ("albumartist", albumartist.to_owned()),
            ("album", metadata.album.clone()),
            ("date", metadata.year.clone()),
            ("track", format!("{}/{}", chapter.index, count)),
            ("compilation", u8::from(compilation).to_string()),
        ],
    };
    match ffmpeg.cut(&cut, cancelled) {
        Ok(()) => {}
        Err(ffmpeg::Error::Failed(_)) => return Ok(false),
        Err(ffmpeg::Error::Cancelled) => return Err(SplitError::Cancelled),
        Err(error) => return Err(SplitError::Ffmpeg(error)),
    }
    if let Some(source_id) = &metadata.source_id {
        let sidecar = destination.with_extension("muzik.json");
        let payload = json!({
            "version": 1,
            "downloaded_at": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            "source_id": source_id,
        });
        let text = serde_json::to_string_pretty(&payload).map_err(io::Error::other)?;
        fs::write(sidecar, format!("{text}\n"))?;
    }
    Ok(true)
}

fn extract_metadata(source: &Path) -> Metadata {
    let source_json = read_json(&source.with_extension("muzik.json")).or_else(|| {
        read_json(
            &source
                .parent()
                .unwrap_or(Path::new("."))
                .join(".muzik.json"),
        )
    });
    let source_id = source_json
        .as_ref()
        .and_then(|data| data.get("source_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(data) = &source_json {
        let empty = Value::Null;
        let resolved = data
            .get("resolved")
            .filter(|value| value.is_object())
            .unwrap_or(&empty);
        let candidate = data
            .get("candidate")
            .filter(|value| value.is_object())
            .unwrap_or(&empty);
        let candidate_metadata = candidate
            .get("metadata")
            .filter(|value| value.is_object())
            .unwrap_or(&empty);
        let sources = [resolved, candidate_metadata, data, candidate];
        if sources.iter().any(|source| {
            ["title", "track", "artist", "album", "year"]
                .iter()
                .any(|key| field(source, key).is_some())
        }) {
            let artist =
                first_field(&sources, &["artist"]).unwrap_or_else(|| "Unknown Artist".into());
            let album = first_field(&sources, &["album"])
                .or_else(|| field(resolved, "title"))
                .unwrap_or_else(|| "Unknown Album".into());
            let year = first_field(&sources, &["year"]).unwrap_or_else(|| "Unknown".into());
            return Metadata {
                artist,
                album: clean_album_name(&album),
                year: year.chars().take(4).collect(),
                source_id,
            };
        }
    }
    if let Some(info) = read_json(&sidecar_path(source, ".info.json")) {
        let title = field(&info, "title").unwrap_or_else(|| {
            source
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let explicit_artist = field(&info, "artist").filter(|artist| artist != "null");
        let (parsed_artist, parsed_album, parsed_year) = parse_title(&title);
        let artist = explicit_artist
            .clone()
            .or(parsed_artist)
            .or_else(|| field(&info, "uploader"))
            .unwrap_or_else(|| "Unknown Artist".into());
        let album = field(&info, "album")
            .or_else(|| {
                if explicit_artist.is_none() {
                    parsed_album
                } else {
                    None
                }
            })
            .unwrap_or(title);
        let year = parsed_year
            .or_else(|| {
                field(&info, "upload_date")
                    .or_else(|| field(&info, "date"))
                    .map(|value| value.chars().take(4).collect())
            })
            .unwrap_or_else(|| "Unknown".into());
        return Metadata {
            artist,
            album: clean_album_name(&album),
            year,
            source_id,
        };
    }
    let tags = muzik_tags::read(source, &[]).ok();
    let field = |key: &str| tags.as_ref().and_then(|tags| tags.fields.get(key)).cloned();
    Metadata {
        artist: field("artist").unwrap_or_else(|| "Unknown Artist".into()),
        album: clean_album_name(&field("album").unwrap_or_else(|| "Unknown Album".into())),
        year: field("date")
            .map(|year| year.chars().take(4).collect())
            .unwrap_or_else(|| "Unknown".into()),
        source_id,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn field(data: &Value, key: &str) -> Option<String> {
    let value = data.get(key)?;
    match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn first_field(sources: &[&Value], keys: &[&str]) -> Option<String> {
    sources
        .iter()
        .find_map(|source| keys.iter().find_map(|key| field(source, key)))
}

fn parse_title(title: &str) -> (Option<String>, Option<String>, Option<String>) {
    let Ok(year_re) = Regex::new(r"[\(\[]((?:19|20)\d{2})[\)\]]") else {
        return (None, Some(clean_album_name(title)), None);
    };
    let year = year_re
        .captures(title)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str().to_owned());
    let title = year_re.splitn(title, 2).next().unwrap_or(title).trim();
    if let Some((artist, album)) = title.split_once(" - ") {
        (
            Some(artist.trim().into()),
            Some(clean_album_name(album.trim())),
            year,
        )
    } else {
        (None, Some(clean_album_name(title)), year)
    }
}

fn clean_album_name(album: &str) -> String {
    let Ok(year) = Regex::new(r"\s*[\(\[](?:19|20)\d{2}[\)\]]") else {
        return album.trim().to_owned();
    };
    let Ok(noise) = Regex::new(
        r"(?i)\s*[\(\[]\s*(?:full\s+album|complete\s+album|full\s+lp|official\s+album|remaster(?:ed)?|deluxe(?:\s+edition)?|bonus\s+tracks?)\s*[\)\]]",
    ) else {
        return album.trim().to_owned();
    };
    noise
        .replace_all(&year.replace_all(album, ""), "")
        .trim()
        .to_owned()
}

fn parse_artist_title(title: &str) -> Option<(&str, &str)> {
    let separator = Regex::new(r"\s+[-–—]\s+").ok()?;
    let mut parts = separator.splitn(title, 2);
    let artist = parts.next()?.trim();
    let song = parts.next()?.trim();
    (!artist.is_empty() && !song.is_empty()).then_some((artist, song))
}

fn strip_featured(title: &str) -> (String, Vec<String>) {
    let Ok(pattern) = Regex::new(
        r"(?i)\s*(?:[\(\[]\s*(?:feat|ft|featuring)\.?\s+([^\)\]]+?)\s*[\)\]]|(?:feat|ft|featuring)\.?\s+(.+)$)",
    ) else {
        return (title.trim().into(), Vec::new());
    };
    let Some(capture) = pattern.captures(title) else {
        return (title.trim().into(), Vec::new());
    };
    let names = capture
        .get(1)
        .or_else(|| capture.get(2))
        .map_or("", |name| name.as_str());
    let Ok(separator) = Regex::new(r"(?i)\s*(?:,|&|/|\bx\b|\band\b)\s*") else {
        return (title.trim().into(), Vec::new());
    };
    let featured = separator
        .split(names)
        .filter(|name| !name.trim().is_empty())
        .map(|name| name.trim().to_owned())
        .collect();
    let clean = pattern.replacen(title, 1, "").trim().to_owned();
    (
        if clean.is_empty() {
            title.trim().into()
        } else {
            clean
        },
        featured,
    )
}

fn safe_filename(title: &str) -> String {
    let mut slug = String::new();
    for character in title.nfkd() {
        if character.is_ascii_control() || "<>:\"/\\|?*".contains(character) {
            continue;
        }
        if character.is_whitespace() {
            slug.push('-');
        } else if character.is_ascii() {
            slug.push(character.to_ascii_lowercase());
        }
    }
    let slug = slug
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "unknown".into()
    } else {
        slug
    }
}

fn file_hash(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65_536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let chunk = buffer
            .get(..count)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
        hash.update(chunk);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn place_cover(source: &Path, output: &Path) {
    for extension in THUMB_EXTS {
        let thumbnail = sidecar_path(source, extension);
        if thumbnail.exists() {
            let _ = fs::copy(thumbnail, output.join(format!("cover{extension}")));
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn cancellation_stops_active_ffmpeg_and_keeps_source() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Arc;
        use std::time::Duration;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("album.mp3");
        let sidecar = sidecar_path(&source, ".chapters.txt");
        fs::write(&source, b"source audio").unwrap();
        fs::write(&sidecar, b"0:00 Song\n").unwrap();
        let marker = temp.path().join("started");
        let binary = temp.path().join("ffmpeg");
        fs::write(
            &binary,
            format!("#!/bin/sh\ntouch '{}'\nexec sleep 30\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancelled);
        let marker_for_thread = marker.clone();
        let watcher = std::thread::spawn(move || {
            for _ in 0..500 {
                if marker_for_thread.exists() {
                    trigger.store(true, Ordering::SeqCst);
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            trigger.store(true, Ordering::SeqCst);
        });
        let result = split_audio_with_binary(
            &source,
            &[Chapter {
                index: 1,
                start: 0,
                end: Some(1),
                title: "Song".into(),
            }],
            &temp.path().join("output"),
            &SplitOptions::default(),
            &cancelled,
            &mut |_| panic!("cancelled track cannot finish"),
            &Ffmpeg::at(&binary),
        );
        watcher.join().unwrap();
        assert!(marker.exists(), "ffmpeg did not start");
        assert!(matches!(result, Err(SplitError::Cancelled)));
        assert!(source.exists());
        assert!(sidecar.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_complete_earlier_split_is_reused_and_a_partial_one_is_refused() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("album.opus");
        fs::write(&source, b"downloaded again").unwrap();
        fs::write(
            sidecar_path(&source, ".chapters.txt"),
            "0:00 One\n1:00 Two\n",
        )
        .unwrap();
        let binary = temp.path().join("ffmpeg");
        fs::write(&binary, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let chapters = [
            Chapter {
                index: 1,
                start: 0,
                end: Some(60),
                title: "One".into(),
            },
            Chapter {
                index: 2,
                start: 60,
                end: None,
                title: "Two (feat. Guest)".into(),
            },
        ];
        let output = temp.path().join("splits");
        fs::create_dir_all(&output).unwrap();
        fs::write(output.join("01-one.opus"), b"track").unwrap();
        let options = SplitOptions {
            keep_source: true,
            cache_dir: Some(temp.path().join("cache")),
            ..SplitOptions::default()
        };
        let split = |options: &SplitOptions| {
            split_audio_with_binary(
                &source,
                &chapters,
                &output,
                options,
                &AtomicBool::new(false),
                &mut |_| {},
                &Ffmpeg::at(&binary),
            )
        };
        assert!(matches!(
            split(&options),
            Err(SplitError::OutputNotEmpty(folder)) if folder == output
        ));
        fs::write(output.join("02-two.opus"), b"track").unwrap();
        assert_eq!(split(&options).unwrap(), output);
        assert!(source.exists());
        let options = SplitOptions {
            keep_source: false,
            cache_dir: Some(temp.path().join("fresh cache")),
            ..options
        };
        assert_eq!(split(&options).unwrap(), output);
        assert!(!source.exists());
    }

    #[test]
    fn chapter_validation_keeps_input_safe() {
        let chapters = [Chapter {
            index: 1,
            start: 10,
            end: Some(9),
            title: "Song".into(),
        }];
        assert!(matches!(
            validate_chapters(&chapters, Path::new("album.opus"), false),
            Err(SplitError::InvalidChapter(_))
        ));
    }

    #[test]
    fn compilation_title_and_featured_credit() {
        assert_eq!(
            parse_artist_title("Jean-Luc – Song (feat. A & B)"),
            Some(("Jean-Luc", "Song (feat. A & B)"))
        );
        assert_eq!(
            strip_featured("Song (feat. A & B)"),
            ("Song".into(), vec!["A".into(), "B".into()])
        );
    }

    #[test]
    fn source_metadata_precedes_download_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Album.opus");
        fs::write(&source, []).unwrap();
        fs::write(
            source.with_extension("muzik.json"),
            r#"{"source_id":"vid1","resolved":{"artist":"Main","album":"Record","year":2005}}"#,
        )
        .unwrap();
        fs::write(
            sidecar_path(&source, ".info.json"),
            r#"{"artist":"Other","album":"Other"}"#,
        )
        .unwrap();
        let metadata = extract_metadata(&source);
        assert_eq!(metadata.artist, "Main");
        assert_eq!(metadata.album, "Record");
        assert_eq!(metadata.year, "2005");
        assert_eq!(metadata.source_id.as_deref(), Some("vid1"));
    }

    #[test]
    fn ffmpeg_split_writes_tracks_and_keeps_source_when_requested() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Album.mp3");
        let result = std::process::Command::new("ffmpeg")
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:a",
                "libmp3lame",
                "-y",
            ])
            .arg(&source)
            .output()
            .unwrap();
        assert!(result.status.success());
        fs::write(
            source.with_extension("muzik.json"),
            r#"{"source_id":"vid1","resolved":{"artist":"Main","album":"Record","year":2005}}"#,
        )
        .unwrap();
        fs::write(
            sidecar_path(&source, ".chapters.txt"),
            "00:00 One\n00:01 Two\n",
        )
        .unwrap();
        fs::write(sidecar_path(&source, ".jpg"), b"cover").unwrap();
        let chapters = vec![
            Chapter {
                index: 1,
                start: 0,
                end: Some(1),
                title: "One".into(),
            },
            Chapter {
                index: 2,
                start: 1,
                end: None,
                title: "Two".into(),
            },
        ];
        let output = temp.path().join("splits");
        let options = SplitOptions {
            jobs: 2,
            keep_source: true,
            cache_dir: Some(temp.path().join("cache")),
            ..SplitOptions::default()
        };
        assert_eq!(
            split_audio(&source, &chapters, &output, &options).unwrap(),
            output
        );
        assert!(source.exists());
        assert_eq!(fs::read(output.join("cover.jpg")).unwrap(), b"cover");
        for (index, title) in [(1, "One"), (2, "Two")] {
            let track = output.join(format!("0{index}-{}.mp3", title.to_lowercase()));
            assert!(track.exists());
            let tags = muzik_tags::read(&track, &[]).unwrap();
            assert_eq!(tags.fields.get("title").map(String::as_str), Some(title));
            assert_eq!(tags.fields.get("artist").map(String::as_str), Some("Main"));
            let sidecar = read_json(&track.with_extension("muzik.json")).unwrap();
            assert_eq!(
                sidecar.get("source_id").and_then(Value::as_str),
                Some("vid1")
            );
        }
        assert_eq!(
            default_output(&source).unwrap(),
            temp.path().parent().unwrap().join("splits/record")
        );
        assert_eq!(
            split_audio(&source, &chapters, &output, &options).unwrap(),
            output
        );
        let options = SplitOptions {
            force: true,
            keep_source: false,
            ..options
        };
        assert_eq!(
            split_audio(&source, &chapters, &output, &options).unwrap(),
            output
        );
        assert!(!source.exists());
        assert!(!sidecar_path(&source, ".chapters.txt").exists());
        assert!(!sidecar_path(&source, ".jpg").exists());
        assert!(output.join("01-one.mp3").exists());
    }
}
