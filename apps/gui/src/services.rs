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
                detail: version(&first_line).unwrap_or(first_line),
                optional,
            }
        }
        Err(error) => ServiceStatus {
            name,
            available: Some(false),
            detail: if error.kind() == std::io::ErrorKind::NotFound {
                format!("Not installed. Install {executable}.")
            } else {
                format!("Cannot run {executable}: {error}")
            },
            optional,
        },
    }
}

fn version(line: &str) -> Option<String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let after_version = words
        .iter()
        .position(|word| word.eq_ignore_ascii_case("version"))
        .and_then(|index| words.get(index + 1));
    after_version
        .or_else(|| words.last())
        .filter(|word| word.chars().any(|character| character.is_ascii_digit()))
        .map(|word| (*word).to_owned())
}

fn check_soulseek() -> ServiceStatus {
    let config = app_config::load(&app_config::path()).unwrap_or(Value::Null);
    let Some(settings) = SessionSettings::configured(&config) else {
        return ServiceStatus {
            name: "Soulseek",
            available: None,
            detail: "Add your account in the Soulseek section. Until then, Spotify tracks come from a YouTube search.".into(),
            optional: true,
        };
    };
    let host = settings
        .server_host
        .clone()
        .unwrap_or_else(|| "server.slsknet.org".into());
    let port = settings.server_port.unwrap_or(2416);
    match Session::shared(settings) {
        Ok(_session) => ServiceStatus {
            name: "Soulseek",
            available: Some(true),
            detail: format!("{host}:{port}"),
            optional: true,
        },
        Err(error) => ServiceStatus {
            name: "Soulseek",
            available: Some(false),
            detail: format!("Cannot connect: {error}"),
            optional: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{check_binary, version};

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
        assert!(status.detail.contains("Not installed"));
    }

    #[test]
    fn detail_keeps_only_the_version() {
        assert_eq!(
            version("ffmpeg version 9.0.2 Copyright (c) 2000-2026 the FFmpeg developers"),
            Some("9.0.2".into())
        );
        assert_eq!(version("bandsnatch 0.3.3"), Some("0.3.3".into()));
        assert_eq!(version("2026.08.19"), Some("2026.08.19".into()));
        assert_eq!(version("usage: tool"), None);
    }
}
