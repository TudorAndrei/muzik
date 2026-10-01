//! Read and update the existing muzik config file.

use crate::config_choices::{
    choices_for_field, AudioFallback, AudioSource, DuplicatePolicy, MetadataSource, QualityPolicy,
    DEFAULT_AUDIO_PREFERENCE,
};
use crate::paths::{self, Paths};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub fn path() -> PathBuf {
    Paths::user().config_file()
}

pub fn gui_defaults(paths: &Paths) -> Value {
    json!({
        "output": paths.downloads(),
        "splits": paths.splits(),
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
        "metadata_source": MetadataSource::default(),
        "audio_source": AudioSource::default(),
        "prefer": DEFAULT_AUDIO_PREFERENCE,
        "fallback": AudioFallback::default(),
        "interactive": true,
        "quality_policy": QualityPolicy::default(),
        "duplicates": DuplicatePolicy::default(),
        "min_bitrate": 256,
        "auto_decide": true,
        "agent_model": "gpt-6-luna"
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

pub fn load_gui_defaults(paths: &Paths) -> Result<Value, String> {
    let standard = gui_defaults(paths);
    let saved = load(&paths.config_file()).unwrap_or_else(|_| json!({}));
    let Some(section) = saved.get("native_gui").and_then(Value::as_object) else {
        return Ok(standard);
    };
    let mut defaults = standard.clone();
    let Some(values) = defaults.as_object_mut() else {
        return Err("GUI defaults are not a mapping".into());
    };
    values.extend(section.clone());
    validate(defaults, &standard).or(Ok(standard))
}

pub fn save_gui_defaults(paths: &Paths, params: &Value) -> Result<Value, String> {
    let path = &paths.config_file();
    let changes = params
        .as_object()
        .ok_or("config params must be an object")?;
    let mut defaults = load_gui_defaults(paths)?;
    let values = defaults
        .as_object_mut()
        .ok_or("GUI defaults are not a mapping")?;
    for (key, value) in changes {
        if !values.contains_key(key) {
            return Err(format!("unknown config field: {key}"));
        }
        values.insert(key.clone(), value.clone());
    }
    let defaults = validate(defaults, &gui_defaults(paths))?;
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

pub fn save_section_string(
    path: &Path,
    section: &str,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{key} must be a non-empty string"));
    }
    let mut config = load(path)?;
    let root = config
        .as_object_mut()
        .ok_or("config file is not a mapping")?;
    let section_value = root.entry(section).or_insert_with(|| json!({}));
    if !section_value.is_object() {
        *section_value = json!({});
    }
    section_value
        .as_object_mut()
        .ok_or("config section is not a mapping")?
        .insert(key.to_owned(), json!(value));
    let parent = path.parent().ok_or("config path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let yaml = serde_saphyr::to_string(&config).map_err(|error| error.to_string())?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".config.yaml.")
        .tempfile_in(parent)
        .map_err(|error| error.to_string())?;
    use std::io::Write;
    temporary
        .write_all(yaml.as_bytes())
        .map_err(|error| error.to_string())?;
    temporary.flush().map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

fn validate(value: Value, standard: &Value) -> Result<Value, String> {
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
            if choices_for_field(key).is_some_and(|choices| !choices.contains(&text)) {
                return Err(format!("invalid {key}: {text}"));
            }
        }
        let item = if matches!(key.as_str(), "output" | "splits" | "config") {
            item.as_str()
                .map(|text| {
                    paths::expand_home(Path::new(text))
                        .to_string_lossy()
                        .into_owned()
                })
                .map_or_else(|| item.clone(), Value::String)
        } else {
            item.clone()
        };
        valid.insert(key.clone(), item);
    }
    Ok(Value::Object(valid))
}

#[cfg(test)]
mod tests {
    use super::{load, load_gui_defaults, save_gui_defaults, save_section_string};
    use crate::paths::Paths;
    use serde_json::json;
    use std::fs;

    #[test]
    fn saves_gui_settings_without_changing_spotify_settings(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let paths = Paths::under(dir.path());
        let path = paths.config_file();
        fs::create_dir_all(&paths.config)?;
        fs::write(&path, "spotify:\n  client_id: saved\n")?;
        let saved = save_gui_defaults(&paths, &json!({"jobs": 3, "audio_source": "soulseek"}))
            .map_err(std::io::Error::other)?;
        assert_eq!(saved["jobs"], 3);
        assert_eq!(saved["audio_source"], "soulseek");
        assert_eq!(saved["output"], json!(paths.downloads()));
        assert_eq!(
            load_gui_defaults(&paths).map_err(std::io::Error::other)?,
            saved
        );
        let text = fs::read_to_string(&path)?;
        assert!(text.contains("client_id: saved"));
        Ok(())
    }

    #[test]
    fn rejects_invalid_gui_settings_without_writing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let paths = Paths::under(dir.path());
        for value in [
            json!({"raw": "song.flac"}),
            json!({"jobs": -1}),
            json!({"audio_source": "invalid"}),
        ] {
            assert!(save_gui_defaults(&paths, &value).is_err());
            assert!(!paths.config_file().exists());
        }
        Ok(())
    }

    #[test]
    fn save_keeps_an_unreadable_config_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let paths = Paths::under(dir.path());
        let path = paths.config_file();
        fs::create_dir_all(&paths.config)?;
        fs::write(&path, "spotify: [unfinished")?;
        assert!(save_gui_defaults(&paths, &json!({"jobs": 2})).is_err());
        assert_eq!(fs::read_to_string(path)?, "spotify: [unfinished");
        Ok(())
    }

    #[test]
    fn saves_spotify_client_id_and_keeps_other_settings() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config/config.yaml");
        fs::create_dir_all(path.parent().ok_or("config path has no parent")?)?;
        fs::write(
            &path,
            "spotify:\n  redirect_port: '9000'\nsoulseek:\n  username: user\n",
        )?;
        save_section_string(&path, "spotify", "client_id", "  new-id  ")
            .map_err(std::io::Error::other)?;
        let config = load(&path).map_err(std::io::Error::other)?;
        assert_eq!(config["spotify"]["client_id"], "new-id");
        assert_eq!(config["spotify"]["redirect_port"], "9000");
        assert_eq!(config["soulseek"]["username"], "user");
        Ok(())
    }
}
