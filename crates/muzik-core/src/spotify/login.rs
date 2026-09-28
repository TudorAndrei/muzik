//! Spotify browser login with PKCE and one loopback callback.

use super::{
    account_name, save_tokens, settings, tokens_from_payload, Settings, Tokens, TOKEN_URL,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use url::Url;

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const SUCCESS_PAGE: &str = "<html><body><h3>muzik is connected to Spotify.</h3><p>You can close this tab.</p></body></html>";
const SCOPES: &str = "playlist-read-private playlist-read-collaborative user-library-read";

pub fn login(
    config_path: &Path,
    token_path: &Path,
    port_override: Option<u16>,
    cancel: &AtomicBool,
) -> Result<String, String> {
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
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let verifier = random_url_token(64)?;
    let state = random_url_token(16)?;
    let url = authorize_url(&settings, &verifier, &state)?;
    open::that(url.as_str())
        .map_err(|error| format!("Unable to open Spotify login in the browser: {error}"))?;
    let code = wait_for_code(&listener, &state, cancel, Duration::from_secs(300))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    let mut response = ureq::post(TOKEN_URL)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .send_form([
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", settings.redirect_uri().as_str()),
            ("client_id", settings.client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .map_err(|error| format!("Unable to reach Spotify: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Spotify rejected the login ({})",
            response.status()
        ));
    }
    let payload: serde_json::Value = response
        .body_mut()
        .read_json()
        .map_err(|error| format!("Spotify returned an invalid token response: {error}"))?;
    let tokens: Tokens = tokens_from_payload(&payload, "")?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    save_tokens(token_path, &tokens)?;
    account_name(&settings, token_path, tokens)
}

fn random_url_token(length: usize) -> Result<String, String> {
    let mut bytes = vec![0_u8; length];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("Cannot create Spotify login secret: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn authorize_url(settings: &Settings, verifier: &str, state: &str) -> Result<Url, String> {
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(AUTHORIZE_URL).map_err(|error| error.to_string())?;
    url.query_pairs_mut()
        .append_pair("client_id", &settings.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &settings.redirect_uri())
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &challenge)
        .append_pair("state", state)
        .append_pair("scope", SCOPES);
    Ok(url)
}

fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String, String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .map_err(|error| error.to_string())?;
                let mut line = String::new();
                BufReader::new(
                    stream
                        .try_clone()
                        .map_err(|error| error.to_string())?
                        .take(8192),
                )
                .read_line(&mut line)
                .map_err(|error| error.to_string())?;
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
                        answer(&mut stream, "400 Bad Request", &error)?;
                        return Err(error);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(format!("Spotify callback failed: {error}")),
        }
    }
    Err(
        "No Spotify answer was received. Check the redirect URI in your Spotify application."
            .into(),
    )
}

fn callback_result(url: &Url, expected_state: &str) -> Result<String, String> {
    let fields = url
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    if fields.get("state").map(|value| value.as_ref()) != Some(expected_state) {
        return Err("Spotify returned the wrong login state".into());
    }
    if let Some(error) = fields.get("error") {
        return Err(format!("Spotify refused the login: {error}"));
    }
    fields
        .get("code")
        .filter(|code| !code.is_empty())
        .map(|code| code.to_string())
        .ok_or_else(|| "Spotify sent no authorization code".into())
}

fn answer(stream: &mut TcpStream, status: &str, body: &str) -> Result<(), String> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{authorize_url, callback_result, wait_for_code, Settings};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;
    use url::Url;

    #[test]
    fn authorize_url_has_the_expected_pkce_fields() -> Result<(), Box<dyn std::error::Error>> {
        let settings = Settings {
            client_id: "my-client".into(),
            redirect_port: 8888,
        };
        let url = authorize_url(&settings, "verifier", "state")?;
        let params = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            params.get("response_type").map(|value| value.as_ref()),
            Some("code")
        );
        assert_eq!(
            params
                .get("code_challenge_method")
                .map(|value| value.as_ref()),
            Some("S256")
        );
        assert_eq!(
            params.get("redirect_uri").map(|value| value.as_ref()),
            Some("http://127.0.0.1:8888/callback")
        );
        assert_eq!(
            params.get("state").map(|value| value.as_ref()),
            Some("state")
        );
        let standard = authorize_url(
            &settings,
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            "state",
        )?;
        let standard_params = standard
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            standard_params
                .get("code_challenge")
                .map(|value| value.as_ref()),
            Some("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        );
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
