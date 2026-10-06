use muzik_library::{Item, SqlValue, path_from_sql, scalar_text};
use muzik_media::quality::MeasuredQuality;
use muzik_soulseek::ranking::RankedCandidate;
use muzik_soulseek::types::Candidate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Serialize)]
pub struct CachedCandidate {
    pub query: String,
    pub score: f64,
    pub candidate: Candidate,
}

pub struct FlaggedTrack {
    pub artist: String,
    pub sort_artist: String,
    pub album: String,
    pub title: String,
    pub duration: Option<f64>,
    pub quality: MeasuredQuality,
}

pub fn scan_library(
    items: Vec<Item>,
    directory: &Path,
    min_bitrate: u32,
    mut measure: impl FnMut(&Path) -> Result<Option<MeasuredQuality>, String>,
) -> Result<(usize, Vec<FlaggedTrack>), String> {
    let mut scanned = 0_usize;
    let mut flagged = Vec::new();
    for item in items {
        let Some(path) = item.field("path").and_then(path_from_sql) else {
            continue;
        };
        let path = if path.is_absolute() {
            path
        } else {
            directory.join(path)
        };
        if !path.is_file() {
            continue;
        }
        let Some(measured) = measure(&path)? else {
            continue;
        };
        scanned += 1;
        if measured.lossless
            || measured
                .bitrate_kbps
                .is_some_and(|rate| rate >= min_bitrate)
        {
            continue;
        }
        let artist = item
            .field("artist")
            .and_then(scalar_text)
            .unwrap_or_default();
        let title = item
            .field("title")
            .and_then(scalar_text)
            .unwrap_or_default();
        let album = item
            .field("album")
            .and_then(scalar_text)
            .unwrap_or_default();
        let album_artist = item
            .field("albumartist")
            .and_then(scalar_text)
            .unwrap_or_default();
        let duration = item
            .field("length")
            .and_then(scalar_number)
            .filter(|value| *value > 0.0);
        flagged.push(FlaggedTrack {
            sort_artist: if album_artist.is_empty() {
                artist.clone()
            } else {
                album_artist
            },
            artist,
            album,
            title,
            duration,
            quality: measured,
        });
    }
    Ok((scanned, flagged))
}

fn scalar_number(value: &SqlValue) -> Option<f64> {
    match value {
        SqlValue::Integer(number) => number.to_string().parse().ok(),
        SqlValue::Real(number) => Some(*number),
        SqlValue::Text(text) => text.parse().ok(),
        _ => None,
    }
}

pub fn select_upgrade(
    track: &FlaggedTrack,
    ranked: &[RankedCandidate],
    prefer: &str,
) -> Option<(Candidate, f64)> {
    let current = quality_score(
        &track.quality.format,
        track.quality.bitrate_kbps,
        track.quality.sample_rate,
        track.quality.bit_depth,
        prefer,
    );
    ranked
        .iter()
        .flat_map(|ranked| {
            ranked
                .candidate
                .files
                .iter()
                .map(move |file| (ranked, file))
        })
        .filter_map(|(ranked, file)| {
            let one_file = Candidate {
                files: vec![file.clone()],
                ..ranked.candidate.clone()
            };
            let wanted = Wanted {
                artist: &track.artist,
                title: &track.title,
                album: &track.album,
                duration: track.duration,
            };
            if !safe_match(&one_file, &wanted) {
                return None;
            }
            let format = muzik_soulseek::ranking::format(file);
            let score = quality_score(
                format,
                file.bitrate_kbps,
                file.sample_rate_hz,
                file.bit_depth,
                prefer,
            );
            (score > current).then_some((one_file, score, ranked.score))
        })
        .max_by(|left, right| {
            left.1
                .total_cmp(&right.1)
                .then_with(|| left.2.total_cmp(&right.2))
        })
        .map(|(candidate, _, ranking)| (candidate, ranking))
}

pub const DURATION_TOLERANCE: f64 = 10.0;

pub struct Wanted<'a> {
    pub artist: &'a str,
    pub title: &'a str,
    pub album: &'a str,
    pub duration: Option<f64>,
}

pub(crate) fn tokens(value: &str) -> HashSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|word| {
            word.len() >= 2
                && !matches!(
                    word.as_str(),
                    "the" | "and" | "feat" | "ft" | "official" | "audio"
                )
        })
        .collect()
}

fn overlap(need: &HashSet<String>, haystack: &HashSet<String>) -> bool {
    !need.is_empty() && need.intersection(haystack).count() * 3 >= need.len() * 2
}

