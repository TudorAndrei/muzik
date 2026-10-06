//! Bandcamp collection access with cookies exported from a browser.

use muzik_core::paths::Paths;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantNames};

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:128.0) Gecko/20100101 Firefox/128.0";
const PAGE_LIMIT: u64 = 64 * 1024 * 1024;
const REQUEST_BUDGET: Duration = Duration::from_secs(120);
const DOWNLOAD_ATTEMPTS: u32 = 5;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}

/// Download formats that Bandcamp offers for a purchase.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    usage::ValueEnum,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantNames,
)]
pub enum BandcampFormat {
    #[default]
    #[serde(rename = "flac")]
    #[strum(to_string = "flac")]
    #[usage(name = "flac")]
    Flac,
    #[serde(rename = "wav")]
    #[strum(to_string = "wav")]
    #[usage(name = "wav")]
    Wav,
    #[serde(rename = "aac-hi")]
    #[strum(to_string = "aac-hi")]
    #[usage(name = "aac-hi")]
    AacHi,
    #[serde(rename = "mp3-320")]
    #[strum(to_string = "mp3-320")]
    #[usage(name = "mp3-320")]
    Mp3_320,
    #[serde(rename = "aiff-lossless")]
    #[strum(to_string = "aiff-lossless")]
    #[usage(name = "aiff-lossless")]
    AiffLossless,
    #[serde(rename = "vorbis")]
    #[strum(to_string = "vorbis")]
    #[usage(name = "vorbis")]
    Vorbis,
    #[serde(rename = "mp3-v0")]
    #[strum(to_string = "mp3-v0")]
    #[usage(name = "mp3-v0")]
    Mp3V0,
    #[serde(rename = "alac")]
    #[strum(to_string = "alac")]
    #[usage(name = "alac")]
    Alac,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
}

pub fn parse_cookies(text: &str) -> Result<Vec<Cookie>> {
    let text = text.trim();
    let mut cookies = if text.starts_with('[') || text.starts_with('{') {
        json_cookies(text)?
    } else if text.lines().any(|line| line.split('\t').count() >= 7) {
        netscape_cookies(text)
    } else if !text.is_empty() && !text.contains(['=', ';']) && !text.contains(char::is_whitespace)
    {
        vec![Cookie {
            name: "identity".into(),
            value: text.to_owned(),
        }]
    } else {
        header_cookies(text)
    };
    cookies.retain(|cookie| !cookie.name.is_empty());
    if !cookies.iter().any(|cookie| cookie.name == "identity") {
        return Err("The cookies have no Bandcamp login. Log in to Bandcamp in the browser, then copy the cookies again.".into());
    }
    Ok(cookies)
}

fn json_cookies(text: &str) -> Result<Vec<Cookie>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| format!("The cookie JSON is not valid: {error}"))?;
    let list = match &value {
        Value::Array(list) => list.clone(),
        Value::Object(map) => map
            .get("cookies")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    Ok(list
        .iter()
        .filter(|cookie| {
            ["domain", "Host raw", "host"]
                .iter()
                .find_map(|key| cookie[key].as_str())
                .is_none_or(|domain| domain.contains("bandcamp.com"))
        })
        .filter_map(|cookie| {
            let name = ["name", "Name raw"]
                .iter()
                .find_map(|key| cookie[key].as_str())?;
            let value = ["value", "Content raw"]
                .iter()
                .find_map(|key| cookie[key].as_str())?;
            Some(Cookie {
                name: name.to_owned(),
                value: value.to_owned(),
            })
        })
        .collect())
}

fn netscape_cookies(text: &str) -> Vec<Cookie> {
    text.lines()
        .map(|line| line.strip_prefix("#HttpOnly_").unwrap_or(line))
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let [domain, _, _, _, _, name, value] = fields.get(..7)? else {
                return None;
            };
            domain.contains("bandcamp.com").then(|| Cookie {
                name: name.trim().to_owned(),
                value: value.trim().to_owned(),
            })
        })
        .collect()
}

