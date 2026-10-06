//! Rank peer results by audio quality and match to the search text.

use crate::types::{Candidate, FileEntry};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct RankedCandidate {
    pub candidate: Candidate,
    pub score: f64,
}

pub fn search_query(query: &str, prefer: &str) -> String {
    let query = query.trim();
    let tokens: HashSet<_> = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    let suffix = if prefer == "lossless" {
        (!tokens.contains("flac") && !tokens.contains("lossless")).then_some("flac")
    } else if prefer != "any" && !prefer.is_empty() {
        (!tokens.contains(prefer)).then_some(prefer)
    } else {
        None
    };
    suffix.map_or_else(|| query.to_owned(), |suffix| format!("{query} {suffix}"))
}

pub fn rank(
    candidates: Vec<Candidate>,
    query: &str,
    prefer: &str,
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

pub fn format(file: &FileEntry) -> &str {
    let extension = file.name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    if extension.eq_ignore_ascii_case("aif") {
        "aiff"
    } else if extension.eq_ignore_ascii_case("flac") {
        "flac"
    } else if extension.eq_ignore_ascii_case("alac") {
        "alac"
    } else if extension.eq_ignore_ascii_case("wav") {
        "wav"
    } else if extension.eq_ignore_ascii_case("aiff") {
        "aiff"
    } else if extension.eq_ignore_ascii_case("ape") {
        "ape"
    } else if extension.eq_ignore_ascii_case("wv") {
        "wv"
    } else if extension.eq_ignore_ascii_case("mp3") {
        "mp3"
    } else if extension.eq_ignore_ascii_case("m4a") {
        "m4a"
    } else if extension.eq_ignore_ascii_case("aac") {
        "aac"
    } else if extension.eq_ignore_ascii_case("opus") {
        "opus"
    } else if extension.eq_ignore_ascii_case("ogg") {
        "ogg"
    } else {
        ""
    }
}

pub fn is_lossless(format: &str) -> bool {
    matches!(format, "flac" | "alac" | "wav" | "aiff" | "ape" | "wv")
}

fn quality(file: &FileEntry, prefer: &str) -> f64 {
    let fmt = format(file);
    let mut score = if is_lossless(fmt) {
        100.0
    } else if fmt == "mp3" {
        50.0
    } else if !fmt.is_empty() {
        40.0
    } else {
        0.0
    };
    if (prefer == "lossless" && is_lossless(fmt))
        || (prefer == "mp3-320" && fmt == "mp3" && file.bitrate_kbps == Some(320))
        || prefer == fmt
    {
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

fn score(candidate: &Candidate, query: &str, prefer: &str) -> f64 {
    let audio: Vec<_> = candidate
        .files
        .iter()
        .filter(|file| !format(file).is_empty())
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

fn tokens(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn parent(path: &str) -> &str {
    path.rsplit_once(['/', '\\'])
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn numbered(value: &str) -> bool {
    let mut characters = value
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .peekable();
    let mut digits = 0;
    while characters.peek().is_some_and(char::is_ascii_digit) {
        let _ = characters.next();
        digits += 1;
    }
    (1..=2).contains(&digits)
        && characters
            .next()
            .is_some_and(|character| !character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{rank, search_query};
    use crate::types::{Candidate, FileEntry};

    fn candidate(name: &str) -> Candidate {
        Candidate {
            username: "peer".into(),
            slots: 1,
            speed: 100_000,
            files: vec![FileEntry {
                name: name.into(),
                size: 100,
                bitrate_kbps: Some(320),
                duration_seconds: None,
                vbr: None,
                sample_rate_hz: None,
                bit_depth: None,
            }],
        }
    }

    #[test]
    fn lossless_preference_ranks_matching_flac_above_mp3() {
        let ranked = rank(
            vec![
                candidate("Album\\01 Song.mp3"),
                candidate("Album\\01 Song.flac"),
            ],
            "Song",
            "lossless",
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
        assert_eq!(search_query("Artist Song", "lossless"), "Artist Song flac");
        assert_eq!(
            search_query("Artist Song flac", "lossless"),
            "Artist Song flac"
        );
    }
}
