//! Chapter parsing and sidecar discovery.

use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static CHAPTER_LINE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^(\d+:\d{2}(?::\d{2})?)\s+(.+)$").ok());
static CUE_TRACK: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*TRACK\s+(\d+)\s+\S+").ok());
static CUE_TITLE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"(?i)^\s*TITLE\s+"?(.*?)"?\s*$"#).ok());
static CUE_INDEX: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*INDEX\s+01\s+(\d{2}):(\d{2}):(\d{2})").ok());
static TRACK_PREFIX: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^\s*\d{1,3}\s*[.):\-]\s+").ok());
static YTDLP_RANGE_SUFFIX: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\.\)\s*$").ok());

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
pub fn parse_chapters(text: &str) -> Vec<Chapter> {
    let entries: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let captures = CHAPTER_LINE.as_ref()?.captures(line.trim())?;
            let start = timestamp_seconds(captures.get(1)?.as_str())?;
            Some((start, captures.get(2)?.as_str().trim().to_owned()))
        })
        .collect();
    entries
        .iter()
        .enumerate()
        .map(|(position, (start, title))| Chapter {
            index: u32::try_from(position).map_or(u32::MAX, |index| index.saturating_add(1)),
            start: *start,
            end: position
                .checked_add(1)
                .and_then(|next| entries.get(next))
                .map(|entry| entry.0),
            title: title.clone(),
        })
        .collect()
}

/// Parse the `chapters` array in yt-dlp metadata.
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
    for (position, entry) in entries.iter().enumerate() {
        let start = json_seconds(entry.get("start_time"), 0)?;
        let end = entry
            .get("end_time")
            .filter(|value| !value.is_null())
            .map(|value| json_seconds(Some(value), 0))
            .transpose()?;
        let title = entry
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.is_empty())
            .map_or_else(
                || format!("Track {}", position.saturating_add(1)),
                str::to_owned,
            );
        chapters.push(Chapter {
            index: u32::try_from(position).map_or(u32::MAX, |index| index.saturating_add(1)),
            start,
            end,
            title,
        });
    }
    let next_starts: Vec<_> = chapters
        .iter()
        .skip(1)
        .map(|chapter| chapter.start)
        .collect();
    for (chapter, next_start) in chapters.iter_mut().zip(next_starts) {
        if chapter.end.is_none() {
            chapter.end = Some(next_start);
        }
    }
    Ok(chapters)
}

/// Parse track titles and `INDEX 01` times from a CUE sheet.
pub fn parse_cue(text: &str) -> Vec<Chapter> {
    let mut entries: Vec<Chapter> = Vec::new();
    let mut current: Option<(u32, String, Option<i64>)> = None;
    for line in text.lines() {
        if let Some(captures) = CUE_TRACK
            .as_ref()
            .and_then(|pattern| pattern.captures(line))
        {
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
        if let Some(captures) = CUE_TITLE
            .as_ref()
            .and_then(|pattern| pattern.captures(line))
        {
            let parsed = captures
                .get(1)
                .map_or("", |capture| capture.as_str())
                .trim();
            if !parsed.is_empty() {
                *title = parsed.to_owned();
            }
        } else if let Some(captures) = CUE_INDEX
            .as_ref()
            .and_then(|pattern| pattern.captures(line))
        {
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
    entries.sort_by_key(|chapter| chapter.index);
    let next_starts: Vec<_> = entries
        .iter()
        .skip(1)
        .map(|chapter| chapter.start)
        .collect();
    for (chapter, next_start) in entries.iter_mut().zip(next_starts) {
        chapter.end = Some(next_start);
    }
    entries
}

/// Find chapters in the same order as the Python workflow.
pub fn find_chapters(audio: &Path) -> Result<Vec<Chapter>, Error> {
    let txt = sidecar_path(audio, ".chapters.txt");
    if txt.metadata().is_ok_and(|metadata| metadata.len() > 0) {
        return Ok(normalize(parse_chapters(&read_text(&txt)?)));
    }

    let info = sidecar_path(audio, ".info.json");
    if info.exists() {
        let chapters = parse_info_json(&read_text(&info)?)?;
        if !chapters.is_empty() {
            return Ok(normalize(chapters));
        }
    }

    let cue = sidecar_path(audio, ".cue");
    let cue = if cue.exists() {
        Some(cue)
    } else {
        let parent = match audio.parent() {
            Some(parent) => parent,
            None => Path::new("."),
        };
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

pub fn sidecar_path(audio: &Path, extension: &str) -> PathBuf {
    let stem = match audio.file_stem() {
        Some(stem) => stem,
        None => std::ffi::OsStr::new(""),
    }
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
    if !number.is_finite() || number < i64::MIN as f64 || number >= i64::MAX as f64 {
        return Err(Error::InvalidValue("chapter time"));
    }
    Ok(number as i64)
}

fn normalize(mut chapters: Vec<Chapter>) -> Vec<Chapter> {
    for chapter in &mut chapters {
        let original = chapter.title.clone();
        let without_prefix = TRACK_PREFIX.as_ref().map_or_else(
            || original.clone().into(),
            |pattern| pattern.replace(&original, ""),
        );
        let cleaned = YTDLP_RANGE_SUFFIX.as_ref().map_or_else(
            || without_prefix.clone(),
            |pattern| pattern.replace(without_prefix.trim(), ""),
        );
        let cleaned = cleaned.trim();
        if !cleaned.is_empty() {
            chapter.title = cleaned.to_owned();
        }
    }
    chapters
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let chapters = parse_cue("TITLE \"Album\"\n TRACK 02 AUDIO\n TITLE \"Two\"\n INDEX 01 03:12:38\n TRACK 01 AUDIO\n TITLE \"One\"\n INDEX 01 00:00:00\n");
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
            sidecar_path(&audio, ".info.json"),
            r#"{"chapters":[{"start_time":0,"title":"02) JSON"}]}"#,
        )
        .expect("info file");
        assert_eq!(
            find_chapters(&audio).expect("JSON chapters")[0].title,
            "JSON"
        );
        fs::write(sidecar_path(&audio, ".chapters.txt"), "00:00 03 - Text\n").expect("text file");
        assert_eq!(
            find_chapters(&audio).expect("text chapters")[0].title,
            "Text"
        );
        fs::write(sidecar_path(&audio, ".chapters.txt"), "invalid\n").expect("text file");
        assert!(find_chapters(&audio).expect("invalid text").is_empty());
    }

    #[test]
    fn empty_text_falls_back_and_multiple_cues_need_a_matching_name() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let audio = temp.path().join("Audio.flac");
        fs::write(&audio, []).expect("audio file");
        fs::write(sidecar_path(&audio, ".chapters.txt"), []).expect("empty text file");
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
            sidecar_path(&audio, ".cue"),
            "TRACK 01 AUDIO\nINDEX 01 00:00:00\n",
        )
        .expect("matching cue");
        assert_eq!(find_chapters(&audio).expect("matching cue").len(), 1);
    }
}
