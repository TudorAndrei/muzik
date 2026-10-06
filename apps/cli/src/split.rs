//! Chapter review and the native ffmpeg splitter command.

use std::path::PathBuf;

use anyhow::{Context, anyhow, bail};
use muzik_core::chapters::{self, Chapter};
use muzik_media::splitter::{self, SplitOptions};

use crate::Split;

pub fn run(args: &Split) -> anyhow::Result<PathBuf> {
    if !args.path.is_file() {
        bail!("File not found: {}", args.path.display());
    }
    let mut chapters = chapters::find_chapters(&args.path)?;
    if chapters.is_empty() {
        bail!("No chapters found. Add a .chapters.txt or .info.json sidecar.");
    }
    show_chapters(&chapters);
    if args.review {
        let Some(reviewed) = review_chapters(chapters)? else {
            println!("Split cancelled.");
            return Ok(PathBuf::new());
        };
        chapters = reviewed;
    }
    let output = match &args.output {
        Some(path) => path.clone(),
        None => splitter::default_output(&args.path)?,
    };
    let options = SplitOptions {
        jobs: args.jobs,
        keep_source: args.keep_source,
        force: args.force,
        compilation: false,
        cache_dir: None,
    };
    println!(
        "Splitting {} tracks to {}",
        chapters.len(),
        output.display()
    );
    let output =
        splitter::split_audio(&args.path, &chapters, &output, &options).map_err(split_error)?;
    println!("Split complete: {}", output.display());
    Ok(output)
}

pub(crate) fn split_error(error: splitter::SplitError) -> anyhow::Error {
    match error {
        splitter::SplitError::OutputNotEmpty(_) => {
            anyhow!("{error} Use --force to replace them.")
        }
        other => other.into(),
    }
}

pub(crate) fn show_chapters(chapters: &[Chapter]) {
    println!("  #  Start     Title");
    for chapter in chapters {
        println!(
            "{:>3}  {:>8}  {}",
            chapter.index,
            clock(chapter.start),
            chapter.title
        );
    }
}

fn clock(seconds: i64) -> String {
    let hours = seconds / 3600;
    let minutes = seconds.rem_euclid(3600) / 60;
    let seconds = seconds.rem_euclid(60);
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

pub(crate) fn review_chapters(mut chapters: Vec<Chapter>) -> anyhow::Result<Option<Vec<Chapter>>> {
    loop {
        let choice = dialoguer::Select::new()
            .with_prompt("Continue, edit, or abort?")
            .items(["Continue", "Edit", "Abort"])
            .default(0)
            .interact_opt()?;
        match choice {
            Some(0) => return Ok(Some(chapters)),
            Some(1) => {
                let edited = edit_chapters(&chapters)?;
                if edited.is_empty() {
                    eprintln!("No valid chapters in the edited file. The prior list remains.");
                } else {
                    chapters = edited;
                    show_chapters(&chapters);
                }
            }
            _ => return Ok(None),
        }
    }
}

pub(crate) fn edit_chapters(chapters: &[Chapter]) -> anyhow::Result<Vec<Chapter>> {
    let mut text = String::new();
    for chapter in chapters {
        text.push_str(&format!("{} {}\n", clock(chapter.start), chapter.title));
    }
    let text = edit::edit_with_builder(
        text,
        edit::Builder::new()
            .prefix("muzik-chapters-")
            .suffix(".chapters.txt"),
    )
    .context("Cannot open editor")?;
    Ok(chapters::parse_chapters(&text))
}
