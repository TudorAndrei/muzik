//! Chapter lookup after local sidecars have no usable chapters.

use crate::Result;
use crate::ytdlp::YtDlp;
use muzik_core::MetadataSource;
use muzik_core::chapters::{self, Chapter};
use muzik_metadata::{MetadataClient, ReleaseSearch};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Apply the configured metadata source to one audio file.
/// Local chapter sidecars are read by the caller before this function.
pub fn discover(
    source: &Path,
    selected: MetadataSource,
    cancelled: &AtomicBool,
) -> Result<Vec<Chapter>> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    if matches!(selected, MetadataSource::Youtube | MetadataSource::Auto) {
        let chapters = match youtube(source, cancelled) {
            Ok(chapters) => chapters,
            Err(error) if selected == MetadataSource::Auto && !cancelled.load(Ordering::SeqCst) => {
                let _ = error;
                Vec::new()
            }
            Err(error) => return Err(error),
        };
        if !chapters.is_empty() {
            return Ok(chapters);
        }
    }
    if matches!(selected, MetadataSource::Musicbrainz | MetadataSource::Auto) {
        return match musicbrainz(source, cancelled) {
            Ok(chapters) => Ok(chapters),
            Err(_) if selected == MetadataSource::Auto && !cancelled.load(Ordering::SeqCst) => {
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        };
    }
    Ok(Vec::new())
}

