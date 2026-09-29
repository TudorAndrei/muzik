use crate::services;
use chrono::{DateTime, Local};
use muzik_core::app_config;
use muzik_core::downloads::{human_size, scan};
use muzik_core::paths;
use muzik_core::spotify;
use muzik_core::watchlist::Repository;
use serde_json::{json, Value};
use std::path::Path;

pub fn handles(command: &str) -> bool {
    matches!(
        command,
        "hello"
            | "config.get"
            | "config.save"
            | "library.scan"
            | "services.check"
            | "soulseek.get"
            | "soulseek.save"
            | "spotify.set_client_id"
            | "spotify.logout"
            | "spotify.status"
            | "spotify.playlists"
            | "watchlist.add"
            | "watchlist.rename"
            | "watchlist.remove"
    )
}

pub fn dispatch(command: &str, params: &Value) -> Result<Value, String> {
    let path = app_config::path();
    match command {
        "hello" => Ok(json!({
            "protocol_version": 1,
            "defaults": app_config::load_gui_defaults(&path)?,
            "item_actions": [
                "run", "retry", "download_again", "check_quality_again",
                "parse_again", "split_again", "organize_again", "run_all_again"
            ]
        })),
        "config.get" => Ok(json!({"defaults": app_config::load_gui_defaults(&path)?})),
        "config.save" => Ok(json!({"defaults": app_config::save_gui_defaults(&path, params)?})),
        "library.scan" => library_scan(params),
        "services.check" => Ok(json!({"services": services::check()})),
        "soulseek.get" => soulseek_settings(&path),
        "soulseek.save" => {
            save_soulseek(&path, params)?;
            soulseek_settings(&path)
        }
        "spotify.set_client_id" => {
            let client_id = params
                .get("client_id")
                .and_then(Value::as_str)
                .ok_or("client_id must be a non-empty string")?;
            let client_id = spotify::set_client_id(&path, client_id)?;
            Ok(json!({"client_id": client_id}))
        }
        "spotify.logout" => Ok(json!({"removed": spotify::clear_tokens(&spotify::token_path())?})),
        "spotify.status" => spotify::status(&path, &spotify::token_path()),
        "spotify.playlists" => spotify::list_playlists(&path, &spotify::token_path())
            .map(|playlists| json!({"playlists": playlists})),
        "watchlist.add" | "watchlist.rename" | "watchlist.remove" => watchlist_edit(
            &Repository::new(Repository::default_path()),
            command,
            params,
        ),
        _ => Err(format!("unknown command: {command}")),
    }
}

fn required_string<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{key} must be a non-empty string."))
}

const SOULSEEK_HOST: &str = "server.slsknet.org";
const SOULSEEK_PORT: u64 = 2416;

fn soulseek_settings(path: &Path) -> Result<Value, String> {
    let config = app_config::load(path)?;
    let section = &config["soulseek"];
    let text = |key: &str| section[key].as_str().unwrap_or("").to_owned();
    let port = section["server_port"]
        .as_u64()
        .or_else(|| section["server_port"].as_str()?.parse().ok())
        .unwrap_or(SOULSEEK_PORT);
    let host = Some(text("server_host"))
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| SOULSEEK_HOST.to_owned());
    Ok(json!({
        "username": text("username"),
        "has_password": !text("password").is_empty(),
        "server_host": host,
        "server_port": port,
    }))
}

fn save_soulseek(path: &Path, params: &Value) -> Result<(), String> {
    let username = required_string(params, "username")?;
    let host = params["server_host"]
        .as_str()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .unwrap_or(SOULSEEK_HOST);
    let port = params["server_port"]
        .as_u64()
        .or_else(|| params["server_port"].as_str()?.trim().parse().ok())
        .filter(|port| (1..=65535).contains(port))
        .ok_or("Enter a server port from 1 to 65535.")?;
    let password = params["password"].as_str().unwrap_or("");
    let has_password = !app_config::load(path)?["soulseek"]["password"]
        .as_str()
        .unwrap_or("")
        .is_empty();
    if password.trim().is_empty() && !has_password {
        return Err("Enter the Soulseek password.".into());
    }
    app_config::save_section_string(path, "soulseek", "username", username)?;
    if !password.trim().is_empty() {
        app_config::save_section_string(path, "soulseek", "password", password)?;
    }
    app_config::save_section_string(path, "soulseek", "server_host", host)?;
    app_config::save_section_string(path, "soulseek", "server_port", &port.to_string())
}

