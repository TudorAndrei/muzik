//! Rust handlers for GPUI requests that no longer need the Python service.

use crate::services;
use chrono::{DateTime, Local};
use muzik_core::app_config;
use muzik_core::downloads::{human_size, scan};
use muzik_core::paths;
use muzik_core::spotify;
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
            | "spotify.set_client_id"
            | "spotify.logout"
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
        "spotify.set_client_id" => {
            let client_id = params
                .get("client_id")
                .and_then(Value::as_str)
                .ok_or("client_id must be a non-empty string")?;
            let client_id = spotify::set_client_id(&path, client_id)?;
            Ok(json!({"client_id": client_id}))
        }
        "spotify.logout" => Ok(json!({"removed": spotify::clear_tokens(&spotify::token_path())?})),
        _ => Err(format!("unknown command: {command}")),
    }
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
    use super::library_scan;
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
}
