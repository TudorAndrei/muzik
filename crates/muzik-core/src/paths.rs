use etcetera::app_strategy::{choose_native_strategy, AppStrategy, AppStrategyArgs};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    pub beets: PathBuf,
}

impl Paths {
    pub fn user() -> Self {
        Self {
            data: data_dir(),
            config: config_dir(),
            cache: cache_dir(),
            beets: crate::default_config_path(),
        }
    }

    pub fn under(root: &Path) -> Self {
        Self {
            data: root.join("data"),
            config: root.join("config"),
            cache: root.join("cache"),
            beets: root.join("beets/config.yaml"),
        }
    }

    pub fn spotify_token(&self) -> PathBuf {
        self.config.join("spotify-token.json")
    }

    pub fn bandcamp_cookies(&self) -> PathBuf {
        self.config.join("bandcamp_cookies.txt")
    }

    pub fn bandcamp_user(&self) -> PathBuf {
        self.config.join("bandcamp_user")
    }

    pub fn database(&self) -> PathBuf {
        self.data.join("muzik.db")
    }

    pub fn config_file(&self) -> PathBuf {
        self.config.join("config.yaml")
    }

    pub fn downloads(&self) -> PathBuf {
        self.data.join("downloads")
    }

    pub fn splits(&self) -> PathBuf {
        self.data.join("splits")
    }

    pub fn soulseek(&self) -> PathBuf {
        self.data.join("soulseek")
    }
}

pub fn expand_home(path: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home().join(rest),
        Err(_) => path.to_path_buf(),
    }
}

fn native() -> Option<impl AppStrategy> {
    choose_native_strategy(AppStrategyArgs {
        top_level_domain: "com".to_owned(),
        author: "tudorandrei".to_owned(),
        app_name: "muzik".to_owned(),
    })
    .ok()
}

pub fn data_dir() -> PathBuf {
    native().map_or_else(|| PathBuf::from("muzik"), |strategy| strategy.data_dir())
}

pub fn download_dir() -> PathBuf {
    data_dir().join("downloads")
}

pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        return data_dir();
    }
    native().map_or_else(
        || PathBuf::from("muzik/config"),
        |strategy| strategy.config_dir(),
    )
}

pub fn cache_dir() -> PathBuf {
    native().map_or_else(
        || PathBuf::from("muzik/cache"),
        |strategy| strategy.cache_dir(),
    )
}

fn home() -> PathBuf {
    etcetera::home_dir().unwrap_or_default()
}

/// Move the folders of muzik 2.x on macOS to the folders named by the app bundle ID.
pub fn migrate_legacy(paths: &Paths) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    if !cfg!(target_os = "macos") {
        return Ok(Vec::new());
    }
    let Ok(home) = etcetera::home_dir() else {
        return Ok(Vec::new());
    };
    let mut moved = Vec::new();
    for (old, new, required) in [
        (
            home.join("Library/Application Support/muzik"),
            &paths.data,
            true,
        ),
        (home.join("Library/Caches/muzik"), &paths.cache, false),
    ] {
        match move_dir(&old, new)? {
            Move::Moved => moved.push((old, new.clone())),
            Move::Conflict if required => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "both {} and {} exist; merge them, then remove {}",
                        old.display(),
                        new.display(),
                        old.display()
                    ),
                ))
            }
            Move::Conflict | Move::Skipped => {}
        }
    }
    Ok(moved)
}

#[derive(Debug, PartialEq, Eq)]
enum Move {
    Moved,
    Skipped,
    Conflict,
}

fn move_dir(old: &Path, new: &Path) -> io::Result<Move> {
    match fs::symlink_metadata(old) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(Move::Skipped),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Move::Skipped),
        Err(error) => return Err(error),
    }
    if new.exists() {
        if fs::read_dir(new)?.next().is_some() {
            return Ok(Move::Conflict);
        }
        fs::remove_dir(new)?;
    }
    if let Some(parent) = new.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::rename(old, new) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Move::Skipped),
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    match std::os::unix::fs::symlink(new, old) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(Move::Moved)
}

#[cfg(test)]
mod tests {
    use super::{expand_home, home, move_dir, Move};
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn expand_home_replaces_only_a_leading_tilde() {
        assert_eq!(expand_home(Path::new("~")), home());
        assert_eq!(expand_home(Path::new("~/Music")), home().join("Music"));
        assert_eq!(
            expand_home(Path::new("/music/~/x")),
            PathBuf::from("/music/~/x")
        );
        assert_eq!(expand_home(Path::new("~other")), PathBuf::from("~other"));
    }

    #[test]
    fn move_dir_moves_the_old_folder_and_links_it_to_the_new_one() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("Application Support/muzik");
        let new = root
            .path()
            .join("Application Support/com.tudorandrei.muzik");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("muzik.db"), "state").unwrap();

        assert_eq!(move_dir(&old, &new).unwrap(), Move::Moved);
        assert_eq!(fs::read_to_string(new.join("muzik.db")).unwrap(), "state");
        assert!(fs::symlink_metadata(&old).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(old.join("muzik.db")).unwrap(), "state");

        assert_eq!(move_dir(&old, &new).unwrap(), Move::Skipped);
    }

    #[test]
    fn move_dir_replaces_an_empty_new_folder() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let new = root.path().join("new");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("cover.jpg"), "image").unwrap();
        fs::create_dir_all(&new).unwrap();

        assert_eq!(move_dir(&old, &new).unwrap(), Move::Moved);
        assert!(new.join("cover.jpg").is_file());
    }

    #[test]
    fn move_dir_keeps_both_folders_when_the_new_one_has_files() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("old");
        let new = root.path().join("new");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("muzik.db"), "old").unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("muzik.db"), "new").unwrap();

        assert_eq!(move_dir(&old, &new).unwrap(), Move::Conflict);
        assert_eq!(fs::read_to_string(old.join("muzik.db")).unwrap(), "old");
        assert_eq!(fs::read_to_string(new.join("muzik.db")).unwrap(), "new");
    }
}
