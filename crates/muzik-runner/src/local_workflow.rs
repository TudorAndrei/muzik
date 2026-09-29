//! Local audio workflow adapter with native split and Beets import.

use crate::gates::{self, Gate};
use muzik_core::watchlist::Stage;
use muzik_core::{
    chapters::Chapter, paths, splitter, ChapterAnswer, DecisionKind, DuplicateAnswer,
    DuplicatePolicy, KEEP_CURRENT_TAGS,
};
use muzik_import::apply::{AlbumDecision, DuplicateDecision, MatchDecision};
use muzik_import::beets::{self, ImportRequest};
use muzik_import::plan::{AlbumPlan, PlannedCandidate};
use muzik_library::{Library, SqlValue};
use muzik_workflow::{
    classify_input, run_workflow_with_events, ChapterReview, SplitProgress, SplitTask,
    WorkflowEvent, WorkflowInput, WorkflowOperations, WorkflowOptions, WorkflowRequest,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub struct LocalRequest {
    pub(crate) request: WorkflowRequest,
    pub(crate) options: WorkflowOptions,
}

/// Return `None` when this request needs the remote workflow handler.
pub fn supported(params: &Value) -> Option<Result<LocalRequest, String>> {
    let raw = params.get("raw")?.as_str()?.trim();
    if !matches!(classify_input(raw), WorkflowInput::Local(_)) {
        return None;
    }
    Some(parse(raw, params))
}

pub(crate) fn parse(raw: &str, params: &Value) -> Result<LocalRequest, String> {
    let mut options = WorkflowOptions::default();
    for (key, target) in [
        ("review", &mut options.review),
        ("no_organize", &mut options.no_organize),
        ("tag_only", &mut options.tag_only),
        ("no_split", &mut options.no_split),
        ("dry_run", &mut options.dry_run),
        ("keep_source", &mut options.keep_source),
        ("force", &mut options.force),
    ] {
        if let Some(value) = params.get(key) {
            *target = value
                .as_bool()
                .ok_or_else(|| format!("{key} must be a boolean"))?;
        }
    }
    if let Some(value) = params.get("jobs") {
        options.jobs = value
            .as_u64()
            .and_then(|number| usize::try_from(number).ok())
            .ok_or("jobs must be a non-negative integer")?;
    }
    options.config = path(params, "config")?;
    if let Some(value) = params.get("interactive") {
        options.interactive = value.as_bool().ok_or("interactive must be a boolean")?;
    }
    if let Some(value) = params.get("min_bitrate") {
        options.min_bitrate = value
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .ok_or("min_bitrate must be a non-negative integer")?;
    }
    if let Some(value) = params.get("audio_source") {
        options.audio_source = choice(value, "audio_source")?;
    }
    if let Some(value) = params.get("fallback") {
        options.fallback = choice(value, "fallback")?;
    }
    if let Some(value) = params.get("metadata_source") {
        options.metadata_source = choice(value, "metadata_source")?;
    }
    if let Some(value) = params.get("quality_policy") {
        options.quality_policy = choice(value, "quality_policy")?;
    }
    if let Some(value) = params.get("duplicates") {
        options.duplicates = choice(value, "duplicates")?;
    }
    if let Some(value) = params.get("prefer") {
        options.prefer = value.as_str().ok_or("prefer must be a string")?.to_owned();
    }
    let request = WorkflowRequest {
        raw: raw.to_owned(),
        output: path(params, "output")?.unwrap_or_else(paths::download_dir),
        splits: path(params, "splits")?.unwrap_or_else(|| paths::data_dir().join("splits")),
    };
    Ok(LocalRequest { request, options })
}

fn choice<T: std::str::FromStr<Err = muzik_core::ChoiceError>>(
    value: &Value,
    name: &str,
) -> Result<T, String> {
    value
        .as_str()
        .ok_or_else(|| format!("{name} must be a string"))?
        .parse()
        .map_err(|error: muzik_core::ChoiceError| error.to_string())
}

fn path(params: &Value, key: &str) -> Result<Option<PathBuf>, String> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| format!("{key} must be a string"))?;
    if value.trim().is_empty() {
        return Ok(None);
    }
    if let Some(rest) = value.strip_prefix("~/") {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        return Ok(Some(PathBuf::from(home).join(rest)));
    }
    Ok(Some(PathBuf::from(value)))
}

