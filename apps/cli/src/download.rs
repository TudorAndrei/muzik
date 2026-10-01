use std::fs;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use muzik_workflow::Error;
use muzik_workflow::ytdlp::{self, YtDlp};

use crate::{Download, paths};

pub fn run(args: &Download) -> Result<(), String> {
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
        Err(Error::NoAudio) => Vec::new(),
        Err(error) => return Err(error.to_string()),
    };
    println!("Download complete: {}", output.display());
    for file in files {
        let chapters = chapter_count(&file);
        let next = if chapters > 0 { "split" } else { "organize" };
        println!(
            "{}: {chapters} chapters; next: muzik {next}",
            file.display()
        );
    }
    Ok(())
}

fn chapter_count(audio: &Path) -> usize {
    let Some(stem) = audio.file_stem() else {
        return 0;
    };
    let json_path = audio.with_file_name(format!("{}.info.json", stem.to_string_lossy()));
    let Ok(text) = fs::read_to_string(json_path) else {
        return 0;
    };
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    data.get("chapters")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}
