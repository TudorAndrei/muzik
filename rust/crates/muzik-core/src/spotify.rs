//! Spotify settings and saved token path shared by the two apps.

use crate::{app_config, paths};
use std::fs;
use std::path::{Path, PathBuf};

pub fn token_path() -> PathBuf {
    paths::config_dir().join("spotify-token.json")
}

pub fn set_client_id(path: &Path, client_id: &str) -> Result<String, String> {
    let value = client_id.trim();
    app_config::save_section_string(path, "spotify", "client_id", value)?;
    Ok(value.to_owned())
}

pub fn clear_tokens(path: &Path) -> Result<bool, String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::clear_tokens;
    use std::fs;

    #[test]
    fn logout_removes_existing_tokens() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("spotify-token.json");
        fs::write(&path, "saved tokens")?;
        assert!(clear_tokens(&path).map_err(std::io::Error::other)?);
        assert!(!clear_tokens(&path).map_err(std::io::Error::other)?);
        Ok(())
    }
}
