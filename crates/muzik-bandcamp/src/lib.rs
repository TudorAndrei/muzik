//! Bandcamp collection access with cookies exported from a browser.

use serde::{Deserialize, Serialize};
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantNames};

mod collection;
mod download;
mod login;

pub use collection::{Purchase, collection};
pub use download::{audio_files, download};
pub use login::{Cookie, Login, status};

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:128.0) Gecko/20100101 Firefox/128.0";
const PAGE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}

/// Download formats that Bandcamp offers for a purchase.
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
    usage_rs::ValueEnum,
    AsRefStr,
    Display,
    EnumString,
    IntoStaticStr,
    VariantNames,
)]
pub enum BandcampFormat {
    #[default]
    #[serde(rename = "flac")]
    #[strum(to_string = "flac")]
    #[usage(name = "flac")]
    Flac,
    #[serde(rename = "wav")]
    #[strum(to_string = "wav")]
    #[usage(name = "wav")]
    Wav,
    #[serde(rename = "aac-hi")]
    #[strum(to_string = "aac-hi")]
    #[usage(name = "aac-hi")]
    AacHi,
    #[serde(rename = "mp3-320")]
    #[strum(to_string = "mp3-320")]
    #[usage(name = "mp3-320")]
    Mp3_320,
    #[serde(rename = "aiff-lossless")]
    #[strum(to_string = "aiff-lossless")]
    #[usage(name = "aiff-lossless")]
    AiffLossless,
    #[serde(rename = "vorbis")]
    #[strum(to_string = "vorbis")]
    #[usage(name = "vorbis")]
    Vorbis,
    #[serde(rename = "mp3-v0")]
    #[strum(to_string = "mp3-v0")]
    #[usage(name = "mp3-v0")]
    Mp3V0,
    #[serde(rename = "alac")]
    #[strum(to_string = "alac")]
    #[usage(name = "alac")]
    Alac,
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::VariantNames;
    #[test]
    fn formats_round_trip_through_their_bandcamp_names() {
        assert_eq!(
            BandcampFormat::VARIANTS,
            [
                "flac",
                "wav",
                "aac-hi",
                "mp3-320",
                "aiff-lossless",
                "vorbis",
                "mp3-v0",
                "alac"
            ]
        );
        for &name in BandcampFormat::VARIANTS {
            let format: BandcampFormat = name.parse().unwrap();
            assert_eq!(format.as_ref(), name);
            assert_eq!(format.to_string(), name);
            assert_eq!(serde_json::to_value(format).unwrap(), name);
        }
        assert_eq!(BandcampFormat::default(), BandcampFormat::Flac);
    }
}
