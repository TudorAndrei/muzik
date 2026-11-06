//! Resumable purchase downloads and archive extraction.

use crate::{BandcampFormat, Login, Result, USER_AGENT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const REQUEST_BUDGET: Duration = Duration::from_secs(120);
const DOWNLOAD_ATTEMPTS: u32 = 5;
const MAX_ARCHIVE_ENTRIES: usize = 10_000;
const MAX_ARCHIVE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

struct PartialDownload<'a> {
    path: &'a Path,
    identity: &'a str,
}

#[derive(Deserialize, Serialize)]
struct Representation {
    identity: String,
    etag: Option<String>,
    total: Option<u64>,
    name: String,
}

impl PartialDownload<'_> {
    fn metadata_path(&self) -> PathBuf {
        self.path.with_extension("part.json")
    }

    fn representation(&self) -> Option<Representation> {
        let saved: Representation =
            serde_json::from_slice(&fs::read(self.metadata_path()).ok()?).ok()?;
        (saved.identity == self.identity && saved.etag.is_some() && safe_name(&saved.name))
            .then_some(saved)
    }

    fn discard(&self) {
        let _ = fs::remove_file(self.path);
        let _ = fs::remove_file(self.metadata_path());
    }
}

/// # Errors
/// Returns an error when the download fails or is cancelled, when the ZIP file is not safe, or when it has no audio files.
pub fn download(
    login: &Login,
    download_page: &str,
    format: BandcampFormat,
    destination: &Path,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Vec<PathBuf>> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("Bandcamp download cancelled".into());
    }
    let folder = destination
        .file_name()
        .ok_or("The Bandcamp download folder has no name.")?
        .to_string_lossy();
    let format = format.as_ref();
    let partial = destination.with_file_name(format!(".{folder}.{format}.part"));
    let identity = format!("{download_page}#{format}");
    let partial = PartialDownload {
        path: &partial,
        identity: &identity,
    };
    fs::create_dir_all(
        destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?;
    let mut locate = || {
        let blob = login.page_blob(download_page)?;
        let item = blob
            .get("digital_items")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .ok_or("Bandcamp has no download for this purchase.")?;
        let downloads = item["downloads"]
            .as_object()
            .ok_or("Bandcamp has no download for this purchase.")?;
        downloads
            .get(format)
            .and_then(|download| download.get("url"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                format!(
                    "Bandcamp has no {format} download for this purchase. It has: {}.",
                    downloads.keys().cloned().collect::<Vec<_>>().join(", ")
                )
                .into()
            })
    };
    let name = transfer(
        &mut locate,
        &login.header(),
        &partial,
        cancelled,
        on_progress,
    )?;
    install(&partial, &name, destination)
}

fn install(partial: &PartialDownload<'_>, name: &str, destination: &Path) -> Result<Vec<PathBuf>> {
    if Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        if let Err(error) = extract(partial.path, destination) {
            partial.discard();
            return Err(error);
        }
    } else {
        let stage = staging(destination)?;
        let payload = stage.path().join("content");
        fs::create_dir(&payload)?;
        fs::copy(partial.path, payload.join(name))?;
        publish(stage, destination)?;
    }
    partial.discard();
    Ok(audio_files(destination))
}

fn staging(destination: &Path) -> Result<tempfile::TempDir> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    Ok(tempfile::Builder::new()
        .prefix(".muzik-bandcamp-")
        .tempdir_in(parent)?)
}

fn publish(stage: tempfile::TempDir, destination: &Path) -> Result<()> {
    let payload = stage.path().join("content");
    if audio_files(&payload).is_empty() {
        return Err("The Bandcamp download has no audio files.".into());
    }
    let backup = stage.path().join("previous");
    let had_destination = match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_dir() && !metadata.is_symlink() => {
            fs::rename(destination, &backup)?;
            true
        }
        Ok(_) => return Err("The Bandcamp destination is not a regular directory.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if let Err(error) = fs::rename(&payload, destination) {
        if had_destination && let Err(restore) = fs::rename(&backup, destination) {
            let recovery = stage.keep();
            return Err(format!("The Bandcamp purchase could not be installed: {error}. Its previous files could not be restored: {restore}. Recover them from {}.", recovery.join("previous").display()).into());
        }
        return Err(error.into());
    }
    Ok(())
}