fn header_cookies(text: &str) -> Vec<Cookie> {
    let text = text
        .strip_prefix("Cookie:")
        .or_else(|| text.strip_prefix("cookie:"))
        .unwrap_or(text);
    text.split(';')
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            Some(Cookie {
                name: name.trim().to_owned(),
                value: value.trim().to_owned(),
            })
        })
        .collect()
}

fn netscape(cookies: &[Cookie]) -> String {
    let mut text = String::from("# Netscape HTTP Cookie File\n");
    for cookie in cookies {
        text.push_str(&format!(
            ".bandcamp.com\tTRUE\t/\tTRUE\t0\t{}\t{}\n",
            cookie.name, cookie.value
        ));
    }
    text
}

#[derive(Clone, Debug)]
pub struct Login {
    pub user: String,
    cookies: Vec<Cookie>,
}

impl Login {
    pub fn load(paths: &Paths) -> Option<Self> {
        Self::load_from(&paths.bandcamp_user(), &paths.bandcamp_cookies())
    }

    pub fn load_from(user_file: &Path, cookie_file: &Path) -> Option<Self> {
        let user = fs::read_to_string(user_file).ok()?.trim().to_owned();
        let cookies = parse_cookies(&fs::read_to_string(cookie_file).ok()?).ok()?;
        (!user.is_empty()).then_some(Self { user, cookies })
    }

    pub fn save(paths: &Paths, user: &str, cookie_text: &str) -> Result<Self> {
        Self::save_to(
            &paths.bandcamp_user(),
            &paths.bandcamp_cookies(),
            user,
            cookie_text,
        )
    }

    pub fn save_to(
        user_file: &Path,
        cookie_file: &Path,
        user: &str,
        cookie_text: &str,
    ) -> Result<Self> {
        let cookies = if cookie_text.trim().is_empty() {
            parse_cookies(
                &fs::read_to_string(cookie_file).map_err(|_| "Paste your Bandcamp cookies.")?,
            )?
        } else {
            parse_cookies(cookie_text)?
        };
        let user = user.trim().trim_start_matches('@');
        let user = if user.is_empty() {
            Self {
                user: String::new(),
                cookies: cookies.clone(),
            }
            .account_user()?
        } else {
            user.to_owned()
        };
        if !user
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err("Enter your Bandcamp user name, as in bandcamp.com/<user name>.".into());
        }
        if let Some(parent) = cookie_file.parent() {
            fs::create_dir_all(parent)?;
        }
        write_private(cookie_file, &netscape(&cookies))?;
        write_private(user_file, &format!("{user}\n"))?;
        Ok(Self { user, cookies })
    }

    fn account_user(&self) -> Result<String> {
        let summary: Value = self
            .get("https://bandcamp.com/api/fan/2/collection_summary")?
            .body_mut()
            .read_json()
            .map_err(|error| {
                format!("Bandcamp sent an account summary that is not valid: {error}")
            })?;
        summary["collection_summary"]["username"]
            .as_str()
            .filter(|user| !user.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "Bandcamp did not accept the cookies. Log in to Bandcamp in the browser, then copy the identity cookie again.".into())
    }

    pub fn clear(paths: &Paths) -> Result<bool> {
        let mut removed = false;
        for path in [paths.bandcamp_cookies(), paths.bandcamp_user()] {
            match fs::remove_file(&path) {
                Ok(()) => removed = true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(removed)
    }

    fn header(&self) -> String {
        self.cookies
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn get(&self, url: &str) -> Result<ureq::http::Response<ureq::Body>> {
        ureq::get(url)
            .header("User-Agent", USER_AGENT)
            .header("Cookie", self.header())
            .config()
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .call()
            .map_err(|error| format!("Bandcamp did not answer {url}: {error}").into())
    }

    fn page_blob(&self, url: &str) -> Result<Value> {
        let html = self
            .get(url)?
            .body_mut()
            .with_config()
            .limit(PAGE_LIMIT)
            .read_to_string()
            .map_err(|error| format!("Bandcamp sent a page that is not valid: {error}"))?;
        page_blob(&html).ok_or_else(|| format!("Bandcamp sent no page data for {url}.").into())
    }
}

pub fn status(paths: &Paths) -> Value {
    match Login::load(paths) {
        Some(login) => json!({"logged_in": true, "user": login.user}),
        None => json!({
            "logged_in": false,
            "user": fs::read_to_string(paths.bandcamp_user()).map(|user| user.trim().to_owned()).unwrap_or_default(),
        }),
    }
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| format!("cannot write {}: {error}", path.display()).into())
}

fn page_blob(html: &str) -> Option<Value> {
    let start = html.find("id=\"pagedata\"")?;
    let tag_end = start + html[start..].find('>')?;
    let tag = &html[..tag_end];
    let tag = &tag[tag.rfind('<')?..];
    let blob = tag.split_once("data-blob=\"")?.1;
    let blob = &blob[..blob.find('"')?];
    serde_json::from_str(&html_escape::decode_html_entities(blob)).ok()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Purchase {
    pub key: String,
    pub artist: String,
    pub title: String,
    pub single: bool,
    pub download_page: String,
    pub item_url: Option<String>,
    pub art_url: Option<String>,
}

impl Purchase {
    pub fn label(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} - {}", self.artist, self.title)
        }
    }
}

