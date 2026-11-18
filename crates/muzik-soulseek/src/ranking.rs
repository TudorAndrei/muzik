//! Rank peer results by audio quality and match to the search text.

use crate::types::{Candidate, FileEntry};
use muzik_core::PreferredAudio;
use muzik_core::audio::AudioFormat;
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization;

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

const VERSION_WORDS: [&str; 20] = [
    "album",
    "audio",
    "bonus",
    "clean",
    "deluxe",
    "digital",
    "edit",
    "explicit",
    "hd",
    "hq",
    "mix",
    "mono",
    "original",
    "radio",
    "remaster",
    "remastered",
    "single",
    "stereo",
    "track",
    "version",
];

const COVER_WORDS: [&str; 4] = ["cover", "covers", "karaoke", "tribute"];

struct Word {
    text: String,
    separated: bool,
}

/// Narrow a search result to the best audio file that is this track.
#[must_use]
pub fn track_candidate(
    candidate: &Candidate,
    track: Track<'_>,
    prefer: PreferredAudio,
) -> Option<Candidate> {
    let titles: Vec<Vec<String>> = variants(track.title)
        .iter()
        .map(|text| texts(text))
        .filter(|title| !title.is_empty())
        .fold(Vec::new(), |mut titles, title| {
            if !titles.contains(&title) {
                titles.push(title);
            }
            titles
        });
    if titles.is_empty() {
        return None;
    }
    let artist: HashSet<_> = texts(track.artist).into_iter().collect();
    let file = candidate
        .files
        .iter()
        .filter(|file| format(file).is_some() && is_track(file, &titles, &artist))
        .max_by(|left, right| quality(left, prefer).total_cmp(&quality(right, prefer)))?;
    Some(Candidate {
        files: vec![file.clone()],
        ..candidate.clone()
    })
}

fn is_track(file: &FileEntry, titles: &[Vec<String>], artist: &HashSet<String>) -> bool {
    let path: HashSet<_> = texts(&file.name).into_iter().collect();
    if !artist.is_empty() && artist.is_disjoint(&path) {
        return false;
    }
    let cover = COVER_WORDS.iter().any(|&word| {
        path.contains(word)
            && !artist.contains(word)
            && !titles.iter().flatten().any(|title| title == word)
    });
    if cover {
        return false;
    }
    let folder: HashSet<_> = texts(parent(&file.name)).into_iter().collect();
    let stem = name(&file.name);
    let stem = stem.rsplit_once('.').map_or(stem, |(stem, _)| stem);
    variants(stem).iter().any(|text| {
        let found = words(text);
        titles
            .iter()
            .any(|title| ends_with_title(&found, title, artist, &folder))
    })
}

fn ends_with_title(
    found: &[Word],
    title: &[String],
    artist: &HashSet<String>,
    folder: &HashSet<String>,
) -> bool {
    let Some(start) = found.len().checked_sub(title.len()) else {
        return false;
    };
    let (prefix, rest) = found.split_at(start);
    if !rest.iter().map(|word| &word.text).eq(title) {
        return false;
    }
    let separated = match (prefix.last(), rest.first()) {
        (Some(previous), Some(first)) => first.separated || numeric(&previous.text),
        _ => true,
    };
    separated
        && prefix.iter().all(|word| {
            numeric(&word.text) || artist.contains(&word.text) || folder.contains(&word.text)
        })
}

fn variants(value: &str) -> Vec<String> {
    let mut found = vec![value.to_owned()];
    let credited = muzik_core::featured::parse(value).0;
    let plain = without_versions(&credited);
    let cut = plain
        .rsplit_once(" - ")
        .filter(|(_, tail)| is_version(tail))
        .map(|(head, _)| head.to_owned());
    for text in [Some(credited), Some(plain), cut].into_iter().flatten() {
        if !found.contains(&text) {
            found.push(text);
        }
    }
    found
}