pub fn run(
    local: LocalRequest,
    cancelled: &AtomicBool,
    on_event: &mut dyn FnMut(Value),
    on_import_event: &mut dyn FnMut(Value),
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<Value, muzik_workflow::Error> {
    let mut operations = LocalOperations {
        decide,
        on_import_event,
        cancelled,
    };
    let result = run_workflow_with_events(
        &local.request,
        &local.options,
        &mut operations,
        cancelled,
        &mut |event| on_event(event_record(event)),
    )?;
    Ok(json!({
        "albums": result.plan.albums.len(),
        "singles": result.plan.singles.len(),
        "split_dirs": result.split_dirs,
    }))
}

pub(crate) struct LocalOperations<'a> {
    pub(crate) decide: &'a mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    pub(crate) on_import_event: &'a mut dyn FnMut(Value),
    pub(crate) cancelled: &'a AtomicBool,
}

impl WorkflowOperations for LocalOperations<'_> {
    fn review_chapters(
        &mut self,
        source: &Path,
        chapters: &[Chapter],
        _: &AtomicBool,
    ) -> Result<ChapterReview, String> {
        gates::mark_stage(Stage::Parse);
        let chapters = chapters.iter().map(chapter_record).collect::<Vec<_>>();
        let answer = (self.decide)(
            DecisionKind::ChapterReview,
            json!({"source":source,"chapters":chapters}),
        )?;
        match answer.as_str().and_then(|answer| answer.parse().ok()) {
            Some(ChapterAnswer::Accept) => Ok(ChapterReview::Accept),
            Some(ChapterAnswer::Reject) => Ok(ChapterReview::Reject),
            Some(ChapterAnswer::Edit) => {
                let answer =
                    (self.decide)(DecisionKind::ChapterEdit, json!({"chapters":chapters}))?;
                if answer.is_null() {
                    return Ok(ChapterReview::Reject);
                }
                let edited = answer
                    .as_array()
                    .ok_or("Edited chapters must be a list.")?
                    .iter()
                    .map(parse_chapter)
                    .collect::<Result<Vec<_>, _>>()?;
                if edited.is_empty() {
                    Ok(ChapterReview::Reject)
                } else {
                    Ok(ChapterReview::Edit(edited))
                }
            }
            None => Err("Select a chapter action.".into()),
        }
    }

    fn download_youtube(&mut self, _: &str, _: &Path, _: bool) -> Result<Vec<PathBuf>, String> {
        Err("YouTube download is not part of a local audio job".into())
    }

    fn acquire_soulseek(&mut self, _: &str) -> Result<Vec<PathBuf>, String> {
        Err("Soulseek is not part of a local audio job".into())
    }

    fn organize(&mut self, target: &Path, options: &WorkflowOptions) -> Result<(), String> {
        let _permit = gates::enter(Gate::Import, Stage::Organize, self.cancelled)?;
        if options.tag_only {
            let count =
                beets::write_library_tags(target, options.config.as_deref(), options.dry_run)?;
            (self.on_import_event)(
                json!({"event":"message","data":{"message":format!("Wrote library tags for {count} item(s).")}}),
            );
            return Ok(());
        }
        (self.on_import_event)(
            json!({"event":"step_started","data":{"name":"import","path":target}}),
        );
        let preview = beets::plan_import_with_cancel(
            ImportRequest {
                source: target.to_path_buf(),
                config_path: options.config.clone().filter(|path| path.exists()),
                dry_run: options.dry_run,
                force: options.force,
                ..ImportRequest::default()
            },
            &|| self.cancelled.load(std::sync::atomic::Ordering::SeqCst),
        )?;
        let mut decisions = Vec::with_capacity(preview.plan.albums.len());
        for (index, album) in preview.plan.albums.iter().enumerate() {
            if self.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("import cancelled".into());
            }
            let task = album_task(index, album);
            (self.on_import_event)(
                json!({"event":"message","data":{"message":format!("Import group {} of {}: {}",index + 1,preview.plan.albums.len(),album.source_dir.display())}}),
            );
            let choice = if options.interactive {
                let answer = (self.decide)(DecisionKind::ImportMatch, json!({"task":task}))?;
                match answer.as_str() {
                    Some(KEEP_CURRENT_TAGS) => MatchDecision::AsIs,
                    Some(id) => album
                        .candidates
                        .iter()
                        .position(|candidate| candidate_id(candidate) == id)
                        .map(MatchDecision::Candidate)
                        .ok_or("Select a valid match ID.")?,
                    None if answer.is_null() => MatchDecision::Skip,
                    _ => return Err("Select a valid match ID.".into()),
                }
            } else {
                MatchDecision::AsIs
            };
            let duplicate = if choice == MatchDecision::Skip || album.duplicates.is_empty() {
                None
            } else if options.force {
                Some(DuplicateDecision::Replace)
            } else {
                Some(match options.duplicates {
                    DuplicatePolicy::Skip => DuplicateDecision::Skip,
                    DuplicatePolicy::KeepAll => DuplicateDecision::Keep,
                    DuplicatePolicy::RemoveOld => DuplicateDecision::Replace,
                    DuplicatePolicy::Ask if !options.interactive => DuplicateDecision::Skip,
                    DuplicatePolicy::Ask => {
                        let answer = (self.decide)(
                            DecisionKind::ImportDuplicate,
                            json!({"task":task,"duplicates":duplicate_views(album, &preview.paths.library)?}),
                        )?;
                        match answer.as_str().and_then(|answer| answer.parse().ok()) {
                            Some(DuplicateAnswer::Skip) => DuplicateDecision::Skip,
                            Some(DuplicateAnswer::KeepAll) => DuplicateDecision::Keep,
                            Some(DuplicateAnswer::RemoveOld) => DuplicateDecision::Replace,
                            None => return Err("Select a valid duplicate action.".into()),
                        }
                    }
                })
            };
            decisions.push(AlbumDecision { choice, duplicate });
        }
        let outcome = beets::apply_import_with_cancel(preview, &decisions, &|| {
            self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
        })?;
        (self.on_import_event)(
            json!({"event":"step_finished","data":{"name":"import","items":outcome.apply.destinations.len(),"skipped":outcome.apply.skipped_albums + outcome.apply.skipped_incremental}}),
        );
        Ok(())
    }

    fn split(&mut self, task: &SplitTask, options: &WorkflowOptions) -> Result<(), String> {
        self.split_with_cancel(task, options, &AtomicBool::new(false), &mut |_| {})
    }

    fn split_with_cancel(
        &mut self,
        task: &SplitTask,
        options: &WorkflowOptions,
        cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(SplitProgress),
    ) -> Result<(), String> {
        let _permit = gates::enter(Gate::Process, Stage::Split, cancelled)?;
        let settings = splitter::SplitOptions {
            jobs: options.jobs,
            keep_source: options.keep_source,
            force: options.force,
            compilation: false,
            cache_dir: None,
        };
        let actual = splitter::split_audio_with_cancel(
            &task.source,
            &task.chapters,
            &task.output,
            &settings,
            cancelled,
            on_progress,
        )
        .map_err(|error| error.to_string())?;
        if actual != task.output {
            return Err(format!(
                "The split cache points to {}, but this workflow needs {}.",
                actual.display(),
                task.output.display()
            ));
        }
        Ok(())
    }
}

