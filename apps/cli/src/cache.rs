use bytesize::ByteSize;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::paths;

pub fn list() -> io::Result<()> {
    let files = cache_files()?;
    if files.is_empty() {
        println!("Cache is empty. ({})", paths::cache_dir().display());
        return Ok(());
    }
    let mut total = 0_u64;
    for file in &files {
        let metadata = fs::metadata(file)?;
        total += metadata.len();
        println!("{}\t{}", file.display(), ByteSize(metadata.len()));
    }
    println!("Total: {} file(s), {}", files.len(), ByteSize(total));
    Ok(())
}

pub fn size() -> io::Result<()> {
    let files = cache_files()?;
    let mut total = 0_u64;
    for file in &files {
        total += fs::metadata(file)?.len();
    }
    println!(
        "Cache: {}\n  {} file(s), {}",
        paths::cache_dir().display(),
        files.len(),
        ByteSize(total)
    );
    Ok(())
}

pub fn clear(key: Option<&str>) -> io::Result<()> {
    if let Some(key) = key {
        validate_key(key)?;
        let mut removed = false;
        for ext in ["txt", "json"] {
            let path = paths::cache_dir().join(format!("{key}.{ext}"));
            match fs::remove_file(&path) {
                Ok(()) => removed = true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        if removed {
            println!("Cleared cache entry: {key}");
        } else {
            println!("Cache entry not found: {key}");
        }
        return Ok(());
    }
    let files = cache_files()?;
    if files.is_empty() {
        println!("Cache is already empty.");
        return Ok(());
    }
    if !confirm(&format!("Delete all {} cache entries?", files.len()))? {
        return Ok(());
    }
    for file in &files {
        fs::remove_file(file)?;
    }
    println!("Cleared {} cache entries.", files.len());
    Ok(())
}

pub fn purge() -> io::Result<()> {
    let files = cache_files()?;
    let downloads = paths::download_dir();
    let splits = paths::data_dir().join("splits");
    let download_count = child_count(&downloads)?;
    let split_count = child_count(&splits)?;
    if files.is_empty() && download_count == 0 && split_count == 0 {
        println!("Nothing to purge.");
        return Ok(());
    }
    println!(
        "This will delete {} cache files, {download_count} downloaded files, and {split_count} split entries.",
        files.len()
    );
    if !confirm("Continue?")? {
        return Ok(());
    }
    for file in &files {
        fs::remove_file(file)?;
    }
    for directory in [&downloads, &splits] {
        if directory.exists() {
            fs::remove_dir_all(directory)?;
            fs::create_dir_all(directory)?;
        }
    }
    println!("Purge complete.");
    Ok(())
}

pub fn clean(max_age_days: u64) -> io::Result<()> {
    let age = Duration::from_secs(max_age_days.saturating_mul(24 * 60 * 60));
    let cutoff = SystemTime::now()
        .checked_sub(age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0_usize;
    for file in cache_files()? {
        let metadata = fs::metadata(&file)?;
        if metadata.len() == 0 || metadata.modified()? < cutoff {
            fs::remove_file(file)?;
            removed += 1;
        }
    }
    println!("Removed {removed} stale cache entries.");
    Ok(())
}

fn cache_files() -> io::Result<Vec<PathBuf>> {
    let root = paths::cache_dir();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = fs::read_dir(root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    files.retain(|path| path.is_file());
    files.sort_by_key(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok());
    files.reverse();
    Ok(files)
}

fn child_count(directory: &Path) -> io::Result<usize> {
    if directory.exists() {
        fs::read_dir(directory)?.try_fold(0_usize, |count, entry| entry.map(|_| count + 1))
    } else {
        Ok(0)
    }
}

fn validate_key(key: &str) -> io::Result<()> {
    if !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cache keys may contain only letters, digits, '_' and '-'",
        ))
    }
}

fn confirm(prompt: &str) -> io::Result<bool> {
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES"))
}
