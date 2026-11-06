use crate::Result;
use std::fs;
use std::path::PathBuf;

pub fn with_backups<T>(
    paths: impl IntoIterator<Item = PathBuf>,
    update: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let mut paths = paths.into_iter().peekable();
    if paths.peek().is_none() {
        return update();
    }
    let backup = tempfile::tempdir()?;
    let mut files = Vec::new();
    for (index, path) in paths.enumerate() {
        let saved = backup.path().join(index.to_string());
        fs::copy(&path, &saved)?;
        files.push((path, saved));
    }
    let result = update();
    if result.is_err() {
        for (path, saved) in files {
            if let Err(error) = fs::copy(saved, &path) {
                tracing::warn!(path = %path.display(), %error, "cannot restore tag backup");
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_updates_commit_together_or_restore_the_original_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let paths = [directory.path().join("one"), directory.path().join("two")];
        for fail in [false, true] {
            for path in &paths {
                fs::write(path, b"original").unwrap();
            }
            let result = with_backups(paths.clone(), || {
                for path in &paths {
                    fs::write(path, b"updated")?;
                }
                if fail {
                    return Err(crate::Error::Message("database write failed".into()));
                }
                Ok(())
            });
            assert_eq!(result.is_err(), fail);
            for path in &paths {
                assert_eq!(
                    fs::read(path).unwrap(),
                    if fail {
                        b"original".as_slice()
                    } else {
                        b"updated".as_slice()
                    }
                );
            }
        }
    }
}
