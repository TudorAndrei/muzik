//! Spotify browser login with PKCE and one loopback callback.

use super::{account_name, client, settings, Error, Result};
use rspotify::clients::OAuthClient;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use url::Url;

const SUCCESS_PAGE: &str = "<html><body><h3>muzik is connected to Spotify.</h3><p>You can close this tab.</p></body></html>";

pub fn login(
    config_path: &Path,
    token_path: &Path,
    port_override: Option<u16>,
    cancel: &AtomicBool,
) -> Result<String> {
    let mut settings = settings(config_path)?;
    if let Some(port) = port_override {
        settings.redirect_port = port;
    }
    if settings.client_id.is_empty() {
        return Err(
            "No Spotify client ID is configured. Run 'muzik spotify set-client-id <id>'.".into(),
        );
    }
    let listener = TcpListener::bind(("127.0.0.1", settings.redirect_port)).map_err(|error| {
        format!(
            "Unable to listen on 127.0.0.1:{} for the Spotify redirect: {error}",
            settings.redirect_port
        )
    })?;
    listener.set_nonblocking(true)?;
    let mut spotify = client(&settings, token_path);
    let url = spotify.get_authorize_url(Some(64))?;
    open::that(url.as_str())
        .map_err(|error| format!("Unable to open Spotify login in the browser: {error}"))?;
    let code = wait_for_code(
        &listener,
        &spotify.oauth.state,
        cancel,
        Duration::from_secs(300),
    )?;
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    spotify.request_token(&code)?;
    account_name(&spotify)
}

fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_nonblocking(false)?;
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let mut line = String::new();
                BufReader::new(stream.try_clone()?.take(8192)).read_line(&mut line)?;
                let target = line.split_whitespace().nth(1).unwrap_or("");
                if !line.starts_with("GET ") || !target.starts_with("/callback?") {
                    answer(&mut stream, "404 Not Found", "Not found")?;
                    continue;
                }
                let callback = Url::parse(&format!("http://127.0.0.1{target}"))
                    .map_err(|error| format!("Invalid Spotify callback: {error}"))?;
                let result = callback_result(&callback, state);
                match result {
                    Ok(code) => {
                        answer(&mut stream, "200 OK", SUCCESS_PAGE)?;
                        return Ok(code);
                    }
                    Err(error) => {
                        answer(&mut stream, "400 Bad Request", &error.to_string())?;
                        return Err(error);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(format!("Spotify callback failed: {error}").into()),
        }
    }
    Err(
        "No Spotify answer was received. Check the redirect URI in your Spotify application."
            .into(),
    )
}

fn callback_result(url: &Url, expected_state: &str) -> Result<String> {
    let fields = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    if fields.get("state").map(|value| value.as_ref()) != Some(expected_state) {
        return Err("Spotify returned the wrong login state".into());
    }
    if let Some(error) = fields.get("error") {
        return Err(format!("Spotify refused the login: {error}").into());
    }
    fields
        .get("code")
        .filter(|code| !code.is_empty())
        .map(|code| code.to_string())
        .ok_or_else(|| "Spotify sent no authorization code".into())
}

fn answer(stream: &mut TcpStream, status: &str, body: &str) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    Ok(stream.write_all(response.as_bytes())?)
}

#[cfg(test)]
mod tests {
    use super::{callback_result, client, wait_for_code};
    use crate::Settings;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use url::Url;

    #[test]
    fn authorize_url_has_the_expected_pkce_fields() -> Result<(), Box<dyn std::error::Error>> {
        let settings = Settings {
            client_id: "my-client".into(),
            redirect_port: 8888,
        };
        let mut spotify = client(&settings, Path::new("token.json"));
        let url = Url::parse(&spotify.get_authorize_url(Some(64))?)?;
        let params = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        let param = |key: &str| params.get(key).map(|value| value.to_string());
        assert_eq!(param("response_type").as_deref(), Some("code"));
        assert_eq!(param("code_challenge_method").as_deref(), Some("S256"));
        assert_eq!(
            param("redirect_uri").as_deref(),
            Some("http://127.0.0.1:8888/callback")
        );
        assert_eq!(param("state"), Some(spotify.oauth.state.clone()));
        let scopes = param("scope").unwrap_or_default();
        assert!(scopes.contains("user-library-read"));
        assert!(scopes.contains("playlist-read-private"));
        Ok(())
    }

    #[test]
    fn callback_accepts_only_the_current_login_state() -> Result<(), Box<dyn std::error::Error>> {
        let good = Url::parse("http://127.0.0.1/callback?code=answer&state=expected")?;
        assert_eq!(callback_result(&good, "expected")?, "answer");
        assert!(callback_result(&good, "other").is_err());
        Ok(())
    }

    #[test]
    fn loopback_callback_returns_the_code_and_an_http_answer(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let browser = std::thread::spawn(move || -> std::io::Result<String> {
            let mut stream = TcpStream::connect(address)?;
            stream.write_all(
                b"GET /callback?code=answer&state=expected HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            )?;
            let mut response = String::new();
            stream.read_to_string(&mut response)?;
            Ok(response)
        });
        let code = wait_for_code(
            &listener,
            "expected",
            &AtomicBool::new(false),
            Duration::from_secs(2),
        )?;
        assert_eq!(code, "answer");
        let response = browser.join().map_err(|_| "browser thread failed")??;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        Ok(())
    }
}
