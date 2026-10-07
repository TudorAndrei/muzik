//! Shared values for the workflow settings stored in config files.

use crate::audio::AudioFormat;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantNames};

/// An invalid value for a workflow setting.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid {setting}: {value}")]
pub struct ChoiceError {
    pub setting: &'static str,
    pub value: String,
}

/// Audio quality that workflows and Soulseek searches prefer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PreferredAudio {
    #[default]
    Lossless,
    Best,
    Any,
    Mp3_320,
    Format(AudioFormat),
}

impl PreferredAudio {
    /// Common preferences shown in the desktop app. Any audio format name is also valid.
    pub const CHOICES: &'static [&'static str] =
        &["lossless", "best", "mp3", "flac", "mp3-320", "any"];

    /// Search term to add to a Soulseek query, unless the query already has it.
    pub fn search_suffix(self, tokens: &HashSet<String>) -> Option<String> {
        match self {
            Self::Lossless => (!tokens.contains("flac") && !tokens.contains("lossless"))
                .then(|| "flac".to_owned()),
            Self::Any => None,
            other => {
                let text = other.to_string();
                (!tokens.contains(&text)).then_some(text)
            }
        }
    }

    pub fn bonus(self, format: Option<AudioFormat>, lossless: bool, bitrate: Option<u32>) -> bool {
        match self {
            Self::Lossless => lossless,
            Self::Mp3_320 => format == Some(AudioFormat::Mp3) && bitrate == Some(320),
            Self::Format(wanted) => format == Some(wanted),
            Self::Best | Self::Any => false,
        }
    }
}

impl fmt::Display for PreferredAudio {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lossless => formatter.write_str("lossless"),
            Self::Best => formatter.write_str("best"),
            Self::Any => formatter.write_str("any"),
            Self::Mp3_320 => formatter.write_str("mp3-320"),
            Self::Format(format) => fmt::Display::fmt(format, formatter),
        }
    }
}

impl FromStr for PreferredAudio {
    type Err = ChoiceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "lossless" => Ok(Self::Lossless),
            "best" => Ok(Self::Best),
            "any" | "" => Ok(Self::Any),
            "mp3-320" => Ok(Self::Mp3_320),
            _ => value
                .parse::<AudioFormat>()
                .map(Self::Format)
                .map_err(|_| ChoiceError {
                    setting: "prefer",
                    value: value.to_owned(),
                }),
        }
    }
}

impl Serialize for PreferredAudio {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PreferredAudio {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

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
        AudioFallback, AudioSource, ChoiceError, DuplicatePolicy, MetadataSource, PreferredAudio,
        QualityPolicy, SyncPreset,
    };
    use crate::audio::AudioFormat;
    use std::collections::HashSet;

    #[test]
    fn preferred_audio_choices_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        for &value in PreferredAudio::CHOICES {
            let parsed: PreferredAudio = value.parse()?;
            assert_eq!(parsed.to_string(), value);
            assert_eq!(serde_json::to_value(parsed)?, value);
            assert_eq!(
                serde_json::from_value::<PreferredAudio>(serde_json::json!(value))?,
                parsed
            );
        }
        Ok(())
    }

    #[test]
    fn preferred_audio_parses_formats_and_rejects_unknown_text() {
        assert_eq!(
            "ogg".parse::<PreferredAudio>(),
            Ok(PreferredAudio::Format(AudioFormat::Ogg))
        );
        assert_eq!("".parse::<PreferredAudio>(), Ok(PreferredAudio::Any));
        let error = "banana".parse::<PreferredAudio>().unwrap_err();
        assert_eq!(error.to_string(), "invalid prefer: banana");
    }

    #[test]
    fn preferred_audio_adds_a_search_suffix_once() {
        let tokens = |words: &[&str]| -> HashSet<String> {
            words.iter().map(|word| (*word).to_owned()).collect()
        };
        assert_eq!(
            PreferredAudio::Lossless.search_suffix(&tokens(&[])),
            Some("flac".into())
        );
        assert_eq!(
            PreferredAudio::Lossless.search_suffix(&tokens(&["flac"])),
            None
        );
        assert_eq!(
            PreferredAudio::Best.search_suffix(&tokens(&[])),
            Some("best".into())
        );
        assert_eq!(PreferredAudio::Any.search_suffix(&tokens(&[])), None);
    }

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
