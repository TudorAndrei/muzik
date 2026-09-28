use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use yt_dlp::executor::Executor;

use crate::{Download, paths};

pub async fn run(args: &Download) -> Result<(), String> {
    let output = args.output.clone().unwrap_or_else(paths::download_dir);
    fs::create_dir_all(&output).map_err(|error| error.to_string())?;
    let output = fs::canonicalize(output).map_err(|error| error.to_string())?;
    let before = audio_files(&output).map_err(|error| error.to_string())?;

    println!("Downloading: {}", args.url);
    println!("Output: {}", output.display());
    let command = Executor::new(
        "yt-dlp",
        build_args(args, &output),
        Duration::from_secs(24 * 60 * 60),
    );
    let result = command.execute().await.map_err(|error| error.to_string())?;
    print!("{}", result.stdout);
    eprint!("{}", result.stderr);

    let after = audio_files(&output).map_err(|error| error.to_string())?;
    let new_files = after.difference(&before).collect::<Vec<_>>();
    println!("Download complete: {}", output.display());
    for file in new_files {
        let chapters = chapter_count(file);
        let next = if chapters > 0 { "split" } else { "organize" };
        println!(
            "{}: {chapters} chapters; next: muzik {next}",
            file.display()
        );
    }
    Ok(())
}

fn build_args(args: &Download, output: &Path) -> Vec<String> {
    let mut flags = Vec::new();
    if let Ok(browser) = env::var("MUZIK_YTDLP_COOKIES_FROM_BROWSER")
        && !browser.trim().is_empty()
    {
        flags.extend(["--cookies-from-browser".to_owned(), browser]);
    } else if let Ok(file) = env::var("MUZIK_YTDLP_COOKIES")
        && !file.trim().is_empty()
    {
        flags.extend(["--cookies".to_owned(), file]);
    }
    for runtime in ["node", "bun"] {
        if executable_on_path(runtime) {
            flags.extend(["--js-runtimes".to_owned(), runtime.to_owned()]);
            break;
        }
    }
    flags.extend([
        "--paths".to_owned(),
        output.to_string_lossy().into_owned(),
        "--format".to_owned(),
        args.format.clone(),
        "--extract-audio".to_owned(),
        "--audio-quality".to_owned(),
        args.quality.clone(),
        "--embed-metadata".to_owned(),
        "--add-metadata".to_owned(),
        "--write-thumbnail".to_owned(),
        "--convert-thumbnails".to_owned(),
        "jpg".to_owned(),
        "--output".to_owned(),
        "%(title)s [%(id)s].%(ext)s".to_owned(),
    ]);
    if !args.no_chapters {
        flags.extend([
            "--write-info-json".to_owned(),
            "--embed-chapters".to_owned(),
        ]);
    }
    if let Some(archive) = &args.archive_file {
        flags.extend([
            "--download-archive".to_owned(),
            archive.to_string_lossy().into_owned(),
        ]);
    }
    if args.force_overwrites {
        flags.push("--force-overwrites".to_owned());
    }
    flags.push(args.url.clone());
    flags
}

fn executable_on_path(name: &str) -> bool {
    env::var_os("PATH")
        .map(|value| {
            env::split_paths(&value).any(|directory| {
                let candidate = directory.join(name);
                candidate.is_file()
            })
        })
        .unwrap_or(false)
}

fn audio_files(directory: &Path) -> io::Result<BTreeSet<PathBuf>> {
    fs::read_dir(directory)?
        .filter_map(|entry| match entry {
            Ok(entry) => {
                let path = entry.path();
                path.is_file()
                    .then_some(path)
                    .filter(|path| {
                        path.extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| {
                                ["flac", "mp3", "m4a", "opus", "wav", "aac"]
                                    .contains(&ext.to_ascii_lowercase().as_str())
                            })
                    })
                    .map(Ok)
            }
            Err(error) => Some(Err(error)),
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Download, build_args};

    #[test]
    fn youtube_command_keeps_audio_and_metadata_options() {
        let args = Download {
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_owned(),
            output: None,
            format: "bestaudio".to_owned(),
            quality: "0".to_owned(),
            no_chapters: false,
            archive_file: Some("archive.txt".into()),
            force_overwrites: false,
        };
        let flags = build_args(&args, Path::new("/music/downloads"));
        for flag in [
            "--extract-audio",
            "--embed-metadata",
            "--write-info-json",
            "--embed-chapters",
            "--download-archive",
        ] {
            assert!(flags.iter().any(|value| value == flag));
        }
        assert_eq!(flags.last().map(String::as_str), Some(args.url.as_str()));
    }
}
