//! Saved login and browser cookie input.

use crate::{PAGE_LIMIT, Result, USER_AGENT};
use muzik_core::paths::Paths;
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
}

fn parse_cookies(text: &str) -> Result<Vec<Cookie>> {
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
    let list = match value {
        Value::Array(list) => list,
        Value::Object(mut map) => match map.remove("cookies") {
            Some(Value::Array(list)) => list,
            _ => Vec::new(),
        },
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
        let _ = writeln!(
            text,
            ".bandcamp.com\tTRUE\t/\tTRUE\t0\t{}\t{}",
            cookie.name, cookie.value
        );
    }
    text
}

#[derive(Clone, Debug)]
pub struct Login {
    pub user: String,
    cookies: Vec<Cookie>,
}

impl Login {
    #[must_use]
    pub fn load(paths: &Paths) -> Option<Self> {
        Self::load_from(&paths.bandcamp_user(), &paths.bandcamp_cookies())
    }

    fn load_from(user_file: &Path, cookie_file: &Path) -> Option<Self> {
        let user = fs::read_to_string(user_file).ok()?.trim().to_owned();
        let cookies = parse_cookies(&fs::read_to_string(cookie_file).ok()?).ok()?;
        (!user.is_empty()).then_some(Self { user, cookies })
    }

    /// # Errors
    /// Returns an error when the cookies or user name are not valid, or when the files cannot be written.
    pub fn save(paths: &Paths, user: Option<&str>, cookie_text: &str) -> Result<Self> {
        Self::save_to(
            &paths.bandcamp_user(),
            &paths.bandcamp_cookies(),
            user,
            cookie_text,
        )
    }

    fn save_to(
        user_file: &Path,
        cookie_file: &Path,
        user: Option<&str>,
        cookie_text: &str,
    ) -> Result<Self> {
        let cookies = if cookie_text.trim().is_empty() {
            parse_cookies(
                &fs::read_to_string(cookie_file).map_err(|_| "Paste your Bandcamp cookies.")?,
            )?
        } else {
            parse_cookies(cookie_text)?
        };
        let user = match user
            .map(|user| user.trim().trim_start_matches('@'))
            .filter(|user| !user.is_empty())
        {
            Some(user) => user.to_owned(),
            None => Self::account_user(&cookies)?,
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

    fn account_user(cookies: &[Cookie]) -> Result<String> {
        let summary: Value = request("https://bandcamp.com/api/fan/2/collection_summary", cookies)?
            .body_mut()
            .read_json()
            .map_err(|error| {
                format!("Bandcamp sent an account summary that is not valid: {error}")
            })?;
        summary
            .pointer("/collection_summary/username")
            .and_then(Value::as_str)
            .filter(|user| !user.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "Bandcamp did not accept the cookies. Log in to Bandcamp in the browser, then copy the identity cookie again.".into())
    }

    /// # Errors
    /// Returns an error when a login file exists but cannot be removed.
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

    pub(super) fn header(&self) -> String {
        cookie_header(&self.cookies)
    }

    pub(super) fn page_blob(&self, url: &str) -> Result<Value> {
        let html = request(url, &self.cookies)?
            .body_mut()
            .with_config()
            .limit(PAGE_LIMIT)
            .read_to_string()
            .map_err(|error| format!("Bandcamp sent a page that is not valid: {error}"))?;
        page_blob(&html).ok_or_else(|| format!("Bandcamp sent no page data for {url}.").into())
    }
}

fn cookie_header(cookies: &[Cookie]) -> String {
    cookies
        .iter()
        .map(|cookie| format!("{}={}", cookie.name, cookie.value))
        .collect::<Vec<_>>()
        .join("; ")
}

fn request(url: &str, cookies: &[Cookie]) -> Result<ureq::http::Response<ureq::Body>> {
    ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .header("Cookie", cookie_header(cookies))
        .config()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .call()
        .map_err(|error| format!("Bandcamp did not answer {url}: {error}").into())
}

#[must_use]
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
    let tag_end = start.checked_add(html.get(start..)?.find('>')?)?;
    let tag = html.get(..tag_end)?;
    let tag = tag.get(tag.rfind('<')?..)?;
    let blob = tag.split_once("data-blob=\"")?.1;
    let blob = blob.get(..blob.find('"')?)?;
    serde_json::from_str(&html_escape::decode_html_entities(blob)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cookies_parse_from_a_header_a_cookie_file_and_json() {
        let header =
            parse_cookies("Cookie: client_id=abc; identity=7%09token%7B; js_logged_in=1").unwrap();
        assert!(header.contains(&Cookie {
            name: "identity".into(),
            value: "7%09token%7B".into()
        }));
        let file = parse_cookies(
            "# Netscape HTTP Cookie File\n#HttpOnly_.bandcamp.com\tTRUE\t/\tTRUE\t0\tidentity\tsecret\n.other.com\tTRUE\t/\tTRUE\t0\tidentity\tnope\n",
        )
        .unwrap();
        assert_eq!(
            file,
            vec![Cookie {
                name: "identity".into(),
                value: "secret".into()
            }]
        );
        let json = parse_cookies(
            r#"[{"domain":".bandcamp.com","name":"identity","value":"secret"},{"domain":".other.com","name":"x","value":"y"}]"#,
        )
        .unwrap();
        assert_eq!(json, file);
        assert_eq!(
            parse_cookies("7%09token%2B%7B").unwrap(),
            vec![Cookie {
                name: "identity".into(),
                value: "7%09token%2B%7B".into()
            }]
        );
        assert!(parse_cookies("client_id=abc").is_err());
        assert!(parse_cookies("").is_err());
    }

    #[test]
    fn a_saved_login_loads_again_and_keeps_the_cookies_private() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("bandcamp_user");
        let cookies = directory.path().join("bandcamp_cookies.txt");
        assert!(Login::save_to(&user, &cookies, Some("bad name"), "identity=x").is_err());
        Login::save_to(
            &user,
            &cookies,
            Some("@listener"),
            "identity=secret; js_logged_in=1",
        )
        .unwrap();
        let login = Login::load_from(&user, &cookies).unwrap();
        assert_eq!(login.user, "listener");
        assert_eq!(login.header(), "identity=secret; js_logged_in=1");
        Login::save_to(&user, &cookies, Some("listener"), "").unwrap();
        assert!(Login::load_from(&user, &cookies).is_some());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&cookies).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
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
}
