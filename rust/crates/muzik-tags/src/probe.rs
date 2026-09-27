//! Audio properties from lofty.

use std::fs::File;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::mp4::{Mp4Codec, Mp4File};

use lofty::file::{AudioFile, FileType, TaggedFileExt};

use crate::TagsError;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioProperties {
    pub format: String,
    pub codec: String,
    pub duration_seconds: Option<f64>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub size_bytes: u64,
}

pub fn probe(path: impl AsRef<Path>) -> Result<AudioProperties, TagsError> {
    let path = path.as_ref();
    let audio = lofty::read_from_path(path)?;
    let properties = audio.properties();
    let duration = properties.duration().as_secs_f64();
    let format = match audio.file_type() {
        FileType::Mpeg => "mp3",
        FileType::Flac => "flac",
        FileType::Mp4 => "m4a",
        FileType::Opus => "opus",
        FileType::Vorbis => "ogg",
        FileType::Wav => "wav",
        FileType::Aiff => "aiff",
        FileType::WavPack => "wv",
        FileType::Ape => "ape",
        FileType::Aac => "aac",
        FileType::Mpc => "mpc",
        FileType::Speex => "spx",
        FileType::Custom(name) => name,
        _ => "unknown",
    };
    let codec = if audio.file_type() == FileType::Mp4 {
        let mut reader = File::open(path)?;
        match Mp4File::read_from(&mut reader, ParseOptions::default())?
            .properties()
            .codec()
        {
            Some(Mp4Codec::AAC) => "aac",
            Some(Mp4Codec::ALAC) => "alac",
            Some(Mp4Codec::FLAC) => "flac",
            Some(Mp4Codec::MP3) => "mp3",
            _ => "m4a",
        }
    } else {
        format
    };
    Ok(AudioProperties {
        format: format.into(),
        codec: codec.into(),
        duration_seconds: (duration > 0.0).then_some(duration),
        bitrate_kbps: properties.audio_bitrate().or(properties.overall_bitrate()),
        sample_rate_hz: properties.sample_rate(),
        bit_depth: properties.bit_depth(),
        channels: properties.channels(),
        size_bytes: path.metadata()?.len(),
    })
}