fn transfer(
    locate: &mut dyn FnMut() -> Result<String>,
    cookie: &str,
    partial: &PartialDownload<'_>,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<String> {
    if cancelled.load(Ordering::SeqCst) {
        partial.discard();
        return Err("Bandcamp download cancelled".into());
    }
    let mut url = locate()?;
    let mut failures: u32 = 0;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            partial.discard();
            return Err("Bandcamp download cancelled".into());
        }
        let before = saved_bytes(partial.path);
        let attempt = fetch(&url, cookie, partial, cancelled, on_progress);
        let error = match attempt {
            Ok(name) => return Ok(name),
            Err(error) => error,
        };
        failures = if saved_bytes(partial.path) > before {
            0
        } else {
            failures.saturating_add(1)
        };
        if failures >= DOWNLOAD_ATTEMPTS {
            return Err(error);
        }
        if failures > 0 {
            wait(Duration::from_secs(1 << failures), cancelled);
        }
        if !cancelled.load(Ordering::SeqCst) {
            url = locate()?;
        }
    }
}

fn fetch(
    url: &str,
    cookie: &str,
    partial: &PartialDownload<'_>,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<String> {
    let saved = partial.representation();
    let offset = if saved.is_some() {
        saved_bytes(partial.path)
    } else {
        0
    };
    let mut request = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .header("Accept-Encoding", "identity");
    if !cookie.is_empty() {
        crate::login::credential_url(url)?;
        request = request.header("Cookie", cookie);
    }
    if offset > 0
        && let Some(saved) = &saved
    {
        request = request.header("Range", format!("bytes={offset}-"));
        if let Some(etag) = &saved.etag {
            request = request.header("If-Range", etag);
        }
    }
    let response = request
        .config()
        .https_only(!cookie.is_empty())
        .http_status_as_error(false)
        .timeout_global(Some(REQUEST_BUDGET))
        .build()
        .call()
        .map_err(|error| format!("Bandcamp did not send the download: {error}"))?;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let etag = header("etag").filter(|etag| etag.starts_with('"') && etag.ends_with('"'));
    if response.status() == 416 {
        if let Some(saved) = saved
            && offset > 0
            && saved.total == Some(offset)
            && saved.etag == etag
            && header("content-range").as_deref() == Some(format!("bytes */{offset}").as_str())
        {
            on_progress(offset, Some(offset));
            return Ok(saved.name);
        }
        partial.discard();
        return Err("Bandcamp did not accept the saved part of the download.".into());
    }
    if response.status() != 200 && response.status() != 206 {
        return Err(format!(
            "Bandcamp did not send the download: HTTP {}",
            response.status()
        )
        .into());
    }
    let name = header("content-disposition")
        .and_then(|value| disposition_name(&value))
        .ok_or("Bandcamp sent a download without a safe file name.")?;
    let resumed = response.status() == 206;
    if resumed
        && offset > 0
        && saved
            .as_ref()
            .is_none_or(|saved| saved.etag != etag || saved.name != name)
    {
        partial.discard();
        return Err("Bandcamp changed the saved download.".into());
    }
    let total = if resumed {
        match header("content-range").and_then(|value| content_range(&value)) {
            Some((start, Some(total))) if start == offset => Some(total),
            _ => {
                partial.discard();
                return Err("Bandcamp sent the wrong part of the download.".into());
            }
        }
    } else {
        header("content-length").and_then(|value| value.parse().ok())
    };
    receive(
        response,
        partial,
        Representation {
            identity: partial.identity.to_owned(),
            etag,
            total,
            name,
        },
        if resumed { offset } else { 0 },
        cancelled,
        on_progress,
    )
}

