use serde_json::Value;
use std::fs;
use std::path::Path;
use thiserror::Error;

const DEFAULTS: &str = include_str!("config_default.yaml");

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
}

/// The beets defaults, user config, and muzik overrides as one YAML tree.
#[derive(Clone, Debug)]
pub struct BeetsConfig {
    values: Value,
}

impl BeetsConfig {
    /// Load the installed beets defaults and an optional user file.
    /// `overrides` has the highest priority.
    pub fn load(user_path: &Path, overrides: Value) -> Result<Self, Error> {
        let user = match fs::read_to_string(user_path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(Error::Read {
                    path: user_path.display().to_string(),
                    source,
                })
            }
        };
        tracing::debug!(path = %user_path.display(), "loading beets config");
        Self::from_layers(&user, overrides)
    }

    /// Parse the user YAML and merge it with embedded beets defaults.
    pub fn from_layers(user_yaml: &str, overrides: Value) -> Result<Self, Error> {
        let mut values = parse_layer(DEFAULTS, "defaults")?;
        let user = if user_yaml.trim().is_empty() {
            Value::Object(Default::default())
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

    pub fn values(&self) -> &Value {
        &self.values
    }

    /// Read nested keys such as `["match", "strong_rec_thresh"]`.
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
    if let (Some(base), Value::Object(overlay)) = (base.as_object_mut(), &overlay) {
        for (key, value) in overlay {
            merge(base.entry(key).or_insert(Value::Null), value.clone());
        }
    } else {
        *base = overlay;
    }
}