pub fn safe_match(candidate: &Candidate, wanted: &Wanted<'_>) -> bool {
    if candidate.username.trim().is_empty() || candidate.files.is_empty() {
        return false;
    }
    let files = candidate
        .files
        .iter()
        .filter(|file| !muzik_soulseek::ranking::format(file).is_empty())
        .collect::<Vec<_>>();
    if files.len() != candidate.files.len() {
        return false;
    }
    let names = files
        .iter()
        .map(|file| file.name.as_str())
        .collect::<Vec<_>>();
    let all_text = tokens(&names.join(" "));
    let title_text = if files.len() == 1 {
        tokens(files[0].name.rsplit(['/', '\\']).next().unwrap_or(""))
    } else {
        let common_parent = files[0]
            .name
            .rsplit_once(['/', '\\'])
            .map(|(parent, _)| parent);
        if common_parent.is_none()
            || files.iter().any(|file| {
                file.name.rsplit_once(['/', '\\']).map(|(parent, _)| parent) != common_parent
            })
        {
            return false;
        }
        tokens(common_parent.unwrap_or(""))
    };
    if !overlap(&tokens(wanted.artist), &all_text) || !overlap(&tokens(wanted.title), &title_text) {
        return false;
    }
    let source_versions = version_tokens(&format!("{} {}", wanted.title, wanted.album));
    if version_tokens(&names.join(" "))
        .iter()
        .any(|version| !source_versions.contains(version))
    {
        return false;
    }
    let Some(expected) = wanted.duration else {
        return true;
    };
    let durations = files
        .iter()
        .map(|file| file.duration_seconds.map(f64::from))
        .collect::<Option<Vec<_>>>();
    durations
        .is_some_and(|values| (values.iter().sum::<f64>() - expected).abs() <= DURATION_TOLERANCE)
}

fn version_tokens(value: &str) -> HashSet<String> {
    tokens(value)
        .into_iter()
        .filter(|word| {
            matches!(
                word.as_str(),
                "live"
                    | "remix"
                    | "remaster"
                    | "remastered"
                    | "cover"
                    | "instrumental"
                    | "karaoke"
                    | "demo"
                    | "extended"
            )
        })
        .collect()
}

fn quality_score(
    format: &str,
    bitrate: Option<u32>,
    sample_rate: Option<u32>,
    bit_depth: Option<u32>,
    prefer: &str,
) -> f64 {
    let lossless = muzik_soulseek::ranking::is_lossless(format)
        || muzik_core::audio::is_lossless_codec(format);
    let mut score = if lossless {
        100.0
    } else if format == "mp3" {
        50.0
    } else if !format.is_empty() {
        40.0
    } else {
        0.0
    };
    if (prefer == "lossless" && lossless)
        || (prefer == "mp3-320" && format == "mp3" && bitrate == Some(320))
        || prefer == format
    {
        score += 30.0;
    }
    if let Some(rate) = bitrate {
        score += f64::from(rate.min(320)) / 10.0;
    }
    if let Some(rate) = sample_rate {
        score += f64::from(rate.min(192_000)) / 48_000.0;
    }
    if let Some(depth) = bit_depth {
        score += f64::from(depth) / 4.0;
    }
    score
}

pub fn candidate_id(candidate: &Candidate) -> Result<String, String> {
    let bytes = serde_json::to_vec(candidate).map_err(|error| error.to_string())?;
    let digest = Sha256::digest(bytes);
    let hex = format!("{digest:x}");
    Ok(hex.get(..16).unwrap_or(&hex).to_owned())
}

fn cache_path(root: &Path, id: &str) -> Result<PathBuf, String> {
    if id.len() != 16 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("candidate ID must contain 16 hexadecimal digits".into());
    }
    Ok(root.join(format!("soulseek_{id}.json")))
}

