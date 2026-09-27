//! Saved watchlist reads and local checks for the desktop app.

use muzik_core::{app_config, paths, watchlist};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Clone)]
pub struct Options {
    output: PathBuf,
    splits: PathBuf,
    cache: PathBuf,
    config: Option<PathBuf>,
    no_organize: bool,
    no_split: bool,
    quality_policy: String,
}

impl Options {
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let overrides = params
            .as_object()
            .ok_or("watchlist params must be an object")?;
        let mut defaults = app_config::load_gui_defaults(&app_config::path())?;
        let saved = defaults
            .as_object_mut()
            .ok_or("GUI defaults are not an object")?;
        saved.extend(overrides.clone());
        Self::from_values(saved)
    }

    fn from_values(values: &Map<String, Value>) -> Result<Self, String> {
        let output = path(values, "output")?.unwrap_or_else(paths::download_dir);
        let splits = path(values, "splits")?.unwrap_or_else(|| paths::data_dir().join("splits"));
        Ok(Self {
            output,
            splits,
            cache: paths::cache_dir(),
            config: path(values, "config")?,
            no_organize: boolean(values, "no_organize")?,
            no_split: boolean(values, "no_split")?,
            quality_policy: values
                .get("quality_policy")
                .and_then(Value::as_str)
                .ok_or("quality_policy must be a string")?
                .to_owned(),
        })
    }

    pub fn saved(&self, repository: &watchlist::Repository) -> Result<Value, String> {
        watchlist::view(repository.load()?, &self.output, &self.cache)
    }

    pub fn checked(&self, repository: &watchlist::Repository) -> Result<Value, String> {
        let mut document = repository.load()?;
        watchlist::reconcile(
            &mut document,
            watchlist::ReconcileOptions {
                output: &self.output,
                splits: &self.splits,
                cache: &self.cache,
                config: self.config.as_deref(),
                no_organize: self.no_organize,
                no_split: self.no_split,
                quality_policy: &self.quality_policy,
            },
        )?;
        Ok(document)
    }

    pub fn view(&self, document: Value) -> Result<Value, String> {
        watchlist::view(document, &self.output, &self.cache)
    }
}

fn path(values: &Map<String, Value>, key: &str) -> Result<Option<PathBuf>, String> {
    match values.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.trim().is_empty() => Ok(None),
        Some(Value::String(value)) => {
            let expanded = if value == "~" {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or("HOME is not set")?
            } else if let Some(rest) = value.strip_prefix("~/") {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or("HOME is not set")?
                    .join(rest)
            } else {
                PathBuf::from(value)
            };
            Ok(Some(expanded))
        }
        _ => Err(format!("{key} must be a string")),
    }
}

fn boolean(values: &Map<String, Value>, key: &str) -> Result<bool, String> {
    values
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{key} must be a boolean"))
}

pub fn stamp(path: &Path) -> Result<(u128, u64), String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
        Err(error) => return Err(error.to_string()),
    };
    let modified = metadata
        .modified()
        .map_err(|error| error.to_string())?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    Ok((modified, metadata.len()))
}

#[cfg(test)]
mod tests {
    use super::{path, stamp, Options};
    use serde_json::json;
    use std::fs;

    #[test]
    fn load_uses_the_given_output_and_saved_cards() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("watchlist.json");
        let output = dir.path().join("audio");
        fs::create_dir(&output)?;
        let repo = muzik_core::watchlist::Repository::new(file.clone());
        repo.add("https://www.youtube.com/playlist?list=PL123")
            .map_err(std::io::Error::other)?;
        let options = Options::from_values(
            json!({"output": output, "splits": dir.path().join("splits"),
                   "config": "", "no_organize": false, "no_split": false,
                   "quality_policy": "off"})
            .as_object()
            .ok_or("options are not an object")?,
        )
        .map_err(std::io::Error::other)?;
        let saved = options.saved(&repo).map_err(std::io::Error::other)?;
        assert_eq!(saved["playlists"][0]["playlist_id"], "PL123");
        assert!(stamp(&file)?.1 > 0);
        Ok(())
    }

    #[test]
    fn request_paths_expand_the_home_directory() -> Result<(), Box<dyn std::error::Error>> {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        let values = json!({"output": "~/Music"});
        let values = values.as_object().ok_or("options are not an object")?;
        assert_eq!(
            path(values, "output")?,
            Some(std::path::PathBuf::from(home).join("Music"))
        );
        Ok(())
    }
}
