//! Spotify settings, saved token, and API client shared by the two apps.

use chrono::{DateTime, TimeDelta, Utc};
use muzik_core::app_config;
use rspotify_model::{Id, Page, PrivateUser, Token};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use url::Url;

mod login;
pub use login::login;
mod api;
pub use api::{PlaylistRef, list_playlists};
mod reader;
pub use reader::load_playlist_document;

const API: &str = "https://api.spotify.com/v1";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Persist(#[from] tempfile::PersistError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Config(#[from] muzik_core::Error),
    #[error(transparent)]
    Id(#[from] rspotify_model::IdError),
    #[error("cancelled")]
    Cancelled,
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

#[derive(Clone, Debug)]
pub struct Settings {
    pub client_id: String,
    pub redirect_port: u16,
}

impl Settings {
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.redirect_port)
    }
}

pub fn set_client_id(path: &Path, client_id: &str) -> Result<String> {
    let value = client_id.trim();
    app_config::save_section_string(path, "spotify", "client_id", value)?;
    Ok(value.to_owned())
}

pub fn clear_tokens(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot remove {}: {error}", path.display()).into()),
    }
}

pub fn settings(path: &Path) -> Result<Settings> {
    let config = app_config::load(path).unwrap_or_else(|_| json!({}));
    let saved = config.get("spotify");
    let value = |environment: &str, key: &str, fallback: &str| {
        std::env::var(environment)
            .ok()
            .filter(|text| !text.trim().is_empty())
            .or_else(|| {
                let saved = saved.and_then(|saved| saved.get(key))?;
                saved
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| saved.as_i64().map(|number| number.to_string()))
                    .filter(|text| !text.trim().is_empty())
            })
            .unwrap_or_else(|| fallback.to_owned())
            .trim()
            .to_owned()
    };
    let port = value("MUZIK_SPOTIFY_REDIRECT_PORT", "redirect_port", "8888")
        .parse::<u16>()
        .map_err(|error| format!("invalid Spotify redirect port: {error}"))?;
    if port == 0 {
        return Err("Spotify redirect port must be from 1 to 65535".into());
    }
    Ok(Settings {
        client_id: value("MUZIK_SPOTIFY_CLIENT_ID", "client_id", ""),
        redirect_port: port,
    })
}

#[derive(Deserialize)]
struct LegacyToken {
    access_token: String,
    refresh_token: String,
    expires_at: f64,
    #[serde(default)]
    scope: String,
}

impl LegacyToken {
    fn upgrade(self) -> Option<Token> {
        let since_epoch = std::time::Duration::try_from_secs_f64(self.expires_at).ok()?;
        Some(Token {
            access_token: self.access_token,
            expires_at: DateTime::UNIX_EPOCH
                .checked_add_signed(TimeDelta::from_std(since_epoch).ok()?),
            refresh_token: Some(self.refresh_token),
            scopes: self.scope.split_whitespace().map(str::to_owned).collect(),
            ..Token::default()
        })
    }
}

fn load_token(path: &Path) -> Option<Token> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice::<Token>(&bytes)
        .ok()
        .or_else(|| {
            serde_json::from_slice::<LegacyToken>(&bytes)
                .ok()?
                .upgrade()
        })
        .filter(|token| {
            !token.access_token.is_empty()
                && token
                    .refresh_token
                    .as_deref()
                    .is_some_and(|refresh| !refresh.is_empty())
        })
}

fn save_token(path: &Path, token: &Token) -> Result<()> {
    let mut token = token.clone();
    if token.refresh_token.is_none() {
        token.refresh_token = load_token(path).and_then(|saved| saved.refresh_token);
    }
    let parent = path.parent().ok_or("token path has no parent")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::Builder::new()
        .prefix(".spotify-token.json.")
        .tempfile_in(parent)?;
    serde_json::to_writer_pretty(&mut file, &token)?;
    use std::io::Write;
    file.write_all(b"\n")?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

fn request_token(form: &[(&str, &str)]) -> Result<Token> {
    let mut response = ureq::post(TOKEN_URL)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .send_form(form.iter().copied())
        .map_err(|error| format!("Unable to reach Spotify: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Spotify rejected the token request ({})", response.status()).into());
    }
    let mut token: Token = response
        .body_mut()
        .read_json()
        .map_err(|error| format!("Spotify returned an invalid token response: {error}"))?;
    token.expires_at = Utc::now().checked_add_signed(token.expires_in);
    Ok(token)
}

struct Client {
    client_id: String,
    token_path: PathBuf,
    token: Token,
}

impl Client {
    fn connect(config_path: &Path, token_path: &Path) -> Result<Self> {
        let token = load_token(token_path)
            .ok_or("muzik is not connected to Spotify. Run 'muzik spotify login'.")?;
        Ok(Self {
            client_id: settings(config_path)?.client_id,
            token_path: token_path.to_path_buf(),
            token,
        })
    }

