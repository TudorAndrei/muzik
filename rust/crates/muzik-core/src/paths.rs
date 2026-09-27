use std::env;
use std::path::PathBuf;

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
