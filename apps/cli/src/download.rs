use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use muzik_core::chapters::Sidecar;
use muzik_workflow::Error;
use muzik_workflow::ytdlp::{self, YtDlp};

use crate::{Download, paths};

pub fn run(args: &Download) -> anyhow::Result<()> {
    let output = args.output.clone().unwrap_or_else(paths::download_dir);
    println!("Downloading: {}", args.url);
    println!("Output: {}", output.display());
    let request = ytdlp::Download {
        target: &args.url,
        output: &output,
        format: &args.format,
        quality: &args.quality,
        chapters: !args.no_chapters,
        archive: args.archive_file.as_deref(),
        playlist: true,
        force: args.force_overwrites,
    };
    let files = match YtDlp::default().download(&request, &AtomicBool::new(false)) {
        Ok(files) => files,
        Err(Error::NoAudio) => {
            println!(
                "Download complete: {} (no new audio files)",
                output.display()
            );
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    println!("Download complete: {}", output.display());
    for file in files {
        match chapter_count(&file) {
            Some(chapters) if chapters > 0 => {
                println!("{}: {chapters} chapters; next: muzik split", file.display());
            }
            Some(_) => println!("{}: 0 chapters; next: muzik organize", file.display()),
            None => println!("{}: no chapter data; next: muzik organize", file.display()),
        }
    }
    Ok(())
}

fn chapter_count(audio: &Path) -> Option<usize> {
    let text = fs::read_to_string(Sidecar::Info.path(audio)).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get("chapters")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
}
