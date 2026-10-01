use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub data: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
}

impl Paths {
    pub fn user() -> Self {
        Self {
            data: data_dir(),
            config: config_dir(),
            cache: cache_dir(),
        }
    }

    pub fn under(root: &Path) -> Self {
        Self {
            data: root.join("data"),
            config: root.join("config"),
            cache: root.join("cache"),
        }
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

pub fn data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/muzik")
    } else if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
            .join("muzik")
    } else {
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local/share"))
            .join("muzik")
    }
}

pub fn download_dir() -> PathBuf {
    data_dir().join("downloads")
}

pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        data_dir()
    } else if cfg!(target_os = "windows") {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
            .join("muzik")
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join("muzik")
    }
}

pub fn cache_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Caches/muzik")
    } else if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
            .join("muzik/Cache")
    } else {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".cache"))
            .join("muzik")
    }
}

fn home() -> PathBuf {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{expand_home, home};
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
}
