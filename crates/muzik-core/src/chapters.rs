//! Chapter parsing and sidecar discovery.

use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static CHAPTER_LINE: LazyLock<Regex> = LazyLock::new(|| fixed(r"^(\d+:\d{2}(?::\d{2})?)\s+(.+)$"));
static CUE_TRACK: LazyLock<Regex> = LazyLock::new(|| fixed(r"(?i)^\s*TRACK\s+(\d+)\s+\S+"));
static CUE_TITLE: LazyLock<Regex> = LazyLock::new(|| fixed(r#"(?i)^\s*TITLE\s+"?(.*?)"?\s*$"#));
static CUE_INDEX: LazyLock<Regex> =
    LazyLock::new(|| fixed(r"(?i)^\s*INDEX\s+01\s+(\d{2}):(\d{2}):(\d{2})"));
static TRACK_PREFIX: LazyLock<Regex> = LazyLock::new(|| fixed(r"^\s*\d{1,3}\s*[.):\-]\s+"));
static YTDLP_RANGE_SUFFIX: LazyLock<Regex> = LazyLock::new(|| fixed(r"\.\)\s*$"));

#[expect(
    clippy::expect_used,
    reason = "the patterns are fixed literals covered by tests"
)]
fn fixed(pattern: &str) -> Regex {
    Regex::new(pattern).expect("fixed chapter pattern")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chapter {
    pub index: u32,
    pub start: i64,
    pub end: Option<i64>,
    pub title: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read chapter sidecar: {0}")]
    Io(#[from] std::io::Error),
    #[error("cannot parse chapter JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid chapter value: {0}")]
    InvalidValue(&'static str),
}

/// Parse editable `MM:SS Title` or `HH:MM:SS Title` lines.
#[must_use]
pub fn parse_chapters(text: &str) -> Vec<Chapter> {
    ordered(
        text.lines()
            .filter_map(|line| {
                let captures = CHAPTER_LINE.captures(line.trim())?;
                let start = timestamp_seconds(captures.get(1)?.as_str())?;
                Some(Chapter {
                    index: 0,
                    start,
                    end: None,
                    title: captures.get(2)?.as_str().trim().to_owned(),
                })
            })
            .collect(),
    )
}

fn ordered(mut chapters: Vec<Chapter>) -> Vec<Chapter> {
    for chapter in &mut chapters {
        chapter.start = chapter.start.max(0);
    }
    chapters.sort_by_key(|chapter| chapter.start);
    chapters.dedup_by_key(|chapter| chapter.start);
    let next_starts: Vec<_> = chapters
        .iter()
        .skip(1)
        .map(|chapter| Some(chapter.start))
        .chain([None])
        .collect();
    for ((position, chapter), next_start) in chapters.iter_mut().enumerate().zip(next_starts) {
        chapter.index = u32::try_from(position).map_or(u32::MAX, |index| index.saturating_add(1));
        chapter.end = chapter
            .end
            .filter(|end| *end > chapter.start)
            .or(next_start);
    }
    chapters
}

/// Read a track list from a video description or comment.
pub fn parse_tracklist(text: &str) -> Vec<Chapter> {
    static START: LazyLock<Regex> = LazyLock::new(|| {
        fixed(
            r"^\s*(?:\d{1,3}[.)-]\s*)?[\[(]?(\d{1,2}:\d{2}(?::\d{2})?)[\])]?[\s\-–—•|:.)\]]*(.+?)\s*$",
        )
    });
    static END: LazyLock<Regex> = LazyLock::new(|| {
        fixed(
            r"^\s*(?:\d{1,3}[.)-]\s*)?(.+?)\s*[-–—•|(\[]*\s*[\[(]?(\d{1,2}:\d{2}(?::\d{2})?)[\])]?\s*$",
        )
    });
    static RANGE: LazyLock<Regex> = LazyLock::new(|| {
        fixed(
            r"^\s*(?:\d{1,3}[.)-]\s*)?(.+?)\s*[\[(]\s*(\d{1,2}:\d{2}(?::\d{2})?)\s*[-–—]\s*\d{1,2}:\d{2}(?::\d{2})?\s*[\])]\s*$",
        )
    });
    let mut entries = Vec::new();
    for line in text.lines() {
        let pair = RANGE
            .captures(line)
            .and_then(|capture| Some((capture.get(2)?.as_str(), capture.get(1)?.as_str())))
            .or_else(|| {
                START
                    .captures(line)
                    .and_then(|capture| Some((capture.get(1)?.as_str(), capture.get(2)?.as_str())))
            })
            .or_else(|| {
                END.captures(line)
                    .and_then(|capture| Some((capture.get(2)?.as_str(), capture.get(1)?.as_str())))
            });
        if let Some((time, title)) = pair {
            let title = title
                .trim()
                .trim_matches(|character: char| "-–—•|:. \t".contains(character))
                .trim();
            if let Some(start) = timestamp_seconds(time).filter(|_| !title.is_empty()) {
                entries.push(Chapter {
                    index: 0,
                    start,
                    end: None,
                    title: title.to_owned(),
                });
            }
        }
    }
    let chapters = ordered(entries);
    if chapters.len() < 2 {
        return Vec::new();
    }
    chapters
}

/// Select a useful track list from `YouTube` comments, with pinned comments first.
pub fn best_comment_tracklist(metadata: &Value) -> Vec<Chapter> {
    metadata
        .get("comments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|comment| {
            let chapters = parse_tracklist(comment.get("text")?.as_str()?);
            (chapters.len() >= 2).then_some((
                (
                    comment.get("is_pinned") == Some(&Value::Bool(true)),
                    comment.get("author_is_uploader") == Some(&Value::Bool(true)),
                ),
                chapters,
            ))
        })
        .max_by_key(|(rank, _)| *rank)
        .map_or_else(Vec::new, |(_, chapters)| chapters)
}

/// Parse the `chapters` array in yt-dlp metadata.
///
/// # Errors
/// Returns an error if the text is not JSON or a chapter value is invalid.
pub fn parse_info_json(text: &str) -> Result<Vec<Chapter>, Error> {
    let data: Value = serde_json::from_str(text)?;
    let Some(raw) = data.get("chapters").filter(|value| {
        !matches!(value, Value::Null | Value::Bool(false))
            && value.as_str() != Some("")
            && value.as_i64() != Some(0)
    }) else {
        return Ok(Vec::new());
    };
    let entries = raw.as_array().ok_or(Error::InvalidValue("chapters"))?;
    let mut chapters = Vec::with_capacity(entries.len());
    for entry in entries {
        let start = json_seconds(entry.get("start_time"), 0)?;
        let end = entry
            .get("end_time")
            .filter(|value| !value.is_null())
            .map(|value| json_seconds(Some(value), 0))
            .transpose()?;
        let title = entry
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        chapters.push(Chapter {
            index: 0,
            start,
            end,
            title,
        });
    }
    let mut chapters = ordered(chapters);
    for chapter in &mut chapters {
        if chapter.title.is_empty() {
            chapter.title = format!("Track {}", chapter.index);
        }
    }
    Ok(chapters)
}

/// Parse track titles and `INDEX 01` times from a CUE sheet.
pub fn parse_cue(text: &str) -> Vec<Chapter> {
    let mut entries: Vec<Chapter> = Vec::new();
    let mut current: Option<(u32, String, Option<i64>)> = None;
    for line in text.lines() {
        if let Some(captures) = CUE_TRACK.captures(line) {
            if let Some((index, title, Some(start))) = current.take() {
                entries.push(Chapter {
                    index,
                    start,
                    end: None,
                    title,
                });
            }
            current = captures
                .get(1)
                .and_then(|capture| capture.as_str().parse::<u32>().ok())
                .map(|index| (index, format!("Track {index}"), None));
            continue;
        }
        let Some((_, title, start)) = current.as_mut() else {
            continue;
        };
        if let Some(captures) = CUE_TITLE.captures(line) {
            let parsed = captures
                .get(1)
                .map_or("", |capture| capture.as_str())
                .trim();
            if !parsed.is_empty() {
                parsed.clone_into(title);
            }
        } else if let Some(captures) = CUE_INDEX.captures(line) {
            let parts: Option<Vec<i64>> = (1..=3)
                .map(|position| captures.get(position)?.as_str().parse::<i64>().ok())
                .collect();
            if let Some([minutes, seconds, frames]) = parts.as_deref() {
                *start = minutes
                    .checked_mul(60)
                    .and_then(|time| time.checked_add(*seconds))
                    .and_then(|time| time.checked_add(i64::from(*frames >= 38)));
            }
        }
    }
    if let Some((index, title, Some(start))) = current {
        entries.push(Chapter {
            index,
            start,
            end: None,
            title,
        });
    }
    ordered(entries)
}

/// Find chapters in the configured sidecar order.
///
/// # Errors
/// Returns an error if a sidecar cannot be read or parsed.
pub fn find_chapters(audio: &Path) -> Result<Vec<Chapter>, Error> {
    find_chapters_with_info(audio, true)
}

/// Find local text and CUE chapters, with optional yt-dlp metadata.
///
/// # Errors
/// Returns an error if a sidecar cannot be read or parsed.
pub fn find_chapters_with_info(audio: &Path, include_info: bool) -> Result<Vec<Chapter>, Error> {
    let txt = Sidecar::Chapters.path(audio);
    if txt.metadata().is_ok_and(|metadata| metadata.len() > 0) {
        return Ok(normalize(parse_chapters(&read_text(&txt)?)));
    }

    let info = Sidecar::Info.path(audio);
    if include_info && info.exists() {
        let chapters = parse_info_json(&read_text(&info)?)?;
        if !chapters.is_empty() {
            return Ok(normalize(chapters));
        }
    }

    let cue = Sidecar::Cue.path(audio);
    let cue = if cue.exists() {
        Some(cue)
    } else {
        let parent = audio.parent().unwrap_or_else(|| Path::new("."));
        let candidates: Vec<_> = fs::read_dir(parent)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "cue"))
            .collect();
        (candidates.len() == 1)
            .then(|| candidates.into_iter().next())
            .flatten()
    };
    if let Some(cue) = cue {
        let chapters = parse_cue(&read_text(&cue)?);
        if !chapters.is_empty() {
            return Ok(normalize(chapters));
        }
    }
    Ok(Vec::new())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sidecar {
    Chapters,
    Info,
    Metadata,
    Muzik,
    Cue,
    Jpg,
    Jpeg,
    Png,
    Webp,
}

impl Sidecar {
    pub const THUMBNAILS: [Self; 4] = [Self::Jpg, Self::Jpeg, Self::Png, Self::Webp];

    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Chapters => ".chapters.txt",
            Self::Info => ".info.json",
            Self::Metadata => ".metadata.txt",
            Self::Muzik => ".muzik.json",
            Self::Cue => ".cue",
            Self::Jpg => ".jpg",
            Self::Jpeg => ".jpeg",
            Self::Png => ".png",
            Self::Webp => ".webp",
        }
    }

    #[must_use]
    pub fn path(self, audio: &Path) -> PathBuf {
        sidecar_path(audio, self.suffix())
    }
}

