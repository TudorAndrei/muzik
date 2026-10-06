use crate::Result;
use muzik_core::app_config;
use muzik_core::paths::Paths;
use muzik_soulseek::session::{
    self, Session, SessionSettings, DEFAULT_SERVER_HOST, DEFAULT_SERVER_PORT,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

pub struct SoulseekAccount<'a> {
    pub username: Option<&'a str>,
    pub password: Option<&'a str>,
    pub server_host: Option<&'a str>,
    pub server_port: Option<u64>,
}

pub fn soulseek_account(config_file: &Path) -> Result<Value> {
    let config = app_config::load(config_file)?;
    let section = &config["soulseek"];
    let text = |key: &str| section[key].as_str().unwrap_or("").to_owned();
    let port = section["server_port"]
        .as_u64()
        .or_else(|| section["server_port"].as_str()?.parse().ok())
        .unwrap_or(u64::from(DEFAULT_SERVER_PORT));
    let host = Some(text("server_host"))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER_HOST.to_owned());
    Ok(json!({
        "username": text("username"),
        "has_password": !text("password").is_empty() || session::saved_password().is_some(),
        "server_host": host,
        "server_port": port,
    }))
}

pub fn save_soulseek_account(config_file: &Path, account: &SoulseekAccount<'_>) -> Result<()> {
    let config = app_config::load(config_file)?;
    let saved = |key: &str| config["soulseek"][key].as_str().unwrap_or("").to_owned();
    let username = account
        .username
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| Some(saved("username")).filter(|value| !value.is_empty()))
        .ok_or("username must be a non-empty string.")?;
    let host = account
        .server_host
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .unwrap_or(DEFAULT_SERVER_HOST);
    let port = account
        .server_port
        .unwrap_or(u64::from(DEFAULT_SERVER_PORT));
    if !(1..=65535).contains(&port) {
        return Err("Enter a server port from 1 to 65535.".into());
    }
    let password = account.password.unwrap_or("");
    if password.trim().is_empty()
        && saved("password").is_empty()
        && session::saved_password().is_none()
    {
        return Err("Enter the Soulseek password.".into());
    }
    app_config::save_section_string(config_file, "soulseek", "username", &username)?;
    if !password.trim().is_empty() {
        app_config::save_section_string(config_file, "soulseek", "password", password)?;
    }
    app_config::save_section_string(config_file, "soulseek", "server_host", host)?;
    app_config::save_section_string(config_file, "soulseek", "server_port", &port.to_string())?;
    move_soulseek_password(config_file)
}

pub fn move_soulseek_password(config_file: &Path) -> Result<()> {
    let config = app_config::load(config_file)?;
    let password = config["soulseek"]["password"].as_str().unwrap_or("").trim();
    if password.is_empty() || session::save_password(password).is_err() {
        return Ok(());
    }
    Ok(app_config::remove_section_key(
        config_file,
        "soulseek",
        "password",
    )?)
}

#[derive(Debug, Serialize)]
pub struct ServiceStatus {
    name: &'static str,
    available: Option<bool>,
    detail: String,
    optional: bool,
}

pub fn check_services(paths: &Paths) -> Vec<ServiceStatus> {
    vec![
        check_binary("ffmpeg", "ffmpeg", &["-version"], false),
        check_binary("yt-dlp", "yt-dlp", &["--version"], false),
        check_soulseek(paths),
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

fn check_soulseek(paths: &Paths) -> ServiceStatus {
    let config = app_config::load(&paths.config_file()).unwrap_or(Value::Null);
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
        .unwrap_or_else(|| DEFAULT_SERVER_HOST.into());
    let port = settings.server_port.unwrap_or(DEFAULT_SERVER_PORT);
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
    use super::{check_binary, save_soulseek_account, soulseek_account, version, SoulseekAccount};
    use std::fs;

    fn account<'a>(
        username: Option<&'a str>,
        password: Option<&'a str>,
        server_port: Option<u64>,
    ) -> SoulseekAccount<'a> {
        SoulseekAccount {
            username,
            password,
            server_host: None,
            server_port,
        }
    }

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
        assert_eq!(version("tool 0.3.3"), Some("0.3.3".into()));
        assert_eq!(version("2026.08.19"), Some("2026.08.19".into()));
        assert_eq!(version("usage: tool"), None);
    }

    #[test]
    fn soulseek_account_saves_without_returning_the_password(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.yaml");
        fs::write(&path, "native_gui:\n  jobs: 2\n")?;
        assert!(
            save_soulseek_account(&path, &account(Some("listener"), None, Some(2416))).is_err()
        );
        save_soulseek_account(
            &path,
            &account(Some("listener"), Some("secret"), Some(2242)),
        )?;
        let settings = soulseek_account(&path)?;
        assert_eq!(settings["username"], "listener");
        assert_eq!(settings["has_password"], true);
        assert_eq!(settings["server_host"], "server.slsknet.org");
        assert_eq!(settings["server_port"], 2242);
        assert!(settings.get("password").is_none());
        save_soulseek_account(&path, &account(Some("renamed"), Some(""), Some(2242)))?;
        let saved = fs::read_to_string(&path)?;
        assert!(saved.contains("secret"));
        assert!(saved.contains("renamed"));
        assert!(saved.contains("jobs: 2"));
        Ok(())
    }

    #[test]
    fn saved_user_name_is_kept_when_none_is_given() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.yaml");
        save_soulseek_account(&path, &account(Some("a"), Some("b"), None))?;
        save_soulseek_account(&path, &account(None, None, Some(2242)))?;
        let settings = soulseek_account(&path)?;
        assert_eq!(settings["username"], "a");
        assert_eq!(settings["has_password"], true);
        assert_eq!(settings["server_port"], 2242);
        Ok(())
    }

    #[test]
    fn account_needs_a_user_name() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.yaml");
        let error = save_soulseek_account(&path, &account(None, Some("b"), None))
            .err()
            .ok_or("expected an error")?;
        assert!(error.to_string().contains("username"));
        Ok(())
    }

    #[test]
    fn port_out_of_range_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.yaml");
        let error = save_soulseek_account(&path, &account(Some("a"), Some("b"), Some(70000)))
            .err()
            .ok_or("expected an error")?;
        assert_eq!(error.to_string(), "Enter a server port from 1 to 65535.");
        Ok(())
    }
}
