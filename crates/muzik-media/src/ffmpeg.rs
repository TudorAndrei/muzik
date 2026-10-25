//! Typed ffmpeg commands for splitting and converting audio.

use crate::process::{self, Stopped, background_command};
use std::fs;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Encoding {
    Mp3 {
        kbps: u32,
    },
    Opus {
        kbps: u32,
    },
    Flac {
        sample_rate: Option<u32>,
        bit_depth: Option<u32>,
    },
}

impl Encoding {
    #[must_use]
    pub const fn extension(&self) -> &'static str {
        match self {
            Self::Mp3 { .. } => "mp3",
            Self::Opus { .. } => "opus",
            Self::Flac { .. } => "flac",
        }
    }
}

/// Copy the audio between `start` and `end` seconds without re-encoding.
pub struct Cut<'a> {
    pub source: &'a Path,
    pub destination: &'a Path,
    pub start: i64,
    /// `None` keeps all audio after `start`.
    pub end: Option<i64>,
    /// Replaces all source tags.
    pub tags: &'a TrackTags,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrackTags {
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub date: String,
    pub track: u32,
    pub track_total: usize,
    pub compilation: bool,
}

impl TrackTags {
    fn metadata_args(&self) -> [String; 7] {
        [
            format!("title={}", self.title),
            format!("artist={}", self.artist),
            format!("albumartist={}", self.album_artist),
            format!("album={}", self.album),
            format!("date={}", self.date),
            format!("track={}/{}", self.track, self.track_total),
            format!("compilation={}", u8::from(self.compilation)),
        ]
    }
}

/// Re-encode the first audio stream and keep the source tags.
pub struct Convert<'a> {
    pub source: &'a Path,
    pub destination: &'a Path,
    pub encoding: &'a Encoding,
    pub tags_in_stream: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ffmpeg cancelled")]
    Cancelled,
    #[error("cannot run ffmpeg: {0}")]
    Run(#[source] io::Error),
    #[error("ffmpeg failed: {0}")]
    Failed(String),
}

pub struct Ffmpeg {
    executable: PathBuf,
}

impl Default for Ffmpeg {
    fn default() -> Self {
        Self::at("ffmpeg")
    }
}

impl Ffmpeg {
    pub fn at(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    /// Stop ffmpeg and remove the partial file when `cancelled` becomes true.
    pub(crate) fn cut(&self, cut: &Cut<'_>, cancelled: &AtomicBool) -> Result<(), Error> {
        let mut errors = tempfile::tempfile().map_err(Error::Run)?;
        let mut command = background_command(&self.executable);
        command
            .arg("-i")
            .arg(cut.source)
            .args(["-nostdin", "-v", "error", "-y", "-ss"])
            .arg(timestamp(cut.start));
        if let Some(end) = cut.end {
            command.arg("-to").arg(timestamp(end));
        }
        command.args(["-vn", "-c:a", "copy", "-map_metadata", "-1"]);
        for tag in cut.tags.metadata_args() {
            command.arg("-metadata").arg(tag);
        }
        command
            .arg(cut.destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(errors.try_clone().map_err(Error::Run)?);
        let mut child = process::spawn(command).map_err(Error::Run)?;
        let status = match process::wait(&mut child, None, cancelled) {
            Ok(status) => status,
            Err(Stopped::Io(error)) => return Err(Error::Run(error)),
            Err(Stopped::Cancelled | Stopped::TimedOut) => {
                let _ = fs::remove_file(cut.destination);
                return Err(Error::Cancelled);
            }
        };
        if status.success() {
            return Ok(());
        }
        let mut text = String::new();
        errors
            .rewind()
            .and_then(|()| errors.read_to_string(&mut text))
            .map_err(Error::Run)?;
        let text = text.trim();
        Err(Error::Failed(if text.is_empty() {
            status.to_string()
        } else {
            text.to_owned()
        }))
    }

    /// # Errors
    /// Returns an error when ffmpeg cannot run or exits with a failure.
    pub fn convert(&self, convert: &Convert<'_>) -> Result<(), Error> {
        let mut command = background_command(&self.executable);
        command
            .args(["-nostdin", "-v", "error", "-y", "-i"])
            .arg(convert.source)
            .args(["-map", "0:a:0", "-map_metadata"])
            .arg(if convert.tags_in_stream {
                "0:s:a:0"
            } else {
                "0"
            });
        match convert.encoding {
            Encoding::Mp3 { kbps } => {
                command
                    .args(["-c:a", "libmp3lame", "-b:a"])
                    .arg(format!("{kbps}k"))
                    .args(["-id3v2_version", "3", "-f", "mp3"]);
            }
            Encoding::Opus { kbps } => {
                command
                    .args(["-c:a", "libopus", "-b:a"])
                    .arg(format!("{kbps}k"))
                    .args(["-f", "opus"]);
            }
            Encoding::Flac {
                sample_rate,
                bit_depth,
            } => {
                command.args(["-c:a", "flac"]);
                if let Some(rate) = sample_rate {
                    command.arg("-ar").arg(rate.to_string());
                }
                if let Some(depth) = bit_depth {
                    command
                        .args(["-sample_fmt", "s32", "-bits_per_raw_sample"])
                        .arg(depth.to_string());
                }
                command.args(["-f", "flac"]);
            }
        }
        let output = command
            .arg(convert.destination)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(Error::Run)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Error::Failed(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }
}

fn timestamp(seconds: i64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_reencodes_audio_and_keeps_the_title() {
        let temp = tempfile::tempdir().unwrap();
        let ffmpeg = Ffmpeg::default();
        let sources = [("flac", false), ("opus", true)];
        for (format, tags_in_stream) in sources {
            let source = temp.path().join(format!("source.{format}"));
            let made = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-f", "lavfi", "-i", "sine=duration=1"])
                .args(["-metadata", "title=Song", "-y"])
                .arg(&source)
                .output()
                .unwrap();
            assert!(made.status.success());
            for encoding in [
                Encoding::Mp3 { kbps: 128 },
                Encoding::Opus { kbps: 96 },
                Encoding::Flac {
                    sample_rate: Some(44_100),
                    bit_depth: Some(16),
                },
            ] {
                let destination = temp
                    .path()
                    .join(format!("{format}-to.{}", encoding.extension()));
                ffmpeg
                    .convert(&Convert {
                        source: &source,
                        destination: &destination,
                        encoding: &encoding,
                        tags_in_stream,
                    })
                    .unwrap();
                let tags = muzik_tags::read(&destination, &[]).unwrap();
                assert_eq!(tags.fields.get("title").map(String::as_str), Some("Song"));
            }
        }
    }

    #[test]
    fn convert_reports_the_ffmpeg_error() {
        let temp = tempfile::tempdir().unwrap();
        let result = Ffmpeg::default().convert(&Convert {
            source: &temp.path().join("missing.flac"),
            destination: &temp.path().join("out.mp3"),
            encoding: &Encoding::Mp3 { kbps: 128 },
            tags_in_stream: false,
        });
        assert!(
            matches!(&result, Err(Error::Failed(message)) if message.contains("missing.flac")),
            "{result:?}"
        );
    }
}