fn sidecar_path(audio: &Path, extension: &str) -> PathBuf {
    let stem = audio
        .file_stem()
        .unwrap_or_else(|| std::ffi::OsStr::new(""))
        .to_string_lossy();
    audio.with_file_name(format!("{stem}{extension}"))
}

fn read_text(path: &Path) -> Result<String, std::io::Error> {
    let bytes = fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn timestamp_seconds(timestamp: &str) -> Option<i64> {
    let parts: Option<Vec<i64>> = timestamp
        .split(':')
        .map(|part| part.parse::<i64>().ok())
        .collect();
    match parts?.as_slice() {
        [minutes, seconds] => minutes.checked_mul(60)?.checked_add(*seconds),
        [hours, minutes, seconds] => hours
            .checked_mul(3600)?
            .checked_add(minutes.checked_mul(60)?)?
            .checked_add(*seconds),
        _ => None,
    }
}

fn json_seconds(value: Option<&Value>, default: i64) -> Result<i64, Error> {
    let Some(value) = value else {
        return Ok(default);
    };
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse::<f64>().ok(),
        Value::Bool(value) => Some(f64::from(u8::from(*value))),
        _ => None,
    }
    .ok_or(Error::InvalidValue("chapter time"))?;
    truncate_to_i64(number).ok_or(Error::InvalidValue("chapter time"))
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "std has no checked f64 to i64 conversion; the range is checked first"
)]
fn truncate_to_i64(number: f64) -> Option<i64> {
    (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0)
        .contains(&number)
        .then_some(number as i64)
}

