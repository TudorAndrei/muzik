//! Shared values for the workflow settings stored in config files.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

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
    (
        $(#[$doc:meta])*
        $name:ident, $setting:literal, $default:ident,
        $( $variant:ident => $value:literal ),+ $(,)?
    ) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize, usage::ValueEnum)]
        #[serde(rename_all = "lowercase")]
        pub enum $name {
            $( $variant, )+
        }

        impl $name {
            /// Values accepted in config files and command-line arguments.
            pub const CHOICES: &'static [&'static str] = &[$( $value, )+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::$default
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ChoiceError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $( $value => Ok(Self::$variant), )+
                    _ => Err(ChoiceError {
                        setting: $setting,
                        value: value.to_owned(),
                    }),
                }
            }
        }
    };
}

config_choice! {
    /// Audio source for search and Spotify export tracks.
    AudioSource, "audio source", Youtube,
    Youtube => "youtube",
    Soulseek => "soulseek",
    Auto => "auto",
}

/// Return the allowed values for a workflow config field.
pub fn choices_for_field(field: &str) -> Option<&'static [&'static str]> {
    match field {
        "audio_source" => Some(AudioSource::CHOICES),
        "fallback" => Some(AudioFallback::CHOICES),
        "metadata_source" => Some(MetadataSource::CHOICES),
        "quality_policy" => Some(QualityPolicy::CHOICES),
        _ => None,
    }
}

config_choice! {
    /// Source to try when Soulseek has no acceptable result.
    AudioFallback, "audio fallback", Youtube,
    Youtube => "youtube",
    None => "none",
}

config_choice! {
    /// Source for track and chapter metadata.
    MetadataSource, "metadata source", Auto,
    None => "none",
    Youtube => "youtube",
    Musicbrainz => "musicbrainz",
    Auto => "auto",
}

config_choice! {
    /// Response to a measured audio quality gap.
    QualityPolicy, "quality policy", Off,
    Off => "off",
    Ask => "ask",
    Auto => "auto",
}

#[cfg(test)]
mod tests {
    use super::{AudioFallback, AudioSource, MetadataSource, QualityPolicy};

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
        Ok(())
    }

    #[test]
    fn defaults_match_workflow_config() {
        assert_eq!(AudioSource::default(), AudioSource::Youtube);
        assert_eq!(AudioFallback::default(), AudioFallback::Youtube);
        assert_eq!(MetadataSource::default(), MetadataSource::Auto);
        assert_eq!(QualityPolicy::default(), QualityPolicy::Off);
    }
}
