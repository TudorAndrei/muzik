use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantArray};

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantArray,
    Serialize,
    Deserialize,
)]
#[strum(ascii_case_insensitive)]
pub enum AudioFormat {
    #[strum(to_string = "mp3")]
    #[serde(rename = "mp3")]
    Mp3,
    #[strum(to_string = "flac")]
    #[serde(rename = "flac")]
    Flac,
    #[strum(to_string = "m4a")]
    #[serde(rename = "m4a")]
    M4a,
    #[strum(to_string = "mp4")]
    #[serde(rename = "mp4")]
    Mp4,
    #[strum(to_string = "opus")]
    #[serde(rename = "opus")]
    Opus,
    #[strum(to_string = "ogg")]
    #[serde(rename = "ogg")]
    Ogg,
    #[strum(to_string = "wav")]
    #[serde(rename = "wav")]
    Wav,
    #[strum(to_string = "aiff", serialize = "aif")]
    #[serde(rename = "aiff", alias = "aif")]
    Aiff,
    #[strum(to_string = "ape")]
    #[serde(rename = "ape")]
    Ape,
    #[strum(to_string = "wv")]
    #[serde(rename = "wv")]
    WavPack,
    #[strum(to_string = "aac")]
    #[serde(rename = "aac")]
    Aac,
    #[strum(to_string = "alac")]
    #[serde(rename = "alac")]
    Alac,
    #[strum(to_string = "mpc")]
    #[serde(rename = "mpc")]
    Mpc,
    #[strum(to_string = "spx")]
    #[serde(rename = "spx")]
    Speex,
}

impl AudioFormat {
    pub fn from_extension(extension: &str) -> Option<Self> {
        extension.parse().ok()
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        path.extension()
            .and_then(|extension| extension.to_str())
            .and_then(Self::from_extension)
    }

    pub fn is_lossless(self) -> bool {
        matches!(
            self,
            Self::Flac | Self::Alac | Self::Wav | Self::Aiff | Self::Ape | Self::WavPack
        )
    }
}

pub fn is_audio(path: &Path) -> bool {
    AudioFormat::from_path(path).is_some()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    Mp3,
    Aac,
    Opus,
    Vorbis,
    Flac,
    Alac,
    Ape,
    WavPack,
    Tta,
    WmaV1,
    WmaV2,
    Pcm(String),
    Dsd(String),
    Other(String),
}

impl Codec {
    pub fn from_ffprobe(name: &str) -> Self {
        match name {
            "mp3" => Self::Mp3,
            "aac" => Self::Aac,
            "opus" => Self::Opus,
            "vorbis" => Self::Vorbis,
            "flac" => Self::Flac,
            "alac" => Self::Alac,
            "ape" => Self::Ape,
            "wavpack" => Self::WavPack,
            "tta" => Self::Tta,
            "wmav1" => Self::WmaV1,
            "wmav2" => Self::WmaV2,
            _ if name.starts_with("pcm_") => Self::Pcm(name.to_owned()),
            _ if name.starts_with("dsd_") => Self::Dsd(name.to_owned()),
            _ => Self::Other(name.to_owned()),
        }
    }

    pub fn is_lossless(&self) -> bool {
        matches!(
            self,
            Self::Flac
                | Self::Alac
                | Self::Ape
                | Self::WavPack
                | Self::Tta
                | Self::Pcm(_)
                | Self::Dsd(_)
        )
    }

    pub fn audio_format(&self) -> Option<AudioFormat> {
        match self {
            Self::Mp3 => Some(AudioFormat::Mp3),
            Self::Aac => Some(AudioFormat::Aac),
            Self::Opus => Some(AudioFormat::Opus),
            Self::Flac => Some(AudioFormat::Flac),
            Self::Alac => Some(AudioFormat::Alac),
            Self::Ape => Some(AudioFormat::Ape),
            _ => None,
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Mp3 => "mp3",
            Self::Aac => "aac",
            Self::Opus => "opus",
            Self::Vorbis => "vorbis",
            Self::Flac => "flac",
            Self::Alac => "alac",
            Self::Ape => "ape",
            Self::WavPack => "wavpack",
            Self::Tta => "tta",
            Self::WmaV1 => "wmav1",
            Self::WmaV2 => "wmav2",
            Self::Pcm(name) | Self::Dsd(name) | Self::Other(name) => name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::VariantArray;

    #[test]
    fn audio_files_include_ogg_and_aiff() {
        assert!(is_audio(Path::new("a/b.OGG")));
        assert!(is_audio(Path::new("x.aiff")));
        assert!(is_audio(Path::new("x.flac")));
        assert!(!is_audio(Path::new("x.jpg")));
        assert!(!is_audio(Path::new("x")));
        assert!(!is_audio(Path::new("x.chapters.txt")));
    }

    #[test]
    fn aiff_accepts_both_extensions() {
        for text in ["AIF", "aif", "aiff"] {
            assert_eq!(AudioFormat::from_extension(text), Some(AudioFormat::Aiff));
        }
        assert_eq!(AudioFormat::Aiff.to_string(), "aiff");
    }

    #[test]
    fn formats_round_trip_through_text_and_serde() -> Result<(), Box<dyn std::error::Error>> {
        for &format in AudioFormat::VARIANTS {
            let text = format.to_string();
            assert_eq!(text.parse::<AudioFormat>()?, format);
            assert_eq!(serde_json::to_value(format)?, text);
            assert_eq!(serde_json::from_value::<AudioFormat>(text.into())?, format);
        }
        assert_eq!(
            serde_json::from_value::<AudioFormat>("aif".into())?,
            AudioFormat::Aiff
        );
        Ok(())
    }

    #[test]
    fn formats_come_from_paths() {
        assert_eq!(
            AudioFormat::from_path(Path::new("x.OGG")),
            Some(AudioFormat::Ogg)
        );
        assert_eq!(AudioFormat::from_path(Path::new("x.jpg")), None);
    }

    #[test]
    fn lossless_formats_are_exactly_the_lossless_set() {
        let lossless: Vec<_> = AudioFormat::VARIANTS
            .iter()
            .copied()
            .filter(|format| format.is_lossless())
            .collect();
        assert_eq!(
            lossless,
            [
                AudioFormat::Flac,
                AudioFormat::Wav,
                AudioFormat::Aiff,
                AudioFormat::Ape,
                AudioFormat::WavPack,
                AudioFormat::Alac,
            ]
        );
    }

    #[test]
    fn codec_names_survive_a_round_trip() {
        for name in ["pcm_s16be", "dsd_msbf", "wmav2", "wavpack", "vp9", "mp3"] {
            assert_eq!(Codec::from_ffprobe(name).to_string(), name);
        }
    }

    #[test]
    fn lossless_codecs_cover_pcm_and_dsd() {
        for name in [
            "flac",
            "pcm_s16be",
            "pcm_s24le",
            "dsd_lsbf_planar",
            "wavpack",
            "tta",
        ] {
            assert!(Codec::from_ffprobe(name).is_lossless(), "{name}");
        }
        for name in ["mp3", "aac", "opus", "vorbis"] {
            assert!(!Codec::from_ffprobe(name).is_lossless(), "{name}");
        }
    }

    #[test]
    fn codecs_map_to_formats_only_when_names_agree() {
        assert_eq!(Codec::Vorbis.audio_format(), None);
        assert_eq!(Codec::Flac.audio_format(), Some(AudioFormat::Flac));
    }
}
