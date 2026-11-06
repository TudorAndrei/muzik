use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const DEFAULTS: &str = include_str!("config_default.yaml");

/// Find the existing library config at the legacy beets location.
pub fn default_config_path() -> PathBuf {
    if let Some(root) = crate::paths::profile() {
        return crate::paths::Paths::under(&root).beets;
    }
    if let Some(directory) = env::var_os("BEETSDIR") {
        let path = PathBuf::from(directory);
        let path = if path.starts_with("~") {
            env::var_os("HOME")
                .or_else(|| env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(path.strip_prefix("~").unwrap_or(&path))
        } else {
            path
        };
        let path = if path.is_absolute() {
            path
        } else {
            env::current_dir().unwrap_or_default().join(path)
        };
        return path.join("config.yaml");
    }

    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let fallback = home.join(".config");
    let mut directories = vec![fallback.clone()];
    if cfg!(target_os = "macos") {
        directories.push(home.join("Library/Application Support"));
    }
    if cfg!(target_os = "windows") {
        directories.push(home.join("AppData/Roaming"));
        if let Some(appdata) = env::var_os("APPDATA") {
            directories.push(PathBuf::from(appdata));
        }
    } else {
        if let Some(xdg_home) = env::var_os("XDG_CONFIG_HOME") {
            directories.push(PathBuf::from(xdg_home));
        }
        if let Some(xdg_dirs) = env::var_os("XDG_CONFIG_DIRS") {
            directories.extend(env::split_paths(&xdg_dirs));
        } else {
            directories.push(PathBuf::from("/etc/xdg"));
        }
        directories.push(PathBuf::from("/etc"));
    }
    directories
        .iter()
        .map(|directory| directory.join("beets/config.yaml"))
        .find(|path| path.is_file())
        .unwrap_or_else(|| fallback.join("beets/config.yaml"))
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot read beets config at {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid {layer} config: {source}")]
    Parse {
        layer: &'static str,
        #[source]
        source: Box<serde_saphyr::Error>,
    },
    #[error("{layer} config must be a mapping")]
    NotMapping { layer: &'static str },
    #[error("beets config has no {key} path")]
    MissingPath { key: String },
    #[error(
        "cannot resolve beets {key} path: HOME and USERPROFILE have no absolute home directory"
    )]
    HomeUnavailable { key: String },
}

/// The beets defaults, user config, and muzik overrides as one YAML tree.
#[derive(Clone, Debug)]
pub struct BeetsConfig {
    values: Value,
}

impl BeetsConfig {
    /// Load the installed beets defaults and an optional user file.
    /// `overrides` has the highest priority.
    ///
    /// # Errors
    /// Returns an error if the user file cannot be read or a config layer is invalid.
    pub fn load(user_path: &Path, overrides: Value) -> Result<Self, Error> {
        let user = match fs::read_to_string(user_path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(Error::Read {
                    path: user_path.display().to_string(),
                    source,
                });
            }
        };
        tracing::debug!(path = %user_path.display(), "loading beets config");
        Self::from_layers(&user, overrides)
    }

    /// Parse the user YAML and merge it with embedded beets defaults.
    ///
    /// # Errors
    /// Returns an error if a layer is not valid YAML or is not a mapping.
    pub fn from_layers(user_yaml: &str, overrides: Value) -> Result<Self, Error> {
        let mut values = parse_layer(DEFAULTS, "defaults")?;
        let user = if user_yaml.trim().is_empty() {
            Value::Object(serde_json::Map::default())
        } else {
            parse_layer(user_yaml, "user")?
        };
        if !overrides.is_object() {
            return Err(Error::NotMapping { layer: "overrides" });
        }
        merge(&mut values, user);
        merge(&mut values, overrides);
        Ok(Self { values })
    }

    #[must_use]
    pub const fn values(&self) -> &Value {
        &self.values
    }

    /// Resolve a configured path against the directory of its config file.
    ///
    /// # Errors
    /// Returns an error if the key has no path string or a home path has no home directory.
    pub fn configured_path(&self, config_path: &Path, key: &str) -> Result<PathBuf, Error> {
        let raw = self
            .get(&[key])
            .and_then(Value::as_str)
            .ok_or_else(|| Error::MissingPath {
                key: key.to_owned(),
            })?;
        let expanded = if raw == "~" || raw.starts_with("~/") {
            let home = ["HOME", "USERPROFILE"]
                .into_iter()
                .filter_map(env::var_os)
                .map(PathBuf::from)
                .find(|path| path.is_absolute())
                .ok_or_else(|| Error::HomeUnavailable {
                    key: key.to_owned(),
                })?;
            home.join(raw.trim_start_matches('~').trim_start_matches('/'))
        } else {
            PathBuf::from(raw)
        };
        Ok(if expanded.is_absolute() {
            expanded
        } else {
            config_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(expanded)
        })
    }

    /// Read nested keys such as `["match", "strong_rec_thresh"]`.
    #[must_use]
    pub fn get(&self, path: &[&str]) -> Option<&Value> {
        path.iter()
            .try_fold(&self.values, |value, key| value.get(key))
    }
}

fn parse_layer(yaml: &str, layer: &'static str) -> Result<Value, Error> {
    let value: Value = serde_saphyr::from_str(yaml).map_err(|source| Error::Parse {
        layer,
        source: Box::new(source),
    })?;
    if !value.is_object() {
        return Err(Error::NotMapping { layer });
    }
    Ok(value)
}

fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base), Value::Object(overlay)) => {
            for (key, value) in overlay {
                merge(base.entry(key).or_insert(Value::Null), value);
            }
        }
        (base, overlay) => *base = overlay,
    }
}
