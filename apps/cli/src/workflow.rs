//! Connect the shared workflow service to the native CLI commands.

use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use muzik_core::{AudioFallback, AudioSource, app_config, paths, splitter};
use muzik_soulseek::session::SessionSettings;
use muzik_workflow::{
    AudioProcessingResult, QualityCheckedAudio, SplitTask, WorkflowInput, WorkflowOperations,
    WorkflowOptions, WorkflowRequest, classify_input, find_audio_inputs, playlist, run_workflow,
};
use yt_dlp::executor::Executor;

use crate::{Download, Organize, SoulseekDownload, Workflow, download, organize, soulseek, split};

pub fn queue(args: &Workflow) -> Result<(), String> {
    if args.compilation {
        return Err("--compilation does not work with --queue yet.".into());
    }
    let absolute = |path: &Path| {
        std::path::absolute(path)
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| error.to_string())
    };
    let raw = if matches!(classify_input(&args.raw), WorkflowInput::Local(_)) {
        absolute(Path::new(&args.raw))?
    } else {
        args.raw.clone()
    };
    let mut params = serde_json::Map::new();
    for (key, path) in [
        ("output", &args.output),
        ("splits", &args.splits),
        ("config", &args.config),
    ] {
        if let Some(path) = path {
            params.insert(key.into(), serde_json::json!(absolute(path)?));
        }
    }
    params.extend(
        serde_json::json!({
            "raw": raw,
            "review": args.review,
            "no_split": args.no_split,
            "no_organize": args.no_organize,
            "tag_only": args.tag_only,
            "dry_run": args.dry_run,
            "jobs": args.jobs,
            "keep_source": args.keep_source,
            "force": args.force,
            "audio_source": args.audio_source,
            "metadata_source": args.metadata_source,
            "quality_policy": args.quality_policy,
            "min_bitrate": args.min_bitrate,
            "prefer": args.prefer,
            "fallback": args.fallback,
            "interactive": !args.no_interactive,
        })
        .as_object()
        .cloned()
        .unwrap_or_default(),
    );
    let jobs = crate::jobs::open()?;
    let id = jobs
        .workflow(&serde_json::Value::Object(params))
        .map_err(|error| error.to_string())?;
    println!("Queued {} as {}.", args.raw, muzik_runner::job_id(id));
    crate::jobs::drain(&jobs)
}

pub fn run(args: &Workflow) -> Result<(), String> {
    let input = classify_input(&args.raw);
    let request = WorkflowRequest {
        raw: args.raw.clone(),
        output: args.output.clone().unwrap_or_else(paths::download_dir),
        splits: args
            .splits
            .clone()
            .unwrap_or_else(|| paths::data_dir().join("splits")),
    };
    let options = WorkflowOptions {
        review: args.review,
        no_split: args.no_split,
        no_organize: args.no_organize,
        import: args.import,
        tag_only: args.tag_only,
        dry_run: args.dry_run,
        jobs: args.jobs,
        config: args.config.clone(),
        keep_source: args.keep_source,
        force: args.force,
        audio_source: args.audio_source,
        metadata_source: args.metadata_source,
        quality_policy: args.quality_policy,
        min_bitrate: args.min_bitrate,
        prefer: args.prefer.clone(),
        fallback: args.fallback,
        interactive: !args.no_interactive,
        ..WorkflowOptions::default()
    };
    let mut operations = CliOperations {
        compilation: args.compilation,
        prefer: args.prefer.clone(),
        interactive: !args.no_interactive,
        audio_source: args.audio_source,
        fallback: args.fallback,
        output: request.output.clone(),
        youtube_acquired: false,
    };
    let cancelled = AtomicBool::new(false);
    if let WorkflowInput::YoutubePlaylist { url, playlist_id } = &input {
        let result = playlist::run_youtube_playlist(
            &request,
            &options,
            &mut operations,
            &cancelled,
            playlist_id,
            url,
            &mut |_| {},
        )
        .map_err(|error| error.to_string())?;
        show_result(&result.processing, &options);
        let failures = result
            .items
            .iter()
            .filter(|item| !item.completed)
            .collect::<Vec<_>>();
        for item in &failures {
            eprintln!("{}: {}", item.id, item.error.as_deref().unwrap_or("failed"));
        }
        if !failures.is_empty() {
            return Err(format!("{} playlist item(s) failed", failures.len()));
        }
        return Ok(());
    }
    if let WorkflowInput::SpotifyExport(path) = &input {
        let result = playlist::run_spotify_export(
            &request,
            &options,
            &mut operations,
            &cancelled,
            path,
            &mut |_| {},
        )
        .map_err(|error| error.to_string())?;
        show_result(&result.processing, &options);
        println!("{} Spotify track(s) processed", result.items.len());
        return Ok(());
    }
    let result = run_workflow(&request, &options, &mut operations, &cancelled)
        .map_err(|error| error.to_string())?;
    show_result(&result, &options);
    Ok(())
}