fn chapter_record(chapter: &Chapter) -> Value {
    json!({"index":chapter.index,"start":chapter.start,"end":chapter.end,"title":chapter.title})
}

fn parse_chapter(value: &Value) -> Result<Chapter, String> {
    let index = value["index"]
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .filter(|index| *index > 0)
        .ok_or("Chapter index must be a positive integer.")?;
    let start = value["start"]
        .as_i64()
        .filter(|start| *start >= 0)
        .ok_or("Chapter start must be a non-negative integer.")?;
    let end = if value["end"].is_null() {
        None
    } else {
        Some(
            value["end"]
                .as_i64()
                .filter(|end| *end > start)
                .ok_or("Chapter end must be after its start.")?,
        )
    };
    let title = value["title"]
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or("Chapter title must not be empty.")?
        .to_owned();
    Ok(Chapter {
        index,
        start,
        end,
        title,
    })
}

fn duplicate_views(album: &AlbumPlan, database: &Path) -> Result<Vec<Value>, String> {
    let library = Library::open_read_only(database).map_err(|error| error.to_string())?;
    album.duplicates.iter().map(|duplicate| {
        let existing = library.album(duplicate.album_id).map_err(|error| error.to_string())?;
        let items = library.items_for_album(duplicate.album_id).map_err(|error| error.to_string())?;
        let first = items.first();
        Ok(json!({
            "path":first.and_then(|item| item.field("path")).and_then(sql_text),
            "artist":existing.as_ref().and_then(|item| item.field("albumartist")).and_then(sql_text)
                .or_else(|| first.and_then(|item| item.field("artist")).and_then(sql_text)),
            "album":existing.as_ref().and_then(|item| item.field("album")).and_then(sql_text)
                .or_else(|| first.and_then(|item| item.field("album")).and_then(sql_text)),
        }))
    }).collect()
}

