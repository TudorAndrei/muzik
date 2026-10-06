//! Process downloaded audio already on disk.

use std::fs;
use std::path::PathBuf;

use anyhow::bail;
use muzik_core::chapters;

use crate::{Archive, Organize, Split, organize, split};

pub fn run(args: &Archive) -> anyhow::Result<()> {
    if !args.directory.is_dir() {
        bail!("Directory not found: {}", args.directory.display());
    }
    let mut audio = fs::read_dir(&args.directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<PathBuf>, _>>()?;
    audio.retain(|path| path.is_file() && muzik_core::audio::is_audio(path));
    audio.sort();
    if audio.is_empty() {
        println!("No audio files found in {}", args.directory.display());
        return Ok(());
    }
    println!(
        "Archive: {} ({} audio files)",
        args.directory.display(),
        audio.len()
    );
    let mut processed = 0;
    let mut skipped = 0;
    let mut failed = 0;
    if !args.skip_split {
        for path in &audio {
            let chapters = match chapters::find_chapters(path) {
                Ok(chapters) => chapters,
                Err(error) => {
                    eprintln!("Cannot read chapters for {}: {error}", path.display());
                    failed += 1;
                    continue;
                }
            };
            if chapters.is_empty() {
                println!("No chapters: {}", path.display());
                skipped += 1;
                continue;
            }
            let Some(stem) = path.file_stem() else {
                failed += 1;
                continue;
            };
            let output = args.output.join(stem);
            if args.dry_run {
                println!(
                    "Would split {} tracks to {}",
                    chapters.len(),
                    output.display()
                );
                continue;
            }
            let request = Split {
                path: path.clone(),
                review: false,
                jobs: args.jobs,
                output: Some(output),
                keep_source: args.keep_source,
                force: false,
            };
            match split::run(&request) {
                Ok(_) => processed += 1,
                Err(error) => {
                    eprintln!("Split failed for {}: {error:#}", path.display());
                    failed += 1;
                }
            }
        }
        println!("Split summary: {processed} processed, {skipped} skipped, {failed} failed");
    }
    if !args.skip_organize {
        if args.dry_run {
            println!("Would organize audio under {}", args.output.display());
        } else if args.output.is_dir() && fs::read_dir(&args.output)?.next().is_some() {
            organize::run(&Organize {
                directory: args.output.clone(),
                import: args.import,
                tag_only: args.tag_only,
                dry_run: false,
                config: args.config.clone(),
            })?;
        } else {
            println!("No split output found in {}", args.output.display());
        }
    }
    if failed > 0 {
        bail!("{failed} audio file(s) failed to split");
    }
    println!("Archive processing complete.");
    Ok(())
}
