use std::fs;
use std::io;
use std::path::Path;

const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "m4a", "opus", "wav", "aac"];

pub fn list(directory: &Path) -> io::Result<()> {
    if !directory.exists() {
        println!("No downloads found. ({})", directory.display());
        return Ok(());
    }

    let mut entries = fs::read_dir(directory)?
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|entry| {
            entry.path().is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
                    })
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_lowercase());

    if entries.is_empty() {
        println!("No downloads found. ({})", directory.display());
        return Ok(());
    }

    println!("Downloaded audio in {}", directory.display());
    let mut total_bytes = 0_u64;
    let mut with_id = 0_usize;
    for entry in &entries {
        let metadata = entry.metadata()?;
        total_bytes += metadata.len();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let id = youtube_id_from_name(&name);
        if id.is_some() {
            with_id += 1;
        }
        let stem = entry
            .path()
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        println!(
            "{}\t{}\t{}",
            title_from_name(&stem),
            id.unwrap_or(""),
            human_size(metadata.len())
        );
    }
    println!(
        "Total: {} file(s), {}; {} with a YouTube id.",
        entries.len(),
        human_size(total_bytes),
        with_id
    );
    Ok(())
}

fn youtube_id_from_name(name: &str) -> Option<&str> {
    name.as_bytes()
        .windows(13)
        .enumerate()
        .find_map(|(i, part)| {
            (part.first() == Some(&b'[')
                && part.last() == Some(&b']')
                && part.get(1..12).is_some_and(|id| {
                    id.iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
                }))
            .then(|| name.get(i + 1..i + 12))
            .flatten()
        })
}

fn title_from_name(stem: &str) -> String {
    if let Some(id) = youtube_id_from_name(stem) {
        let marker = format!("[{id}]");
        let title = stem.replace(&marker, "");
        let title = title.trim().trim_end_matches('-').trim();
        if !title.is_empty() {
            return title.to_owned();
        }
    }
    stem.to_owned()
}

fn human_size(bytes: u64) -> String {
    let mut scale = 1_u128;
    for unit in ["B", "KB", "MB", "GB"] {
        if u128::from(bytes) < scale.saturating_mul(1024) {
            let tenths = u128::from(bytes)
                .saturating_mul(10)
                .saturating_add(scale / 2)
                / scale;
            return format!("{}.{:01} {unit}", tenths / 10, tenths % 10);
        }
        scale = scale.saturating_mul(1024);
    }
    let tenths = u128::from(bytes)
        .saturating_mul(10)
        .saturating_add(scale / 2)
        / scale;
    format!("{}.{:01} TB", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::{title_from_name, youtube_id_from_name};

    #[test]
    fn reads_downloaded_youtube_id_and_title() {
        assert_eq!(
            youtube_id_from_name("Some song [dQw4w9WgXcQ].flac"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(title_from_name("Some song [dQw4w9WgXcQ]"), "Some song");
    }
}