fn show_result(result: &AudioProcessingResult, options: &WorkflowOptions) {
    println!(
        "Workflow: {} album(s), {} single(s)",
        result.plan.albums.len(),
        result.plan.singles.len()
    );
    if options.dry_run {
        for album in &result.plan.albums {
            println!(
                "Would split {} ({} chapters)",
                album.source.display(),
                album.chapters.len()
            );
        }
        for target in &result.organize_targets {
            println!("Would organize {}", target.display());
        }
    } else {
        println!("Workflow complete.");
    }
}

struct CliOperations {
    compilation: bool,
    prefer: String,
    interactive: bool,
    audio_source: AudioSource,
    fallback: AudioFallback,
    output: PathBuf,
    youtube_acquired: bool,
}

impl WorkflowOperations for CliOperations {
    fn download_youtube(
        &mut self,
        url: &str,
        output: &Path,
        force: bool,
    ) -> Result<Vec<PathBuf>, String> {
        let before = known_audio(output)?;
        let (target, video_id) = match classify_input(url) {
            WorkflowInput::Search(_) => {
                let id = youtube_print(&format!("ytsearch1:{url}"), "id")?;
                if id.len() != 11
                    || !id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                {
                    return Err("YouTube search returned an invalid video ID".into());
                }
                (format!("https://www.youtube.com/watch?v={id}"), Some(id))
            }
            WorkflowInput::YoutubeVideo { video_id, .. } => (url.to_owned(), Some(video_id)),
            _ => (url.to_owned(), None),
        };
        let request = Download {
            url: target,
            output: Some(output.to_path_buf()),
            format: "bestaudio".into(),
            quality: "0".into(),
            no_chapters: false,
            archive_file: None,
            force_overwrites: force,
        };
        // The CLI entry point already has a Tokio runtime. The downloader needs
        // its own runtime because this shared workflow service is synchronous.
        std::thread::spawn(move || -> Result<(), String> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            runtime.block_on(download::run(&request))
        })
        .join()
        .map_err(|_| "YouTube downloader stopped unexpectedly".to_owned())??;
        let after =
            find_audio_inputs(&[output.to_path_buf()]).map_err(|error| error.to_string())?;
        self.youtube_acquired = true;
        Ok(after
            .into_iter()
            .filter(|path| {
                !before.contains(path)
                    || video_id.as_ref().is_some_and(|id| {
                        path.file_stem()
                            .and_then(|stem| stem.to_str())
                            .is_some_and(|stem| stem.contains(&format!("[{id}]")))
                    })
            })
            .collect())
    }

    fn acquire_soulseek(&mut self, query: &str) -> Result<Vec<PathBuf>, String> {
        self.youtube_acquired = false;
        let query = if matches!(classify_input(query), WorkflowInput::YoutubeVideo { .. }) {
            youtube_title(query)?
        } else {
            query.to_owned()
        };
        let output = paths::data_dir().join("soulseek");
        let before = known_audio(&output)?;
        soulseek::download(&SoulseekDownload {
            query: Some(query),
            candidate: None,
            prefer: self.prefer.clone(),
            limit: 10,
            output: Some(output.clone()),
            no_interactive: !self.interactive,
            no_organize: true,
            dry_run: false,
        })?;
        let after = known_audio(&output)?;
        Ok(after.difference(&before).cloned().collect())
    }

    fn check_quality(
        &mut self,
        audio_files: &[PathBuf],
        options: &WorkflowOptions,
        cancelled: &AtomicBool,
    ) -> Result<QualityCheckedAudio, String> {
        let from_youtube = std::mem::take(&mut self.youtube_acquired)
            || audio_files
                .iter()
                .any(|path| muzik_core::chapters::sidecar_path(path, ".info.json").is_file());
        if options.dry_run || !from_youtube {
            return Ok(QualityCheckedAudio {
                audio_files: audio_files.to_vec(),
                pre_split_dirs: Vec::new(),
            });
        }
        let result = muzik_workflow::quality::check_youtube_quality(
            audio_files.to_vec(),
            options.quality_policy,
            options.min_bitrate,
            &options.prefer,
            cancelled,
            &mut |event| {
                if let Some(message) = event
                    .pointer("/data/message")
                    .and_then(serde_json::Value::as_str)
                {
                    eprintln!("{message}");
                }
            },
            &mut |_, payload| {
                let interactive = self.interactive && io::stdin().is_terminal();
                confirm_quality(
                    interactive,
                    &payload,
                    &mut io::stdin().lock(),
                    &mut io::stderr().lock(),
                )
                .map(serde_json::Value::Bool)
            },
        )?;
        Ok(QualityCheckedAudio {
            audio_files: result.audio_files,
            pre_split_dirs: result.pre_split_dirs,
        })
    }

    fn soulseek_ready(&self) -> bool {
        app_config::load(&app_config::path())
            .ok()
            .is_some_and(|config| SessionSettings::configured(&config).is_some())
    }

    fn acquire_spotify_track(
        &mut self,
        track: &playlist::SpotifyTrack,
    ) -> Result<Vec<PathBuf>, String> {
        let query = format!("{} - {}", track.artist, track.title);
        if self.audio_source == AudioSource::Youtube
            || self.audio_source == AudioSource::Auto && !self.soulseek_ready()
        {
            return self.download_youtube(&query, &self.output.clone(), false);
        }
        match self.acquire_soulseek(&query) {
            Ok(files) if !files.is_empty() => Ok(files),
            Ok(_) | Err(_) if self.fallback == AudioFallback::Youtube => {
                self.download_youtube(&query, &self.output.clone(), false)
            }
            Ok(files) => Ok(files),
            Err(error) => Err(error),
        }
    }

    fn youtube_playlist_video_ids(&mut self, url: &str) -> Result<Vec<String>, String> {
        let url = url.to_owned();
        let output = std::thread::spawn(move || -> Result<String, String> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            let executor = Executor::new(
                "yt-dlp",
                vec!["--flat-playlist".into(), "--print".into(), "id".into(), url],
                Duration::from_secs(600),
            );
            runtime
                .block_on(executor.execute())
                .map(|result| result.stdout)
                .map_err(|error| error.to_string())
        })
        .join()
        .map_err(|_| "playlist lookup stopped unexpectedly".to_owned())??;
        Ok(output
            .lines()
            .map(str::trim)
            .filter(|id| {
                id.len() == 11
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
            .map(str::to_owned)
            .collect())
    }

    fn split(&mut self, task: &SplitTask, options: &WorkflowOptions) -> Result<(), String> {
        let chapters = if options.review {
            split::show_chapters(&task.chapters);
            split::review_chapters(task.chapters.clone())?
                .ok_or_else(|| "Workflow cancelled.".to_owned())?
        } else {
            task.chapters.clone()
        };
        let settings = splitter::SplitOptions {
            jobs: options.jobs,
            keep_source: options.keep_source,
            force: options.force,
            compilation: self.compilation,
            cache_dir: None,
        };
        println!(
            "Splitting {} to {}",
            task.source.display(),
            task.output.display()
        );
        let actual = splitter::split_audio(&task.source, &chapters, &task.output, &settings)
            .map_err(split::split_error)?;
        if actual != task.output {
            return Err(format!(
                "The split cache points to {}, but this workflow needs {}.",
                actual.display(),
                task.output.display()
            ));
        }
        Ok(())
    }

    fn organize(&mut self, target: &Path, options: &WorkflowOptions) -> Result<(), String> {
        organize::run(&Organize {
            directory: target.to_path_buf(),
            import: options.import,
            tag_only: options.tag_only,
            dry_run: options.dry_run,
            config: options.config.clone(),
        })
    }
}

