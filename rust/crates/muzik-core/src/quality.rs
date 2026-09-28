//! Measured audio quality for library scans and workflow decisions.

use crate::QualityPolicy;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MeasuredQuality {
    pub format: String,
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
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|error| format!("cannot start ffprobe: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let document: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("invalid ffprobe output: {error}"))?;
    Ok(from_probe(
        &document,
        path.metadata().ok().map(|value| value.len()),
    ))
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

fn from_probe(document: &Value, size: Option<u64>) -> Option<MeasuredQuality> {
    let audio = document["streams"]
        .as_array()?
        .iter()
        .find(|stream| stream["codec_type"] == "audio")?;
    let format = audio["codec_name"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    let bitrate_kbps = number(&audio["bit_rate"])
        .or_else(|| number(&document["format"]["bit_rate"]))
        .and_then(|value| u32::try_from(value / 1000).ok());
    Some(MeasuredQuality {
        lossless: matches!(
            format.as_str(),
            "flac"
                | "alac"
                | "wav"
                | "pcm_s16le"
                | "pcm_s24le"
                | "pcm_s32le"
                | "aiff"
                | "ape"
                | "wavpack"
        ),
        format,
        bitrate_kbps,
        sample_rate: number(&audio["sample_rate"]).and_then(|value| u32::try_from(value).ok()),
        bit_depth: number(&audio["bits_per_raw_sample"])
            .or_else(|| number(&audio["bits_per_sample"]))
            .and_then(|value| u32::try_from(value).ok()),
        channels: number(&audio["channels"]).and_then(|value| u32::try_from(value).ok()),
        size,
    })
}

fn number(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

#[cfg(test)]
mod tests {
    use super::{decide, from_probe, QualityDecision};
    use crate::QualityPolicy;
    use serde_json::json;

    #[test]
    fn probe_uses_audio_stream_and_container_bitrate() {
        let document = json!({"streams":[
            {"codec_type":"video","codec_name":"h264"},
            {"codec_type":"audio","codec_name":"mp3","sample_rate":"44100","channels":2}
        ],"format":{"bit_rate":"192000"}});
        let measured = from_probe(&document, Some(500)).unwrap();
        assert_eq!(measured.format, "mp3");
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
        let measured = from_probe(
            &json!({"streams":[{"codec_type":"audio","codec_name":"flac"}]}),
            None,
        )
        .unwrap();
        assert_eq!(
            decide(&measured, QualityPolicy::Auto, 320),
            QualityDecision::Keep
        );
        assert!(measured.lossless);
    }
}
