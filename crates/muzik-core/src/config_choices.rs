//! Shared values for the workflow settings stored in config files.

use serde::{Deserialize, Serialize};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantNames};

/// An invalid value for a workflow setting.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid {setting}: {value}")]
pub struct ChoiceError {
    pub setting: &'static str,
    pub value: String,
}

/// Default audio preference for workflows and Soulseek searches.
pub const DEFAULT_AUDIO_PREFERENCE: &str = "lossless";

/// Common preferences shown in the desktop app. Custom format names remain valid.
pub const PREFERRED_AUDIO_CHOICES: &[&str] = &["lossless", "best", "mp3", "flac", "mp3-320", "any"];

macro_rules! config_choice {
    ($name:ident, $setting:literal, $error:ident) => {
        impl $name {
            /// Values accepted in config files and command-line arguments.
            pub const CHOICES: &'static [&'static str] = <Self as strum::VariantNames>::VARIANTS;

            pub fn as_str(self) -> &'static str {
                self.into()
            }
        }

        fn $error(value: &str) -> ChoiceError {
            ChoiceError {
                setting: $setting,
                value: value.to_owned(),
            }
        }
    };
}

/// Audio source for search and Spotify export tracks.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    usage::ValueEnum,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantNames,
)]
#[serde(rename_all = "lowercase")]
#[strum(
    serialize_all = "lowercase",
    parse_err_ty = ChoiceError,
    parse_err_fn = audio_source_error
)]
pub enum AudioSource {
    #[default]
    Youtube,
    Soulseek,
    Auto,
}

config_choice!(AudioSource, "audio source", audio_source_error);

/// Return the allowed values for a workflow config field.
pub fn choices_for_field(field: &str) -> Option<&'static [&'static str]> {
    match field {
        "audio_source" => Some(AudioSource::CHOICES),
        "fallback" => Some(AudioFallback::CHOICES),
        "metadata_source" => Some(MetadataSource::CHOICES),
        "quality_policy" => Some(QualityPolicy::CHOICES),
        "duplicates" => Some(DuplicatePolicy::CHOICES),
        _ => None,
    }
}

/// Source to try when Soulseek has no acceptable result.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    usage::ValueEnum,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantNames,
)]
#[serde(rename_all = "lowercase")]
#[strum(
    serialize_all = "lowercase",
    parse_err_ty = ChoiceError,
    parse_err_fn = audio_fallback_error
)]
pub enum AudioFallback {
    #[default]
    Youtube,
    None,
}

config_choice!(AudioFallback, "audio fallback", audio_fallback_error);

/// Source for track and chapter metadata.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    usage::ValueEnum,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantNames,
)]
#[serde(rename_all = "lowercase")]
#[strum(
    serialize_all = "lowercase",
    parse_err_ty = ChoiceError,
    parse_err_fn = metadata_source_error
)]
pub enum MetadataSource {
    None,
    Youtube,
    Musicbrainz,
    #[default]
    Auto,
}

config_choice!(MetadataSource, "metadata source", metadata_source_error);

/// Response to a measured audio quality gap.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    usage::ValueEnum,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantNames,
)]
#[serde(rename_all = "lowercase")]
#[strum(
    serialize_all = "lowercase",
    parse_err_ty = ChoiceError,
    parse_err_fn = quality_policy_error
)]
pub enum QualityPolicy {
    #[default]
    Off,
    Ask,
    Auto,
}

config_choice!(QualityPolicy, "quality policy", quality_policy_error);

/// Response to an album that is already in the library.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    usage::ValueEnum,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantNames,
)]
#[serde(rename_all = "snake_case")]
#[strum(
    serialize_all = "snake_case",
    parse_err_ty = ChoiceError,
    parse_err_fn = duplicate_policy_error
)]
pub enum DuplicatePolicy {
    #[default]
    Skip,
    Ask,
    KeepAll,
    RemoveOld,
}

config_choice!(DuplicatePolicy, "duplicate policy", duplicate_policy_error);

/// Audio formats that a sync target device receives.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Deserialize,
    Eq,
    Hash,
    PartialEq,
    Serialize,
    usage::ValueEnum,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantNames,
)]
#[serde(rename_all = "kebab-case")]
#[strum(
    serialize_all = "kebab-case",
    parse_err_ty = ChoiceError,
    parse_err_fn = sync_preset_error
)]
pub enum SyncPreset {
    /// Highest quality that the FiiO Snowsky Echo Mini plays.
    #[default]
    EchoMini,
    /// MP3 for all audio; MP3 files unchanged.
    Mp3,
    /// Opus for lossless audio; other lossy audio unchanged.
    Opus,
}

config_choice!(SyncPreset, "sync preset", sync_preset_error);

#[cfg(test)]
mod tests {
    use super::{
        AudioFallback, AudioSource, ChoiceError, DuplicatePolicy, MetadataSource, QualityPolicy,
        SyncPreset,
    };

    #[test]
    fn choices_parse_and_serialize_as_config_strings() -> Result<(), Box<dyn std::error::Error>> {
        macro_rules! check {
            ($type:ty) => {
                for &value in <$type>::CHOICES {
                    let parsed: $type = value.parse()?;
                    assert_eq!(parsed.to_string(), value);
                    assert_eq!(parsed.as_ref(), value);
                    assert_eq!(serde_json::to_value(parsed)?, value);
                    assert_eq!(
                        serde_json::from_value::<$type>(serde_json::json!(value))?,
                        parsed
                    );
                }
                assert!("invalid".parse::<$type>().is_err());
            };
        }
        check!(AudioSource);
        check!(AudioFallback);
        check!(MetadataSource);
        check!(QualityPolicy);
        check!(DuplicatePolicy);
        check!(SyncPreset);
        assert_eq!(
            "lossy".parse::<QualityPolicy>(),
            Err(ChoiceError {
                setting: "quality policy",
                value: "lossy".into(),
            })
        );
        Ok(())
    }

    #[test]
    fn defaults_match_workflow_config() {
        assert_eq!(AudioSource::default(), AudioSource::Youtube);
        assert_eq!(AudioFallback::default(), AudioFallback::Youtube);
        assert_eq!(MetadataSource::default(), MetadataSource::Auto);
        assert_eq!(QualityPolicy::default(), QualityPolicy::Off);
        assert_eq!(DuplicatePolicy::default(), DuplicatePolicy::Skip);
    }
}
