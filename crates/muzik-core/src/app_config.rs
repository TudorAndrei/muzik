//! Read and update the existing muzik config file.

use crate::Result;
use crate::config_choices::{
    AudioFallback, AudioSource, DuplicatePolicy, MetadataSource, PreferredAudio, QualityPolicy,
};
use crate::paths::{self, Paths};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[must_use]
pub fn path() -> PathBuf {
    Paths::user().config_file()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the flat native_gui config section and GUI form fields"
)]
pub struct GuiDefaults {
    pub output: PathBuf,
    pub splits: PathBuf,
    pub review: bool,
    pub no_split: bool,
    pub no_organize: bool,
    #[serde(rename = "import_")]
    pub import: bool,
    pub tag_only: bool,
    pub dry_run: bool,
    pub jobs: usize,
    pub config: PathBuf,
    pub keep_source: bool,
    pub force: bool,
    pub metadata_source: MetadataSource,
    pub audio_source: AudioSource,
    pub prefer: PreferredAudio,
    pub fallback: AudioFallback,
    pub interactive: bool,
    pub quality_policy: QualityPolicy,
    pub duplicates: DuplicatePolicy,
    pub min_bitrate: u32,
    pub auto_decide: bool,
    pub agent_model: String,
}

impl Default for GuiDefaults {
    fn default() -> Self {
        Self {
            output: PathBuf::new(),
            splits: PathBuf::new(),
            review: false,
            no_split: false,
            no_organize: false,
            import: false,
            tag_only: false,
            dry_run: false,
            jobs: 0,
            config: PathBuf::new(),
            keep_source: false,
            force: false,
            metadata_source: MetadataSource::default(),
            audio_source: AudioSource::default(),
            prefer: PreferredAudio::default(),
            fallback: AudioFallback::default(),
            interactive: true,
            quality_policy: QualityPolicy::default(),
            duplicates: DuplicatePolicy::default(),
            min_bitrate: 256,
            auto_decide: true,
            agent_model: "gpt-6-luna".into(),
        }
    }
}

impl GuiDefaults {
    #[must_use]
    pub fn standard(paths: &Paths) -> Self {
        Self {
            output: paths.downloads(),
            splits: paths.splits(),
            ..Self::default()
        }
    }

    fn checked(mut self, paths: &Paths) -> Result<Self> {
        let standard = Self::standard(paths);
        for (path, fallback) in [
            (&mut self.output, standard.output),
            (&mut self.splits, standard.splits),
            (&mut self.config, PathBuf::new()),
        ] {
            if path.as_os_str().as_encoded_bytes().contains(&0) {
                return Err("a folder must not contain a null byte".into());
            }
            *path = if path.as_os_str().is_empty() {
                fallback
            } else {
                paths::expand_home(path)
            };
        }
        Ok(self)
    }
}

/// # Errors
/// Returns an error if the file cannot be read, is not valid YAML, or is not a mapping.
pub fn load(path: &Path) -> Result<Value> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(error.into()),
    };
    if contents.trim().is_empty() {
        return Ok(json!({}));
    }
    let value: Value = serde_saphyr::from_str(&contents)?;
    if !value.is_object() {
        return Err("muzik config must be a mapping".into());
    }
    Ok(value)
}

#[must_use]
pub fn load_gui_defaults(paths: &Paths) -> GuiDefaults {
    load(&paths.config_file())
        .ok()
        .and_then(|config| config.get("native_gui").cloned())
        .and_then(|section| serde_json::from_value::<GuiDefaults>(section).ok())
        .and_then(|defaults| defaults.checked(paths).ok())
        .unwrap_or_else(|| GuiDefaults::standard(paths))
}

/// # Errors
/// Returns an error if the params are not valid GUI settings or the config file cannot be read or written.
pub fn save_gui_defaults(paths: &Paths, params: &Value) -> Result<GuiDefaults> {
    let changes = params
        .as_object()
        .ok_or("config params must be an object")?;
    let mut values = serde_json::to_value(load_gui_defaults(paths))?;
    if let Some(values) = values.as_object_mut() {
        values.extend(changes.clone());
    }
    let defaults = serde_json::from_value::<GuiDefaults>(values)?.checked(paths)?;
    let path = paths.config_file();
    let mut config = load(&path)?;
    config
        .as_object_mut()
        .ok_or("config file is not a mapping")?
        .insert("native_gui".into(), serde_json::to_value(&defaults)?);
    write(&path, &config)?;
    Ok(defaults)
}

/// # Errors
/// Returns an error if the value is empty or the config file cannot be read or written.
pub fn save_section_string(path: &Path, section: &str, key: &str, value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{key} must be a non-empty string").into());
    }
    save_section_value(path, section, key, json!(value))
}

/// # Errors
/// Returns an error if the config file cannot be read, is not a mapping, or cannot be written.
pub fn save_section_value(path: &Path, section: &str, key: &str, value: Value) -> Result<()> {
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
        .insert(key.to_owned(), value);
    write(path, &config)
}

/// # Errors
/// Returns an error if the config file cannot be read or written.
pub fn remove_section_key(path: &Path, section: &str, key: &str) -> Result<()> {
    let mut config = load(path)?;
    let removed = config
        .get_mut(section)
        .and_then(Value::as_object_mut)
        .and_then(|entries| entries.remove(key))
        .is_some();
    if removed {
        write(path, &config)?;
    }
    Ok(())
}

fn write(path: &Path, config: &Value) -> Result<()> {
    let parent = path.parent().ok_or("config path has no parent")?;
    fs::create_dir_all(parent)?;
    let yaml = serde_saphyr::to_string(&config)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".config.yaml.")
        .tempfile_in(parent)?;
    temporary.write_all(yaml.as_bytes())?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{load, load_gui_defaults, save_gui_defaults, save_section_string};
    use crate::config_choices::AudioSource;
    use crate::paths::Paths;
    use serde_json::json;
    use std::fs;

    #[test]
    fn saves_gui_settings_without_changing_spotify_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let path = paths.config_file();
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(&path, "spotify:\n  client_id: saved\n").unwrap();
        let saved =
            save_gui_defaults(&paths, &json!({"jobs": 3, "audio_source": "soulseek"})).unwrap();
        assert_eq!(saved.jobs, 3);
        assert_eq!(saved.audio_source, AudioSource::Soulseek);
        assert_eq!(saved.output, paths.downloads());
        assert_eq!(load_gui_defaults(&paths), saved);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("client_id: saved"));
    }

    #[test]
    fn rejects_invalid_gui_settings_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        for value in [
            json!({"raw": "song.flac"}),
            json!({"jobs": -1}),
            json!({"audio_source": "invalid"}),
        ] {
            assert!(save_gui_defaults(&paths, &value).is_err());
            assert!(!paths.config_file().exists());
        }
    }

    #[test]
    fn save_keeps_an_unreadable_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let path = paths.config_file();
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(&path, "spotify: [unfinished").unwrap();
        assert!(save_gui_defaults(&paths, &json!({"jobs": 2})).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "spotify: [unfinished");
    }

    #[test]
    fn saves_spotify_client_id_and_keeps_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config/config.yaml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "spotify:\n  redirect_port: '9000'\nsoulseek:\n  username: user\n",
        )
        .unwrap();
        save_section_string(&path, "spotify", "client_id", "  new-id  ").unwrap();
        let config = load(&path).unwrap();
        assert_eq!(config["spotify"]["client_id"], "new-id");
        assert_eq!(config["spotify"]["redirect_port"], "9000");
        assert_eq!(config["soulseek"]["username"], "user");
    }
}
