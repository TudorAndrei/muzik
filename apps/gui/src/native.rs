use chrono::{DateTime, Local};
use muzik_bandcamp as bandcamp;
use muzik_core::app_config;
use muzik_core::downloads::{human_size, scan};
use muzik_core::paths::Paths;
use muzik_runner::setup;
use muzik_spotify as spotify;
use serde_json::{json, Value};
use std::path::Path;

pub fn handles(command: &str) -> bool {
    matches!(
        command,
        "hello"
            | "bandcamp.get"
            | "bandcamp.save"
            | "bandcamp.logout"
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
    )
}

pub fn dispatch(paths: &Paths, command: &str, params: &Value) -> Result<Value, String> {
    let path = paths.config_file();
    match command {
        "hello" => Ok(json!({
            "protocol_version": 1,
            "defaults": app_config::load_gui_defaults(paths)?,
            "item_actions": [
                "run", "retry", "download_again", "check_quality_again",
                "parse_again", "split_again", "organize_again", "run_all_again"
            ]
        })),
        "bandcamp.get" => Ok(bandcamp::status(paths)),
        "bandcamp.save" => {
            bandcamp::Login::save(
                paths,
                params["user"].as_str().unwrap_or(""),
                params["cookies"].as_str().unwrap_or(""),
            )?;
            muzik_runner::watchlist::ensure_sources(paths)?;
            Ok(bandcamp::status(paths))
        }
        "bandcamp.logout" => {
            bandcamp::Login::clear(paths)?;
            Ok(bandcamp::status(paths))
        }
        "config.get" => Ok(json!({"defaults": app_config::load_gui_defaults(paths)?})),
        "config.save" => Ok(json!({"defaults": app_config::save_gui_defaults(paths, params)?})),
        "library.scan" => library_scan(paths, params),
        "services.check" => Ok(json!({"services": setup::check_services(paths)})),
        "soulseek.get" => setup::soulseek_account(&path),
        "soulseek.save" => {
            let username = params["username"]
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or("username must be a non-empty string.")?;
            let server_port = params["server_port"]
                .as_u64()
                .or_else(|| params["server_port"].as_str()?.trim().parse().ok())
                .ok_or("Enter a server port from 1 to 65535.")?;
            setup::save_soulseek_account(
                &path,
                &setup::SoulseekAccount {
                    username: Some(username),
                    password: params["password"].as_str(),
                    server_host: params["server_host"].as_str(),
                    server_port: Some(server_port),
                },
            )?;
            setup::soulseek_account(&path)
        }
        "spotify.set_client_id" => {
            let client_id = params
                .get("client_id")
                .and_then(Value::as_str)
                .ok_or("client_id must be a non-empty string")?;
            let client_id = spotify::set_client_id(&path, client_id)?;
            Ok(json!({"client_id": client_id}))
        }
        "spotify.logout" => Ok(json!({"removed": spotify::clear_tokens(&paths.spotify_token())?})),
        "spotify.status" => spotify::status(&path, &paths.spotify_token()),
        "spotify.playlists" => spotify::list_playlists(&path, &paths.spotify_token())
            .map(|playlists| json!({"playlists": playlists})),
        _ => Err(format!("unknown command: {command}")),
    }
}

pub fn library_scan(paths: &Paths, params: &Value) -> Result<Value, String> {
    let output = params
        .get("output")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map_or_else(|| paths.downloads(), |value| Path::new(value).to_path_buf());
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
    use super::library_scan;
    use serde_json::json;
    use std::fs;

    #[test]
    fn library_scan_reports_existing_audio() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Track [dQw4w9WgXcQ].mp3"), b"audio")?;
        let result = library_scan(
            &muzik_core::paths::Paths::under(dir.path()),
            &json!({"output": dir.path()}),
        )
        .map_err(std::io::Error::other)?;
        assert_eq!(result["total_size"], "5.0 B");
        assert_eq!(result["items"][0]["title"], "Track");
        assert_eq!(result["items"][0]["youtube_id"], "dQw4w9WgXcQ");
        assert!(result["items"][0]["modified"]
            .as_str()
            .is_some_and(|date| !date.is_empty()));
        Ok(())
    }
}