fn sql_text(value: &SqlValue) -> Option<String> {
    match value {
        SqlValue::Text(text) => Some(text.clone()),
        SqlValue::Blob(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        SqlValue::Integer(value) => Some(value.to_string()),
        SqlValue::Real(value) => Some(value.to_string()),
        _ => None,
    }
}

fn album_task(index: usize, album: &AlbumPlan) -> Value {
    let current = album.items.first().map(|item| &item.match_item);
    json!({
        "task_id":format!("native:{index}"),
        "paths":album.items.iter().map(|item| &item.source).collect::<Vec<_>>(),
        "is_album":true,
        "item_count":album.items.len(),
        "current_artist":current.map(|item| item.artist.as_str()),
        "current_album":current.map(|item| item.album.as_str()),
        "current_year":current.map(|item| item.year),
        "matches":album.candidates.iter().map(|item| json!({
            "candidate_id":candidate_id(item),
            "artist":item.release.artist,
            "album":item.release.title,
            "year":item.release.year,
            "country":item.release.country,
            "media":item.release.media,
            "label":item.release.label,
            "track_count":item.release.tracks.len(),
            "distance":item.distance,
            "score":match_score(item.distance),
        })).collect::<Vec<_>>(),
    })
}

fn candidate_id(candidate: &PlannedCandidate) -> String {
    format!("release:{}", candidate.release.id.0)
}

fn match_score(distance: f64) -> u64 {
    ((1.0 - distance.clamp(0.0, 1.0)) * 100.0).round() as u64
}

pub(crate) fn event_record(event: WorkflowEvent) -> Value {
    let (name, data) = match event {
        WorkflowEvent::InputClassified(_) => ("message", json!({"message":"Reading local audio."})),
        WorkflowEvent::AcquisitionStarted => ("step_started", json!({"name":"read"})),
        WorkflowEvent::AcquisitionCompleted { files } => {
            ("step_finished", json!({"name":"read","files":files}))
        }
        WorkflowEvent::PlanReady { albums, singles } => (
            "message",
            json!({"message":format!("Found {albums} album(s) and {singles} single(s).")}),
        ),
        WorkflowEvent::SplitStarted(task) => (
            "progress_started",
            json!({"task_id":"local-split","description":format!("Splitting {}",task.source.display()),"total":task.chapters.len()}),
        ),
        WorkflowEvent::SplitProgress { progress, .. } => (
            "progress_advanced",
            json!({"task_id":"local-split","completed":progress.completed,"total":progress.total}),
        ),
        WorkflowEvent::SplitCompleted { .. } => (
            "progress_finished",
            json!({"task_id":"local-split","success":true}),
        ),
        WorkflowEvent::OrganizeStarted { .. } | WorkflowEvent::OrganizeCompleted { .. } => {
            ("message", json!({"message":"Organizing audio."}))
        }
        WorkflowEvent::Completed => (
            "message",
            json!({"message":"Local audio workflow complete."}),
        ),
    };
    json!({"event":name,"data":data})
}

#[cfg(test)]
mod tests {
    use super::{parse, run, supported};
    use muzik_core::{ChapterAnswer, DecisionKind, DuplicatePolicy};
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn parses_shared_source_and_quality_settings() -> Result<(), String> {
        let request = parse(
            "track.flac",
            &json!({
                "audio_source":"soulseek",
                "fallback":"none",
                "metadata_source":"musicbrainz",
                "quality_policy":"ask",
                "min_bitrate":192,
                "prefer":"flac"
            }),
        )?;
        assert_eq!(request.options.audio_source.as_str(), "soulseek");
        assert_eq!(request.options.fallback.as_str(), "none");
        assert_eq!(request.options.metadata_source.as_str(), "musicbrainz");
        assert_eq!(request.options.quality_policy.as_str(), "ask");
        assert_eq!(request.options.min_bitrate, 192);
        assert_eq!(request.options.prefer, "flac");
        Ok(())
    }

    #[test]
    fn a_duplicate_album_is_skipped_by_default_without_a_question(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let config = dir.path().join("config.yaml");
        let database = dir.path().join("library.db");
        fs::write(
            &config,
            format!(
                "directory: {}\nlibrary: {}\nstatefile: {}\nimport:\n  autotag: false\n",
                dir.path().join("Music").display(),
                database.display(),
                dir.path().join("state.pickle").display()
            ),
        )?;
        let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../crates/muzik-tags/tests/fixtures/mediafile.flac");
        let mut asked = Vec::new();
        for round in 0..2 {
            let audio = dir.path().join(format!("round-{round}")).join("track.flac");
            fs::create_dir_all(audio.parent().ok_or("no parent")?)?;
            fs::copy(&fixture, &audio)?;
            let request =
                supported(&json!({"raw":audio,"config":config,"no_split":true,"interactive":true}))
                    .ok_or("local request was not selected")??;
            assert_eq!(request.options.duplicates, DuplicatePolicy::Skip);
            run(
                request,
                &AtomicBool::new(false),
                &mut |_| {},
                &mut |_| {},
                &mut |kind, _| {
                    asked.push(kind);
                    Ok(json!(muzik_core::KEEP_CURRENT_TAGS))
                },
            )?;
        }
        assert_eq!(
            asked,
            [DecisionKind::ImportMatch, DecisionKind::ImportMatch]
        );
        assert_eq!(
            muzik_library::Library::open_read_only(&database)?
                .items()?
                .len(),
            1
        );
        let probe = dir.path().join("probe.flac");
        fs::write(&probe, b"audio")?;
        let request = supported(&json!({"raw":probe,"duplicates":"ask"}))
            .ok_or("local request was not selected")??;
        assert_eq!(request.options.duplicates, DuplicatePolicy::Ask);
        assert!(supported(&json!({"raw":probe,"duplicates":"merge"}))
            .ok_or("local request was not selected")?
            .is_err());
        Ok(())
    }

    #[test]
    fn local_dry_run_reports_plan_without_changing_audio() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let audio = dir.path().join("album.flac");
        fs::write(&audio, b"audio")?;
        fs::write(dir.path().join("album.chapters.txt"), "0:00 First\n")?;
        let splits = dir.path().join("splits");
        let request =
            supported(&json!({"raw":audio,"splits":splits,"no_organize":true,"dry_run":true}))
                .ok_or("local request was not selected")??;
        let mut events = Vec::new();
        let result = run(
            request,
            &AtomicBool::new(false),
            &mut |event| events.push(event),
            &mut |_| {},
            &mut |_, _| Err("unexpected decision".into()),
        )?;
        assert_eq!(result["albums"], 1);
        assert!(events.iter().any(|event| event["event"] == "message"));
        assert!(audio.exists());
        assert!(!splits.exists());
        Ok(())
    }

    #[test]
    fn chapter_reject_keeps_local_audio_as_a_single() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let audio = dir.path().join("album.flac");
        fs::write(&audio, b"audio")?;
        fs::write(dir.path().join("album.chapters.txt"), "0:00 First\n")?;
        let request =
            supported(&json!({"raw":audio,"no_organize":true,"review":true,"dry_run":true}))
                .ok_or("local request was not selected")??;
        let mut decisions = Vec::new();
        let result = run(
            request,
            &AtomicBool::new(false),
            &mut |_| {},
            &mut |_| {},
            &mut |kind, payload| {
                decisions.push((kind, payload));
                Ok(json!(ChapterAnswer::Reject))
            },
        )?;
        assert_eq!(result["albums"], 0);
        assert_eq!(result["singles"], 1);
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].0, DecisionKind::ChapterReview);
        assert_eq!(decisions[0].1["chapters"][0]["title"], "First");
        Ok(())
    }
}