pub fn collection(login: &Login) -> Result<Vec<Purchase>> {
    let blob = login.page_blob(&format!("https://bandcamp.com/{}", login.user))?;
    if blob["fan_data"]["is_own_page"] != true {
        return Err(format!(
            "Bandcamp did not accept the login for \"{}\". Check the user name, or copy the cookies again.",
            login.user
        )
        .into());
    }
    let fan_id = blob["fan_data"]["fan_id"].clone();
    let mut details: Vec<Value> = Vec::new();
    let cache = &blob["item_cache"]["collection"];
    let sequence = blob["collection_data"]["sequence"].as_array();
    match (sequence, cache.as_object()) {
        (Some(sequence), Some(cache)) => details.extend(
            sequence
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|key| cache.get(key).cloned()),
        ),
        (None, Some(cache)) => details.extend(cache.values().cloned()),
        _ => {}
    }
    let mut urls = blob["collection_data"]["redownload_urls"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut more = blob["collection_data"]["item_count"].as_u64().unwrap_or(0)
        > blob["collection_data"]["batch_size"].as_u64().unwrap_or(0);
    let mut token = blob["collection_data"]["last_token"].clone();
    while more {
        let mut response = ureq::post("https://bandcamp.com/api/fancollection/1/collection_items")
            .header("User-Agent", USER_AGENT)
            .header("Cookie", login.header())
            .config()
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .send_json(json!({"fan_id": fan_id, "older_than_token": token}))
            .map_err(|error| format!("Bandcamp did not send the next collection page: {error}"))?;
        let page: Value = response
            .body_mut()
            .with_config()
            .limit(PAGE_LIMIT)
            .read_json()
            .map_err(|error| {
                format!("Bandcamp sent a collection page that is not valid: {error}")
            })?;
        details.extend(page["items"].as_array().cloned().unwrap_or_default());
        if let Some(page_urls) = page["redownload_urls"].as_object() {
            urls.extend(page_urls.clone());
        }
        more = page["more_available"] == true;
        token = page["last_token"].clone();
    }
    Ok(purchases(&details, &urls))
}