fn without_versions(value: &str) -> String {
    let mut kept = String::new();
    let mut rest = value;
    while let Some(open) = rest.find(['(', '[']) {
        let (before, group) = rest.split_at(open);
        let close = if group.starts_with('(') { ')' } else { ']' };
        let Some(end) = group.find(close) else {
            break;
        };
        let (group, after) = group.split_at(end.saturating_add(1));
        let inner = group.get(1..end).unwrap_or_default();
        let credit = inner.trim_start().to_lowercase().starts_with("with ");
        kept.push_str(before);
        if !credit && !is_version(inner) {
            kept.push_str(group);
        }
        rest = after;
    }
    kept.push_str(rest);
    kept.trim().to_owned()
}

fn is_version(value: &str) -> bool {
    let found = texts(value);
    !found.is_empty()
        && found
            .iter()
            .all(|word| numeric(word) || VERSION_WORDS.contains(&word.as_str()))
}

fn numeric(value: &str) -> bool {
    value.chars().all(char::is_numeric)
}

fn texts(value: &str) -> Vec<String> {
    words(value).into_iter().map(|word| word.text).collect()
}

fn words(value: &str) -> Vec<Word> {
    let mut found = Vec::new();
    let mut current = String::new();
    let mut separated = false;
    for character in value.nfc() {
        if character.is_alphanumeric() {
            current.extend(character.to_lowercase());
        } else {
            if !current.is_empty() {
                found.push(Word {
                    text: std::mem::take(&mut current),
                    separated,
                });
                separated = false;
            }
            separated |= !character.is_whitespace();
        }
    }
    if !current.is_empty() {
        found.push(Word {
            text: current,
            separated,
        });
    }
    found
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
        track_candidate(
            &folder(names),
            Track { artist, title },
            PreferredAudio::Best,
        )
        .and_then(|candidate| candidate.files.into_iter().next())
        .map(|file| file.name)
    }

    #[test]
    fn track_title_keeps_words_that_are_not_a_version() {
        let album = [
            "Artist\\Album\\02 Song.flac",
            "Artist\\Album\\03 Song - Part Two.flac",
        ];
        assert_eq!(
            picked(&album, "Artist", "Song - Part Two").as_deref(),
            Some(album[1])
        );
        assert_eq!(
            picked(&album[..1], "Artist", "Song - Remastered 2009").as_deref(),
            Some(album[0])
        );
        assert!(picked(&album[..1], "Artist", "Song (Live)").is_none());
        assert!(picked(&["Artist\\Album\\01 (Intro).flac"], "Artist", "(Intro)").is_some());
    }

    #[test]
    fn track_file_needs_a_separate_title_and_the_artist() {
        assert!(picked(&["Artist\\Twenty One\\01 Twenty One.flac"], "Artist", "One").is_none());
        assert!(
            picked(
                &["Artist\\Album\\Artist - Album - 01 - One.flac"],
                "Artist",
                "One"
            )
            .is_some()
        );
        assert!(picked(&["Various\\Hits\\01 Home.flac"], "Artist A", "Home").is_none());
        assert!(
            picked(
                &["Tribute to Artist A\\Cover Band\\01 Home.flac"],
                "Artist A",
                "Home"
            )
            .is_none()
        );
    }

    #[test]
    fn track_file_choice_prefers_quality_credits_and_equal_unicode() {
        assert_eq!(
            picked(
                &["Artist\\Album\\01 Home.mp3", "Artist\\Album\\01 Home.flac"],
                "Artist",
                "Home"
            )
            .as_deref(),
            Some("Artist\\Album\\01 Home.flac")
        );
        assert!(
            picked(
                &["Artist\\Album\\01 Home feat. Guest.flac"],
                "Artist",
                "Home (feat. Guest)"
            )
            .is_some()
        );
        assert!(
            picked(
                &["Artist\\Album\\01 Cafe\u{301}.flac"],
                "Artist",
                "Calf\u{e9}"
            )
            .is_some()
        );
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
                "Dirty Harry (feat. Bootie Brown) - 2023 Remaster"
            )
            .is_some()
        );
        assert!(picked(&["Someone\\Hits\\07 Twenty One.mp3"], "Someone", "One").is_none());
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
