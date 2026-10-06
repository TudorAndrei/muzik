//! Measured audio quality for library scans and workflow decisions.

use muzik_core::audio::{AudioFormat, Codec};
use muzik_core::QualityPolicy;
use muzik_tags::AudioProperties;
use std::path::Path;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeasuredQuality {
    pub format: Codec,
    pub lossless: bool,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    pub size: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QualityDecision {
    Keep,
    Ask,
    Replace,
}

pub fn measure(path: &Path) -> Result<Option<MeasuredQuality>, String> {
    if let Ok(properties) = muzik_tags::probe(path) {
        return Ok(Some(MeasuredQuality::from(properties)));
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let format = match extension.as_str() {
        "dsf" => Codec::Dsd("dsd_lsbf_planar".into()),
        "dff" => Codec::Dsd("dsd_msbf".into()),
        "wma" => Codec::WmaV2,
        "tta" => Codec::Tta,
        _ => return Ok(None),
    };
    Ok(Some(MeasuredQuality {
        lossless: format.is_lossless(),
        format,
        bitrate_kbps: None,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        size: path.metadata().ok().map(|metadata| metadata.len()),
    }))
}

impl From<AudioProperties> for MeasuredQuality {
    fn from(properties: AudioProperties) -> Self {
        let depth = properties.bit_depth.unwrap_or(16);
        let format = match properties.format {
            AudioFormat::Mp3 => Codec::Mp3,
            AudioFormat::Flac => Codec::Flac,
            AudioFormat::M4a | AudioFormat::Mp4 | AudioFormat::Alac => properties
                .codec
                .unwrap_or_else(|| Codec::Other(properties.format.to_string())),
            AudioFormat::Opus => Codec::Opus,
            AudioFormat::Ogg => Codec::Vorbis,
            AudioFormat::Wav => Codec::Pcm(format!("pcm_s{depth}le")),
            AudioFormat::Aiff => Codec::Pcm(format!("pcm_s{depth}be")),
            AudioFormat::Ape => Codec::Ape,
            AudioFormat::WavPack => Codec::WavPack,
            AudioFormat::Aac => Codec::Aac,
            AudioFormat::Mpc => Codec::Other("musepack".into()),
            AudioFormat::Speex => Codec::Other("speex".into()),
        };
        Self {
            lossless: format.is_lossless(),
            format,
            bitrate_kbps: properties.bitrate_kbps,
            sample_rate: properties.sample_rate_hz,
            bit_depth: properties.bit_depth.map(u32::from),
            channels: properties.channels.map(u32::from),
            size: Some(properties.size_bytes),
        }
    }
}

pub fn decide(
    quality: &MeasuredQuality,
    policy: QualityPolicy,
    min_bitrate_kbps: u32,
) -> QualityDecision {
    if policy == QualityPolicy::Off
        || quality.lossless
        || quality
            .bitrate_kbps
            .is_some_and(|bitrate| bitrate >= min_bitrate_kbps)
    {
        return QualityDecision::Keep;
    }
    match policy {
        QualityPolicy::Off => QualityDecision::Keep,
        QualityPolicy::Ask => QualityDecision::Ask,
        QualityPolicy::Auto => QualityDecision::Replace,
    }
}

#[cfg(test)]
mod tests {
    use super::{decide, measure, MeasuredQuality, QualityDecision};
    use muzik_core::audio::{AudioFormat, Codec};
    use muzik_core::QualityPolicy;
    use muzik_tags::AudioProperties;

    fn properties(format: AudioFormat, codec: Option<Codec>) -> AudioProperties {
        AudioProperties {
            format,
            codec,
            duration_seconds: Some(60.0),
            bitrate_kbps: Some(192),
            sample_rate_hz: Some(44_100),
            bit_depth: None,
            channels: Some(2),
            size_bytes: 500,
        }
    }

    #[test]
    fn lossy_audio_below_the_minimum_follows_the_policy() {
        let measured = MeasuredQuality::from(properties(AudioFormat::Mp3, None));
        assert_eq!(measured.format, Codec::Mp3);
        assert_eq!(measured.bitrate_kbps, Some(192));
        assert_eq!(measured.sample_rate, Some(44_100));
        assert_eq!(measured.size, Some(500));
        assert_eq!(
            decide(&measured, QualityPolicy::Ask, 256),
            QualityDecision::Ask
        );
        assert_eq!(
            decide(&measured, QualityPolicy::Auto, 256),
            QualityDecision::Replace
        );
        assert_eq!(
            decide(&measured, QualityPolicy::Off, 256),
            QualityDecision::Keep
        );
    }

    #[test]
    fn lossless_audio_needs_no_replacement() {
        let measured = MeasuredQuality::from(properties(AudioFormat::M4a, Some(Codec::Alac)));
        assert!(measured.lossless);
        assert_eq!(
            decide(&measured, QualityPolicy::Auto, 320),
            QualityDecision::Keep
        );
    }

    #[test]
    fn big_endian_pcm_from_aiff_is_lossless() {
        let measured = MeasuredQuality::from(properties(AudioFormat::Aiff, None));
        assert_eq!(measured.format, Codec::Pcm("pcm_s16be".into()));
        assert!(measured.lossless);
    }

    #[test]
    fn formats_lofty_cannot_read_are_named_from_the_extension() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let dsd = directory.path().join("track.DSF");
        std::fs::write(&dsd, b"not audio").map_err(|error| error.to_string())?;
        let measured = measure(&dsd)?.ok_or("a DSF file must have a quality")?;
        assert!(measured.lossless);
        assert_eq!(measured.size, Some(9));

        let broken = directory.path().join("track.mp3");
        std::fs::write(&broken, b"not audio").map_err(|error| error.to_string())?;
        assert_eq!(measure(&broken)?, None);
        Ok(())
    }
}
