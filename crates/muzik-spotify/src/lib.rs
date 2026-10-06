//! Spotify settings and saved token path shared by the two apps.

use muzik_core::app_config;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const PROFILE_URL: &str = "https://api.spotify.com/v1/me";

mod login;
pub use login::login;
mod api;
pub use api::{list_playlists, PlaylistRef};
mod reader;
pub use reader::load_playlist_document;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: f64,
    #[serde(default)]
    pub scope: String,
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

pub fn load_tokens(path: &Path) -> Option<Tokens> {
    let bytes = fs::read(path).ok()?;
    let tokens: Tokens = serde_json::from_slice(&bytes).ok()?;
    (!tokens.access_token.is_empty() && !tokens.refresh_token.is_empty()).then_some(tokens)
}

pub fn save_tokens(path: &Path, tokens: &Tokens) -> Result<(), String> {
    let parent = path.parent().ok_or("token path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut file = tempfile::Builder::new()
        .prefix(".spotify-token.json.")
        .tempfile_in(parent)
        .map_err(|error| error.to_string())?;
    serde_json::to_writer_pretty(&mut file, tokens).map_err(|error| error.to_string())?;
    use std::io::Write;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    file.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn status(config_path: &Path, token_path: &Path) -> Result<Value, String> {
    let settings = settings(config_path)?;
    let mut result = json!({
        "client_id": settings.client_id,
        "redirect_uri": settings.redirect_uri(),
        "connected": false,
    });
    if let Some(tokens) = load_tokens(token_path) {
        match account_name(&settings, token_path, tokens) {
            Ok(name) => {
                result["connected"] = json!(true);
                result["account_name"] = json!(name);
            }
            Err(error) => result["error"] = json!(error),
        }
    }
    Ok(result)
}

fn account_name(settings: &Settings, path: &Path, mut tokens: Tokens) -> Result<String, String> {
    let profile = get_json(settings, path, &mut tokens, PROFILE_URL)?;
    Ok(profile["display_name"]
        .as_str()
        .or_else(|| profile["id"].as_str())
        .unwrap_or("")
        .to_owned())
}

fn get_json(
    settings: &Settings,
    path: &Path,
    tokens: &mut Tokens,
    url: &str,
) -> Result<Value, String> {
    get_json_optional(settings, path, tokens, url)?.ok_or("Spotify resource was not found".into())
}

fn get_json_optional(
    settings: &Settings,
    path: &Path,
    tokens: &mut Tokens,
    url: &str,
) -> Result<Option<Value>, String> {
    if expired(tokens) {
        *tokens = refresh_tokens(settings, path, tokens)?;
    }
    let mut refreshed = false;
    for attempt in 0..3 {
        let mut response = ureq::get(url)
            .header("Authorization", format!("Bearer {}", tokens.access_token))
            .config()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .call()
            .map_err(|error| format!("Unable to reach Spotify: {error}"))?;
        if response.status().as_u16() == 401 && !refreshed {
            *tokens = refresh_tokens(settings, path, tokens)?;
            refreshed = true;
            continue;
        }
        if response.status().as_u16() == 429 && attempt < 2 {
            let seconds = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(1)
                .min(30);
            std::thread::sleep(Duration::from_secs(seconds));
            continue;
        }
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(format!(
                "Spotify rejected the request ({})",
                response.status()
            ));
        }
        let document: Value = response
            .body_mut()
            .read_json()
            .map_err(|error| format!("Spotify returned an invalid response: {error}"))?;
        if !document.is_object() {
            return Err("Spotify returned an invalid response".into());
        }
        return Ok(Some(document));
    }
    Err("Spotify did not accept the refreshed token".into())
}

fn refresh_tokens(settings: &Settings, path: &Path, old: &Tokens) -> Result<Tokens, String> {
    if settings.client_id.is_empty() {
        return Err("No Spotify client ID is configured".into());
    }
    let mut response = ureq::post(TOKEN_URL)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .send_form([
            ("grant_type", "refresh_token"),
            ("refresh_token", old.refresh_token.as_str()),
            ("client_id", settings.client_id.as_str()),
        ])
        .map_err(|error| format!("Unable to reach Spotify: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Spotify rejected the token refresh ({})",
            response.status()
        ));
    }
    let payload: Value = response
        .body_mut()
        .read_json()
        .map_err(|error| format!("Spotify returned an invalid token response: {error}"))?;
    let tokens = tokens_from_payload(&payload, &old.refresh_token)?;
    save_tokens(path, &tokens)?;
    Ok(tokens)
}

fn tokens_from_payload(payload: &Value, fallback_refresh: &str) -> Result<Tokens, String> {
    let access_token = payload["access_token"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("Spotify returned no access token")?;
    let refresh_token = payload["refresh_token"]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback_refresh);
    if refresh_token.is_empty() {
        return Err("Spotify returned no refresh token".into());
    }
    let lifetime = payload["expires_in"].as_f64().unwrap_or(3600.0);
    Ok(Tokens {
        access_token: access_token.to_owned(),
        refresh_token: refresh_token.to_owned(),
        expires_at: now() + lifetime,
        scope: payload["scope"].as_str().unwrap_or("").to_owned(),
    })
}

fn expired(tokens: &Tokens) -> bool {
    now() >= tokens.expires_at - 60.0
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::{clear_tokens, load_tokens, save_tokens, status, tokens_from_payload};
    use serde_json::json;
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
    fn reads_and_writes_existing_spotify_token_data() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("spotify-token.json");
        fs::write(
            &path,
            r#"{"access_token":"access","refresh_token":"refresh","expires_at":1800000000.0,"scope":"user-library-read"}"#,
        )?;
        let tokens = load_tokens(&path).ok_or("saved tokens were not read")?;
        assert_eq!(tokens.access_token, "access");
        assert_eq!(tokens.refresh_token, "refresh");
        save_tokens(&path, &tokens).map_err(std::io::Error::other)?;
        assert_eq!(
            load_tokens(&path)
                .ok_or("saved tokens were not read")?
                .scope,
            "user-library-read"
        );

        let refreshed = tokens_from_payload(
            &json!({"access_token":"new","expires_in":3600}),
            &tokens.refresh_token,
        )
        .map_err(std::io::Error::other)?;
        assert_eq!(refreshed.refresh_token, "refresh");
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