fn youtube(source: &Path, cancelled: &AtomicBool) -> Result<Vec<Chapter>> {
    let path = chapters::sidecar_path(source, ".info.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let metadata: Value = serde_json::from_str(&text)?;
    let from_description = metadata
        .get("description")
        .and_then(Value::as_str)
        .map(chapters::parse_tracklist)
        .unwrap_or_default();
    if !from_description.is_empty() {
        return Ok(from_description);
    }
    let from_comments = chapters::best_comment_tracklist(&metadata);
    if !from_comments.is_empty() {
        return Ok(from_comments);
    }
    let url = metadata
        .get("webpage_url")
        .or_else(|| metadata.get("original_url"))
        .and_then(Value::as_str)
        .filter(|url| {
            url.starts_with("https://www.youtube.com/watch?")
                || url.starts_with("https://youtu.be/")
        });
    let Some(url) = url else {
        return Ok(Vec::new());
    };
    let comments = YtDlp::default().video(url, true, cancelled)?;
    Ok(chapters::best_comment_tracklist(&comments))
}

fn musicbrainz(source: &Path, cancelled: &AtomicBool) -> Result<Vec<Chapter>> {
    let tags = muzik_tags::read(source, &[]).unwrap_or_default();
    let info = fs::read_to_string(chapters::sidecar_path(source, ".info.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let release_id = tags.fields.get("mb_albumid").map(String::as_str);
    let album = tags.fields.get("album").map(String::as_str).or_else(|| {
        info.as_ref()
            .and_then(|value| value.get("album"))
            .and_then(Value::as_str)
    });
    let artist = tags
        .fields
        .get("albumartist")
        .or_else(|| tags.fields.get("artist"))
        .map(String::as_str)
        .or_else(|| {
            info.as_ref()
                .and_then(|value| value.get("artist").or_else(|| value.get("album_artist")))
                .and_then(Value::as_str)
        });
    if release_id.is_none() && album.is_none() {
        return Ok(Vec::new());
    }
    let client = MetadataClient::new("muzik/0.1.0 (https://github.com/tudor-d/muzik)");
    let release = if let Some(id) = release_id {
        Some(client.lookup_release(id)?)
    } else if let Some(album) = album {
        if cancelled.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let hits = client.search_releases(
            &ReleaseSearch {
                release: album.to_owned(),
                artist: artist.map(str::to_owned),
                ..ReleaseSearch::default()
            },
            3,
        )?;
        let strong = hits
            .into_iter()
            .filter(|hit| hit.score.is_some_and(|score| score >= 95))
            .collect::<Vec<_>>();
        if strong.len() == 1 {
            Some(client.lookup_release(&strong[0].id.0)?)
        } else {
            None
        }
    } else {
        None
    };
    if cancelled.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    let chapters = release.map_or_else(Vec::new, |release| chapters_from_tracks(&release.tracks));
    let Some(total) = chapters.last().and_then(|chapter| chapter.end) else {
        return Ok(chapters);
    };
    let Ok(properties) = muzik_tags::probe(source) else {
        return Ok(Vec::new());
    };
    let Some(duration) = properties.duration_seconds else {
        return Ok(Vec::new());
    };
    Ok(validate_duration(chapters, total, duration))
}

fn validate_duration(
    mut chapters: Vec<Chapter>,
    release_total: i64,
    audio_duration: f64,
) -> Vec<Chapter> {
    let difference = (audio_duration - release_total as f64).abs();
    if difference > 15.0_f64.max(audio_duration * 0.05) {
        return Vec::new();
    }
    if let Some(last) = chapters.last_mut() {
        // An open end makes ffmpeg keep all audio after the last start time.
        last.end = None;
    }
    chapters
}

fn chapters_from_tracks(tracks: &[muzik_core::TrackCandidate]) -> Vec<Chapter> {
    if tracks.len() < 2
        || tracks
            .iter()
            .any(|track| !track.length_seconds.is_some_and(|time| time > 0.0))
    {
        return Vec::new();
    }
    let mut start = 0_i64;
    let mut chapters = Vec::with_capacity(tracks.len());
    for (position, track) in tracks.iter().enumerate() {
        let Some(length) = track.length_seconds else {
            return Vec::new();
        };
        let rounded = length.round();
        if !rounded.is_finite() || rounded <= 0.0 || rounded > i64::MAX as f64 {
            return Vec::new();
        }
        let Some(end) = start.checked_add(rounded as i64) else {
            return Vec::new();
        };
        chapters.push(Chapter {
            index: u32::try_from(position + 1).unwrap_or(u32::MAX),
            start,
            end: Some(end),
            title: track.title.clone(),
        });
        start = end;
    }
    chapters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_description_provides_chapters_when_embedded_chapters_are_missing()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("album.opus");
        fs::write(
            chapters::sidecar_path(&source, ".info.json"),
            r#"{"description":"0:00 First\n3:04 Second"}"#,
        )?;
        let found = discover(&source, MetadataSource::Youtube, &AtomicBool::new(false))?;
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].start, 184);
        Ok(())
    }

    #[test]
    fn musicbrainz_track_lengths_make_chapter_boundaries() {
        let tracks = [
            muzik_core::TrackCandidate {
                recording_id: None,
                release_track_id: None,
                title: "One".into(),
                artist: "A".into(),
                length_seconds: Some(61.2),
                index: 1,
                medium: 1,
                medium_index: 1,
            },
            muzik_core::TrackCandidate {
                recording_id: None,
                release_track_id: None,
                title: "Two".into(),
                artist: "A".into(),
                length_seconds: Some(80.0),
                index: 2,
                medium: 1,
                medium_index: 2,
            },
        ];
        let found = chapters_from_tracks(&tracks);
        assert_eq!(found[0].end, Some(61));
        assert_eq!(found[1].start, 61);
        let checked = validate_duration(found, 141, 148.0);
        assert_eq!(checked[1].end, None);
        assert_eq!(checked[0].end, Some(61));
    }

    #[test]
    fn pinned_comment_wins_when_description_has_no_tracklist()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("album.opus");
        fs::write(
            chapters::sidecar_path(&source, ".info.json"),
            r#"{
            "description":"Album upload",
            "comments":[
                {"text":"0:00 Other\n1:00 Other end"},
                {"text":"0:00 First\n2:00 Second","is_pinned":true}
            ]
        }"#,
        )?;
        let found = discover(&source, MetadataSource::Youtube, &AtomicBool::new(false))?;
        assert_eq!(found[0].title, "First");
        assert_eq!(found[1].start, 120);
        Ok(())
    }
}
