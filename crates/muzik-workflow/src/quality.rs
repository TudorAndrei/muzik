//! Safe quality replacement for a freshly acquired `YouTube` audio file.

use muzik_core::audio::AudioFormat;
use muzik_core::chapters::sidecar_path;
use muzik_core::paths::Paths;
use muzik_core::{DecisionKind, JobEvent, PreferredAudio, QualityPolicy, Severity, app_config};
use muzik_media::quality::{self, MeasuredQuality, QualityDecision};
use muzik_soulseek::fetch::Timeouts;
use muzik_soulseek::ranking::{format as file_format, rank};
use muzik_soulseek::session::{Session, SessionSettings};
use muzik_soulseek::types::Candidate;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::Result;
use crate::upgrade::{DURATION_TOLERANCE, Wanted, safe_match, tokens};

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
    fn download_dir(&self) -> Result<PathBuf>;
    fn measure(&mut self, path: &Path) -> Result<Option<MeasuredQuality>>;
    fn track(&mut self, path: &Path) -> Result<Option<Track>>;
    fn search(
        &mut self,
        query: &str,
        prefer: PreferredAudio,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Candidate>>;
    fn download(
        &mut self,
        candidate: &Candidate,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Vec<PathBuf>>;
    fn duration(&mut self, path: &Path) -> Result<Option<f64>>;
}

/// Return the original audio if a replacement cannot be verified. The caller
/// owns any returned replacement directory and must remove it after import.
///
/// # Errors
/// Returns an error when the check is cancelled, the replacement reply is not a boolean, or the
/// decision callback fails.
#[expect(
    clippy::too_many_arguments,
    reason = "public entry point that takes independent settings and callbacks from callers in other crates"
)]
#[expect(
    clippy::needless_pass_by_value,
    reason = "muzik-runner passes the files by value; a slice would change its pub signature"
)]
pub fn check_youtube_quality(
    paths: &Paths,
    audio_files: Vec<PathBuf>,
    policy: QualityPolicy,
    min_bitrate: u32,
    prefer: PreferredAudio,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<QualityUpgradeResult> {
    let mut backend = SoulseekBackend {
        paths: paths.clone(),
        session: None,
    };
    check_with_backend(
        &mut backend,
        &audio_files,
        policy,
        min_bitrate,
        prefer,
        cancelled,
        on_event,
        decide,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "takes the check_youtube_quality arguments plus the backend that tests replace"
)]
fn check_with_backend(
    backend: &mut dyn Backend,
    audio_files: &[PathBuf],
    policy: QualityPolicy,
    min_bitrate: u32,
    prefer: PreferredAudio,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<QualityUpgradeResult> {
    let keep = QualityUpgradeResult::keep(audio_files);
    let Some(primary) = audio_files.first().filter(|_| policy != QualityPolicy::Off) else {
        return Ok(keep);
    };
    check_cancelled(cancelled)?;
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
    let Some((track, candidate)) =
        find_candidate(backend, primary, &current, prefer, cancelled, on_event)?
    else {
        return Ok(keep);
    };
    on_event(JobEvent::CandidatesFound {
        source: "soulseek".into(),
        candidates: vec![candidate_payload(&candidate)],
    });
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
    let Some((destination, files)) = download_candidate(backend, &candidate, cancelled, on_event)?
    else {
        return Ok(keep);
    };
    let accepted = if let [replacement] = files.as_slice() {
        accept_single(
            backend,
            primary,
            replacement,
            destination,
            &current,
            &track,
            on_event,
        )?
    } else {
        accept_album(backend, &files, destination, &current, &track, on_event)
    };
    Ok(accepted.unwrap_or(keep))
}

fn find_candidate(
    backend: &mut dyn Backend,
    primary: &Path,
    current: &MeasuredQuality,
    prefer: PreferredAudio,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<Option<(Track, Candidate)>> {
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
            return Ok(None);
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
            return Ok(None);
        }
    };
    check_cancelled(cancelled)?;
    let selected = rank(candidates, &query, prefer, 20)
        .into_iter()
        .map(|item| item.candidate)
        .find(|candidate| {
            safe_match(
                candidate,
                &Wanted {
                    artist: &track.artist,
                    title: &track.title,
                    album: "",
                    duration: Some(track.duration),
                },
            ) && better(candidate, current)
        });
    let Some(candidate) = selected else {
        on_event(message(
            "Quality check: no safe, better Soulseek file was found.".into(),
            false,
        ));
        return Ok(None);
    };
    Ok(Some((track, candidate)))
}

fn download_candidate(
    backend: &mut dyn Backend,
    candidate: &Candidate,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<Option<(tempfile::TempDir, Vec<PathBuf>)>> {
    let output = match backend.download_dir() {
        Ok(output) => output,
        Err(error) => {
            on_event(message(
                format!("Quality check: cannot prepare download: {error}"),
                true,
            ));
            return Ok(None);
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
            return Ok(None);
        }
    };
    let files = match backend.download(candidate, destination.path(), cancelled) {
        Ok(files) => files,
        Err(error) if cancelled.load(Ordering::SeqCst) => return Err(error),
        Err(error) => {
            on_event(message(
                format!("Quality check: Soulseek download failed: {error}"),
                true,
            ));
            return Ok(None);
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
        return Ok(None);
    }
    Ok(Some((destination, files)))
}

fn accept_single(
    backend: &mut dyn Backend,
    primary: &Path,
    replacement: &Path,
    destination: tempfile::TempDir,
    current: &MeasuredQuality,
    track: &Track,
    on_event: &mut dyn FnMut(JobEvent),
) -> Result<Option<QualityUpgradeResult>> {
    let measured = backend.measure(replacement).ok().flatten();
    let duration = backend.duration(replacement).ok().flatten();
    if !measured
        .as_ref()
        .is_some_and(|value| measured_better(value, current))
        || !duration.is_some_and(|value| (value - track.duration).abs() <= DURATION_TOLERANCE)
    {
        on_event(message(
            "Quality check: downloaded audio failed quality or duration checks.".into(),
            true,
        ));
        return Ok(None);
    }
    if let Err(error) = copy_chapter_sidecars(primary, replacement) {
        on_event(message(
            format!("Quality check: cannot copy chapters: {error}"),
            true,
        ));
        return Ok(None);
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
    Ok(Some(QualityUpgradeResult {
        audio_files: vec![replacement],
        pre_split_dirs: Vec::new(),
    }))
}

fn accept_album(
    backend: &mut dyn Backend,
    files: &[PathBuf],
    destination: tempfile::TempDir,
    current: &MeasuredQuality,
    track: &Track,
    on_event: &mut dyn FnMut(JobEvent),
) -> Option<QualityUpgradeResult> {
    // Every part of a multi-file album must pass a post-download quality check.
    if files.iter().any(|file| {
        !backend
            .measure(file)
            .ok()
            .flatten()
            .as_ref()
            .is_some_and(|value| measured_better(value, current))
    }) {
        on_event(message(
            "Quality check: an album file failed the quality check.".into(),
            true,
        ));
        return None;
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
        return None;
    }
    let root = destination.keep();
    on_event(message(
        "Quality check: Soulseek album is ready.".into(),
        false,
    ));
    Some(QualityUpgradeResult {
        audio_files: Vec::new(),
        pre_split_dirs: vec![root],
    })
}

const fn message(text: String, warning: bool) -> JobEvent {
    JobEvent::Message {
        message: text,
        severity: if warning {
            Severity::Warning
        } else {
            Severity::Info
        },
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::SeqCst) {
        Err("quality check cancelled".into())
    } else {
        Ok(())
    }
}

fn better(candidate: &Candidate, current: &MeasuredQuality) -> bool {
    candidate.files.iter().all(|file| {
        let lossless = file_format(file).is_some_and(AudioFormat::is_lossless);
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

const fn measured_better(new: &MeasuredQuality, current: &MeasuredQuality) -> bool {
    (new.lossless && !current.lossless)
        || (new.lossless == current.lossless
            && matches!((new.bitrate_kbps, current.bitrate_kbps), (Some(new), Some(old)) if new > old))
}

fn candidate_payload(candidate: &Candidate) -> Value {
    let first = candidate.files.first();
    json!({
        "username":candidate.username,
        "title":first.and_then(|file| file.name.rsplit(['/', '\\']).next()).unwrap_or("Audio file"),
        "quality":{"format":first.and_then(file_format).map_or_else(String::new, |format| format.to_string().to_ascii_uppercase()),"bitrate":first.and_then(|file| file.bitrate_kbps)},
        "files":candidate.files,
    })
}

fn copy_chapter_sidecars(original: &Path, replacement: &Path) -> std::io::Result<()> {
    for extension in [".chapters.txt", ".info.json"] {
        let source = sidecar_path(original, extension);
        if source.is_file() {
            std::fs::copy(&source, sidecar_path(replacement, extension))?;
        }
    }
    Ok(())
}

struct SoulseekBackend {
    paths: Paths,
    session: Option<(Arc<Session>, Timeouts)>,
}

impl SoulseekBackend {
    fn session(&mut self) -> Result<(&Session, Timeouts)> {
        if self.session.is_none() {
            let config = app_config::load(&self.paths.config_file())?;
            let settings = SessionSettings::configured(&config)
                .ok_or("Set Soulseek credentials in configuration first.")?;
            self.session = Some((Session::shared(settings)?, Timeouts::configured(&config)));
        }
        self.session
            .as_ref()
            .map(|(session, timeouts)| (session.as_ref(), *timeouts))
            .ok_or_else(|| "Soulseek session is not available".into())
    }
}

impl Backend for SoulseekBackend {
    fn download_dir(&self) -> Result<PathBuf> {
        let path = self.paths.soulseek();
        std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
        Ok(path)
    }

    fn measure(&mut self, path: &Path) -> Result<Option<MeasuredQuality>> {
        Ok(quality::measure(path)?)
    }

    fn track(&mut self, path: &Path) -> Result<Option<Track>> {
        let mut artist = String::new();
        let mut title = String::new();
        for extension in [".muzik.json", ".info.json"] {
            let sidecar = sidecar_path(path, extension);
            if let Ok(bytes) = std::fs::read(sidecar)
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            {
                let sources = [
                    value.get("resolved"),
                    value.pointer("/candidate/metadata"),
                    Some(&value),
                ];
                for source in sources.into_iter().flatten() {
                    if artist.is_empty() {
                        source
                            .get("artist")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .clone_into(&mut artist);
                    }
                    if title.is_empty() {
                        source
                            .get("title")
                            .and_then(Value::as_str)
                            .or_else(|| source.get("track").and_then(Value::as_str))
                            .unwrap_or("")
                            .clone_into(&mut title);
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
        let duration = muzik_tags::probe(path)?.duration_seconds;
        Ok(duration.map(|duration| Track {
            artist,
            title,
            duration,
        }))
    }

    fn search(
        &mut self,
        query: &str,
        prefer: PreferredAudio,
        cancelled: &AtomicBool,
    ) -> Result<Vec<Candidate>> {
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
    ) -> Result<Vec<PathBuf>> {
        let (session, timeouts) = self.session()?;
        Ok(session.fetch(candidate, destination, timeouts.download, cancelled)?)
    }

    fn duration(&mut self, path: &Path) -> Result<Option<f64>> {
        Ok(muzik_tags::probe(path)?.duration_seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use muzik_core::audio::Codec;
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
        fn download_dir(&self) -> Result<PathBuf> {
            Ok(self.root.path().to_path_buf())
        }

        fn measure(&mut self, path: &Path) -> Result<Option<MeasuredQuality>> {
            if self.source_error && path.file_name().is_some_and(|name| name == "source.mp3") {
                return Err("probe failed".into());
            }
            let lossless = path
                .extension()
                .is_some_and(|extension| extension == "flac");
            Ok(Some(MeasuredQuality {
                format: if lossless { Codec::Flac } else { Codec::Mp3 },
                lossless,
                bitrate_kbps: Some(if lossless { 950 } else { 128 }),
                sample_rate: None,
                bit_depth: None,
                channels: None,
                size: None,
            }))
        }

        fn track(&mut self, _path: &Path) -> Result<Option<Track>> {
            Ok(Some(Track {
                artist: "Artist".into(),
                title: "Album".into(),
                duration: 3600.0,
            }))
        }

        fn search(
            &mut self,
            _query: &str,
            _prefer: PreferredAudio,
            _cancelled: &AtomicBool,
        ) -> Result<Vec<Candidate>> {
            Ok(self.candidates.clone())
        }

        fn download(
            &mut self,
            candidate: &Candidate,
            destination: &Path,
            _cancelled: &AtomicBool,
        ) -> Result<Vec<PathBuf>> {
            if self.download_error {
                return Err("peer left".into());
            }
            let mut files = Vec::new();
            for file in &candidate.files {
                let path = destination.join(file.name.rsplit(['/', '\\']).next().unwrap());
                std::fs::write(&path, b"replacement")?;
                files.push(path);
            }
            Ok(files)
        }

        fn duration(&mut self, _path: &Path) -> Result<Option<f64>> {
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
            &[original],
            policy,
            320,
            PreferredAudio::Lossless,
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
            &[original],
            QualityPolicy::Auto,
            320,
            PreferredAudio::Lossless,
            &AtomicBool::new(true),
            &mut |_| {},
            &mut |_, _| Ok(json!(true)),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "quality check cancelled");
    }
}
