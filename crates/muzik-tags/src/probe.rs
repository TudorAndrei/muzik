//! Audio properties from lofty.

use std::fs::File;
use std::path::Path;

use lofty::config::ParseOptions;
use lofty::mp4::{Mp4Codec, Mp4File};
use muzik_core::audio::{AudioFormat, Codec};

use lofty::file::{AudioFile, FileType, TaggedFileExt};

use crate::TagsError;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioProperties {
    pub format: AudioFormat,
    pub codec: Option<Codec>,
    pub duration_seconds: Option<f64>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub size_bytes: u64,
}

/// # Errors
///
/// Returns an error when the format is not supported or the file cannot be read.
pub fn probe(path: impl AsRef<Path>) -> Result<AudioProperties, TagsError> {
    let path = path.as_ref();
    let audio = lofty::read_from_path(path)?;
    let properties = audio.properties();
    let duration = properties.duration().as_secs_f64();
    let file_type = audio.file_type();
    let format = match file_type {
        FileType::Mpeg => AudioFormat::Mp3,
        FileType::Flac => AudioFormat::Flac,
        FileType::Mp4 => AudioFormat::M4a,
        FileType::Opus => AudioFormat::Opus,
        FileType::Vorbis => AudioFormat::Ogg,
        FileType::Wav => AudioFormat::Wav,
        FileType::Aiff => AudioFormat::Aiff,
        FileType::WavPack => AudioFormat::WavPack,
        FileType::Ape => AudioFormat::Ape,
        FileType::Aac => AudioFormat::Aac,
        FileType::Mpc => AudioFormat::Mpc,
        FileType::Speex => AudioFormat::Speex,
        _ => return Err(TagsError::Unsupported(file_type)),
    };
    let codec = if file_type == FileType::Mp4 {
        let mut reader = File::open(path)?;
        match Mp4File::read_from(&mut reader, ParseOptions::default())?
            .properties()
            .codec()
        {
            Some(Mp4Codec::AAC) => Some(Codec::Aac),
            Some(Mp4Codec::ALAC) => Some(Codec::Alac),
            Some(Mp4Codec::FLAC) => Some(Codec::Flac),
            Some(Mp4Codec::MP3) => Some(Codec::Mp3),
            _ => None,
        }
    } else {
        None
    };
    Ok(AudioProperties {
        format,
        codec,
        duration_seconds: (duration > 0.0).then_some(duration),
        bitrate_kbps: properties
            .audio_bitrate()
            .or_else(|| properties.overall_bitrate()),
        sample_rate_hz: properties.sample_rate(),
        bit_depth: properties.bit_depth(),
        channels: properties.channels(),
        size_bytes: path.metadata()?.len(),
    })
}
