//! Read and update the existing muzik config file.

use crate::paths;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub fn path() -> PathBuf {
    paths::config_dir().join("config.yaml")
}

pub fn gui_defaults() -> Value {
    json!({
        "output": paths::download_dir(),
        "splits": paths::data_dir().join("splits"),
        "review": false,
        "no_split": false,
        "no_organize": false,
        "import_": false,
        "tag_only": false,
        "dry_run": false,
        "jobs": 0,
        "config": "",
        "keep_source": false,
        "force": false,
        "metadata_source": "auto",
        "audio_source": "youtube",
        "prefer": "lossless",
        "fallback": "youtube",
        "interactive": true,
        "quality_policy": "off",
        "min_bitrate": 256
    })
}

pub fn load(path: &Path) -> Result<Value, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(error.to_string()),
    };
    if contents.trim().is_empty() {
        return Ok(json!({}));
    }
    let value: Value = serde_saphyr::from_str(&contents).map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("muzik config must be a mapping".into());
    }
    Ok(value)
}

pub fn load_gui_defaults(path: &Path) -> Result<Value, String> {
    let saved = load(path).unwrap_or_else(|_| json!({}));
    let Some(section) = saved.get("native_gui").and_then(Value::as_object) else {
        return Ok(gui_defaults());
    };
    let mut defaults = gui_defaults();
    let Some(values) = defaults.as_object_mut() else {
        return Err("GUI defaults are not a mapping".into());
    };
    values.extend(section.clone());
    validate(defaults).or_else(|_| Ok(gui_defaults()))
}

pub fn save_gui_defaults(path: &Path, params: &Value) -> Result<Value, String> {
    let changes = params
        .as_object()
        .ok_or("config params must be an object")?;
    let mut defaults = load_gui_defaults(path)?;
    let values = defaults
        .as_object_mut()
        .ok_or("GUI defaults are not a mapping")?;
    for (key, value) in changes {
        if !values.contains_key(key) {
            return Err(format!("unknown config field: {key}"));
        }
        values.insert(key.clone(), value.clone());
    }
    let defaults = validate(defaults)?;
    let mut config = load(path)?;
    let sections = config
        .as_object_mut()
        .ok_or("config file is not a mapping")?;
    sections.insert("native_gui".into(), defaults.clone());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let yaml = serde_saphyr::to_string(&config).map_err(|error| error.to_string())?;
    fs::write(path, yaml).map_err(|error| error.to_string())?;
    Ok(defaults)
}

fn validate(value: Value) -> Result<Value, String> {
    let standard = gui_defaults();
    let expected = standard
        .as_object()
        .ok_or("GUI defaults are not a mapping")?;
    let entries = value.as_object().ok_or("GUI config must be a mapping")?;
    if let Some(key) = entries.keys().find(|key| !expected.contains_key(*key)) {
        return Err(format!("unknown config field: {key}"));
    }
    let mut valid = Map::new();
    for (key, default) in expected {
        let item = entries
            .get(key)
            .ok_or_else(|| format!("missing config field: {key}"))?;
        if default.is_boolean() && !item.is_boolean() {
            return Err(format!("{key} must be a boolean"));
        }
        if default.is_number() && item.as_u64().is_none() {
            return Err(format!("{key} must be a non-negative integer"));
        }
        if default.is_string() {
            let text = item
                .as_str()
                .ok_or_else(|| format!("{key} must be a string"))?;
            if text.contains('\0') {
                return Err(format!("{key} must not contain a null byte"));
            }
            if matches!(key.as_str(), "output" | "splits" | "prefer") && text.trim().is_empty() {
                return Err(format!("{key} must not be empty"));
            }
            let choices: &[&str] = match key.as_str() {
                "metadata_source" => &["none", "youtube", "musicbrainz", "auto"],
                "audio_source" => &["youtube", "soulseek", "auto"],
                "fallback" => &["youtube", "none"],
                "quality_policy" => &["off", "ask", "auto"],
                _ => &[],
            };
            if !choices.is_empty() && !choices.contains(&text) {
                return Err(format!("invalid {key}: {text}"));
            }
        }
        let item = if matches!(key.as_str(), "output" | "splits" | "config") {
            item.as_str()
                .map(expand_home)
                .map_or_else(|| item.clone(), Value::String)
        } else {
            item.clone()
        };
        valid.insert(key.clone(), item);
    }
    Ok(Value::Object(valid))
}

fn expand_home(text: &str) -> String {
    if text == "~" || text.starts_with("~/") {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join(text.trim_start_matches('~').trim_start_matches('/'))
            .to_string_lossy()
            .into_owned()
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::{load_gui_defaults, save_gui_defaults};
    use serde_json::json;
    use std::fs;

    #[test]
    fn saves_gui_settings_without_changing_spotify_settings(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.yaml");
        fs::write(&path, "spotify:\n  client_id: saved\n")?;
        let saved = save_gui_defaults(&path, &json!({"jobs": 3, "audio_source": "soulseek"}))
            .map_err(std::io::Error::other)?;
        assert_eq!(saved["jobs"], 3);
        assert_eq!(saved["audio_source"], "soulseek");
        assert_eq!(
            load_gui_defaults(&path).map_err(std::io::Error::other)?,
            saved
        );
        let text = fs::read_to_string(&path)?;
        assert!(text.contains("client_id: saved"));
        Ok(())
    }

    #[test]
    fn rejects_invalid_gui_settings_without_writing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.yaml");
        for value in [
            json!({"raw": "song.flac"}),
            json!({"jobs": -1}),
            json!({"audio_source": "invalid"}),
        ] {
            assert!(save_gui_defaults(&path, &value).is_err());
            assert!(!path.exists());
        }
        Ok(())
    }

    #[test]
    fn save_keeps_an_unreadable_config_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.yaml");
        fs::write(&path, "spotify: [unfinished")?;
        assert!(save_gui_defaults(&path, &json!({"jobs": 2})).is_err());
        assert_eq!(fs::read_to_string(path)?, "spotify: [unfinished");
        Ok(())
    }
}
