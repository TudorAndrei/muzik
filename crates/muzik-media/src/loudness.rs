//! EBU R128 loudness and `ReplayGain` 2.0 values for audio files.

use ebur128::{EbuR128, Mode};
use rayon::prelude::*;
use std::fs::File;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use thiserror::Error;

pub const REFERENCE_LUFS: f64 = -18.0;

#[derive(Debug, Error)]
pub enum LoudnessError {
    #[error("cannot open audio file: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot decode audio: {0}")]
    Decode(#[from] SymphoniaError),
    #[error("the file has no audio track")]
    NoTrack,
    #[error("the audio format changes inside the file")]
    FormatChange,
    #[error("cannot measure loudness: {0}")]
    Meter(#[from] ebur128::Error),
    #[error("the audio is too short or too quiet to measure")]
    Silent,
    #[error("the decoder stopped on malformed data")]
    Malformed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gain {
    pub loudness: f64,
    pub gain: f64,
    pub peak: f64,
}

impl Gain {
    fn new(loudness: f64, peak: f64) -> Result<Self, LoudnessError> {
        if !loudness.is_finite() || !peak.is_finite() {
            return Err(LoudnessError::Silent);
        }
        Ok(Self {
            loudness,
            gain: REFERENCE_LUFS - loudness,
            peak,
        })
    }

    #[must_use]
    pub fn gain_text(self) -> String {
        format!("{:.2} dB", self.gain)
    }

    #[must_use]
    pub fn peak_text(self) -> String {
        format!("{:.6}", self.peak)
    }
}

pub struct Measurement {
    meter: EbuR128,
    track: Gain,
}

impl Measurement {
    #[must_use]
    pub const fn track(&self) -> Gain {
        self.track
    }
}

/// Measure integrated loudness and true peak of one file.
///
/// # Errors
/// Returns an error when the file cannot be decoded or holds no measurable audio.
pub fn measure(path: &Path) -> Result<Measurement, LoudnessError> {
    catch_unwind(AssertUnwindSafe(|| decode(path))).map_err(|_| LoudnessError::Malformed)?
}

#[must_use]
pub fn measure_all(paths: &[PathBuf]) -> Vec<Result<Measurement, LoudnessError>> {
    paths.par_iter().map(|path| measure(path)).collect()
}

/// Gated loudness over all tracks together and the highest track peak.
///
/// # Errors
/// Returns an error when there are no tracks or the combined audio is silent.
pub fn album(tracks: &[Measurement]) -> Result<Gain, LoudnessError> {
    let loudness = EbuR128::loudness_global_multiple(tracks.iter().map(|track| &track.meter))?;
    let peak = tracks
        .iter()
        .map(|track| track.track.peak)
        .reduce(f64::max)
        .ok_or(LoudnessError::Silent)?;
    Gain::new(loudness, peak)
}

fn decode(path: &Path) -> Result<Measurement, LoudnessError> {
    let file = File::open(path)?;
    let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        stream,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or(LoudnessError::NoTrack)?;
    let track_id = track.id;
    let Some(CodecParameters::Audio(params)) = &track.codec_params else {
        return Err(LoudnessError::NoTrack);
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())?;
    let mut meter: Option<(EbuR128, u32, usize)> = None;
    let mut samples: Vec<f32> = Vec::new();
    while let Some(packet) = format.next_packet()? {
        if packet.track_id != track_id {
            continue;
        }
        let buffer = match decoder.decode(&packet) {
            Ok(buffer) => buffer,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(error.into()),
        };
        let rate = buffer.spec().rate();
        let channels = buffer.spec().channels().count();
        if meter.is_none() {
            let count = u32::try_from(channels).map_err(|_| LoudnessError::FormatChange)?;
            let mode = Mode::I | Mode::TRUE_PEAK | Mode::HISTOGRAM;
            meter = Some((EbuR128::new(count, rate, mode)?, rate, channels));
        }
        let Some(state) = &mut meter else {
            return Err(LoudnessError::Silent);
        };
        if state.1 != rate || state.2 != channels {
            return Err(LoudnessError::FormatChange);
        }
        buffer.copy_to_vec_interleaved(&mut samples);
        state.0.add_frames_f32(&samples)?;
    }
    let (meter, _, _) = meter.ok_or(LoudnessError::Silent)?;
    let mut peak = 0.0_f64;
    for channel in 0..meter.channels() {
        peak = peak.max(meter.true_peak(channel)?);
    }
    let track = Gain::new(meter.loudness_global()?, peak)?;
    Ok(Measurement { meter, track })
}

#[cfg(test)]
mod tests {
    use super::{Gain, LoudnessError, REFERENCE_LUFS, album, measure, measure_all};
    use std::f64::consts::TAU;
    use std::path::Path;

    const RATE: u32 = 48_000;

    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        reason = "std has no checked float to integer conversion; the amplitude is at most 1"
    )]
    fn quantize(value: f64) -> i16 {
        (value.clamp(-1.0, 1.0) * f64::from(i16::MAX)).round() as i16
    }

    fn sine_wav(path: &Path, amplitude: f64, seconds: u32, channels: u16) {
        let mut data = Vec::new();
        for frame in 0..RATE.saturating_mul(seconds) {
            let phase = TAU * 1000.0 * f64::from(frame) / f64::from(RATE);
            let sample = quantize(amplitude * phase.sin());
            for _ in 0..channels {
                data.extend_from_slice(&sample.to_le_bytes());
            }
        }
        let data_len = u32::try_from(data.len()).unwrap();
        let block = channels.saturating_mul(2);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&data_len.saturating_add(36).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&RATE.to_le_bytes());
        bytes.extend_from_slice(&RATE.saturating_mul(u32::from(block)).to_le_bytes());
        bytes.extend_from_slice(&block.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.extend_from_slice(&data);
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn flac_mp3_m4a_and_vorbis_decode_but_opus_does_not() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../muzik-tags/tests/fixtures");
        for name in ["blank.flac", "blank.mp3", "blank.m4a", "blank.ogg"] {
            let outcome = measure(&fixtures.join(name));
            assert!(
                matches!(outcome, Ok(_) | Err(LoudnessError::Silent)),
                "{name}: {:?}",
                outcome.err()
            );
        }
        assert!(matches!(
            measure(&fixtures.join("blank.opus")),
            Err(LoudnessError::Decode(_))
        ));
    }

    fn close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} is not within {tolerance} of {expected}"
        );
    }

    #[test]
    fn a_sine_gets_the_gain_to_the_reference_level() {
        let dir = tempfile::tempdir().unwrap();
        let stereo = dir.path().join("stereo.wav");
        let mono = dir.path().join("mono.wav");
        sine_wav(&stereo, 0.5, 2, 2);
        sine_wav(&mono, 0.1, 2, 1);
        let stereo = measure(&stereo).unwrap().track();
        close(stereo.loudness, -6.02, 0.1);
        close(stereo.gain, REFERENCE_LUFS + 6.02, 0.1);
        close(stereo.peak, 0.5, 0.01);
        let mono = measure(&mono).unwrap().track();
        close(mono.gain, REFERENCE_LUFS + 23.01, 0.1);
        close(mono.peak, 0.1, 0.01);
    }

    #[test]
    fn album_loudness_is_gated_over_all_tracks_together() {
        let dir = tempfile::tempdir().unwrap();
        let loud = dir.path().join("loud.wav");
        let quiet = dir.path().join("quiet.wav");
        sine_wav(&loud, 0.5, 2, 2);
        sine_wav(&quiet, 0.25, 2, 2);
        let tracks: Vec<_> = measure_all(&[loud, quiet])
            .into_iter()
            .map(Result::unwrap)
            .collect();
        let gain = album(&tracks).unwrap();
        close(
            gain.loudness,
            10.0 * f64::log10(f64::midpoint(0.25, 0.0625)),
            0.1,
        );
        close(gain.gain, REFERENCE_LUFS - gain.loudness, 1e-9);
        close(gain.peak, 0.5, 0.01);
        assert!(matches!(
            album(&[]),
            Err(LoudnessError::Meter(_) | LoudnessError::Silent)
        ));
    }

    #[test]
    fn values_use_the_replaygain_text_format() {
        let gain = Gain::new(-11.48, 0.988_547_3).unwrap();
        assert_eq!(gain.gain_text(), "-6.52 dB");
        assert_eq!(gain.peak_text(), "0.988547");
    }

    #[test]
    fn silence_and_garbage_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let silent = dir.path().join("silent.wav");
        sine_wav(&silent, 0.0, 1, 2);
        assert!(matches!(measure(&silent), Err(LoudnessError::Silent)));
        let garbage = dir.path().join("garbage.flac");
        std::fs::write(&garbage, b"not audio at all").unwrap();
        assert!(measure(&garbage).is_err());
        assert!(matches!(
            measure(&dir.path().join("missing.mp3")),
            Err(LoudnessError::Io(_))
        ));
    }
}
