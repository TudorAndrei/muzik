//! Inventory of downloaded audio in the existing flat output directory.

use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const AUDIO_EXTENSIONS: &[&str] = &["flac", "mp3", "m4a", "opus", "wav", "aac"];

#[derive(Debug, Serialize)]
pub struct DownloadedItem {
    pub path: PathBuf,
    pub title: String,
    pub youtube_id: Option<String>,
    pub ext: String,
    pub size: u64,
    pub mtime: f64,
    #[serde(skip)]
    pub modified_at: SystemTime,
}

pub fn scan(directory: &Path) -> io::Result<Vec<DownloadedItem>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if !AUDIO_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let metadata = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let modified_at = metadata.modified()?;
        let mtime = modified_at
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |time| time.as_secs_f64());
        items.push(DownloadedItem {
            path,
            title: title_from_name(&stem),
            youtube_id: youtube_id_from_name(&name).map(str::to_owned),
            ext,
            size: metadata.len(),
            mtime,
            modified_at,
        });
    }
    items.sort_by_key(|item| item.title.to_lowercase());
    Ok(items)
}

pub fn youtube_id_from_name(name: &str) -> Option<&str> {
    name.as_bytes()
        .windows(13)
        .enumerate()
        .find_map(|(offset, part)| {
            (part.first() == Some(&b'[')
                && part.last() == Some(&b']')
                && part.get(1..12).is_some_and(|id| {
                    id.iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-')
                }))
            .then(|| name.get(offset + 1..offset + 12))
            .flatten()
        })
}

pub fn title_from_name(stem: &str) -> String {
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

pub fn human_size(bytes: u64) -> String {
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
    use super::scan;
    use std::fs;

    #[test]
    fn scans_flat_audio_and_keeps_existing_youtube_names() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join("Track [dQw4w9WgXcQ].mp3"), b"audio")?;
        fs::write(dir.path().join("cover.jpg"), b"image")?;
        let items = scan(dir.path())?;
        assert_eq!(items.len(), 1);
        assert_eq!(items.first().map(|item| item.title.as_str()), Some("Track"));
        assert_eq!(
            items.first().and_then(|item| item.youtube_id.as_deref()),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(items.first().map(|item| item.size), Some(5));
        Ok(())
    }
}
