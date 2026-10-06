//! Chapter review and the native ffmpeg splitter command.

use std::path::PathBuf;

use muzik_core::chapters::{self, Chapter};
use muzik_media::splitter::{self, SplitOptions};

use crate::Split;

pub fn run(args: &Split) -> Result<PathBuf, String> {
    if !args.path.is_file() {
        return Err(format!("File not found: {}", args.path.display()));
    }
    let mut chapters = chapters::find_chapters(&args.path).map_err(|error| error.to_string())?;
    if chapters.is_empty() {
        return Err("No chapters found. Add a .chapters.txt or .info.json sidecar.".into());
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
        None => splitter::default_output(&args.path).map_err(|error| error.to_string())?,
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

pub(crate) fn split_error(error: splitter::SplitError) -> String {
    match error {
        splitter::SplitError::OutputNotEmpty(_) => {
            format!("{error} Use --force to replace them.")
        }
        other => other.to_string(),
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

pub(crate) fn review_chapters(mut chapters: Vec<Chapter>) -> Result<Option<Vec<Chapter>>, String> {
    loop {
        let choice = dialoguer::Select::new()
            .with_prompt("Continue, edit, or abort?")
            .items(["Continue", "Edit", "Abort"])
            .default(0)
            .interact_opt()
            .map_err(|error| error.to_string())?;
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

pub(crate) fn edit_chapters(chapters: &[Chapter]) -> Result<Vec<Chapter>, String> {
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
    .map_err(|error| format!("Cannot open editor: {error}"))?;
    Ok(chapters::parse_chapters(&text))
}
