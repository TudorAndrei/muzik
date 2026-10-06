//! Spotify settings, saved token, and API client shared by the two apps.

use chrono::{DateTime, TimeDelta, Utc};
use muzik_core::app_config;
use rspotify::clients::OAuthClient;
use rspotify::prelude::Id;
use rspotify::{
    AuthCodePkceSpotify, CallbackError, ClientError, Config, Credentials, OAuth, Token,
    TokenCallback,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::Arc;

mod login;
pub use login::login;
mod api;
pub use api::{list_playlists, PlaylistRef};
mod reader;
pub use reader::load_playlist_document;

const SCOPES: [&str; 3] = [
    "playlist-read-private",
    "playlist-read-collaborative",
    "user-library-read",
];

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

pub fn set_client_id(path: &Path, client_id: &str) -> Result<String, String> {
    let value = client_id.trim();
    app_config::save_section_string(path, "spotify", "client_id", value)?;
    Ok(value.to_owned())
}

pub fn clear_tokens(path: &Path) -> Result<bool, String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
    }
}

pub fn settings(path: &Path) -> Result<Settings, String> {
    let config = app_config::load(path).unwrap_or_else(|_| json!({}));
    let saved = &config["spotify"];
    let value = |environment: &str, key: &str, fallback: &str| {
        std::env::var(environment)
            .ok()
            .filter(|text| !text.trim().is_empty())
            .or_else(|| {
                saved[key]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| saved[key].as_i64().map(|number| number.to_string()))
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

fn save_token(path: &Path, token: &Token) -> Result<(), String> {
    let mut token = token.clone();
    if token.refresh_token.is_none() {
        token.refresh_token = load_token(path).and_then(|saved| saved.refresh_token);
    }
    let parent = path.parent().ok_or("token path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut file = tempfile::Builder::new()
        .prefix(".spotify-token.json.")
        .tempfile_in(parent)
        .map_err(|error| error.to_string())?;
    serde_json::to_writer_pretty(&mut file, &token).map_err(|error| error.to_string())?;
    use std::io::Write;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    file.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

fn client(settings: &Settings, token_path: &Path) -> AuthCodePkceSpotify {
    let path = token_path.to_path_buf();
    let save =
        move |token: Token| save_token(&path, &token).map_err(CallbackError::CustomizedError);
    AuthCodePkceSpotify::with_config(
        Credentials::new_pkce(&settings.client_id),
        OAuth {
            redirect_uri: settings.redirect_uri(),
            scopes: SCOPES.iter().map(|scope| (*scope).to_owned()).collect(),
            ..OAuth::default()
        },
        Config {
            token_refreshing: true,
            token_callback_fn: Arc::new(Some(TokenCallback(Box::new(save)))),
            ..Config::default()
        },
    )
}

fn connected(config_path: &Path, token_path: &Path) -> Result<AuthCodePkceSpotify, String> {
    let token = load_token(token_path)
        .ok_or("muzik is not connected to Spotify. Run 'muzik spotify login'.")?;
    let client = client(&settings(config_path)?, token_path);
    *client
        .token
        .lock()
        .map_err(|_| "the Spotify token is not available")? = Some(token);
    Ok(client)
}

fn failed(error: ClientError) -> String {
    format!("Spotify request failed: {error}")
}

pub fn status(config_path: &Path, token_path: &Path) -> Result<Value, String> {
    let settings = settings(config_path)?;
    let mut result = json!({
        "client_id": settings.client_id,
        "redirect_uri": settings.redirect_uri(),
        "connected": false,
    });
    if load_token(token_path).is_some() {
        match connected(config_path, token_path).and_then(|client| account_name(&client)) {
            Ok(name) => {
                result["connected"] = json!(true);
                result["account_name"] = json!(name);
            }
            Err(error) => result["error"] = json!(error),
        }
    }
    Ok(result)
}

fn account_name(client: &AuthCodePkceSpotify) -> Result<String, String> {
    let user = client.current_user().map_err(failed)?;
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
    use super::{clear_tokens, load_token, save_token, status};
    use std::fs;

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

        let refreshed = rspotify::Token {
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
        let result = status(&config, &dir.path().join("spotify-token.json"))
            .map_err(std::io::Error::other)?;
        assert_eq!(result["connected"], false);
        assert!(result["redirect_uri"]
            .as_str()
            .is_some_and(|value| value.ends_with("/callback")));
        Ok(())
    }
}
