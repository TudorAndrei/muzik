//! Rank peer results by audio quality and match to the search text.

use crate::types::{Candidate, FileEntry};
use muzik_core::PreferredAudio;
use muzik_core::audio::AudioFormat;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct RankedCandidate {
    pub candidate: Candidate,
    pub score: f64,
}

pub fn search_query(query: &str, prefer: PreferredAudio) -> String {
    let query = query.trim();
    let tokens: HashSet<_> = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    prefer
        .search_suffix(&tokens)
        .filter(|_| !query.is_empty())
        .map_or_else(|| query.to_owned(), |suffix| format!("{query} {suffix}"))
}

#[must_use]
pub fn rank(
    candidates: Vec<Candidate>,
    query: &str,
    prefer: PreferredAudio,
    limit: usize,
) -> Vec<RankedCandidate> {
    let mut ranked = candidates
        .into_iter()
        .map(|candidate| {
            let score = score(&candidate, query, prefer);
            RankedCandidate { candidate, score }
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.candidate.username.cmp(&b.candidate.username))
    });
    ranked.truncate(limit);
    ranked
}

#[must_use]
pub fn format(file: &FileEntry) -> Option<AudioFormat> {
    let (_, extension) = file.name.rsplit_once('.')?;
    AudioFormat::from_extension(extension).filter(|format| {
        !matches!(
            format,
            AudioFormat::Mp4 | AudioFormat::Mpc | AudioFormat::Speex
        )
    })
}

fn quality(file: &FileEntry, prefer: PreferredAudio) -> f64 {
    let fmt = format(file);
    let lossless = fmt.is_some_and(AudioFormat::is_lossless);
    let mut score = if lossless {
        100.0
    } else if fmt == Some(AudioFormat::Mp3) {
        50.0
    } else if fmt.is_some() {
        40.0
    } else {
        0.0
    };
    if prefer.bonus(fmt, lossless, file.bitrate_kbps) {
        score += 30.0;
    }
    if let Some(bitrate) = file.bitrate_kbps {
        score += f64::from(bitrate.min(320)) / 10.0;
    }
    if let Some(sample_rate) = file.sample_rate_hz {
        score += f64::from(sample_rate.min(192_000)) / 48_000.0;
    }
    if let Some(bit_depth) = file.bit_depth {
        score += f64::from(bit_depth) / 4.0;
    }
    score
}

fn score(candidate: &Candidate, query: &str, prefer: PreferredAudio) -> f64 {
    let audio: Vec<_> = candidate
        .files
        .iter()
        .filter(|file| format(file).is_some())
        .collect();
    let mut score = audio
        .iter()
        .map(|file| quality(file, prefer))
        .fold(0.0_f64, f64::max);
    if !audio.is_empty() {
        score += f64::from(u32::try_from(audio.len().min(30)).unwrap_or(30));
    }
    let parents: HashSet<_> = audio.iter().map(|file| parent(&file.name)).collect();
    if parents.len() == 1
        && parents
            .iter()
            .next()
            .is_some_and(|parent| !parent.is_empty())
    {
        score += 8.0;
    }
    let numbered = audio
        .iter()
        .filter(|file| numbered(name(&file.name)))
        .count();
    if !audio.is_empty() && numbered >= (audio.len() / 2).max(1) {
        score += 10.0;
    }
    let haystack = candidate
        .files
        .iter()
        .map(|file| file.name.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    for bad in [
        "partial",
        "incomplete",
        "youtube rip",
        "web rip",
        "transcode",
    ] {
        if haystack.contains(bad) {
            score -= 15.0;
        }
    }
    let query_tokens: HashSet<_> = tokens(query)
        .into_iter()
        .filter(|token| {
            token.len() > 2 && !matches!(token.as_str(), "flac" | "mp3" | "lossless" | "the")
        })
        .collect();
    if !query_tokens.is_empty() {
        let found: HashSet<_> = tokens(&haystack).into_iter().collect();
        let overlap = query_tokens.intersection(&found).count();
        let ratio = f64::from(u32::try_from(overlap).unwrap_or(u32::MAX))
            / f64::from(u32::try_from(query_tokens.len()).unwrap_or(u32::MAX));
        score += (20.0 * ratio).min(20.0);
    }
    if candidate.slots > 0 {
        score += 5.0;
    }
    score += f64::from(candidate.speed.min(2_000_000)) / 200_000.0;
    (score * 1000.0).round() / 1000.0
}

#[derive(Clone, Copy, Debug)]
pub struct Track<'a> {
    pub artist: &'a str,
    pub title: &'a str,
}

