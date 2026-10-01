//! Safe quality replacement for a freshly acquired YouTube audio file.

use muzik_core::chapters::sidecar_path;
use muzik_core::quality::{self, MeasuredQuality, QualityDecision};
use muzik_core::{DecisionKind, QualityPolicy, app_config, paths};
use muzik_soulseek::fetch::Timeouts;
use muzik_soulseek::ranking::{format as file_format, rank};
use muzik_soulseek::session::{Session, SessionSettings};
use muzik_soulseek::types::Candidate;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const DURATION_TOLERANCE: f64 = 10.0;

#[derive(Debug)]
pub struct QualityUpgradeResult {
    pub audio_files: Vec<PathBuf>,
    pub pre_split_dirs: Vec<PathBuf>,
}

impl QualityUpgradeResult {
    fn keep(audio_files: &[PathBuf]) -> Self {
        Self {
            audio_files: audio_files.to_vec(),
            pre_split_dirs: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct Track {
    artist: String,
    title: String,
    duration: f64,
}

trait Backend {
    fn download_dir(&self) -> Result<PathBuf, String>;
    fn measure(&mut self, path: &Path) -> Result<Option<MeasuredQuality>, String>;
    fn track(&mut self, path: &Path) -> Result<Option<Track>, String>;
    fn search(
        &mut self,
        query: &str,
        prefer: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Candidate>, String>;
    fn download(
        &mut self,
        candidate: &Candidate,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Vec<PathBuf>, String>;
    fn duration(&mut self, path: &Path) -> Result<Option<f64>, String>;
}

/// Return the original audio if a replacement cannot be verified. The caller
/// owns any returned replacement directory and must remove it after import.
pub fn check_youtube_quality(
    audio_files: Vec<PathBuf>,
    policy: QualityPolicy,
    min_bitrate: u32,
    prefer: &str,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<QualityUpgradeResult, String> {
    let mut backend = SoulseekBackend { session: None };
    check_with_backend(
        &mut backend,
        audio_files,
        policy,
        min_bitrate,
        prefer,
        cancelled,
        on_event,
        decide,
    )
}

#[allow(clippy::too_many_arguments)]
fn check_with_backend(
    backend: &mut dyn Backend,
    audio_files: Vec<PathBuf>,
    policy: QualityPolicy,
    min_bitrate: u32,
    prefer: &str,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<QualityUpgradeResult, String> {
    let keep = QualityUpgradeResult::keep(&audio_files);
    if policy == QualityPolicy::Off || audio_files.is_empty() {
        return Ok(keep);
    }
    check_cancelled(cancelled)?;
    let primary = &audio_files[0];
    let current = match backend.measure(primary) {
        Ok(Some(current)) => current,
        Ok(None) => return Ok(keep),
        Err(error) => {
            on_event(message(
                format!("Quality check: cannot measure source audio: {error}"),
                true,
            ));
            return Ok(keep);
        }
    };
    let quality_decision = quality::decide(&current, policy, min_bitrate);
    on_event(message(
        format!(
            "Quality check: {} is {}, {} kbps.",
            primary.display(),
            current.format,
            current
                .bitrate_kbps
                .map_or_else(|| "unknown".into(), |value| value.to_string())
        ),
        false,
    ));
    if quality_decision == QualityDecision::Keep {
        return Ok(keep);
    }
    let no_safe = || QualityUpgradeResult::keep(&audio_files);
    let track = match backend.track(primary) {
        Ok(Some(track))
            if !tokens(&track.artist).is_empty() && !tokens(&track.title).is_empty() =>
        {
            track
        }
        _ => {
            on_event(message(
                "Quality check: source artist or title is unknown.".into(),
                true,
            ));
            return Ok(no_safe());
        }
    };
    check_cancelled(cancelled)?;
    let query = format!("{} {}", track.artist, track.title);
    let candidates = match backend.search(&query, prefer, cancelled) {
        Ok(candidates) => candidates,
        Err(error) if cancelled.load(Ordering::SeqCst) => return Err(error),
        Err(error) => {
            on_event(message(
                format!("Quality check: Soulseek search failed: {error}"),
                true,
            ));
            return Ok(no_safe());
        }
    };
    check_cancelled(cancelled)?;
    let selected = rank(candidates, &query, prefer, 20)
        .into_iter()
        .map(|item| item.candidate)
        .find(|candidate| safe_match(candidate, &track) && better(candidate, &current));
    let Some(candidate) = selected else {
        on_event(message(
            "Quality check: no safe, better Soulseek file was found.".into(),
            false,
        ));
        return Ok(no_safe());
    };
    on_event(
        json!({"event":"candidates_found","data":{"candidates":[candidate_payload(&candidate)],"source":"soulseek","limit":10}}),
    );
    if quality_decision == QualityDecision::Ask {
        let answer = decide(
            DecisionKind::QualityReplacement,
            json!({"current":primary,"candidate":candidate_payload(&candidate)}),
        )?;
        check_cancelled(cancelled)?;
        match answer.as_bool() {
            Some(true) => {}
            Some(false) => return Ok(keep),
            None => return Err("Quality replacement needs a boolean reply.".into()),
        }
    }
    let output = match backend.download_dir() {
        Ok(output) => output,
        Err(error) => {
            on_event(message(
                format!("Quality check: cannot prepare download: {error}"),
                true,
            ));
            return Ok(no_safe());
        }
    };
    let destination = match tempfile::Builder::new()
        .prefix("quality-")
        .tempdir_in(output)
    {
        Ok(destination) => destination,
        Err(error) => {
            on_event(message(
                format!("Quality check: cannot prepare download: {error}"),
                true,
            ));
            return Ok(no_safe());
        }
    };
    let files = match backend.download(&candidate, destination.path(), cancelled) {
        Ok(files) => files,
        Err(error) if cancelled.load(Ordering::SeqCst) => return Err(error),
        Err(error) => {
            on_event(message(
                format!("Quality check: Soulseek download failed: {error}"),
                true,
            ));
            return Ok(no_safe());
        }
    };
    check_cancelled(cancelled)?;
    if files.len() != candidate.files.len()
        || files.is_empty()
        || files
            .iter()
            .any(|file| !file.is_file() || !file.starts_with(destination.path()))
    {
        on_event(message(
            "Quality check: Soulseek download is incomplete.".into(),
            true,
        ));
        return Ok(no_safe());
    }
    if files.len() == 1 {
        let replacement = &files[0];
        let measured = backend.measure(replacement).ok().flatten();
        let duration = backend.duration(replacement).ok().flatten();
        if !measured
            .as_ref()
            .is_some_and(|value| measured_better(value, &current))
            || !duration.is_some_and(|value| (value - track.duration).abs() <= DURATION_TOLERANCE)
        {
            on_event(message(
                "Quality check: downloaded audio failed quality or duration checks.".into(),
                true,
            ));
            return Ok(no_safe());
        }
        if let Err(error) = copy_chapter_sidecars(primary, replacement) {
            on_event(message(
                format!("Quality check: cannot copy chapters: {error}"),
                true,
            ));
            return Ok(no_safe());
        }
        let root = destination.keep();
        let replacement = root.join(
            replacement
                .file_name()
                .ok_or("Replacement has no file name")?,
        );
        on_event(message(
            "Quality check: Soulseek replacement is ready.".into(),
            false,
        ));
        return Ok(QualityUpgradeResult {
            audio_files: vec![replacement],
            pre_split_dirs: Vec::new(),
        });
    }
    // Every part of a multi-file album must pass a post-download quality check.
    if files.iter().any(|file| {
        !backend
            .measure(file)
            .ok()
            .flatten()
            .as_ref()
            .is_some_and(|value| measured_better(value, &current))
    }) {
        on_event(message(
            "Quality check: an album file failed the quality check.".into(),
            true,
        ));
        return Ok(no_safe());
    }
    let actual_duration = files
        .iter()
        .map(|file| backend.duration(file).ok().flatten())
        .collect::<Option<Vec<_>>>();
    if !actual_duration.is_some_and(|values| {
        (values.iter().sum::<f64>() - track.duration).abs() <= DURATION_TOLERANCE
    }) {
        on_event(message(
            "Quality check: album duration does not match the source.".into(),
            true,
        ));
        return Ok(no_safe());
    }
    let root = destination.keep();
    on_event(message(
        "Quality check: Soulseek album is ready.".into(),
        false,
    ));
    Ok(QualityUpgradeResult {
        audio_files: Vec::new(),
        pre_split_dirs: vec![root],
    })
}

fn message(text: String, warning: bool) -> Value {
    json!({"event":"message","data":{"message":text,"severity":if warning {"warning"} else {"info"}}})
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::SeqCst) {
        Err("quality check cancelled".into())
    } else {
        Ok(())
    }
}

fn tokens(value: &str) -> HashSet<String> {
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

fn safe_match(candidate: &Candidate, track: &Track) -> bool {
    if candidate.username.trim().is_empty() || candidate.files.is_empty() {
        return false;
    }
    let files = candidate
        .files
        .iter()
        .filter(|file| !file_format(file).is_empty())
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
    if !overlap(&tokens(&track.artist), &all_text) || !overlap(&tokens(&track.title), &title_text) {
        return false;
    }
    let source_versions = version_tokens(&track.title);
    if version_tokens(&names.join(" "))
        .iter()
        .any(|version| !source_versions.contains(version))
    {
        return false;
    }
    let durations = files
        .iter()
        .map(|file| file.duration_seconds.map(f64::from))
        .collect::<Option<Vec<_>>>();
    durations.is_some_and(|values| {
        (values.iter().sum::<f64>() - track.duration).abs() <= DURATION_TOLERANCE
    })
}

fn version_tokens(value: &str) -> HashSet<String> {
    tokens(value)
        .into_iter()
        .filter(|word| {
            matches!(
                word.as_str(),
                "live" | "remix" | "remaster" | "remastered" | "cover" | "instrumental" | "karaoke"
            )
        })
        .collect()
}

fn better(candidate: &Candidate, current: &MeasuredQuality) -> bool {
    candidate.files.iter().all(|file| {
        let format = file_format(file);
        let lossless = matches!(format, "flac" | "alac" | "wav" | "aiff" | "ape" | "wv");
        if lossless && !current.lossless {
            return true;
        }
        if lossless != current.lossless {
            return false;
        }
        match (file.bitrate_kbps, current.bitrate_kbps) {
            (Some(new), Some(old)) => new > old,
            _ => false,
        }
    })
}

fn measured_better(new: &MeasuredQuality, current: &MeasuredQuality) -> bool {
    (new.lossless && !current.lossless)
        || (new.lossless == current.lossless
            && matches!((new.bitrate_kbps, current.bitrate_kbps), (Some(new), Some(old)) if new > old))
}

fn candidate_payload(candidate: &Candidate) -> Value {
    let first = &candidate.files[0];
    json!({
        "username":candidate.username,
        "title":first.name.rsplit(['/', '\\']).next().unwrap_or("Audio file"),
        "quality":{"format":file_format(first).to_ascii_uppercase(),"bitrate":first.bitrate_kbps},
        "files":candidate.files,
    })
}

fn copy_chapter_sidecars(original: &Path, replacement: &Path) -> Result<(), String> {
    for extension in [".chapters.txt", ".info.json"] {
        let source = sidecar_path(original, extension);
        if source.is_file() {
            std::fs::copy(&source, sidecar_path(replacement, extension))
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn quality_download_dir() -> Result<PathBuf, String> {
    let path = paths::data_dir().join("soulseek");
    std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    Ok(path)
}

struct SoulseekBackend {
    session: Option<(Arc<Session>, Timeouts)>,
}

impl SoulseekBackend {
    fn session(&mut self) -> Result<(&Session, Timeouts), String> {
        if self.session.is_none() {
            let config = app_config::load(&app_config::path())?;
            let settings = SessionSettings::configured(&config)
                .ok_or("Set Soulseek credentials in configuration first.")?;
            self.session = Some((
                Session::shared(settings).map_err(|error| error.to_string())?,
                Timeouts::configured(&config),
            ));
        }
        self.session
            .as_ref()
            .map(|(session, timeouts)| (session.as_ref(), *timeouts))
            .ok_or("Soulseek session is not available".into())
    }
}

impl Backend for SoulseekBackend {
    fn download_dir(&self) -> Result<PathBuf, String> {
        quality_download_dir()
    }

    fn measure(&mut self, path: &Path) -> Result<Option<MeasuredQuality>, String> {
        quality::measure(path)
    }

    fn track(&mut self, path: &Path) -> Result<Option<Track>, String> {
        let mut artist = String::new();
        let mut title = String::new();
        for extension in [".muzik.json", ".info.json"] {
            let sidecar = sidecar_path(path, extension);
            if let Ok(bytes) = std::fs::read(sidecar)
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            {
                let sources = [&value["resolved"], &value["candidate"]["metadata"], &value];
                for source in sources {
                    if artist.is_empty() {
                        artist = source["artist"].as_str().unwrap_or("").to_owned();
                    }
                    if title.is_empty() {
                        title = source["title"]
                            .as_str()
                            .or_else(|| source["track"].as_str())
                            .unwrap_or("")
                            .to_owned();
                    }
                }
            }
        }
        if let Ok(tags) = muzik_tags::read(path, &[]) {
            if artist.is_empty() {
                artist = tags.fields.get("artist").cloned().unwrap_or_default();
            }
            if title.is_empty() {
                title = tags.fields.get("title").cloned().unwrap_or_default();
            }
        }
        let duration = muzik_tags::probe(path)
            .map_err(|error| error.to_string())?
            .duration_seconds;
        Ok(duration.map(|duration| Track {
            artist,
            title,
            duration,
        }))
    }

    fn search(
        &mut self,
        query: &str,
        prefer: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Candidate>, String> {
        let (session, timeouts) = self.session()?;
        Ok(session
            .search(query, prefer, 20, timeouts.search, cancelled)?
            .into_iter()
            .map(|item| item.candidate)
            .collect())
    }

    fn download(
        &mut self,
        candidate: &Candidate,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Vec<PathBuf>, String> {
        let (session, timeouts) = self.session()?;
        session.fetch(candidate, destination, timeouts.download, cancelled)
    }

    fn duration(&mut self, path: &Path) -> Result<Option<f64>, String> {
        muzik_tags::probe(path)
            .map(|properties| properties.duration_seconds)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use muzik_soulseek::types::FileEntry;

    struct FakeBackend {
        root: tempfile::TempDir,
        candidates: Vec<Candidate>,
        source_error: bool,
        download_error: bool,
    }

    impl FakeBackend {
        fn new() -> Self {
            Self {
                root: tempfile::tempdir().unwrap(),
                candidates: vec![candidate(
                    "Artist/Album/Artist - Album.flac",
                    3600,
                    Some(950),
                )],
                source_error: false,
                download_error: false,
            }
        }

        fn original(&self) -> PathBuf {
            let path = self.root.path().join("source.mp3");
            std::fs::write(&path, b"source").unwrap();
            path
        }
    }

    impl Backend for FakeBackend {
        fn download_dir(&self) -> Result<PathBuf, String> {
            Ok(self.root.path().to_path_buf())
        }

        fn measure(&mut self, path: &Path) -> Result<Option<MeasuredQuality>, String> {
            if self.source_error && path.file_name().is_some_and(|name| name == "source.mp3") {
                return Err("probe failed".into());
            }
            let lossless = path
                .extension()
                .is_some_and(|extension| extension == "flac");
            Ok(Some(MeasuredQuality {
                format: if lossless { "flac" } else { "mp3" }.into(),
                lossless,
                bitrate_kbps: Some(if lossless { 950 } else { 128 }),
                sample_rate: None,
                bit_depth: None,
                channels: None,
                size: None,
            }))
        }

        fn track(&mut self, _path: &Path) -> Result<Option<Track>, String> {
            Ok(Some(Track {
                artist: "Artist".into(),
                title: "Album".into(),
                duration: 3600.0,
            }))
        }

        fn search(
            &mut self,
            _query: &str,
            _prefer: &str,
            _cancelled: &AtomicBool,
        ) -> Result<Vec<Candidate>, String> {
            Ok(self.candidates.clone())
        }

        fn download(
            &mut self,
            candidate: &Candidate,
            destination: &Path,
            _cancelled: &AtomicBool,
        ) -> Result<Vec<PathBuf>, String> {
            if self.download_error {
                return Err("peer left".into());
            }
            let mut files = Vec::new();
            for file in &candidate.files {
                let path = destination.join(file.name.rsplit(['/', '\\']).next().unwrap());
                std::fs::write(&path, b"replacement").map_err(|error| error.to_string())?;
                files.push(path);
            }
            Ok(files)
        }

        fn duration(&mut self, _path: &Path) -> Result<Option<f64>, String> {
            Ok(Some(3600.0))
        }
    }

    fn candidate(name: &str, duration: u32, bitrate: Option<u32>) -> Candidate {
        Candidate {
            username: "peer".into(),
            slots: 1,
            speed: 100,
            files: vec![FileEntry {
                name: name.into(),
                size: 100,
                bitrate_kbps: bitrate,
                duration_seconds: Some(duration),
                vbr: None,
                sample_rate_hz: None,
                bit_depth: None,
            }],
        }
    }

    fn run(
        backend: &mut FakeBackend,
        original: PathBuf,
        policy: QualityPolicy,
        answer: bool,
    ) -> QualityUpgradeResult {
        check_with_backend(
            backend,
            vec![original],
            policy,
            320,
            "lossless",
            &AtomicBool::new(false),
            &mut |_| {},
            &mut |kind, _| {
                assert_eq!(kind, muzik_core::DecisionKind::QualityReplacement);
                Ok(json!(answer))
            },
        )
        .unwrap()
    }

    #[test]
    fn rejects_wrong_title_or_duration_before_download() {
        let track = Track {
            artist: "Artist".into(),
            title: "Album".into(),
            duration: 3600.0,
        };
        assert!(!safe_match(
            &candidate("Artist/Other/Artist - Other.flac", 3600, Some(950)),
            &track
        ));
        assert!(!safe_match(
            &candidate("Artist/Album/Artist - Album.flac", 100, Some(950)),
            &track
        ));
        assert!(!safe_match(
            &candidate("Artist/Album/Artist - Album Remix.flac", 3600, Some(950)),
            &track
        ));
    }

    #[test]
    fn keeps_source_when_probe_fails_or_download_fails() {
        let mut backend = FakeBackend::new();
        let original = backend.original();
        backend.source_error = true;
        let result = run(&mut backend, original.clone(), QualityPolicy::Auto, true);
        assert_eq!(result.audio_files, vec![original.clone()]);
        backend.source_error = false;
        backend.download_error = true;
        let result = run(&mut backend, original.clone(), QualityPolicy::Auto, true);
        assert_eq!(result.audio_files, vec![original.clone()]);
        assert!(original.is_file());
    }

    #[test]
    fn ask_requires_acceptance_and_auto_copies_chapters() {
        let mut backend = FakeBackend::new();
        let original = backend.original();
        std::fs::write(sidecar_path(&original, ".chapters.txt"), "00:00 Intro").unwrap();
        let refused = run(&mut backend, original.clone(), QualityPolicy::Ask, false);
        assert_eq!(refused.audio_files, vec![original.clone()]);
        let accepted = run(&mut backend, original.clone(), QualityPolicy::Auto, true);
        assert_ne!(accepted.audio_files, vec![original.clone()]);
        assert!(original.is_file());
        assert_eq!(
            std::fs::read_to_string(sidecar_path(&accepted.audio_files[0], ".chapters.txt"))
                .unwrap(),
            "00:00 Intro"
        );
    }

    #[test]
    fn cancellation_stops_before_search() {
        let mut backend = FakeBackend::new();
        let original = backend.original();
        let error = check_with_backend(
            &mut backend,
            vec![original],
            QualityPolicy::Auto,
            320,
            "lossless",
            &AtomicBool::new(true),
            &mut |_| {},
            &mut |_, _| Ok(json!(true)),
        )
        .unwrap_err();
        assert_eq!(error, "quality check cancelled");
    }
}