fn purchases(details: &[Value], urls: &serde_json::Map<String, Value>) -> Vec<Purchase> {
    let mut seen = std::collections::HashSet::new();
    details
        .iter()
        .filter_map(|item| {
            let key = format!(
                "{}{}",
                item["sale_item_type"].as_str()?,
                item["sale_item_id"]
            );
            let download_page = urls.get(&key)?.as_str()?.to_owned();
            seen.insert(key.clone()).then(|| Purchase {
                artist: item["band_name"].as_str().unwrap_or("").to_owned(),
                title: item["item_title"]
                    .as_str()
                    .or_else(|| item["album_title"].as_str())
                    .unwrap_or(&key)
                    .to_owned(),
                single: item["tralbum_type"] == "t" || item["item_type"] == "track",
                download_page,
                item_url: item["item_url"].as_str().map(str::to_owned),
                art_url: item["item_art_id"]
                    .as_u64()
                    .map(|id| format!("https://f4.bcbits.com/img/a{id:010}_9.jpg")),
                key,
            })
        })
        .collect()
}

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
        let item = blob["digital_items"]
            .as_array()
            .and_then(|items| items.first())
            .ok_or("Bandcamp has no download for this purchase.")?;
        let downloads = item["downloads"]
            .as_object()
            .ok_or("Bandcamp has no download for this purchase.")?;
        downloads[format]["url"]
            .as_str()
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
    let mut failures = 0;
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
            failures + 1
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
        file.write_all(&buffer[..count])?;
        received += count as u64;
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
    let (range, total) = header.strip_prefix("bytes ")?.split_once('/')?;
    let start = range.split_once('-')?.0.trim().parse().ok()?;
    Some((start, total.trim().parse().ok()))
}

