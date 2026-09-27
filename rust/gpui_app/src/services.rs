//! Service checks used by the native Settings page.

use muzik_core::app_config;
use muzik_soulseek::session::{Session, SessionSettings};
use serde::Serialize;
use serde_json::Value;
use std::env;
use std::process::Command;

#[derive(Debug, Serialize)]
pub struct ServiceStatus {
    name: &'static str,
    available: Option<bool>,
    detail: String,
    optional: bool,
}

pub fn check() -> Vec<ServiceStatus> {
    vec![
        check_binary("ffmpeg", "ffmpeg", &["-version"], false),
        check_binary("yt-dlp", "yt-dlp", &["--version"], false),
        check_binary("bandsnatch", "bandsnatch", &["--version"], true),
        check_soulseek(),
    ]
}

fn check_binary(
    name: &'static str,
    executable: &str,
    args: &[&str],
    optional: bool,
) -> ServiceStatus {
    match Command::new(executable).args(args).output() {
        Ok(output) => {
            let text = if output.stdout.is_empty() {
                &output.stderr
            } else {
                &output.stdout
            };
            let first_line = String::from_utf8_lossy(text)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_owned();
            ServiceStatus {
                name,
                available: Some(output.status.success()),
                detail: if first_line.is_empty() {
                    executable.to_owned()
                } else {
                    first_line
                },
                optional,
            }
        }
        Err(error) => ServiceStatus {
            name,
            available: Some(false),
            detail: if error.kind() == std::io::ErrorKind::NotFound {
                format!("Not found on PATH (install {executable}).")
            } else {
                format!("Cannot run {executable}: {error}")
            },
            optional,
        },
    }
}

fn check_soulseek() -> ServiceStatus {
    let config = app_config::load(&app_config::path()).unwrap_or(Value::Null);
    let username = setting(&config, "MUZIK_SOULSEEK_USERNAME", "username").unwrap_or_default();
    let password = setting(&config, "MUZIK_SOULSEEK_PASSWORD", "password").unwrap_or_default();
    if username.is_empty() || password.is_empty() {
        return ServiceStatus {
            name: "Soulseek",
            available: None,
            detail: "Not configured (set Soulseek username and password).".into(),
            optional: true,
        };
    }
    let host = setting(&config, "MUZIK_SOULSEEK_SERVER_HOST", "server_host")
        .unwrap_or_else(|| "server.slsknet.org".into());
    let port = setting(&config, "MUZIK_SOULSEEK_SERVER_PORT", "server_port")
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(2416);
    let settings = SessionSettings {
        username,
        password,
        server_host: Some(host.clone()),
        server_port: Some(port),
        enable_listen: None,
        listen_port: None,
    };
    match Session::connect(settings) {
        Ok(session) => {
            session.close();
            ServiceStatus {
                name: "Soulseek",
                available: Some(true),
                detail: format!("Connected: {host}:{port}"),
                optional: true,
            }
        }
        Err(error) => ServiceStatus {
            name: "Soulseek",
            available: Some(false),
            detail: format!("Unreachable: {error}"),
            optional: true,
        },
    }
}

fn setting(config: &Value, environment: &str, key: &str) -> Option<String> {
    env::var(environment)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            config
                .get("soulseek")
                .and_then(|settings| settings.get(key))
                .and_then(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| value.as_u64().map(|number| number.to_string()))
                })
                .filter(|value| !value.trim().is_empty())
        })
}

#[cfg(test)]
mod tests {
    use super::check_binary;

    #[test]
    fn missing_command_is_unavailable() {
        let status = check_binary(
            "missing",
            "muzik-command-that-does-not-exist-2026",
            &["--version"],
            true,
        );
        assert_eq!(status.available, Some(false));
        assert!(status.optional);
        assert!(status.detail.contains("Not found"));
    }
}