pub fn save_candidate(root: &Path, id: &str, candidate: &CachedCandidate) -> Result<(), String> {
    let path = cache_path(root, id)?;
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(candidate).map_err(|error| error.to_string())?;
    fs::write(&path, bytes).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

pub fn load_candidate(root: &Path, id: &str) -> Result<CachedCandidate, String> {
    let path = cache_path(root, id)?;
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let candidate: CachedCandidate = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid Soulseek candidate: {error}"))?;
    if candidate_id(&candidate.candidate)? != id {
        return Err("cached Soulseek candidate ID does not match its files".into());
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::{
        CachedCandidate, FlaggedTrack, Wanted, candidate_id, load_candidate, safe_match,
        save_candidate, scan_library, select_upgrade,
    };
    use muzik_library::{Fields, Library, SqlValue};
    use muzik_media::quality::MeasuredQuality;
    use muzik_soulseek::ranking::RankedCandidate;
    use muzik_soulseek::types::{Candidate, FileEntry};
    use std::fs;
    use std::path::Path;

    fn candidate(names: &[&str]) -> Candidate {
        Candidate {
            username: "peer".into(),
            slots: 1,
            speed: 100_000,
            files: names
                .iter()
                .map(|name| FileEntry {
                    name: (*name).to_owned(),
                    size: 100,
                    bitrate_kbps: None,
                    duration_seconds: None,
                    vbr: None,
                    sample_rate_hz: None,
                    bit_depth: None,
                })
                .collect(),
        }
    }

    #[test]
    fn saved_candidate_round_trips_and_keeps_its_identity() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let selected = candidate(&["Album\\01 Song.flac"]);
        let id = candidate_id(&selected)?;
        save_candidate(
            dir.path(),
            &id,
            &CachedCandidate {
                query: "Artist Song".into(),
                score: 120.0,
                candidate: selected,
            },
        )?;
        let loaded = load_candidate(dir.path(), &id)?;
        assert_eq!(loaded.query, "Artist Song");
        assert_eq!(
            loaded
                .candidate
                .files
                .first()
                .map(|file| file.name.as_str()),
            Some("Album\\01 Song.flac")
        );
        let path = dir.path().join(format!("soulseek_{id}.json"));
        let changed =
            fs::read_to_string(&path)?.replace("Album\\\\01 Song.flac", "Album\\\\02 Song.flac");
        fs::write(&path, changed)?;
        assert!(load_candidate(dir.path(), &id).is_err());
        Ok(())
    }

    fn low_quality_track() -> FlaggedTrack {
        FlaggedTrack {
            artist: "Mara Vale".into(),
            sort_artist: "Mara Vale".into(),
            album: "Night Lines".into(),
            title: "Moon River".into(),
            duration: Some(180.0),
            quality: MeasuredQuality {
                format: "mp3".into(),
                lossless: false,
                bitrate_kbps: Some(128),
                sample_rate: Some(44_100),
                bit_depth: None,
                channels: Some(2),
                size: Some(100),
            },
        }
    }

    fn file(name: &str, duration: Option<u32>) -> FileEntry {
        FileEntry {
            name: name.into(),
            size: 100,
            bitrate_kbps: Some(900),
            duration_seconds: duration,
            vbr: None,
            sample_rate_hz: Some(44_100),
            bit_depth: Some(16),
        }
    }

    fn one(file: FileEntry) -> Candidate {
        Candidate {
            username: "peer".into(),
            slots: 1,
            speed: 1,
            files: vec![file],
        }
    }

    fn moon_river() -> Wanted<'static> {
        Wanted {
            artist: "Mara Vale",
            title: "Moon River",
            album: "Night Lines",
            duration: Some(180.0),
        }
    }

    #[test]
    fn replacement_needs_title_artist_and_duration_evidence() {
        let wanted = moon_river();
        assert!(safe_match(
            &one(file("Mara Vale/Moon River.flac", Some(183))),
            &wanted
        ));
        assert!(!safe_match(
            &one(file("Mara Vale/Another Song.flac", Some(180))),
            &wanted
        ));
        assert!(!safe_match(
            &one(file("Mara Vale/Moon River/Another Song.flac", Some(180))),
            &wanted
        ));
        assert!(!safe_match(
            &one(file("Mara Other/Moon River.flac", Some(180))),
            &wanted
        ));
        assert!(!safe_match(
            &one(file("Mara Vale/Moon Lake.flac", Some(180))),
            &wanted
        ));
        assert!(!safe_match(
            &one(file("Mara Vale/Moon River Live.flac", Some(180))),
            &wanted
        ));
        assert!(!safe_match(
            &one(file("Mara Vale/Moon River.flac", Some(205))),
            &wanted
        ));
        assert!(!safe_match(
            &one(file("Mara Vale/Moon River.flac", None)),
            &wanted
        ));
    }

    #[test]
    fn rejects_wrong_title_or_duration_before_download() {
        let wanted = Wanted {
            artist: "Artist",
            title: "Album",
            album: "",
            duration: Some(3600.0),
        };
        assert!(!safe_match(
            &one(file("Artist/Other/Artist - Other.flac", Some(3600))),
            &wanted
        ));
        assert!(!safe_match(
            &one(file("Artist/Album/Artist - Album.flac", Some(100))),
            &wanted
        ));
        assert!(!safe_match(
            &one(file("Artist/Album/Artist - Album Remix.flac", Some(3600))),
            &wanted
        ));
    }

    #[test]
    fn extended_and_demo_files_need_the_marker_in_the_source() {
        let file = one(file("Mara Vale/Moon River (Extended Mix).flac", Some(180)));
        assert!(!safe_match(&file, &moon_river()));
        assert!(safe_match(
            &file,
            &Wanted {
                title: "Moon River (Extended Mix)",
                ..moon_river()
            }
        ));
    }

    #[test]
    fn remastered_files_need_the_marker_in_the_source_or_album() {
        let file = one(file("Mara Vale/Moon River (Remastered).flac", Some(180)));
        assert!(!safe_match(&file, &moon_river()));
        assert!(safe_match(
            &file,
            &Wanted {
                album: "Night Lines (Remastered)",
                ..moon_river()
            }
        ));
    }

    #[test]
    fn unknown_wanted_duration_skips_the_duration_check() {
        let wanted = Wanted {
            duration: None,
            ..moon_river()
        };
        assert!(safe_match(
            &one(file("Mara Vale/Moon River.flac", None)),
            &wanted
        ));
    }

    #[test]
    fn user_name_is_not_artist_evidence() {
        let mut candidate = one(file("Uploads/Moon River.flac", Some(180)));
        candidate.username = "mara_vale".into();
        assert!(!safe_match(&candidate, &moon_river()));
    }

    #[test]
    fn album_candidates_match_the_title_against_their_folder() {
        let wanted = Wanted {
            artist: "Artist",
            title: "Album",
            album: "",
            duration: Some(360.0),
        };
        let together = Candidate {
            files: vec![
                file("Artist/Album/01 a.flac", Some(180)),
                file("Artist/Album/02 b.flac", Some(180)),
            ],
            ..one(file("x.flac", None))
        };
        assert!(safe_match(&together, &wanted));
        let apart = Candidate {
            files: vec![
                file("Artist/Album/01 a.flac", Some(180)),
                file("Artist/Other/02 b.flac", Some(180)),
            ],
            ..one(file("x.flac", None))
        };
        assert!(!safe_match(&apart, &wanted));
    }

    #[test]
    fn suggestion_caches_only_the_matching_file() {
        let ranked = vec![RankedCandidate {
            candidate: Candidate {
                username: "peer".into(),
                slots: 1,
                speed: 100_000,
                files: vec![
                    file("Mara Vale/Another Song.flac", Some(180)),
                    file("Mara Vale/Moon River.flac", Some(182)),
                ],
            },
            score: 123.0,
        }];
        let (chosen, _) = select_upgrade(&low_quality_track(), &ranked, "lossless")
            .expect("a safe candidate exists");
        assert_eq!(chosen.files.len(), 1);
        assert_eq!(chosen.files[0].name, "Mara Vale/Moon River.flac");
    }

    #[test]
    fn suggestion_requires_better_audio_quality() {
        let mut weaker = file("Mara Vale/Moon River.mp3", Some(180));
        weaker.bitrate_kbps = Some(96);
        weaker.bit_depth = None;
        let ranked = vec![RankedCandidate {
            candidate: Candidate {
                username: "peer".into(),
                slots: 1,
                speed: 100_000,
                files: vec![weaker],
            },
            score: 80.0,
        }];
        assert!(select_upgrade(&low_quality_track(), &ranked, "lossless").is_none());
    }

    #[test]
    fn library_scan_uses_beets_query_and_resolves_relative_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let music = dir.path().join("Music");
        fs::create_dir_all(&music)?;
        fs::write(music.join("song.mp3"), b"audio")?;
        let database = dir.path().join("library.db");
        let mut library = Library::open_or_create(&database)?;
        for (artist, title) in [("Mara Vale", "Moon River"), ("Other", "Elsewhere")] {
            let mut fields = Fields::new();
            fields.insert("path".into(), SqlValue::Text("song.mp3".into()));
            fields.insert("artist".into(), SqlValue::Text(artist.into()));
            fields.insert("title".into(), SqlValue::Text(title.into()));
            fields.insert("length".into(), SqlValue::Real(180.0));
            library.insert_item(&fields, &Fields::new())?;
        }
        drop(library);
        let library = Library::open_read_only(&database)?;
        let items = library.query_items("artist:Mara")?;
        let (scanned, flagged) = scan_library(items, &music, 256, |path: &Path| {
            assert_eq!(path, music.join("song.mp3"));
            Ok(Some(low_quality_track().quality))
        })?;
        assert_eq!(scanned, 1);
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0].artist, "Mara Vale");
        assert_eq!(flagged[0].duration, Some(180.0));
        Ok(())
    }
}
