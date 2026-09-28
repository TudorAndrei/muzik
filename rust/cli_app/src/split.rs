//! Chapter review and the native ffmpeg splitter command.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use muzik_core::chapters::{self, Chapter};
use muzik_core::splitter::{self, SplitOptions};

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
    let output = splitter::split_audio(&args.path, &chapters, &output, &options)
        .map_err(|error| error.to_string())?;
    println!("Split complete: {}", output.display());
    Ok(output)
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
        print!("Continue, edit, or abort? [c/e/a] ");
        io::stdout().flush().map_err(|error| error.to_string())?;
        let mut answer = String::new();
        let bytes = io::stdin()
            .read_line(&mut answer)
            .map_err(|error| error.to_string())?;
        if bytes == 0 {
            return Ok(None);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "c" | "continue" => return Ok(Some(chapters)),
            "a" | "abort" => return Ok(None),
            "e" | "edit" => {
                let edited = edit_chapters(&chapters)?;
                if edited.is_empty() {
                    eprintln!("No valid chapters in the edited file. The prior list remains.");
                } else {
                    chapters = edited;
                    show_chapters(&chapters);
                }
            }
            _ => eprintln!("Enter c, e, or a."),
        }
    }
}

fn edit_chapters(chapters: &[Chapter]) -> Result<Vec<Chapter>, String> {
    let mut file = tempfile::Builder::new()
        .prefix("muzik-chapters-")
        .suffix(".chapters.txt")
        .tempfile()
        .map_err(|error| error.to_string())?;
    for chapter in chapters {
        writeln!(file, "{} {}", clock(chapter.start), chapter.title)
            .map_err(|error| error.to_string())?;
    }
    file.flush().map_err(|error| error.to_string())?;
    let editor = std::env::var("EDITOR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("VISUAL").ok())
        .unwrap_or_else(|| "vi".into());
    let mut parts = editor.split_whitespace();
    let program = parts.next().ok_or("Editor command is empty")?;
    let status = Command::new(program)
        .args(parts)
        .arg(file.path())
        .status()
        .map_err(|error| format!("Cannot open editor: {error}"))?;
    if !status.success() {
        return Err(format!("Editor exited with {status}"));
    }
    let text = std::fs::read_to_string(file.path()).map_err(|error| error.to_string())?;
    Ok(chapters::parse_chapters(&text))
}

pub fn audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "flac" | "mp3" | "m4a" | "opus" | "wav" | "aac" | "ogg" | "aiff" | "aif"
            )
        })
}