fn receive(
    response: ureq::http::Response<ureq::Body>,
    partial: &PartialDownload<'_>,
    representation: Representation,
    offset: u64,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(offset == 0)
        .append(offset > 0)
        .open(partial.path)?;
    fs::write(
        partial.metadata_path(),
        serde_json::to_vec(&representation).map_err(std::io::Error::other)?,
    )?;
    let mut received = offset;
    let mut reader = response.into_body().into_reader();
    let mut buffer = vec![0; 256 * 1024];
    on_progress(received, representation.total);
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err("Bandcamp download cancelled".into());
        }
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("The Bandcamp download stopped: {error}"))?;
        if count == 0 {
            break;
        }
        let chunk = buffer
            .get(..count)
            .ok_or("The Bandcamp download stopped: the reader sent too many bytes.")?;
        let next = u64::try_from(count)
            .ok()
            .and_then(|count| received.checked_add(count))
            .ok_or("The Bandcamp download is too large.")?;
        if representation.total.is_some_and(|total| next > total) {
            partial.discard();
            return Err("The Bandcamp download has too many bytes.".into());
        }
        file.write_all(chunk)?;
        received = next;
        on_progress(received, representation.total);
    }
    file.sync_all()?;
    if representation.total.is_some_and(|total| total != received) {
        return Err("The Bandcamp download ended early.".into());
    }
    Ok(representation.name)
}

fn saved_bytes(partial: &Path) -> u64 {
    fs::metadata(partial).map_or(0, |metadata| metadata.len())
}

fn content_range(header: &str) -> Option<(u64, Option<u64>)> {
    let number = |text: &str| {
        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse::<u64>().ok())
            .flatten()
    };
    let (unit, range) = header.trim().split_once(' ')?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let (range, total) = range.split_once('/')?;
    let (first, last) = range.split_once('-')?;
    let (first, last) = (number(first)?, number(last)?);
    let total = match total {
        "*" => None,
        total => Some(number(total)?),
    };
    (first <= last && total.is_none_or(|total| last < total)).then_some((first, total))
}

fn wait(duration: Duration, cancelled: &AtomicBool) {
    let start = Instant::now();
    while start.elapsed() < duration && !cancelled.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn disposition_name(header: &str) -> Option<String> {
    let encoded = header
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("filename*="))
        .and_then(|value| {
            value.split_once("''").map(|(_, name)| {
                percent_encoding::percent_decode_str(name)
                    .decode_utf8_lossy()
                    .into_owned()
            })
        });
    let plain = || {
        header
            .split(';')
            .map(str::trim)
            .find_map(|part| part.strip_prefix("filename="))
            .map(|value| value.trim_matches('"').to_owned())
    };
    let name = encoded.or_else(plain)?;
    let name = name.trim();
    safe_name(name).then(|| name.to_owned())
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0', ':'])
        && !name.starts_with('.')
}

fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let file = fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| format!("The Bandcamp download is not a valid ZIP file: {error}"))?;
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        return Err("The Bandcamp ZIP file has too many entries.".into());
    }
    let mut files = std::collections::HashSet::new();
    let mut declared = 0_u64;
    for index in 0..zip.len() {
        let entry = zip
            .by_index_raw(index)
            .map_err(|error| format!("The Bandcamp ZIP file did not extract: {error}"))?;
        let name = entry
            .name()
            .map_err(|error| format!("The Bandcamp ZIP file did not extract: {error}"))?
            .into_owned();
        if entry.is_symlink() {
            return Err(format!("The Bandcamp ZIP file contains a link: {name}").into());
        }
        let path = entry
            .enclosed_name()
            .filter(|_| !name.contains(['\\', ':']))
            .ok_or("The Bandcamp ZIP file has an unsafe path.")?;
        if !entry.is_dir() && !files.insert(path) {
            return Err(
                format!("The Bandcamp ZIP file contains the same file twice: {name}").into(),
            );
        }
        declared = declared
            .checked_add(entry.size())
            .filter(|size| *size <= MAX_ARCHIVE_BYTES)
            .ok_or("The Bandcamp ZIP file is too large.")?;
    }
    let stage = staging(destination)?;
    let payload = stage.path().join("content");
    fs::create_dir(&payload)?;
    let mut written = 0_u64;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("The Bandcamp ZIP file did not extract: {error}"))?;
        let path = entry
            .enclosed_name()
            .ok_or("The Bandcamp ZIP file has an unsafe path.")?;
        let target = payload.join(path);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        let remaining = MAX_ARCHIVE_BYTES.saturating_sub(written);
        let bytes = std::io::copy(
            &mut entry.by_ref().take(remaining.saturating_add(1)),
            &mut file,
        )?;
        written = written
            .checked_add(bytes)
            .filter(|size| *size <= MAX_ARCHIVE_BYTES)
            .ok_or("The Bandcamp ZIP file is too large.")?;
    }
    open_folders(&payload)?;
    publish(stage, destination)
}