fn wait(duration: Duration, cancelled: &AtomicBool) {
    let end = Instant::now() + duration;
    while Instant::now() < end && !cancelled.load(Ordering::SeqCst) {
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
    zip::ZipArchive::new(file)
        .map_err(|error| format!("The Bandcamp download is not a valid ZIP file: {error}"))?
        .extract(destination)
        .map_err(|error| format!("The Bandcamp ZIP file did not extract: {error}").into())
}

pub fn audio_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
            if path.is_dir() {
                pending.push(path);
            } else if muzik_core::audio::is_audio(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::VariantNames;

    #[test]
    fn formats_round_trip_through_their_bandcamp_names() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            BandcampFormat::VARIANTS,
            [
                "flac",
                "wav",
                "aac-hi",
                "mp3-320",
                "aiff-lossless",
                "vorbis",
                "mp3-v0",
                "alac"
            ]
        );
        for &name in BandcampFormat::VARIANTS {
            let format: BandcampFormat = name.parse()?;
            assert_eq!(format.as_ref(), name);
            assert_eq!(format.to_string(), name);
            assert_eq!(serde_json::to_value(format)?, name);
        }
        assert_eq!(BandcampFormat::default(), BandcampFormat::Flac);
        Ok(())
    }

    #[test]
    fn cookies_parse_from_a_header_a_cookie_file_and_json() -> Result<()> {
        let header = parse_cookies("Cookie: client_id=abc; identity=7%09token%7B; js_logged_in=1")?;
        assert!(header.contains(&Cookie {
            name: "identity".into(),
            value: "7%09token%7B".into()
        }));
        let file = parse_cookies(
            "# Netscape HTTP Cookie File\n#HttpOnly_.bandcamp.com\tTRUE\t/\tTRUE\t0\tidentity\tsecret\n.other.com\tTRUE\t/\tTRUE\t0\tidentity\tnope\n",
        )?;
        assert_eq!(
            file,
            vec![Cookie {
                name: "identity".into(),
                value: "secret".into()
            }]
        );
        let json = parse_cookies(
            r#"[{"domain":".bandcamp.com","name":"identity","value":"secret"},{"domain":".other.com","name":"x","value":"y"}]"#,
        )?;
        assert_eq!(json, file);
        assert_eq!(
            parse_cookies("7%09token%2B%7B")?,
            vec![Cookie {
                name: "identity".into(),
                value: "7%09token%2B%7B".into()
            }]
        );
        assert!(parse_cookies("client_id=abc").is_err());
        assert!(parse_cookies("").is_err());
        Ok(())
    }

    #[test]
    fn a_saved_login_loads_again_and_keeps_the_cookies_private(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let user = directory.path().join("bandcamp_user");
        let cookies = directory.path().join("bandcamp_cookies.txt");
        assert!(Login::save_to(&user, &cookies, "bad name", "identity=x").is_err());
        Login::save_to(
            &user,
            &cookies,
            "@listener",
            "identity=secret; js_logged_in=1",
        )?;
        let login = Login::load_from(&user, &cookies).ok_or("login did not load")?;
        assert_eq!(login.user, "listener");
        assert_eq!(login.header(), "identity=secret; js_logged_in=1");
        Login::save_to(&user, &cookies, "listener", "")?;
        assert!(Login::load_from(&user, &cookies).is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&cookies)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn page_data_reads_from_the_escaped_attribute() {
        let html = r#"<div id="pagedata" data-blob="{&quot;fan_data&quot;:{&quot;is_own_page&quot;:true,&quot;name&quot;:&quot;A &amp; B&quot;}}"></div>"#;
        let blob = page_blob(html);
        assert_eq!(
            blob,
            Some(json!({"fan_data": {"is_own_page": true, "name": "A & B"}}))
        );
    }

    #[test]
    fn purchases_follow_the_collection_order_and_need_a_download_page() {
        let details = vec![
            json!({"sale_item_type": "p", "sale_item_id": 2, "band_name": "Band", "item_title": "Album", "tralbum_type": "a", "item_art_id": 123}),
            json!({"sale_item_type": "p", "sale_item_id": 1, "band_name": "Band", "item_title": "Song", "tralbum_type": "t"}),
            json!({"sale_item_type": "p", "sale_item_id": 3, "band_name": "Band", "item_title": "Gift"}),
        ];
        let mut urls = serde_json::Map::new();
        urls.insert("p1".into(), json!("https://bandcamp.com/download?id=1"));
        urls.insert("p2".into(), json!("https://bandcamp.com/download?id=2"));
        let list = purchases(&details, &urls);
        assert_eq!(
            list.iter()
                .map(|item| item.key.as_str())
                .collect::<Vec<_>>(),
            ["p2", "p1"]
        );
        assert_eq!(list[0].label(), "Band - Album");
        assert!(!list[0].single);
        assert!(list[1].single);
        assert_eq!(
            list[0].art_url.as_deref(),
            Some("https://f4.bcbits.com/img/a0000000123_9.jpg")
        );
    }

    #[test]
    fn an_interrupted_download_continues_from_the_saved_bytes(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let body: Vec<u8> = (0..200_000u32).map(|index| (index % 251) as u8).collect();
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/album.zip", listener.local_addr()?);
        let served = body.clone();
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
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("range") {
                            range = Some(value.trim().to_owned());
                        }
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
                        served.len() - 1,
                        served.len()
                    )
                } else {
                    "200 OK".to_owned()
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Disposition: attachment; filename=\"Band - Album.zip\"\r\nConnection: close\r\n\r\n",
                    served.len() - start
                )?;
                let end = if cut { served.len() / 2 } else { served.len() };
                stream.write_all(&served[start..end])?;
                ranges.push(range);
            }
            Ok(ranges)
        });
        let directory = tempfile::tempdir()?;
        let partial = directory.path().join(".album.flac.part");
        let mut progress = Vec::new();
        let name = transfer(
            &mut || Ok(url.clone()),
            "identity=secret",
            &partial,
            &AtomicBool::new(false),
            &mut |received, total| progress.push((received, total)),
        )?;
        let ranges = server.join().map_err(|_| "server panicked")??;
        assert_eq!(name, "Band - Album.zip");
        assert_eq!(fs::read(&partial)?, body);
        assert_eq!(ranges, [None, Some(format!("bytes={}-", body.len() / 2))]);
        assert_eq!(
            progress.last(),
            Some(&(body.len() as u64, Some(body.len() as u64)))
        );
        Ok(())
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

    #[test]
    fn archive_entries_stay_inside_the_destination() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
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
        extract(&write("good.zip", &["Album/01 Song.flac"])?, &destination)?;
        assert_eq!(fs::read(destination.join("Album/01 Song.flac"))?, b"audio");

        let unsafe_archive = write("bad.zip", &["../escape.flac"])?;
        assert!(extract(&unsafe_archive, &destination).is_err());
        assert!(!directory.path().join("escape.flac").exists());
        Ok(())
    }
}
