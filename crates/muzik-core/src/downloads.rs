//! Inventory of downloaded audio in the existing flat output directory.

use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
        if !crate::audio::is_audio(&path) {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
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
    name.as_bytes().windows(13).find_map(|part| {
        let id = part.get(1..12)?;
        (part.first() == Some(&b'[')
            && part.last() == Some(&b']')
            && id
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-'))
        .then(|| std::str::from_utf8(id).ok())
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