#[cfg(unix)]
fn open_folders(folder: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::symlink_metadata(folder)?.permissions().mode();
    fs::set_permissions(folder, fs::Permissions::from_mode((mode & 0o777) | 0o700))?;
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            open_folders(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn open_folders(_folder: &Path) -> std::io::Result<()> {
    Ok(())
}

pub fn audio_files(directory: &Path) -> Vec<PathBuf> {
    if fs::symlink_metadata(directory).is_ok_and(|metadata| metadata.is_symlink()) {
        return Vec::new();
    }
    let mut files = walkdir::WalkDir::new(directory)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| muzik_core::audio::is_audio(entry.path()) && entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_interrupted_download_continues_from_the_saved_bytes() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let body = (0..200_000u32)
            .map(|index| u8::try_from(index % 251))
            .collect::<Result<Vec<u8>, _>>()
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/album.zip", listener.local_addr().unwrap());
        let payload = body.clone();
        let server = std::thread::spawn(move || -> std::io::Result<Vec<Option<String>>> {
            let mut ranges = Vec::new();
            for cut in [true, false] {
                let (mut stream, _) = listener.accept()?;
                let mut range = None;
                let mut reader = BufReader::new(stream.try_clone()?);
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line)?;
                    if line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("range")
                    {
                        range = Some(value.trim().to_owned());
                    }
                }
                let start = range
                    .as_deref()
                    .and_then(|range| range.strip_prefix("bytes="))
                    .and_then(|range| range.trim_end_matches('-').parse::<usize>().ok())
                    .unwrap_or(0);
                let status = if start > 0 {
                    format!(
                        "206 Partial Content\r\nContent-Range: bytes {start}-{}/{}",
                        payload.len() - 1,
                        payload.len()
                    )
                } else {
                    "200 OK".to_owned()
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nETag: \"album-v1\"\r\nContent-Length: {}\r\nContent-Disposition: attachment; filename=\"Band - Album.zip\"\r\nConnection: close\r\n\r\n",
                    payload.len() - start
                )?;
                let end = if cut {
                    payload.len() / 2
                } else {
                    payload.len()
                };
                stream.write_all(&payload[start..end])?;
                ranges.push(range);
            }
            Ok(ranges)
        });
        let directory = tempfile::tempdir().unwrap();
        let partial = directory.path().join(".album.flac.part");
        let mut progress = Vec::new();
        let name = transfer(
            &mut || Ok(url.clone()),
            "",
            &PartialDownload {
                path: &partial,
                identity: "purchase",
            },
            &AtomicBool::new(false),
            &mut |received, total| progress.push((received, total)),
        )
        .unwrap();
        let ranges = server.join().unwrap().unwrap();
        assert_eq!(name, "Band - Album.zip");
        assert_eq!(fs::read(&partial).unwrap(), body);
        assert_eq!(ranges, [None, Some(format!("bytes={}-", body.len() / 2))]);
        let size = u64::try_from(body.len()).unwrap();
        assert_eq!(progress.last(), Some(&(size, Some(size))));
    }

    #[test]
    fn download_names_stay_inside_the_destination() {
        assert_eq!(
            disposition_name(r#"attachment; filename="Band - Album.zip""#),
            Some("Band - Album.zip".into())
        );
        assert_eq!(
            disposition_name("attachment; filename*=UTF-8''Band%20-%20Caf%C3%A9.zip"),
            Some("Band - Café.zip".into())
        );
        assert_eq!(
            disposition_name(r#"attachment; filename="GORE - 空の通り.flac""#),
            Some("GORE - 空の通り.flac".into())
        );
        assert_eq!(disposition_name(r#"attachment; filename="../x.zip""#), None);
        assert_eq!(disposition_name(r#"attachment; filename="..""#), None);
        for unsafe_name in ["C:outside.flac", "C:\\outside.flac", "song.flac:stream"] {
            assert_eq!(
                disposition_name(&format!("attachment; filename=\"{unsafe_name}\"")),
                None
            );
        }
    }

    fn assert_one_safe_component(header: &str) {
        if let Some(name) = disposition_name(header) {
            let components: Vec<_> = Path::new(&name).components().collect();
            assert_eq!(
                components,
                [std::path::Component::Normal(name.as_ref())],
                "{header:?}"
            );
            assert!(!name.starts_with('.'), "{header:?}");
        }
    }

    #[test]
    fn any_download_name_is_one_file_name_inside_the_destination() {
        bolero::check!().with_type::<String>().for_each(|text| {
            assert_one_safe_component(text);
            assert_one_safe_component(&format!("attachment; filename=\"{text}\""));
            let encoded =
                percent_encoding::utf8_percent_encode(text, percent_encoding::NON_ALPHANUMERIC);
            let header = format!("attachment; filename*=UTF-8''{encoded}");
            assert_one_safe_component(&header);
            if let Some(name) = disposition_name(&header) {
                assert_eq!(name, text.trim());
            }
        });
    }

    #[test]
    fn resumed_ranges_follow_the_content_range_grammar() {
        assert_eq!(content_range("bytes 100-199/200"), Some((100, Some(200))));
        assert_eq!(content_range("bytes 0-0/*"), Some((0, None)));
        for malformed in [
            "bytes 100-/200",
            "bytes -199/200",
            "bytes 100-199",
            "bytes 200-100/300",
            "bytes 100-200/200",
            "bytes +1-2/3",
            "bytes  1-2/3",
            "bytes 1 -2/3",
            "bytes 1-2/abc",
            "bytes */200",
            "items 1-2/3",
            "bytes 1-18446744073709551616/*",
        ] {
            assert_eq!(content_range(malformed), None, "{malformed}");
        }
    }

    #[test]
    fn any_accepted_content_range_is_well_formed() {
        bolero::check!().with_type::<String>().for_each(|header| {
            let Some((start, total)) = content_range(header) else {
                return;
            };
            let rest = header.trim();
            assert!(rest.get(..6).unwrap().eq_ignore_ascii_case("bytes "));
            let (range, length) = rest.get(6..).unwrap().split_once('/').unwrap();
            let (first, last) = range.split_once('-').unwrap();
            for part in [first, last] {
                assert!(!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
            }
            assert_eq!(first.parse::<u64>().unwrap(), start);
            let last = last.parse::<u64>().unwrap();
            assert!(start <= last);
            match total {
                Some(total) => {
                    assert_eq!(length.parse::<u64>().unwrap(), total);
                    assert!(length.bytes().all(|byte| byte.is_ascii_digit()));
                    assert!(last < total);
                }
                None => assert_eq!(length, "*"),
            }
        });
    }

    #[test]
    fn any_numeric_content_range_is_accepted_only_inside_the_length() {
        bolero::check!()
            .with_type::<(u64, u64, Option<u64>)>()
            .for_each(|&(first, last, total)| {
                let length = total.map_or_else(|| "*".to_owned(), |total| total.to_string());
                let valid = first <= last && total.is_none_or(|total| last < total);
                assert_eq!(
                    content_range(&format!("bytes {first}-{last}/{length}")),
                    valid.then_some((first, total))
                );
            });
    }

    #[test]
    fn archive_entries_stay_inside_the_destination() {
        let directory = tempfile::tempdir().unwrap();
        let write = |name: &str, entries: &[&str]| -> Result<PathBuf, Box<dyn std::error::Error>> {
            let path = directory.path().join(name);
            let mut zip = zip::ZipWriter::new(fs::File::create(&path)?);
            for entry in entries {
                zip.start_file(*entry, zip::write::SimpleFileOptions::default())?;
                zip.write_all(b"audio")?;
            }
            zip.finish()?;
            Ok(path)
        };
        let destination = directory.path().join("out");
        extract(
            &write("good.zip", &["Album/01 Song.flac"]).unwrap(),
            &destination,
        )
        .unwrap();
        assert_eq!(
            fs::read(destination.join("Album/01 Song.flac")).unwrap(),
            b"audio"
        );

        let unsafe_archive = write("bad.zip", &["../escape.flac"]).unwrap();
        assert!(extract(&unsafe_archive, &destination).is_err());
        assert!(!directory.path().join("escape.flac").exists());

        let linked = directory.path().join("link.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&linked).unwrap());
        zip.add_symlink("Album", "/", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.finish().unwrap();
        let linked_destination = directory.path().join("linked");
        assert!(extract(&linked, &linked_destination).is_err());
        assert!(!linked_destination.join("Album").exists());

        let twice = write("twice.zip", &["Album/01 Song.flac", "Album//01 Song.flac"]).unwrap();
        assert!(extract(&twice, &directory.path().join("twice")).is_err());
    }

    fn lexical(path: &Path) -> PathBuf {
        let mut normal = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    normal.pop();
                }
                std::path::Component::CurDir => {}
                other => normal.push(other),
            }
        }
        normal
    }

    fn assert_links_stay_inside(folder: &Path, root: &Path) {
        for entry in fs::read_dir(folder).unwrap().map(Result::unwrap) {
            let path = entry.path();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_symlink() {
                let resolved = fs::canonicalize(&path)
                    .unwrap_or_else(|_| lexical(&folder.join(fs::read_link(&path).unwrap())));
                assert!(resolved.starts_with(root), "{path:?} -> {resolved:?}");
            } else if kind.is_dir() {
                assert_links_stay_inside(&path, root);
            }
        }
    }

    #[test]
    fn any_archive_writes_only_inside_the_destination() {
        let name = |bytes: &[u8]| -> String {
            bytes
                .iter()
                .map(|byte| ['a', '.', '/', '\\', 'b'][usize::from(*byte) % 5])
                .collect()
        };
        bolero::check!()
            .with_type::<Vec<(bool, Vec<u8>, Vec<u8>)>>()
            .for_each(|entries| {
                let directory = tempfile::tempdir().unwrap();
                let root = directory.path().canonicalize().unwrap();
                let archive = root.join("archive.zip");
                let destination = root.join("out");
                fs::create_dir(&destination).unwrap();
                let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
                for (link, path, target) in entries {
                    let options = zip::write::SimpleFileOptions::default();
                    if *link {
                        let _ = zip.add_symlink(name(path), name(target), options);
                    } else if zip.start_file(name(path), options).is_ok() {
                        zip.write_all(b"audio").unwrap();
                    }
                }
                if zip.finish().is_err() {
                    return;
                }
                let _ = extract(&archive, &destination);
                let mut top: Vec<_> = fs::read_dir(&root)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect();
                top.sort();
                assert_eq!(top, ["archive.zip", "out"]);
                assert_links_stay_inside(&destination, &destination);
            });
    }

    #[test]
    fn an_unavailable_download_page_returns_its_error_without_retry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial");
        let cancelled = AtomicBool::new(false);
        let mut requests = 0;
        let result = transfer(
            &mut || {
                requests += 1;
                if requests > 1 {
                    cancelled.store(true, Ordering::SeqCst);
                }
                Err("purchase has no requested format".into())
            },
            "",
            &PartialDownload {
                path: &path,
                identity: "purchase",
            },
            &cancelled,
            &mut |_, _| {},
        );
        assert_eq!(requests, 1);
        assert_eq!(
            result.unwrap_err().to_string(),
            "purchase has no requested format"
        );
    }

    #[test]
    fn cancellation_stops_before_the_download_page_is_requested() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial");
        let partial = PartialDownload {
            path: &path,
            identity: "purchase",
        };
        let mut requested = false;
        let result = transfer(
            &mut || {
                requested = true;
                Err("unexpected request".into())
            },
            "",
            &partial,
            &AtomicBool::new(true),
            &mut |_, _| {},
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(!requested);
    }

    fn response_once(response: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/album", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                request.push_str(&line);
                if line.trim().is_empty() {
                    break;
                }
            }
            stream.write_all(&response).unwrap();
            request.to_ascii_lowercase()
        });
        (url, server)
    }

    #[test]
    fn resume_requires_the_same_purchase_and_a_strong_representation_validator() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial");
        for (saved_identity, saved_etag, response_status, response_etag, expected) in [
            (
                "purchase",
                None,
                "206 Partial Content\r\nContent-Range: bytes 0-2/*",
                "\"new\"",
                None,
            ),
            (
                "purchase",
                Some("\"old\""),
                "206 Partial Content\r\nContent-Range: bytes 3-5/6",
                "\"new\"",
                None,
            ),
            (
                "other purchase",
                Some("\"old\""),
                "200 OK",
                "\"new\"",
                Some(b"new".as_slice()),
            ),
            (
                "purchase",
                None,
                "200 OK",
                "\"new\"",
                Some(b"new".as_slice()),
            ),
            (
                "purchase",
                Some("\"old\""),
                "200 OK",
                "\"new\"",
                Some(b"new".as_slice()),
            ),
        ] {
            fs::write(&path, b"old").unwrap();
            let partial = PartialDownload {
                path: &path,
                identity: "purchase",
            };
            fs::write(
                partial.metadata_path(),
                serde_json::to_vec(&Representation {
                    identity: saved_identity.into(),
                    etag: saved_etag.map(Into::into),
                    total: Some(6),
                    name: "album.flac".into(),
                })
                .unwrap(),
            )
            .unwrap();
            let response = format!(
                "HTTP/1.1 {response_status}\r\nETag: {response_etag}\r\nContent-Length: 3\r\nContent-Disposition: attachment; filename=album.flac\r\nConnection: close\r\n\r\nnew"
            );
            let (url, server) = response_once(response.into_bytes());
            let result = fetch(&url, "", &partial, &AtomicBool::new(false), &mut |_, _| {});
            let request = server.join().unwrap();
            let resumed = saved_identity == "purchase" && saved_etag.is_some();
            assert_eq!(request.contains("range: bytes=3-"), resumed);
            assert_eq!(request.contains("if-range: \"old\""), resumed);
            if let Some(expected) = expected {
                result.unwrap();
                assert_eq!(fs::read(&path).unwrap(), expected);
            } else {
                let message = result.unwrap_err().to_string();
                let expected = if response_status.contains("/*") {
                    "wrong part"
                } else {
                    "changed"
                };
                assert!(message.contains(expected), "{message}");
                assert!(!path.exists());
            }
        }
    }

    #[test]
    fn a_complete_saved_representation_is_accepted_after_a_416() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial");
        fs::write(&path, b"complete").unwrap();
        let partial = PartialDownload {
            path: &path,
            identity: "purchase",
        };
        fs::write(
            partial.metadata_path(),
            serde_json::to_vec(&Representation {
                identity: "purchase".into(),
                etag: Some("\"v1\"".into()),
                total: Some(8),
                name: "album.flac".into(),
            })
            .unwrap(),
        )
        .unwrap();
        let (url, server) = response_once(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */8\r\nETag: \"v1\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
        assert_eq!(
            fetch(&url, "", &partial, &AtomicBool::new(false), &mut |_, _| {}).unwrap(),
            "album.flac"
        );
        assert!(server.join().unwrap().contains("if-range: \"v1\""));
        assert_eq!(fs::read(&path).unwrap(), b"complete");
    }

    #[test]
    fn replacement_publishes_only_complete_new_audio() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("album");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("old.flac"), b"old").unwrap();
        let path = directory.path().join("partial");
        let partial = PartialDownload {
            path: &path,
            identity: "purchase",
        };
        fs::write(&path, b"not audio").unwrap();
        assert!(install(&partial, "notes.txt", &destination).is_err());
        assert_eq!(fs::read(destination.join("old.flac")).unwrap(), b"old");
        fs::write(&path, b"new audio").unwrap();
        let files = install(&partial, "new.flac", &destination).unwrap();
        assert_eq!(files, [destination.join("new.flac")]);
        assert!(!destination.join("old.flac").exists());
    }

    fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, data) in entries {
            zip.start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn a_corrupt_archive_does_not_publish_earlier_audio() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("album.zip");
        archive(
            &path,
            &[("first.flac", b"first"), ("second.flac", b"corrupt-this")],
        );
        let mut bytes = fs::read(&path).unwrap();
        let offset = bytes
            .windows(12)
            .position(|data| data == b"corrupt-this")
            .unwrap();
        bytes[offset] ^= 1;
        fs::write(&path, bytes).unwrap();
        let destination = directory.path().join("out");
        assert!(extract(&path, &destination).is_err());
        assert!(audio_files(&destination).is_empty());
        assert!(!destination.exists());
    }

    #[test]
    fn archive_entry_and_expanded_byte_limits_apply_before_publication() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.zip");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        for index in 0..=MAX_ARCHIVE_ENTRIES {
            zip.start_file(index.to_string(), zip::write::SimpleFileOptions::default())
                .unwrap();
        }
        zip.finish().unwrap();
        let destination = directory.path().join("out");
        assert!(
            extract(&path, &destination)
                .unwrap_err()
                .to_string()
                .contains("too many entries")
        );
        archive(
            &path,
            &[
                ("a.flac", b"a"),
                ("b.flac", b"b"),
                ("c.flac", b"c"),
                ("d.flac", b"d"),
                ("e.flac", b"e"),
                ("f.flac", b"f"),
            ],
        );
        let mut bytes = fs::read(&path).unwrap();
        let offsets = bytes
            .windows(4)
            .enumerate()
            .filter_map(|(index, bytes)| (bytes == b"PK\x01\x02").then_some(index))
            .collect::<Vec<_>>();
        for offset in offsets {
            bytes[offset + 24..offset + 28].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
        }
        fs::write(&path, bytes).unwrap();
        assert!(
            extract(&path, &destination)
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
        assert!(!destination.exists());
    }

    #[cfg(unix)]
    #[test]
    fn purchase_content_never_follows_filesystem_links() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let outside = directory.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("outside.flac"), b"outside").unwrap();
        let destination = directory.path().join("album");
        fs::create_dir(&destination).unwrap();
        symlink(&outside, destination.join("linked-folder")).unwrap();
        symlink(
            outside.join("outside.flac"),
            destination.join("linked.flac"),
        )
        .unwrap();
        assert!(audio_files(&destination).is_empty());
        let root_link = directory.path().join("root-link");
        symlink(&outside, &root_link).unwrap();
        assert!(audio_files(&root_link).is_empty());
        let path = directory.path().join("album.zip");
        archive(&path, &[("linked-folder/new.flac", b"new")]);
        extract(&path, &destination).unwrap();
        assert_eq!(
            audio_files(&destination),
            [destination.join("linked-folder/new.flac")]
        );
        assert!(!outside.join("new.flac").exists());
        assert_eq!(fs::read(outside.join("outside.flac")).unwrap(), b"outside");
        assert!(extract(&path, &root_link).is_err());
    }
}
