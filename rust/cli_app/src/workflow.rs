//! Connect the shared workflow service to the native CLI commands.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use muzik_core::{paths, splitter};
use muzik_workflow::{
    AudioProcessingResult, SplitTask, WorkflowInput, WorkflowOperations, WorkflowOptions,
    WorkflowRequest, classify_input, find_audio_inputs, run_workflow,
};

use crate::{Download, Organize, Workflow, download, organize, split};

pub fn run(args: &Workflow) -> Result<(), String> {
    let input = classify_input(&args.raw);
    match &input {
        WorkflowInput::Local(_) | WorkflowInput::YoutubeVideo { .. } => {}
        WorkflowInput::YoutubePlaylist { .. } => {
            return Err("YouTube playlist workflow is not available in the Rust CLI yet.".into());
        }
        WorkflowInput::SpotifyExport(_) => {
            return Err("Spotify export workflow is not available in the Rust CLI yet.".into());
        }
        WorkflowInput::Search(_) => {
            return Err("Give a local audio path or one YouTube video URL. Soulseek workflow is not available in the Rust CLI yet.".into());
        }
    }
    if args.force && matches!(input, WorkflowInput::YoutubeVideo { .. }) {
        return Err("Forced YouTube download is not available in the Rust CLI yet.".into());
    }
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
        ..WorkflowOptions::default()
    };
    let mut operations = CliOperations {
        compilation: args.compilation,
    };
    let cancelled = AtomicBool::new(false);
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
}

impl WorkflowOperations for CliOperations {
    fn download_youtube(
        &mut self,
        url: &str,
        output: &Path,
        force: bool,
    ) -> Result<Vec<PathBuf>, String> {
        if force {
            return Err("Forced YouTube download is not available in the Rust CLI yet.".into());
        }
        let before = known_audio(output)?;
        let request = Download {
            url: url.to_owned(),
            output: Some(output.to_path_buf()),
            format: "bestaudio".into(),
            quality: "0".into(),
            no_chapters: false,
            archive_file: None,
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
        Ok(after
            .into_iter()
            .filter(|path| !before.contains(path))
            .collect())
    }

    fn acquire_soulseek(&mut self, _query: &str) -> Result<Vec<PathBuf>, String> {
        Err("Soulseek workflow is not available in the Rust CLI yet.".into())
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
        })?;
        assert!(audio.exists());
        assert!(!splits.exists());
        Ok(())
    }
}
