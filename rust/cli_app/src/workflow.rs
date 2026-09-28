//! Connect the shared workflow service to the native CLI commands.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use muzik_core::{app_config, paths, splitter};
use muzik_soulseek::session::SessionSettings;
use muzik_workflow::{
    AudioProcessingResult, SplitTask, WorkflowInput, WorkflowOperations, WorkflowOptions,
    WorkflowRequest, classify_input, find_audio_inputs, playlist, run_workflow,
};
use yt_dlp::executor::Executor;

use crate::{Download, Organize, SoulseekDownload, Workflow, download, organize, soulseek, split};

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
        prefer: args.prefer.clone(),
        fallback: args.fallback,
        interactive: !args.no_interactive,
        ..WorkflowOptions::default()
    };
    let mut operations = CliOperations {
        compilation: args.compilation,
        prefer: args.prefer.clone(),
        interactive: !args.no_interactive,
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
}

impl WorkflowOperations for CliOperations {
    fn download_youtube(
        &mut self,
        url: &str,
        output: &Path,
        force: bool,
    ) -> Result<Vec<PathBuf>, String> {
        let before = known_audio(output)?;
        let target = if matches!(classify_input(url), WorkflowInput::Search(_)) {
            format!("ytsearch1:{url}")
        } else {
            url.to_owned()
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
        let forced_id = if force {
            match classify_input(url) {
                WorkflowInput::YoutubeVideo { video_id, .. } => Some(video_id),
                _ => None,
            }
        } else {
            None
        };
        Ok(after
            .into_iter()
            .filter(|path| {
                !before.contains(path)
                    || forced_id.as_ref().is_some_and(|id| {
                        path.file_stem()
                            .and_then(|stem| stem.to_str())
                            .is_some_and(|stem| stem.contains(&format!("[{id}]")))
                    })
            })
            .collect())
    }

    fn acquire_soulseek(&mut self, query: &str) -> Result<Vec<PathBuf>, String> {
        let output = paths::data_dir().join("soulseek");
        let before = known_audio(&output)?;
        soulseek::download(&SoulseekDownload {
            query: Some(query.to_owned()),
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

    fn soulseek_ready(&self) -> bool {
        app_config::load(&app_config::path())
            .ok()
            .is_some_and(|config| SessionSettings::configured(&config).is_some())
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
            prefer: "lossless".into(),
            fallback: muzik_core::AudioFallback::default(),
            no_interactive: false,
        })?;
        assert!(audio.exists());
        assert!(!splits.exists());
        Ok(())
    }
}
