use muzik_core::paths::{expand_home, Paths};
use muzik_core::watchlist::ReconcileOptions;
use muzik_core::{app_config, ChoiceError};
use muzik_workflow::{WorkflowOptions, WorkflowRequest};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub paths: Paths,
    pub request: WorkflowRequest,
    pub options: WorkflowOptions,
    pub agent_model: Option<String>,
}

impl Settings {
    pub fn resolve(paths: &Paths, params: &Value) -> Result<Self, String> {
        let mut merged = app_config::load_gui_defaults(paths)?;
        merged
            .as_object_mut()
            .ok_or("GUI defaults are not a mapping")?
            .extend(
                params
                    .as_object()
                    .ok_or("request params must be a mapping")?
                    .clone(),
            );
        Self::parse(paths, &merged)
    }

    pub fn parse(paths: &Paths, values: &Value) -> Result<Self, String> {
        let mut options = WorkflowOptions::default();
        for (key, target) in [
            ("review", &mut options.review),
            ("no_organize", &mut options.no_organize),
            ("tag_only", &mut options.tag_only),
            ("no_split", &mut options.no_split),
            ("dry_run", &mut options.dry_run),
            ("keep_source", &mut options.keep_source),
            ("force", &mut options.force),
            ("compilation", &mut options.compilation),
            ("interactive", &mut options.interactive),
        ] {
            if let Some(value) = values.get(key) {
                *target = value
                    .as_bool()
                    .ok_or_else(|| format!("{key} must be a boolean"))?;
            }
        }
        if let Some(value) = values.get("jobs") {
            options.jobs = value
                .as_u64()
                .and_then(|number| usize::try_from(number).ok())
                .ok_or("jobs must be a non-negative integer")?;
        }
        if let Some(value) = values.get("min_bitrate") {
            options.min_bitrate = value
                .as_u64()
                .and_then(|number| u32::try_from(number).ok())
                .ok_or("min_bitrate must be a non-negative integer")?;
        }
        options.config = path(values, "config")?;
        if let Some(value) = values.get("audio_source") {
            options.audio_source = choice(value, "audio_source")?;
        }
        if let Some(value) = values.get("fallback") {
            options.fallback = choice(value, "fallback")?;
        }
        if let Some(value) = values.get("metadata_source") {
            options.metadata_source = choice(value, "metadata_source")?;
        }
        if let Some(value) = values.get("quality_policy") {
            options.quality_policy = choice(value, "quality_policy")?;
        }
        if let Some(value) = values.get("duplicates") {
            options.duplicates = choice(value, "duplicates")?;
        }
        if let Some(value) = values.get("prefer") {
            options.prefer = value.as_str().ok_or("prefer must be a string")?.to_owned();
        }
        let raw = match values.get("raw") {
            None | Some(Value::Null) => String::new(),
            Some(value) => value
                .as_str()
                .ok_or("raw must be a string")?
                .trim()
                .to_owned(),
        };
        let request = WorkflowRequest {
            raw,
            output: path(values, "output")?.unwrap_or_else(|| paths.downloads()),
            splits: path(values, "splits")?.unwrap_or_else(|| paths.splits()),
        };
        let agent_model = (values.get("auto_decide") == Some(&Value::Bool(true))).then(|| {
            values["agent_model"]
                .as_str()
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .unwrap_or(muzik_agent::DEFAULT_MODEL)
                .to_owned()
        });
        Ok(Self {
            paths: paths.clone(),
            request,
            options,
            agent_model,
        })
    }

    pub fn reconcile(&self) -> ReconcileOptions<'_> {
        ReconcileOptions {
            output: &self.request.output,
            splits: &self.request.splits,
            cache: &self.paths.cache,
            config: self.options.config.as_deref(),
            no_organize: self.options.no_organize,
            no_split: self.options.no_split,
            quality_policy: self.options.quality_policy,
        }
    }
}

fn choice<T: std::str::FromStr<Err = ChoiceError>>(value: &Value, name: &str) -> Result<T, String> {
    value
        .as_str()
        .ok_or_else(|| format!("{name} must be a string"))?
        .parse()
        .map_err(|error: ChoiceError| error.to_string())
}

fn path(values: &Value, key: &str) -> Result<Option<PathBuf>, String> {
    match values.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.trim().is_empty() => Ok(None),
        Some(Value::String(text)) => Ok(Some(expand_home(Path::new(text)))),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;
    use muzik_core::paths::Paths;
    use serde_json::json;
    use std::fs;

    #[test]
    fn resolve_merges_saved_defaults_under_the_request() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let paths = Paths::under(dir.path());
        fs::create_dir_all(&paths.config)?;
        fs::write(
            paths.config_file(),
            "native_gui:\n  prefer: mp3\n  audio_source: soulseek\n  auto_decide: false\n",
        )?;
        let settings = Settings::resolve(&paths, &json!({"raw":" song ","prefer":"flac"}))?;
        assert_eq!(settings.request.raw, "song");
        assert_eq!(settings.request.output, paths.downloads());
        assert_eq!(settings.request.splits, paths.splits());
        assert_eq!(settings.options.prefer, "flac");
        assert_eq!(settings.options.audio_source.as_str(), "soulseek");
        assert_eq!(settings.agent_model, None);
        Ok(())
    }

    #[test]
    fn parse_reads_every_choice_and_expands_paths() -> Result<(), Box<dyn std::error::Error>> {
        let paths = Paths::under(std::path::Path::new("/state"));
        let settings = Settings::parse(
            &paths,
            &json!({
                "audio_source":"soulseek",
                "fallback":"none",
                "metadata_source":"musicbrainz",
                "quality_policy":"ask",
                "duplicates":"keep_all",
                "min_bitrate":192,
                "prefer":"flac",
                "output":"~/Music",
                "config":"",
                "auto_decide":true,
                "agent_model":" "
            }),
        )?;
        assert_eq!(settings.options.fallback.as_str(), "none");
        assert_eq!(settings.options.metadata_source.as_str(), "musicbrainz");
        assert_eq!(settings.options.quality_policy.as_str(), "ask");
        assert_eq!(settings.options.duplicates.as_str(), "keep_all");
        assert_eq!(settings.options.min_bitrate, 192);
        assert_eq!(settings.options.config, None);
        assert!(!settings.request.output.starts_with("~"));
        assert_eq!(
            settings.agent_model.as_deref(),
            Some(muzik_agent::DEFAULT_MODEL)
        );
        assert!(Settings::parse(&paths, &json!({"duplicates":"merge"})).is_err());
        assert!(Settings::parse(&paths, &json!({"force":"yes"})).is_err());
        Ok(())
    }
}