/// Narrow a search result to the one audio file that is this track.
#[must_use]
pub fn track_candidate(candidate: &Candidate, track: Track<'_>) -> Option<Candidate> {
    let title = muzik_core::featured::parse(track.title).0;
    let title = title.split(" - ").next().unwrap_or(&title);
    let wanted = tokens(&without_brackets(title));
    if wanted.is_empty() {
        return None;
    }
    let artist: HashSet<_> = tokens(track.artist).into_iter().collect();
    let file = candidate
        .files
        .iter()
        .filter(|file| format(file).is_some())
        .find(|file| {
            let stem = name(&file.name);
            let stem = stem.rsplit_once('.').map_or(stem, |(stem, _)| stem);
            let found = tokens(&without_brackets(stem));
            let Some(prefix) = found
                .len()
                .checked_sub(wanted.len())
                .filter(|&start| found.get(start..) == Some(wanted.as_slice()))
                .and_then(|start| found.get(..start))
            else {
                return false;
            };
            let folder: HashSet<_> = tokens(parent(&file.name)).into_iter().collect();
            prefix.iter().all(|token| {
                token.chars().all(|character| character.is_ascii_digit())
                    || artist.contains(token)
                    || folder.contains(token)
            })
        })?;
    Some(Candidate {
        files: vec![file.clone()],
        ..candidate.clone()
    })
}

fn without_brackets(value: &str) -> String {
    let mut depth = 0_usize;
    value
        .chars()
        .filter(|&character| match character {
            '(' | '[' => {
                depth = depth.saturating_add(1);
                false
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                false
            }
            _ => depth == 0,
        })
        .collect()
}

fn tokens(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rsplit_once(['/', '\\'])
        .map_or("", |(parent, _)| parent)
}

fn numbered(value: &str) -> bool {
    let mut characters = value
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .peekable();
    let mut digits = 0_u32;
    while characters.peek().is_some_and(char::is_ascii_digit) {
        let _ = characters.next();
        digits = digits.saturating_add(1);
    }
    (1..=2).contains(&digits)
        && characters
            .next()
            .is_some_and(|character| !character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{Track, rank, search_query, track_candidate};
    use crate::types::{Candidate, FileEntry};
    use muzik_core::PreferredAudio;

    fn candidate(name: &str) -> Candidate {
        folder(&[name])
    }

    fn folder(names: &[&str]) -> Candidate {
        Candidate {
            username: "peer".into(),
            slots: 1,
            speed: 100_000,
            files: names
                .iter()
                .map(|name| FileEntry {
                    name: (*name).into(),
                    size: 100,
                    bitrate_kbps: Some(320),
                    duration_seconds: None,
                    vbr: None,
                    sample_rate_hz: None,
                    bit_depth: None,
                })
                .collect(),
        }
    }

    fn picked(names: &[&str], artist: &str, title: &str) -> Option<String> {
        track_candidate(&folder(names), Track { artist, title })
            .and_then(|candidate| candidate.files.into_iter().next())
            .map(|file| file.name)
    }

    #[test]
    fn track_download_takes_the_title_file_from_an_album_folder() {
        let album = [
            "Music\\Kevin Morby\\Harlem River (2013)\\cover.jpg",
            "Music\\Kevin Morby\\Harlem River (2013)\\Kevin Morby - Harlem River - 01 - Miles, Miles, Miles.flac",
            "Music\\Kevin Morby\\Harlem River (2013)\\Kevin Morby - Harlem River - 03 - Harlem River.flac",
        ];
        assert_eq!(
            picked(&album, "Kevin Morby", "Harlem River").as_deref(),
            Some(album[2])
        );
        assert_eq!(picked(&album[..2], "Kevin Morby", "Harlem River"), None);
    }

    #[test]
    fn track_title_ignores_credits_and_versions_but_not_other_words() {
        assert!(
            picked(
                &["Gorillaz\\Cracker Island\\05. Dirty Harry [2023 Remaster].flac"],
                "Gorillaz, Bootie Brown",
                "Dirty Harry (feat. Bootie Brown) - Live"
            )
            .is_some()
        );
        assert!(picked(&["Various\\Hits\\07 Twenty One.mp3"], "Someone", "One").is_none());
        assert!(
            picked(
                &["Subcarpaţi\\Album\\02 Oamenii Sunt Încă Frumoși.flac"],
                "Subcarpaţi",
                "Oamenii Sunt Încă Frumoși"
            )
            .is_some()
        );
    }

    #[test]
    fn lossless_preference_ranks_matching_flac_above_mp3() {
        let ranked = rank(
            vec![
                candidate("Album\\01 Song.mp3"),
                candidate("Album\\01 Song.flac"),
            ],
            "Song",
            PreferredAudio::Lossless,
            2,
        );
        assert_eq!(
            ranked
                .first()
                .and_then(|item| item.candidate.files.first())
                .map(|file| file.name.as_str()),
            Some("Album\\01 Song.flac")
        );
    }

    #[test]
    fn query_adds_quality_term_once() {
        assert_eq!(
            search_query("Artist Song", PreferredAudio::Lossless),
            "Artist Song flac"
        );
        assert_eq!(
            search_query("Artist Song flac", PreferredAudio::Lossless),
            "Artist Song flac"
        );
    }
}