    fn get<T: DeserializeOwned>(&mut self, path: &str) -> Result<T> {
        let url = if path.starts_with("https://") {
            let parsed =
                Url::parse(path).map_err(|error| format!("invalid Spotify URL: {error}"))?;
            if parsed.host_str() != Some("api.spotify.com") {
                return Err("Spotify returned a page outside its API".into());
            }
            path.to_owned()
        } else {
            format!("{API}/{path}")
        };
        if self.token.is_expired() {
            self.refresh()?;
        }
        let mut refreshed = false;
        for attempt in 0..3 {
            let mut response = ureq::get(&url)
                .header(
                    "Authorization",
                    format!("Bearer {}", self.token.access_token),
                )
                .config()
                .timeout_global(Some(Duration::from_secs(30)))
                .http_status_as_error(false)
                .build()
                .call()
                .map_err(|error| format!("Unable to reach Spotify: {error}"))?;
            match response.status().as_u16() {
                401 if !refreshed => {
                    self.refresh()?;
                    refreshed = true;
                }
                429 if attempt < 2 => {
                    let seconds = response
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(1)
                        .min(30);
                    std::thread::sleep(Duration::from_secs(seconds));
                }
                status if !(200..300).contains(&status) => {
                    return Err(
                        format!("Spotify rejected the request ({})", response.status()).into(),
                    );
                }
                _ => {
                    return Ok(response.body_mut().read_json().map_err(|error| {
                        format!("Spotify returned an invalid response: {error}")
                    })?);
                }
            }
        }
        Err("Spotify did not accept the refreshed token".into())
    }

    fn pages<T: DeserializeOwned>(&mut self, path: &str) -> Result<Vec<T>> {
        let mut page: Page<T> = self.get(path)?;
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        loop {
            items.append(&mut page.items);
            let Some(next) = page.next.take() else {
                return Ok(items);
            };
            if !seen.insert(next.clone()) {
                return Err("Spotify returned a repeated page".into());
            }
            page = self.get(&next)?;
        }
    }

    fn refresh(&mut self) -> Result<()> {
        if self.client_id.is_empty() {
            return Err("No Spotify client ID is configured".into());
        }
        let refresh = self
            .token
            .refresh_token
            .clone()
            .ok_or("Spotify returned no refresh token")?;
        let mut token = request_token(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", &refresh),
            ("client_id", &self.client_id),
        ])?;
        token.refresh_token.get_or_insert(refresh);
        save_token(&self.token_path, &token)?;
        self.token = token;
        Ok(())
    }
}

pub fn status(config_path: &Path, token_path: &Path) -> Result<Value> {
    let settings = settings(config_path)?;
    let mut result = serde_json::Map::new();
    result.insert("client_id".into(), json!(settings.client_id));
    result.insert("redirect_uri".into(), json!(settings.redirect_uri()));
    result.insert("connected".into(), json!(false));
    if load_token(token_path).is_some() {
        match Client::connect(config_path, token_path)
            .and_then(|mut client| account_name(&mut client))
        {
            Ok(name) => {
                result.insert("connected".into(), json!(true));
                result.insert("account_name".into(), json!(name));
            }
            Err(error) => {
                result.insert("error".into(), json!(error.to_string()));
            }
        }
    }
    Ok(Value::Object(result))
}

fn account_name(client: &mut Client) -> Result<String> {
    let user: PrivateUser = client.get("me")?;
    Ok(user
        .display_name
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| user.id.id().to_owned()))
}

fn utc(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::{Client, clear_tokens, load_token, save_token, status};
    use std::fs;

    #[test]
    fn a_page_outside_the_spotify_api_never_gets_the_token() {
        let mut client = Client {
            client_id: "client".into(),
            token_path: "token.json".into(),
            token: rspotify_model::Token::default(),
        };
        let page = client.get::<serde_json::Value>("https://example.test/steal");
        assert!(page.is_err_and(|error| error.to_string().contains("outside its API")));
    }

    #[test]
    fn logout_removes_existing_tokens() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("spotify-token.json");
        fs::write(&path, "saved tokens")?;
        assert!(clear_tokens(&path).map_err(std::io::Error::other)?);
        assert!(!clear_tokens(&path).map_err(std::io::Error::other)?);
        Ok(())
    }

    #[test]
    fn a_saved_token_of_the_earlier_format_still_loads() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("spotify-token.json");
        fs::write(
            &path,
            r#"{"access_token":"access","refresh_token":"refresh","expires_at":1800000000.0,"scope":"user-library-read"}"#,
        )?;
        let token = load_token(&path).ok_or("saved token was not read")?;
        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token.as_deref(), Some("refresh"));
        assert_eq!(
            token.expires_at.map(|time| time.timestamp()),
            Some(1_800_000_000)
        );
        assert!(token.scopes.contains("user-library-read"));

        let refreshed = rspotify_model::Token {
            access_token: "new".into(),
            refresh_token: None,
            ..token
        };
        save_token(&path, &refreshed).map_err(std::io::Error::other)?;
        let saved = load_token(&path).ok_or("saved token was not read")?;
        assert_eq!(saved.access_token, "new");
        assert_eq!(saved.refresh_token.as_deref(), Some("refresh"));
        Ok(())
    }

    #[test]
    fn status_uses_the_saved_config_when_no_token_exists() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join("config.yaml");
        fs::write(
            &config,
            "spotify:\n  client_id: saved\n  redirect_port: '9123'\n",
        )?;
        let result = status(&config, &dir.path().join("spotify-token.json"))?;
        assert_eq!(result["connected"], false);
        assert!(
            result["redirect_uri"]
                .as_str()
                .is_some_and(|value| value.ends_with("/callback"))
        );
        Ok(())
    }
}
