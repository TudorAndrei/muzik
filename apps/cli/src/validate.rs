use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde_json::Value;

use crate::Validate;

pub fn run(args: &Validate) -> anyhow::Result<()> {
    if !args.path.exists() {
        bail!("not found: {}", args.path.display());
    }
    let mut files = Vec::new();
    collect(&args.path, args.recursive, &mut files)?;
    files.retain(|path| kind(path).is_some());
    files.sort();
    if files.is_empty() {
        println!("No relevant files found.");
        return Ok(());
    }

    let mut valid = 0;
    let mut warnings = 0;
    let mut invalid = 0;
    for path in &files {
        let name = path.strip_prefix(&args.path).unwrap_or(path);
        let name = if name.as_os_str().is_empty() {
            path.file_name().map(Path::new).unwrap_or(path)
        } else {
            name
        };
        match check(path) {
            Ok((file_kind, details, file_warnings)) => {
                valid += 1;
                if !file_warnings.is_empty() {
                    warnings += 1;
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
                invalid += 1;
                println!("{}\tFAIL\t{error:#}", name.display());
            }
        }
    }
    println!(
        "{valid} valid, {warnings} warnings, {invalid} invalid ({} files checked)",
        files.len()
    );
    if invalid > 0 {
        bail!("{invalid} files failed validation")
    } else {
        Ok(())
    }
}

fn collect(path: &Path, recursive: bool, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if path.is_file() {
        files.push(path.to_path_buf());
    } else if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let child = entry.path();
            if child.is_file() || (recursive && child.is_dir()) {
                collect(&child, recursive, files)?;
            }
        }
    }
    Ok(())
}

fn kind(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if muzik_core::audio::is_audio(path) {
        Some("audio")
    } else if name.ends_with(".chapters.txt") {
        Some("chapters")
    } else if name.ends_with(".info.json") {
        Some("info.json")
    } else if name.ends_with(".muzik.json") {
        Some("muzik")
    } else {
        None
    }
}

fn check(path: &Path) -> anyhow::Result<(&'static str, String, Vec<String>)> {
    let file_kind = kind(path).context("unsupported file")?;
    let mut warnings = Vec::new();
    let details = match file_kind {
        "audio" => {
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
        "chapters" => {
            let source = fs::read_to_string(path)?;
            let count = source.lines().filter(|line| chapter_line(line)).count();
            if count == 0 {
                bail!("no valid chapter lines found");
            }
            format!("{count} chapters")
        }
        "info.json" => {
            let value = read_object(path)?;
            let title = value.get("title").and_then(Value::as_str).unwrap_or("?");
            let chapters = value
                .get("chapters")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("title={title} chapters={chapters}")
        }
        "muzik" => {
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
        _ => bail!("unsupported file"),
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

fn chapter_line(line: &str) -> bool {
    let Some((timestamp, title)) = line.trim().split_once(char::is_whitespace) else {
        return false;
    };
    let parts: Vec<_> = timestamp.split(':').collect();
    (parts.len() == 2 || parts.len() == 3)
        && parts.iter().all(|part| part.parse::<u64>().is_ok())
        && !title.trim().is_empty()
}

fn count_audio(root: &Path) -> anyhow::Result<usize> {
    let mut files = Vec::new();
    collect(root, true, &mut files)?;
    Ok(files
        .into_iter()
        .filter(|path| kind(path) == Some("audio"))
        .count())
}

#[cfg(test)]
mod tests {
    use super::{chapter_line, check};
    use std::fs;

    #[test]
    fn chapter_lines_need_a_time_and_title() {
        assert!(chapter_line("01:23 First track"));
        assert!(chapter_line("01:02:03 Last track"));
        assert!(!chapter_line("01:23"));
        assert!(!chapter_line("Track title"));
    }

    #[test]
    fn metadata_sidecar_reports_missing_source_and_tracks() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let sidecar = dir.path().join(".muzik.json");
        fs::write(&sidecar, r#"{"candidate":{"files":[{},{}]}}"#)?;
        let (_, _, warnings) = check(&sidecar)?;
        assert!(warnings.iter().any(|item| item == "missing source"));
        assert!(warnings.iter().any(|item| item == "missing source_id"));
        assert!(
            warnings
                .iter()
                .any(|item| item == "album appears incomplete (0/2 audio files)")
        );
        Ok(())
    }
}