fn watchlist_edit(repository: &Repository, command: &str, params: &Value) -> Result<Value, String> {
    let result = match command {
        "watchlist.add" => json!({"playlist": repository.add(required_string(params, "url")?)?}),
        "watchlist.rename" => json!({"renamed": repository.rename(
            required_string(params, "playlist_id")?,
            required_string(params, "title")?,
        )?}),
        "watchlist.remove" => json!({"removed": repository.remove(
            required_string(params, "playlist_id")?,
        )?}),
        _ => return Err(format!("unknown watchlist edit: {command}")),
    };
    Ok(result)
}

pub fn library_scan(params: &Value) -> Result<Value, String> {
    let output = params
        .get("output")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map_or_else(paths::download_dir, |value| Path::new(value).to_path_buf());
    let path = output.as_path();
    let items = scan(path).map_err(|error| error.to_string())?;
    let total = items
        .iter()
        .fold(0_u64, |size, item| size.saturating_add(item.size));
    let items = items
        .into_iter()
        .map(|item| {
            let modified: DateTime<Local> = item.modified_at.into();
            let size_label = human_size(item.size);
            let mut value = serde_json::to_value(item).map_err(|error| error.to_string())?;
            let fields = value
                .as_object_mut()
                .ok_or("invalid audio inventory item")?;
            fields.insert("size_label".into(), json!(size_label));
            fields.insert(
                "modified".into(),
                json!(modified.format("%Y-%m-%d %H:%M").to_string()),
            );
            Ok(value)
        })
        .collect::<Result<Vec<Value>, String>>()?;
    Ok(json!({"output": output, "total_size": human_size(total), "items": items}))
}

#[cfg(test)]
mod tests {
    use super::{library_scan, save_soulseek, soulseek_settings, watchlist_edit};
    use muzik_core::watchlist::Repository;
    use serde_json::json;
    use std::fs;

    #[test]
    fn library_scan_reports_existing_audio() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Track [dQw4w9WgXcQ].mp3"), b"audio")?;
        let result = library_scan(&json!({"output": dir.path()})).map_err(std::io::Error::other)?;
        assert_eq!(result["total_size"], "5.0 B");
        assert_eq!(result["items"][0]["title"], "Track");
        assert_eq!(result["items"][0]["youtube_id"], "dQw4w9WgXcQ");
        assert!(result["items"][0]["modified"]
            .as_str()
            .is_some_and(|date| !date.is_empty()));
        Ok(())
    }

    #[test]
    fn soulseek_account_saves_without_returning_the_password(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.yaml");
        fs::write(&path, "native_gui:\n  jobs: 2\n")?;
        assert!(
            save_soulseek(&path, &json!({"username": "listener", "server_port": 2416})).is_err()
        );
        save_soulseek(
            &path,
            &json!({"username": "listener", "password": "secret", "server_port": "2242"}),
        )
        .map_err(std::io::Error::other)?;
        let settings = soulseek_settings(&path).map_err(std::io::Error::other)?;
        assert_eq!(settings["username"], "listener");
        assert_eq!(settings["has_password"], true);
        assert_eq!(settings["server_host"], "server.slsknet.org");
        assert_eq!(settings["server_port"], 2242);
        assert!(settings.get("password").is_none());
        save_soulseek(
            &path,
            &json!({"username": "renamed", "password": "", "server_port": 2242}),
        )
        .map_err(std::io::Error::other)?;
        let saved = fs::read_to_string(&path)?;
        assert!(saved.contains("secret"));
        assert!(saved.contains("renamed"));
        assert!(saved.contains("jobs: 2"));
        Ok(())
    }

    #[test]
    fn watchlist_edits_keep_the_existing_file_format() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let repository = Repository::new(dir.path().join("watchlist.json"));
        let added = watchlist_edit(
            &repository,
            "watchlist.add",
            &json!({"url": "https://www.youtube.com/playlist?list=PL123"}),
        )
        .map_err(std::io::Error::other)?;
        assert_eq!(added["playlist"]["playlist_id"], "PL123");
        let renamed = watchlist_edit(
            &repository,
            "watchlist.rename",
            &json!({"playlist_id": "PL123", "title": "  New name  "}),
        )
        .map_err(std::io::Error::other)?;
        assert_eq!(renamed["renamed"], true);
        let saved = repository.load().map_err(std::io::Error::other)?;
        assert_eq!(saved["version"], 3);
        assert_eq!(saved["playlists"][0]["title"], "New name");
        let removed = watchlist_edit(
            &repository,
            "watchlist.remove",
            &json!({"playlist_id": "PL123"}),
        )
        .map_err(std::io::Error::other)?;
        assert_eq!(removed["removed"], true);
        assert_eq!(
            repository.load().map_err(std::io::Error::other)?["playlists"],
            json!([])
        );
        Ok(())
    }
}