fn confirm_quality(
    interactive: bool,
    payload: &serde_json::Value,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<bool, String> {
    if !interactive {
        writeln!(
            output,
            "Keep source audio. Quality policy 'ask' needs an interactive terminal."
        )
        .map_err(|error| error.to_string())?;
        return Ok(false);
    }
    let current = payload
        .get("current")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("source audio");
    let candidate = payload
        .pointer("/candidate/title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Soulseek audio");
    write!(output, "Replace {current} with {candidate}? [y/N] ")
        .map_err(|error| error.to_string())?;
    output.flush().map_err(|error| error.to_string())?;
    let mut answer = String::new();
    input
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn youtube_title(url: &str) -> Result<String, String> {
    youtube_print(url, "title")
}

fn youtube_print(url: &str, field: &str) -> Result<String, String> {
    let url = url.to_owned();
    let field = field.to_owned();
    let output = std::thread::spawn(move || -> Result<String, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let mut args = download::yt_dlp_access_args();
        args.extend(["--skip-download".into(), "--print".into(), field, url]);
        let executor = Executor::new("yt-dlp", args, Duration::from_secs(120));
        runtime
            .block_on(executor.execute())
            .map(|result| result.stdout)
            .map_err(|error| error.to_string())
    })
    .join()
    .map_err(|_| "YouTube metadata lookup stopped unexpectedly".to_owned())??;
    let value = output.trim();
    if value.is_empty() {
        return Err("YouTube metadata has no requested value".into());
    }
    Ok(value.to_owned())
}

fn known_audio(output: &Path) -> Result<HashSet<PathBuf>, String> {
    if !output.exists() {
        return Ok(HashSet::new());
    }
    if !output.is_dir() {
        return Err(format!(
            "Output path is not a directory: {}",
            output.display()
        ));
    }
    let files = find_audio_inputs(&[output.to_path_buf()]).map_err(|error| error.to_string())?;
    Ok(files.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::run;
    use crate::Workflow;
    use std::fs;

    #[test]
    fn quality_confirmation_requires_an_explicit_yes() -> Result<(), Box<dyn std::error::Error>> {
        let payload =
            serde_json::json!({"current":"source.mp3","candidate":{"title":"track.flac"}});
        for (answer, accepted) in [
            ("yes\n", true),
            ("Y\n", true),
            ("n\n", false),
            ("\n", false),
            ("", false),
        ] {
            let mut output = Vec::new();
            let result = super::confirm_quality(
                true,
                &payload,
                &mut std::io::Cursor::new(answer),
                &mut output,
            )?;
            assert_eq!(result, accepted);
            let prompt = String::from_utf8(output)?;
            assert!(prompt.contains("source.mp3"));
            assert!(prompt.contains("track.flac"));
        }
        Ok(())
    }

    #[test]
    fn noninteractive_quality_confirmation_keeps_source_without_reading_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut input = std::io::Cursor::new("yes\n");
        let mut output = Vec::new();
        let accepted =
            super::confirm_quality(false, &serde_json::json!({}), &mut input, &mut output)?;
        assert!(!accepted);
        assert_eq!(input.position(), 0);
        assert!(String::from_utf8(output)?.contains("Keep source audio"));
        Ok(())
    }

    #[test]
    fn local_dry_run_keeps_source_and_does_not_create_split_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let audio = directory.path().join("album.flac");
        fs::write(&audio, b"audio")?;
        fs::write(directory.path().join("album.chapters.txt"), "0:00 First\n")?;
        let splits = directory.path().join("splits");
        run(&Workflow {
            raw: audio.to_string_lossy().into_owned(),
            output: None,
            splits: Some(splits.clone()),
            review: false,
            no_split: false,
            no_organize: false,
            import: false,
            tag_only: false,
            dry_run: true,
            jobs: 0,
            config: None,
            keep_source: false,
            force: false,
            compilation: false,
            audio_source: muzik_core::AudioSource::default(),
            metadata_source: muzik_core::MetadataSource::default(),
            quality_policy: muzik_core::QualityPolicy::default(),
            min_bitrate: 256,
            prefer: "lossless".into(),
            fallback: muzik_core::AudioFallback::default(),
            no_interactive: false,
            queue: false,
        })?;
        assert!(audio.exists());
        assert!(!splits.exists());
        Ok(())
    }
}
