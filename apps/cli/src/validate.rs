use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use muzik_core::chapters::{Sidecar, parse_chapters};
use serde_json::Value;
use strum_macros::Display;

use crate::Validate;

pub fn run(args: &Validate) -> anyhow::Result<()> {
    if !args.path.exists() {
        bail!("not found: {}", args.path.display());
    }
    let mut files = collect(&args.path, args.recursive)?;
    files.retain(|path| kind(path).is_some());
    files.sort();
    if files.is_empty() {
        println!("No relevant files found.");
        return Ok(());
    }

    let mut valid = 0_usize;
    let mut warnings = 0_usize;
    let mut invalid = 0_usize;
    for path in &files {
        let name = path.strip_prefix(&args.path).unwrap_or(path);
        let name = if name.as_os_str().is_empty() {
            path.file_name().map_or(path.as_path(), Path::new)
        } else {
            name
        };
        match check(path) {
            Ok((file_kind, details, file_warnings)) => {
                valid = valid.saturating_add(1);
                if !file_warnings.is_empty() {
                    warnings = warnings.saturating_add(1);
                }
                let status = if file_warnings.is_empty() {
                    "OK"
                } else {
                    "WARN"
                };
                if args.verbose {
                    let mut detail = details;
                    if !file_warnings.is_empty() {
                        if !detail.is_empty() {
                            detail.push_str("; ");
                        }
                        detail.push_str(&file_warnings.join("; "));
                    }
                    println!("{}\t{file_kind}\t{status}\t{detail}", name.display());
                } else {
                    println!("{}\t{file_kind}\t{status}", name.display());
                }
            }
            Err(error) => {
                invalid = invalid.saturating_add(1);
                println!("{}\tFAIL\t{}", name.display(), crate::describe(&error));
            }
        }
    }
    println!(
        "{valid} valid, {warnings} warnings, {invalid} invalid ({} files checked)",
        files.len()
    );
    if invalid > 0 {
        bail!("{invalid} files failed validation")
    }
    Ok(())
}

fn collect(path: &Path, recursive: bool) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !path.exists() {
        return Ok(files);
    }
    let walker = walkdir::WalkDir::new(path)
        .follow_links(true)
        .max_depth(if recursive { usize::MAX } else { 1 });
    for entry in walker {
        let entry = match entry {
            Err(error)
                if error
                    .io_error()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            entry => entry.map_err(std::io::Error::from)?,
        };
        if entry.file_type().is_file() {
            files.push(entry.into_path());
        }
    }
    Ok(files)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Display)]
#[strum(serialize_all = "lowercase")]
enum FileKind {
    Audio,
    Chapters,
    #[strum(to_string = "info.json")]
    InfoJson,
    Muzik,
}

fn kind(path: &Path) -> Option<FileKind> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if muzik_core::audio::is_audio(path) {
        Some(FileKind::Audio)
    } else if name.ends_with(Sidecar::Chapters.suffix()) {
        Some(FileKind::Chapters)
    } else if name.ends_with(Sidecar::Info.suffix()) {
        Some(FileKind::InfoJson)
    } else if name.ends_with(Sidecar::Muzik.suffix()) {
        Some(FileKind::Muzik)
    } else {
        None
    }
}

fn check(path: &Path) -> anyhow::Result<(FileKind, String, Vec<String>)> {
    let file_kind = kind(path).context("unsupported file")?;
    let mut warnings = Vec::new();
    let details = match file_kind {
        FileKind::Audio => {
            let properties = muzik_tags::probe(path)?;
            if metadata_for_audio(path).is_none() {
                warnings.push("metadata sidecar missing".into());
            }
            format!(
                "codec={} duration={:.0}s",
                properties
                    .codec
                    .as_ref()
                    .map_or_else(|| properties.format.to_string(), ToString::to_string),
                properties.duration_seconds.unwrap_or(0.0)
            )
        }
        FileKind::Chapters => {
            let source = fs::read_to_string(path)?;
            let count = parse_chapters(&source).len();
            if count == 0 {
                bail!("no valid chapter lines found");
            }
            format!("{count} chapters")
        }
        FileKind::InfoJson => {
            let value = read_object(path)?;
            let title = value.get("title").and_then(Value::as_str).unwrap_or("?");
            let chapters = value
                .get("chapters")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("title={title} chapters={chapters}")
        }
        FileKind::Muzik => {
            let value = read_object(path)?;
            if value
                .get("source")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                warnings.push("missing source".into());
            }
            if value
                .get("source_id")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                warnings.push("missing source_id".into());
            }
            let expected = value
                .get("candidate")
                .and_then(|candidate| candidate.get("files"))
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if expected > 0 {
                let actual = count_audio(path.parent().context("sidecar has no parent")?)?;
                if actual < expected {
                    warnings.push(format!(
                        "album appears incomplete ({actual}/{expected} audio files)"
                    ));
                }
            }
            format!(
                "source={}",
                value.get("source").and_then(Value::as_str).unwrap_or("?")
            )
        }
    };
    Ok((file_kind, details, warnings))
}

fn metadata_for_audio(path: &Path) -> Option<Value> {
    let local = path.with_extension("muzik.json");
    let album = path.parent()?.join(".muzik.json");
    [local, album]
        .into_iter()
        .find_map(|sidecar| read_object(&sidecar).ok())
}

fn read_object(path: &Path) -> anyhow::Result<Value> {
    let source = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&source)?;
    if !value.is_object() {
        bail!("root is not a JSON object");
    }
    Ok(value)
}

fn count_audio(root: &Path) -> anyhow::Result<usize> {
    Ok(collect(root, true)?
        .into_iter()
        .filter(|path| kind(path) == Some(FileKind::Audio))
        .count())
}

#[cfg(test)]
mod tests {
    use super::check;
    use std::fs;

    #[test]
    fn chapter_count_matches_the_chapters_the_splitter_reads() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join("Mix.chapters.txt");
        fs::write(
            &sidecar,
            "01:23 First track\n1:2 Short seconds\n01:23 Repeated start\n01:02:03 Last track\n01:30\nTrack title\n",
        )
        .unwrap();
        let (_, details, _) = check(&sidecar).unwrap();
        assert_eq!(details, "2 chapters");
    }

    #[test]
    fn metadata_sidecar_reports_missing_source_and_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let sidecar = dir.path().join(".muzik.json");
        fs::write(&sidecar, r#"{"candidate":{"files":[{},{}]}}"#).unwrap();
        let (_, _, warnings) = check(&sidecar).unwrap();
        assert!(warnings.iter().any(|item| item == "missing source"));
        assert!(warnings.iter().any(|item| item == "missing source_id"));
        assert!(
            warnings
                .iter()
                .any(|item| item == "album appears incomplete (0/2 audio files)")
        );
    }
}
