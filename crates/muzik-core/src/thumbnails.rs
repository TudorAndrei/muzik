//! Thumbnail cache shared with saved watchlist items.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn valid_id(id: &str) -> bool {
    (6..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub fn cached_path(id: &str, root: &Path) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    for extension in ["jpg", "png"] {
        let path = root.join(format!("yt_thumbnail_{id}.{extension}"));
        if path
            .metadata()
            .is_ok_and(|meta| meta.is_file() && meta.len() > 0)
        {
            return Some(path);
        }
    }
    None
}

pub fn save(id: &str, content_type: &str, bytes: &[u8], root: &Path) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("Invalid thumbnail ID.".into());
    }
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let extension = match media_type.as_str() {
        "image/jpeg" if bytes.starts_with(b"\xff\xd8\xff") => "jpg",
        "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "png",
        "image/jpeg" | "image/png" => {
            return Err("Thumbnail response has invalid image data.".into())
        }
        _ => return Err("Thumbnail response is not a JPEG or PNG image.".into()),
    };
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let path = root.join(format!("yt_thumbnail_{id}.{extension}"));
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".yt_thumbnail_{id}.{extension}."))
        .suffix(".tmp")
        .tempfile_in(root)
        .map_err(|error| error.to_string())?;
    temporary
        .write_all(bytes)
        .map_err(|error| error.to_string())?;
    temporary.flush().map_err(|error| error.to_string())?;
    temporary
        .persist(&path)
        .map_err(|error| error.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{cached_path, save};

    #[test]
    fn saves_valid_images_to_the_existing_cache_names() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let jpeg = save(
            "abcdefghijk",
            "image/jpeg; charset=binary",
            b"\xff\xd8\xffdata",
            root.path(),
        )
        .map_err(std::io::Error::other)?;
        assert_eq!(
            jpeg.file_name().and_then(|name| name.to_str()),
            Some("yt_thumbnail_abcdefghijk.jpg")
        );
        assert_eq!(cached_path("abcdefghijk", root.path()), Some(jpeg));
        assert!(save(
            "../escape",
            "image/png",
            b"\x89PNG\r\n\x1a\ndata",
            root.path()
        )
        .is_err());
        assert!(save("other_id", "text/html", b"<html>", root.path()).is_err());
        Ok(())
    }
}
