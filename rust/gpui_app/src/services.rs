//! Service checks used by the native Settings page.

use muzik_core::app_config;
use muzik_soulseek::session::{Session, SessionSettings};
use serde::Serialize;
use serde_json::Value;
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
    let Some(settings) = SessionSettings::configured(&config) else {
        return ServiceStatus {
            name: "Soulseek",
            available: None,
            detail: "Not configured (set Soulseek username and password).".into(),
            optional: true,
        };
    };
    let host = settings
        .server_host
        .clone()
        .unwrap_or_else(|| "server.slsknet.org".into());
    let port = settings.server_port.unwrap_or(2416);
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
