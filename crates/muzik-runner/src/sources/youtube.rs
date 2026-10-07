use super::{
    Source, at, cancel_or, check_cancelled, mark_full, organize, required, workflow_error,
};
use crate::gates::{self, Gate};
use crate::watchlist::Adapter;
use crate::{local_workflow, remote_workflow};
use muzik_core::{ChapterAnswer, DecisionKind, chapters};
use muzik_store::watchlist::jobs::{JobError, LoadedSource};
use muzik_store::watchlist::{ItemAction, Playlist, SourceKind, Stage, StageStatus, WatchItem};
use muzik_workflow::quality::{QualityUpgradeResult, check_youtube_quality};
use muzik_workflow::ytdlp::{YtDlp, is_video_id};
use muzik_workflow::{WorkflowInput, WorkflowOperations, classify_input};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub(super) struct Youtube;

impl Source for Youtube {
    fn load(
        &self,
        adapter: &mut Adapter<'_, '_>,
        playlist: &Playlist,
    ) -> Result<LoadedSource, JobError> {
        let source = YtDlp::default()
            .playlist(&playlist.url, adapter.cancelled)
            .map_err(workflow_error)?;
        Ok(items(playlist, &source))
    }

    fn process(
        &self,
        adapter: &mut Adapter<'_, '_>,
        item: &WatchItem,
        action: ItemAction,
        cancelled: &AtomicBool,
    ) -> Result<WatchItem, JobError> {
        if action.stage() != Stage::Download {
            return local_stage(adapter, item, action, cancelled);
        }
        if matches!(action, ItemAction::Run | ItemAction::Retry)
            && ready_quality_directory(item).is_some()
        {
            if !adapter.prepared.settings.options.no_organize {
                return local_stage(adapter, item, ItemAction::OrganizeAgain, cancelled);
            }
            let mut updated = item.clone();
            updated.set(Stage::Organize, StageStatus::Skipped);
            return Ok(updated);
        }
        let url = required(item.video_url.as_deref(), "video_url")?;
        let mut settings = adapter.prepared.settings.clone();
        url.clone_into(&mut settings.request.raw);
        match action {
            ItemAction::DownloadAgain => {
                settings.options.force = true;
                settings.options.no_split = true;
                settings.options.no_organize = true;
            }
            ItemAction::RunAllAgain => settings.options.force = true,
            _ => {}
        }
        let input = classify_input(url);
        if matches!(input, WorkflowInput::Local(_)) {
            return Err(JobError::Operation(
                "The saved YouTube item is not a video URL.".into(),
            ));
        }
        let events = adapter.events;
        let stage = Cell::new(Stage::Download);
        let result = remote_workflow::run(
            input,
            &settings,
            cancelled,
            &stage,
            &mut |event| (events.borrow_mut())(event),
            adapter.on_import_event,
            adapter.decide,
        )
        .map_err(|error| at(stage.get(), workflow_error(error)))?;
        let mut updated = item.clone();
        save_output_paths(adapter, &mut updated, &result);
        if action == ItemAction::DownloadAgain {
            updated.set(Stage::Download, StageStatus::Complete);
            updated.invalidate(&[Stage::Parse, Stage::Split, Stage::Organize]);
        } else {
            let split = result
                .get("split_dirs")
                .and_then(Value::as_array)
                .is_some_and(|dirs| !dirs.is_empty());
            mark_full(&mut updated, &adapter.prepared.settings.options, split);
        }
        Ok(updated)
    }
}

