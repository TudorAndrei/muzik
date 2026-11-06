//! Resumable purchase downloads and archive extraction.

use crate::{BandcampFormat, Login, Result, USER_AGENT};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const REQUEST_BUDGET: Duration = Duration::from_secs(120);
const DOWNLOAD_ATTEMPTS: u32 = 5;

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
    let folder = destination
        .file_name()
        .ok_or("The Bandcamp download folder has no name.")?
        .to_string_lossy();
    let format = format.as_ref();
    let partial = destination.with_file_name(format!(".{folder}.{format}.part"));
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
    fs::create_dir_all(destination)?;
    let target = destination.join(&name);
    fs::rename(&partial, &target)?;
    if target
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        extract(&target, destination)?;
        fs::remove_file(&target)?;
    }
    let files = audio_files(destination);
    if files.is_empty() {
        return Err("The Bandcamp download has no audio files.".into());
    }
    Ok(files)
}

fn transfer(
    locate: &mut dyn FnMut() -> Result<String>,
    cookie: &str,
    partial: &Path,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<String> {
    let mut url = Some(locate()?);
    let mut failures: u32 = 0;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            let _ = fs::remove_file(partial);
            return Err("Bandcamp download cancelled".into());
        }
        let before = saved_bytes(partial);
        let attempt = match url.take() {
            Some(url) => fetch(&url, cookie, partial, cancelled, on_progress),
            None => locate().and_then(|url| fetch(&url, cookie, partial, cancelled, on_progress)),
        };
        let error = match attempt {
            Ok(name) => return Ok(name),
            Err(error) => error,
        };
        failures = if saved_bytes(partial) > before {
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
    }
}

fn fetch(
    url: &str,
    cookie: &str,
    partial: &Path,
    cancelled: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<String> {
    let offset = saved_bytes(partial);
    let mut request = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .header("Cookie", cookie);
    if offset > 0 {
        request = request.header("Range", format!("bytes={offset}-"));
    }
    let response = match request
        .config()
        .timeout_global(Some(REQUEST_BUDGET))
        .build()
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(416)) => {
            let _ = fs::remove_file(partial);
            return Err("Bandcamp did not accept the saved part of the download.".into());
        }
        Err(error) => return Err(format!("Bandcamp did not send the download: {error}").into()),
    };
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
    };
    let name = header("content-disposition")
        .and_then(|value| disposition_name(&value))
        .ok_or("Bandcamp sent a download without a safe file name.")?;
    let resumed = response.status() == 206;
    let total = if resumed {
        match header("content-range").and_then(|value| content_range(&value)) {
            Some((start, total)) if start == offset => total,
            _ => {
                let _ = fs::remove_file(partial);
                return Err("Bandcamp sent the wrong part of the download.".into());
            }
        }
    } else {
        header("content-length").and_then(|value| value.parse().ok())
    };
    let mut file = if resumed {
        fs::OpenOptions::new().append(true).open(partial)
    } else {
        fs::File::create(partial)
    }?;
    let mut received = if resumed { offset } else { 0 };
    let mut reader = response.into_body().into_reader();
    let mut buffer = vec![0; 256 * 1024];
    on_progress(received, total);
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
        file.write_all(chunk)?;
        received = u64::try_from(count)
            .ok()
            .and_then(|count| received.checked_add(count))
            .ok_or("The Bandcamp download is too large.")?;
        on_progress(received, total);
    }
    file.sync_all()?;
    if total.is_some_and(|total| total != received) {
        return Err("The Bandcamp download ended early.".into());
    }
    Ok(name)
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
    let safe = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !name.starts_with('.');
    safe.then(|| name.to_owned())
}

fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let file = fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| format!("The Bandcamp download is not a valid ZIP file: {error}"))?;
    let mut files = std::collections::HashSet::new();
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
        if !entry.is_dir()
            && !files.insert(
                entry
                    .enclosed_name()
                    .unwrap_or_else(|| PathBuf::from(&name)),
            )
        {
            return Err(
                format!("The Bandcamp ZIP file contains the same file twice: {name}").into(),
            );
        }
    }
    zip.extract(destination)
        .map_err(|error| format!("The Bandcamp ZIP file did not extract: {error}"))?;
    open_folders(destination)
        .map_err(|error| format!("The Bandcamp ZIP file did not extract: {error}").into())
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
    let mut files = walkdir::WalkDir::new(directory)
        .min_depth(1)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| muzik_core::audio::is_audio(entry.path()) && !entry.file_type().is_dir())
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
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Disposition: attachment; filename=\"Band - Album.zip\"\r\nConnection: close\r\n\r\n",
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
            "identity=secret",
            &partial,
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
}
