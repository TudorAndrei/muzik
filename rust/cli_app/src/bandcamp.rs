use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use crate::{Bandcamp, paths};

pub fn download(args: &Bandcamp) -> io::Result<()> {
    let user = args
        .user
        .clone()
        .or_else(|| std::env::var("BS_USER").ok())
        .or_else(|| read_stored_user(&paths::config_dir().join("bandcamp_user")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "give a Bandcamp username"))?;
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| paths::data_dir().join("bandcamp"));
    let cookies = args
        .cookies
        .clone()
        .or_else(|| {
            let path = paths::config_dir().join("bandcamp_cookies.txt");
            path.is_file().then_some(path)
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "give --cookies with a Bandcamp cookie file",
            )
        })?;

    migrate_cache(&output)?;
    let mut command = Command::new("bandsnatch");
    command
        .arg("run")
        .arg("--format")
        .arg(&args.format)
        .arg("--output-folder")
        .arg(output)
        .arg("--jobs")
        .arg(args.jobs.to_string())
        .arg("--cookies")
        .arg(cookies);
    if args.dry_run {
        command.arg("--dry-run");
    }
    if args.force {
        command.arg("--force");
    }
    let status = command.arg(user).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "bandsnatch exited with status {status}"
        )))
    }
}

fn read_stored_user(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn migrate_cache(output: &Path) -> io::Result<()> {
    let old_cache = paths::cache_dir().join("bandcamp.cache");
    migrate_cache_from(&old_cache, output)
}

fn migrate_cache_from(old_cache: &Path, output: &Path) -> io::Result<()> {
    let new_cache = output.join("bandcamp-collection-downloader.cache");
    if !old_cache.is_file() || new_cache.exists() {
        return Ok(());
    }
    fs::create_dir_all(output)?;
    fs::copy(old_cache, new_cache)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::migrate_cache_from;

    #[test]
    fn retains_download_history_when_changing_bandcamp_downloaders() {
        let temp = tempfile::tempdir().expect("create test directory");
        let source = temp.path().join("bandcamp.cache");
        let output = temp.path().join("output");
        fs::write(&source, "album-1| My album\n").expect("write old cache");

        migrate_cache_from(&source, &output).expect("move cache");

        let new_cache = output.join("bandcamp-collection-downloader.cache");
        assert_eq!(
            fs::read_to_string(&new_cache).expect("read new cache"),
            "album-1| My album\n"
        );

        fs::write(&new_cache, "album-2| Existing\n").expect("write existing cache");
        migrate_cache_from(&source, &output).expect("preserve existing cache");
        assert_eq!(
            fs::read_to_string(new_cache).expect("read existing cache"),
            "album-2| Existing\n"
        );
    }
}