fn save_output_paths(adapter: &Adapter<'_, '_>, item: &mut WatchItem, result: &Value) {
    let paths = |key: &str| {
        result[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    };
    let audio = paths("audio_files")
        .into_iter()
        .find(|path| path.is_file())
        .or_else(|| adapter.prepared.audio(item));
    item.set_path(Stage::Download, audio);
    let split = paths("split_dirs").into_iter().find(|path| path.is_dir());
    item.set_path(Stage::Split, split);
}

pub(super) fn local_stage(
    adapter: &mut Adapter<'_, '_>,
    item: &WatchItem,
    action: ItemAction,
    cancelled: &AtomicBool,
) -> Result<WatchItem, JobError> {
    let audio = adapter.prepared.audio(item);
    let mut updated = item.clone();
    if action == ItemAction::OrganizeAgain {
        let target = item
            .path(Stage::Split)
            .filter(|path| path.is_dir())
            .map(Path::to_path_buf)
            .or(audio)
            .ok_or_else(|| {
                JobError::Operation("No downloaded audio or split directory is available.".into())
            })?;
        let mut options = adapter.prepared.settings.options.clone();
        options.force = true;
        options.no_organize = false;
        organize(adapter, &target, &options, cancelled)?;
        updated.set(Stage::Organize, StageStatus::Complete);
        return Ok(updated);
    }
    let audio =
        audio.ok_or_else(|| JobError::Operation("Downloaded audio is not available.".into()))?;
    if action == ItemAction::CheckQualityAgain {
        updated.set_path(Stage::Download, Some(audio.clone()));
        let _permit = gates::enter(Gate::Process, cancelled).map_err(|_| JobError::Cancelled)?;
        let options = &adapter.prepared.settings.options;
        let events = adapter.events;
        let result = check_youtube_quality(
            &adapter.prepared.settings.paths,
            vec![audio],
            options.quality_policy,
            options.min_bitrate,
            options.prefer,
            cancelled,
            &mut |event| (events.borrow_mut())(event),
            adapter.decide,
        )
        .map_err(|error| cancel_or(cancelled, error))?;
        apply_quality_result(&mut updated, &result);
        return Ok(updated);
    }
    if action == ItemAction::ParseAgain {
        let video_url = required(item.video_url.as_deref(), "video_url")?;
        let chapter_path = refresh_chapters(&audio, video_url, cancelled, adapter.decide)?;
        updated.complete(Stage::Parse, Some(chapter_path));
        updated.invalidate(&[Stage::Split, Stage::Organize]);
        return Ok(updated);
    }
    let found =
        chapters::find_chapters(&audio).map_err(|error| JobError::Operation(error.to_string()))?;
    if found.is_empty() {
        return Err(JobError::Operation(
            "No chapters were found for this audio.".into(),
        ));
    }
    let stem = audio
        .file_stem()
        .ok_or_else(|| JobError::Operation("Audio file has no name.".into()))?;
    let output = adapter.prepared.settings.request.splits.join(stem);
    let task = muzik_workflow::SplitTask {
        source: audio,
        chapters: found,
        output: output.clone(),
    };
    let mut options = adapter.prepared.settings.options.clone();
    options.force = true;
    options.keep_source = true;
    let stage = Cell::new(Stage::Split);
    let mut local = local_workflow::LocalOperations {
        decide: adapter.decide,
        on_import_event: adapter.on_import_event,
        cancelled,
        stage: &stage,
    };
    local
        .split_with_cancel(&task, &options, cancelled, &mut |_| {})
        .map_err(JobError::Operation)?;
    check_cancelled(cancelled)?;
    updated.complete(Stage::Split, Some(output));
    updated.invalidate(&[Stage::Organize]);
    Ok(updated)
}

fn ready_quality_directory(item: &WatchItem) -> Option<PathBuf> {
    if item.status(Stage::Split) != StageStatus::Complete
        || item.path(Stage::Quality) != item.path(Stage::Split)
    {
        return None;
    }
    item.path(Stage::Split)
        .filter(|path| path.is_dir())
        .map(Path::to_path_buf)
}

fn apply_quality_result(item: &mut WatchItem, result: &QualityUpgradeResult) {
    item.set(Stage::Quality, StageStatus::Complete);
    if let Some(directory) = result.pre_split_dirs.first() {
        item.set_path(Stage::Quality, Some(directory.clone()));
        item.set_path(Stage::Split, Some(directory.clone()));
        item.set(Stage::Parse, StageStatus::Skipped);
        item.set(Stage::Split, StageStatus::Complete);
        item.invalidate(&[Stage::Organize]);
    } else if let Some(replacement) = result.audio_files.first()
        && item.path(Stage::Download) != Some(replacement.as_path())
    {
        item.set_path(Stage::Download, Some(replacement.clone()));
        item.set_path(Stage::Quality, Some(replacement.clone()));
        item.invalidate(&[Stage::Parse, Stage::Split, Stage::Organize]);
    }
}

fn refresh_chapters(
    audio: &Path,
    video_url: &str,
    cancelled: &AtomicBool,
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
) -> Result<PathBuf, JobError> {
    refresh_chapters_with(audio, cancelled, decide, |comments| {
        YtDlp::default()
            .video(video_url, comments, cancelled)
            .map_err(workflow_error)
    })
}

fn refresh_chapters_with(
    audio: &Path,
    cancelled: &AtomicBool,
    decide: &mut dyn FnMut(DecisionKind, Value) -> Result<Value, String>,
    mut fetch: impl FnMut(bool) -> Result<Value, JobError>,
) -> Result<PathBuf, JobError> {
    let metadata = fetch(false)?;
    let info = serde_json::to_string_pretty(&metadata)
        .map_err(|error| JobError::Operation(error.to_string()))?;
    atomic_write(&chapters::sidecar_path(audio, ".info.json"), &(info + "\n"))?;
    check_cancelled(cancelled)?;
    let mut found = chapters::parse_info_json(&metadata.to_string())
        .map_err(|error| JobError::Operation(error.to_string()))?;
    if found.is_empty() {
        found = metadata
            .get("description")
            .and_then(Value::as_str)
            .map(chapters::parse_tracklist)
            .unwrap_or_default();
    }
    if found.is_empty() {
        let comments = fetch(true)?;
        found = chapters::best_comment_tracklist(&comments);
    }
    if found.is_empty() {
        return Err(JobError::Operation(
            "No YouTube chapters were found.".into(),
        ));
    }
    let records = found.iter().map(chapter_record).collect::<Vec<_>>();
    let answer = decide(
        DecisionKind::ChapterReview,
        json!({"source":audio,"chapters":records}),
    )
    .map_err(JobError::Operation)?;
    match answer.as_str().and_then(|answer| answer.parse().ok()) {
        Some(ChapterAnswer::Accept) => {}
        Some(ChapterAnswer::Edit) => {
            let answer = decide(DecisionKind::ChapterEdit, json!({"chapters":records}))
                .map_err(JobError::Operation)?;
            found = answer
                .as_array()
                .ok_or_else(|| JobError::Operation("Edited chapters must be a list.".into()))?
                .iter()
                .map(parse_chapter_record)
                .collect::<Result<Vec<_>, _>>()?;
        }
        Some(ChapterAnswer::Reject) => {
            return Err(JobError::Operation(
                "YouTube chapters were not accepted.".into(),
            ));
        }
        None => return Err(JobError::Operation("Select a chapter action.".into())),
    }
    if found.is_empty() {
        return Err(JobError::Operation(
            "YouTube chapters were not accepted.".into(),
        ));
    }
    check_cancelled(cancelled)?;
    let text = found
        .iter()
        .map(|chapter| format!("{} {}", format_time(chapter.start), chapter.title))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let path = chapters::sidecar_path(audio, ".chapters.txt");
    atomic_write(&path, &text)?;
    Ok(path)
}

fn atomic_write(path: &Path, text: &str) -> crate::Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(text.as_bytes())?;
    file.persist(path)?;
    Ok(())
}