fn normalize(mut chapters: Vec<Chapter>) -> Vec<Chapter> {
    for chapter in &mut chapters {
        let without_prefix = TRACK_PREFIX.replace(&chapter.title, "");
        let cleaned = YTDLP_RANGE_SUFFIX.replace(without_prefix.trim(), "");
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() && cleaned != chapter.title {
            chapter.title = cleaned.to_owned();
        }
    }
    chapters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_well_formed(chapters: &[Chapter]) {
        for (chapter, index) in chapters.iter().zip(1_u32..) {
            assert_eq!(chapter.index, index, "{chapters:?}");
            assert!(chapter.start >= 0, "{chapters:?}");
            assert!(
                chapter.end.is_none_or(|end| end > chapter.start),
                "{chapters:?}"
            );
        }
        assert!(
            chapters
                .windows(2)
                .all(|pair| pair[0].start < pair[1].start),
            "{chapters:?}"
        );
    }

    fn time_text(hours: u8, minutes: u8, seconds: u8) -> String {
        if hours.is_multiple_of(3) {
            format!("{}:{:02}", minutes, seconds % 100)
        } else {
            format!("{}:{:02}:{:02}", hours, minutes % 100, seconds % 100)
        }
    }

    #[test]
    fn any_chapter_text_gives_ordered_chapters() {
        bolero::check!()
            .with_type::<(Vec<(u8, u8, u8, String)>, String)>()
            .for_each(|(lines, raw)| {
                let mut text = lines
                    .iter()
                    .map(|(hours, minutes, seconds, title)| {
                        format!("{} {title}", time_text(*hours, *minutes, *seconds))
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                text.push('\n');
                text.push_str(raw);
                assert_well_formed(&parse_chapters(&text));
            });
    }

    #[test]
    fn any_tracklist_gives_ordered_chapters() {
        bolero::check!()
            .with_type::<(Vec<(u8, u8, u8, u8, String)>, String)>()
            .for_each(|(lines, raw)| {
                let mut text: String = lines
                    .iter()
                    .map(|(shape, hours, minutes, seconds, title)| {
                        let time = time_text(*hours, *minutes, *seconds);
                        match shape % 4 {
                            0 => format!("{time} {title}\n"),
                            1 => format!("{title} - {time}\n"),
                            2 => format!("{shape}. {title} ({time} - {time})\n"),
                            _ => format!("[{time}] | {title}\n"),
                        }
                    })
                    .collect();
                text.push_str(raw);
                assert_well_formed(&parse_tracklist(&text));
            });
    }

    #[test]
    fn any_cue_sheet_gives_ordered_chapters() {
        bolero::check!()
            .with_type::<Vec<(u8, u8, u8, u8, String)>>()
            .for_each(|lines| {
                let text: String = lines
                    .iter()
                    .map(|(kind, first, second, third, text)| match kind % 4 {
                        0 => format!("  TRACK {first:02} AUDIO\n"),
                        1 => format!("    TITLE \"{text}\"\n"),
                        2 => format!(
                            "    INDEX 01 {:02}:{:02}:{:02}\n",
                            first % 100,
                            second % 100,
                            third % 100
                        ),
                        _ => format!("{text}\n"),
                    })
                    .collect();
                assert_well_formed(&parse_cue(&text));
            });
    }

    #[test]
    fn any_info_json_gives_ordered_chapters() {
        bolero::check!()
            .with_type::<(Vec<(Option<f64>, Option<f64>, Option<String>)>, String)>()
            .for_each(|(entries, raw)| {
                let chapters: Vec<Value> = entries
                    .iter()
                    .map(|(start, end, title)| {
                        serde_json::json!({"start_time": start, "end_time": end, "title": title})
                    })
                    .collect();
                let text = serde_json::json!({ "chapters": chapters }).to_string();
                if let Ok(chapters) = parse_info_json(&text) {
                    assert_well_formed(&chapters);
                }
                if let Ok(chapters) = parse_info_json(raw) {
                    assert_well_formed(&chapters);
                }
            });
    }

    #[test]
    fn text_times_and_titles() {
        assert_eq!(
            parse_chapters("00:00 First\n03:12 Second\n01:02:03 Third\ninvalid\n"),
            vec![
                Chapter {
                    index: 1,
                    start: 0,
                    end: Some(192),
                    title: "First".into()
                },
                Chapter {
                    index: 2,
                    start: 192,
                    end: Some(3723),
                    title: "Second".into()
                },
                Chapter {
                    index: 3,
                    start: 3723,
                    end: None,
                    title: "Third".into()
                },
            ]
        );
    }

    #[test]
    fn unordered_and_repeated_starts_become_ordered_chapters() {
        let chapters = parse_chapters("03:00 Late\n00:00 First\n00:00 Again\n");
        assert_eq!(
            chapters
                .iter()
                .map(|chapter| (
                    chapter.index,
                    chapter.start,
                    chapter.end,
                    chapter.title.as_str()
                ))
                .collect::<Vec<_>>(),
            [(1, 0, Some(180), "First"), (2, 180, None, "Late")]
        );
        let chapters = parse_info_json(
            r#"{"chapters":[{"start_time":60,"end_time":60},{"start_time":-5,"end_time":30,"title":"One"}]}"#,
        )
        .expect("valid JSON");
        assert_eq!(
            chapters,
            [
                Chapter {
                    index: 1,
                    start: 0,
                    end: Some(30),
                    title: "One".into()
                },
                Chapter {
                    index: 2,
                    start: 60,
                    end: None,
                    title: "Track 2".into()
                }
            ]
        );
    }

    #[test]
    fn text_skips_a_time_that_exceeds_i64_seconds() {
        let chapters = parse_chapters("9223372036854775807:00 Too late\n00:00 First\n");
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].title, "First");
    }

    #[test]
    fn json_missing_ends_and_fractional_times() {
        let chapters = parse_info_json(r#"{"chapters":[{"start_time":3.9,"title":"One"},{"start_time":95.2,"end_time":120.8}]}"#)
            .expect("valid JSON");
        assert_eq!(chapters[0].end, Some(95));
        assert_eq!(chapters[1].title, "Track 2");
        assert_eq!(chapters[1].end, Some(120));
    }

    #[test]
    fn cue_orders_tracks_and_rounds_frames() {
        let chapters = parse_cue(
            "TITLE \"Album\"\n TRACK 02 AUDIO\n TITLE \"Two\"\n INDEX 01 03:12:38\n TRACK 01 AUDIO\n TITLE \"One\"\n INDEX 01 00:00:00\n",
        );
        assert_eq!(chapters[0].index, 1);
        assert_eq!(chapters[0].end, Some(193));
        assert_eq!(chapters[1].title, "Two");
    }

    #[test]
    fn discovery_order_and_lone_cue() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let audio = temp.path().join("Vol. 1.flac");
        fs::write(&audio, []).expect("audio file");
        fs::write(
            temp.path().join("Album.cue"),
            "TRACK 01 AUDIO\nTITLE \"01. Cued\"\nINDEX 01 00:00:00\n",
        )
        .expect("cue file");
        assert_eq!(
            find_chapters(&audio).expect("cue chapters")[0].title,
            "Cued"
        );
        fs::write(
            Sidecar::Info.path(&audio),
            r#"{"chapters":[{"start_time":0,"title":"02) JSON"}]}"#,
        )
        .expect("info file");
        assert_eq!(
            find_chapters(&audio).expect("JSON chapters")[0].title,
            "JSON"
        );
        fs::write(Sidecar::Chapters.path(&audio), "00:00 03 - Text\n").expect("text file");
        assert_eq!(
            find_chapters(&audio).expect("text chapters")[0].title,
            "Text"
        );
        fs::write(Sidecar::Chapters.path(&audio), "invalid\n").expect("text file");
        assert!(find_chapters(&audio).expect("invalid text").is_empty());
    }

    #[test]
    fn empty_text_falls_back_and_multiple_cues_need_a_matching_name() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let audio = temp.path().join("Audio.flac");
        fs::write(&audio, []).expect("audio file");
        fs::write(Sidecar::Chapters.path(&audio), []).expect("empty text file");
        fs::write(
            temp.path().join("One.cue"),
            "TRACK 01 AUDIO\nINDEX 01 00:00:00\n",
        )
        .expect("first cue");
        fs::write(
            temp.path().join("Two.cue"),
            "TRACK 01 AUDIO\nINDEX 01 00:00:00\n",
        )
        .expect("second cue");
        assert!(find_chapters(&audio).expect("ambiguous cues").is_empty());
        fs::write(
            Sidecar::Cue.path(&audio),
            "TRACK 01 AUDIO\nINDEX 01 00:00:00\n",
        )
        .expect("matching cue");
        assert_eq!(find_chapters(&audio).expect("matching cue").len(), 1);
    }
}