fn chapter_record(chapter: &chapters::Chapter) -> Value {
    json!({"index":chapter.index,"start":chapter.start,"end":chapter.end,"title":chapter.title})
}

fn parse_chapter_record(value: &Value) -> Result<chapters::Chapter, JobError> {
    let index = value["index"]
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| JobError::Operation("Edited chapter index is invalid.".into()))?;
    let start = value["start"]
        .as_i64()
        .ok_or_else(|| JobError::Operation("Edited chapter start is invalid.".into()))?;
    let end = value["end"].as_i64();
    let title = value["title"]
        .as_str()
        .filter(|title| !title.trim().is_empty())
        .ok_or_else(|| JobError::Operation("Edited chapter title is missing.".into()))?;
    Ok(chapters::Chapter {
        index,
        start,
        end,
        title: title.to_owned(),
    })
}

fn format_time(seconds: i64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn items(playlist: &Playlist, source: &Value) -> LoadedSource {
    let old: HashMap<&str, &WatchItem> = playlist
        .items
        .iter()
        .filter_map(|item| Some((item.video_id.as_deref()?, item)))
        .collect();
    let items = source["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .zip(1_u64..)
        .filter_map(|(entry, position)| {
            let id = entry["id"].as_str().or_else(|| entry["url"].as_str())?;
            if !is_video_id(id) {
                return None;
            }
            let saved = old.get(id).copied();
            let listed_title = entry["title"].as_str().filter(|title| {
                !title.is_empty() && (!title.starts_with('[') || !title.ends_with(" video]"))
            });
            let unavailable = listed_title.is_none() && entry["duration"].is_null();
            let title = listed_title
                .or_else(|| saved.map(|item| item.title.as_str()))
                .unwrap_or(id);
            let thumbnail = entry["thumbnail"]
                .as_str()
                .or_else(|| {
                    entry["thumbnails"]
                        .as_array()
                        .and_then(|images| images.last())
                        .and_then(|image| image["url"].as_str())
                })
                .map(str::to_owned)
                .or_else(|| saved.and_then(|item| item.thumbnail_url.clone()));
            let mut item = WatchItem::new(position, title, SourceKind::Youtube);
            item.video_id = Some(id.to_owned());
            item.video_url = Some(format!("https://www.youtube.com/watch?v={id}"));
            item.thumbnail_url = thumbnail;
            item.unavailable = Some(unavailable);
            Some(item)
        })
        .collect();
    LoadedSource {
        title: source["title"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| playlist.title.clone()),
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_quality_result, items, ready_quality_directory, refresh_chapters_with};
    use crate::sources::of;
    use crate::sources::testing::{fixture, library_config, settings, with_adapter};
    use muzik_core::{ChapterAnswer, DecisionKind};
    use muzik_store::watchlist::{ItemAction, Playlist, SourceKind, Stage, StageStatus, WatchItem};
    use muzik_workflow::quality::QualityUpgradeResult;
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn saved_output_paths_resolve_new_and_existing_audio() {
        let directory = tempfile::tempdir().unwrap();
        let audio = directory.path().join("Song [abcdefghijk].flac");
        let split = directory.path().join("split");
        fs::write(&audio, []).unwrap();
        fs::create_dir(&split).unwrap();
        let settings = settings(directory.path(), &json!({"output":directory.path()})).unwrap();
        let replacement = directory.path().join("replacement.flac");
        fs::write(&replacement, []).unwrap();
        with_adapter(&settings, |adapter, _| {
            let mut item = WatchItem::new(1, "Song", SourceKind::Youtube);
            item.video_id = Some("abcdefghijk".into());
            super::save_output_paths(adapter, &mut item, &json!({"split_dirs":[split]}));
            assert_eq!(item.path(Stage::Download), Some(audio.as_path()));
            assert_eq!(item.path(Stage::Split), Some(split.as_path()));
            super::save_output_paths(
                adapter,
                &mut item,
                &json!({"audio_files":[replacement],"split_dirs":[]}),
            );
            assert_eq!(adapter.prepared.audio(&item), Some(replacement.clone()));
        });
    }

    #[test]
    fn multi_file_quality_replacement_imports_as_a_ready_album() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("Album [abcdefghijk].flac");
        let album = directory.path().join("replacement");
        fs::create_dir(&album).unwrap();
        for path in [&original, &album.join("one.flac"), &album.join("two.flac")] {
            fs::copy(fixture(), path).unwrap();
        }
        let mut item = WatchItem::new(1, "Album", SourceKind::Youtube);
        item.video_id = Some("abcdefghijk".into());
        item.video_url = Some("https://www.youtube.com/watch?v=abcdefghijk".into());
        item.complete(Stage::Download, Some(original.clone()));
        apply_quality_result(
            &mut item,
            &QualityUpgradeResult {
                audio_files: Vec::new(),
                pre_split_dirs: vec![album.clone()],
            },
        );
        assert_eq!(item.path(Stage::Download), Some(original.as_path()));
        assert_eq!(ready_quality_directory(&item), Some(album));
        assert_eq!(item.status(Stage::Parse), StageStatus::Skipped);
        assert_eq!(item.status(Stage::Split), StageStatus::Complete);
        assert_eq!(item.status(Stage::Organize), StageStatus::Stale);
        let config = library_config(directory.path()).unwrap();
        let settings = settings(
            directory.path(),
            &json!({"output":directory.path(),"config":config,"interactive":false,"quality_policy":"auto"}),
        )
        .unwrap();
        let result = with_adapter(&settings, |adapter, cancelled| {
            of(SourceKind::Youtube).process(adapter, &item, ItemAction::Retry, cancelled)
        })
        .unwrap();
        assert_eq!(result.status(Stage::Organize), StageStatus::Complete);
        assert_eq!(
            muzik_library::Library::open_read_only(&directory.path().join("library.db"))
                .unwrap()
                .items()
                .unwrap()
                .len(),
            2
        );
        assert!(original.is_file());
    }

    #[test]
    fn youtube_reload_preserves_saved_card_metadata() {
        let mut playlist = Playlist::new("PL1", "u", SourceKind::Youtube, Some("My list"));
        let mut saved = WatchItem::new(1, "Saved title", SourceKind::Youtube);
        saved.video_id = Some("abcdefghijk".into());
        saved.thumbnail_url = Some("https://example.test/image.jpg".into());
        playlist.items.push(saved);
        let loaded = items(
            &playlist,
            &json!({"title":"Current list","entries":[{"id":"abcdefghijk"}]}),
        );
        assert_eq!(loaded.items[0].title, "Saved title");
        assert_eq!(
            loaded.items[0].thumbnail_url.as_deref(),
            Some("https://example.test/image.jpg")
        );
    }

    #[test]
    fn a_private_video_without_title_and_duration_is_unavailable() {
        let loaded = items(
            &Playlist::new("PL1", "u", SourceKind::Youtube, None),
            &json!({"entries":[
                {"id":"abcdefghijk","title":null,"duration":null},
                {"id":"bcdefghijkl","title":"[Private video]","duration":null},
                {"id":"cdefghijklm","title":"Song","duration":245.0}
            ]}),
        );
        let flags: Vec<_> = loaded.items.iter().map(|item| item.unavailable).collect();
        assert_eq!(flags, [Some(true), Some(true), Some(false)]);
        assert_eq!(loaded.items[1].title, "bcdefghijkl");
    }

    #[test]
    fn youtube_new_card_uses_source_title_and_thumbnail() {
        let loaded = items(
            &Playlist::new("PL1", "u", SourceKind::Youtube, None),
            &json!({"title":"Playlist", "entries":[{"id":"abcdefghijk", "title":"Song", "thumbnail":"https://example.test/new.jpg"}]}),
        );
        assert_eq!(loaded.title.as_deref(), Some("Playlist"));
        assert_eq!(loaded.items[0].title, "Song");
        assert_eq!(
            loaded.items[0].thumbnail_url.as_deref(),
            Some("https://example.test/new.jpg")
        );
    }

    #[test]
    fn description_tracklist_orders_times_and_keeps_titles() {
        let chapters = muzik_core::chapters::parse_tracklist(
            "Track list:\n2. Song Two (04:10 - 08:00)\n[0:00] First Song\n4:10 Duplicate\n8:00 - Final Song\n",
        );
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].title, "First Song");
        assert_eq!(chapters[0].end, Some(250));
        assert_eq!(chapters[1].title, "Song Two");
        assert_eq!(chapters[2].start, 480);
    }

    #[test]
    fn comment_tracklist_prefers_pinned_then_uploader() {
        let metadata = json!({"comments":[
            {"text":"0:00 Other\n2:00 End"},
            {"text":"0:00 Uploader\n2:00 End", "author_is_uploader":true},
            {"text":"0:00 Pinned\n2:00 End", "is_pinned":true}
        ]});
        assert_eq!(
            muzik_core::chapters::best_comment_tracklist(&metadata)[0].title,
            "Pinned"
        );
    }

    #[test]
    fn refreshed_chapters_replace_sidecar_after_review() {
        let directory = tempfile::tempdir().unwrap();
        let audio = directory.path().join("Album.flac");
        fs::write(&audio, []).unwrap();
        fs::write(directory.path().join("Album.chapters.txt"), "00:00 Old\n").unwrap();
        let mut asked = false;
        let path = refresh_chapters_with(
            &audio,
            &AtomicBool::new(false),
            &mut |kind, value| {
                assert_eq!(kind, DecisionKind::ChapterReview);
                assert_eq!(value["chapters"][1]["title"], "Second");
                asked = true;
                Ok(json!(ChapterAnswer::Accept))
            },
            |_| {
                Ok(json!({"chapters":[
                    {"start_time":0,"title":"First"},
                    {"start_time":125,"title":"Second"}
                ]}))
            },
        )
        .unwrap();
        assert!(asked);
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "00:00 First\n02:05 Second\n"
        );
        assert!(
            fs::read_to_string(directory.path().join("Album.info.json"))
                .unwrap()
                .contains("Second")
        );
    }
}
