use crate::{Error, Result};
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;

struct Backups {
    directory: tempfile::TempDir,
    files: Vec<(PathBuf, PathBuf)>,
}

impl Backups {
    fn restore(self) -> Option<(PathBuf, Vec<String>)> {
        let mut failures = Vec::new();
        for (path, saved) in &self.files {
            if let Err(error) = fs::copy(saved, path) {
                failures.push(format!(
                    "{} -> {}: {error}",
                    saved.display(),
                    path.display()
                ));
            }
        }
        if failures.is_empty() {
            None
        } else {
            let directory = self.directory.keep();
            tracing::error!(directory = %directory.display(), ?failures, "tag backups retained for recovery");
            Some((directory, failures))
        }
    }
}

/// Keep snapshots until the update commits or rollback completes, including unwinding.
pub fn with_backups<T>(
    paths: impl IntoIterator<Item = PathBuf>,
    update: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let mut paths = paths.into_iter().collect::<Vec<_>>();
    if paths.is_empty() {
        return update();
    }
    for path in &mut paths {
        *path = fs::canonicalize(&path)?;
    }
    paths.sort();
    paths.dedup();
    let mut locks: Vec<File> = Vec::new();
    for path in &paths {
        let mut name = std::ffi::OsString::from(".");
        name.push(
            path.file_name()
                .ok_or_else(|| Error::InvalidSource(path.clone()))?,
        );
        name.push(".muzik-lock");
        // Separate lock files remain in place so another process always locks the same inode.
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_file_name(name))?;
        lock.lock()?;
        locks.push(lock);
    }
    let mut backup = Backups {
        directory: tempfile::tempdir()?,
        files: Vec::new(),
    };
    for (index, path) in paths.into_iter().enumerate() {
        let saved = backup.directory.path().join(index.to_string());
        fs::copy(&path, &saved)?;
        backup.files.push((path, saved));
    }
    let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(update)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(source)) => {
            if let Some((directory, failures)) = backup.restore() {
                Err(Error::TagRestore {
                    source: Box::new(source),
                    directory,
                    failures,
                })
            } else {
                Err(source)
            }
        }
        Err(panic) => {
            backup.restore();
            std::panic::resume_unwind(panic)
        }
    };
    drop(locks);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rollback_does_not_overwrite_an_overlapping_commit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("track");
        fs::write(&path, b"original").unwrap();
        let (entered, ready) = std::sync::mpsc::channel();
        let (attempt, attempted) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let file = &path;
            let failed = scope.spawn(move || {
                with_backups([file.clone()], || -> Result<()> {
                    fs::write(file, b"failed update")?;
                    entered.send(()).unwrap();
                    attempted.recv().unwrap();
                    let _ = finished.recv_timeout(std::time::Duration::from_millis(100));
                    Err("database write failed".into())
                })
            });
            ready.recv().unwrap();
            let committed = scope.spawn(|| {
                attempt.send(()).unwrap();
                let result = with_backups([path.clone()], || {
                    fs::write(&path, b"committed update")?;
                    Ok(())
                });
                let _ = done.send(());
                result
            });
            assert!(failed.join().unwrap().is_err());
            committed.join().unwrap().unwrap();
        });
        assert_eq!(fs::read(path).unwrap(), b"committed update");
    }

    #[test]
    fn a_failed_restore_keeps_the_snapshot_for_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("track");
        fs::write(&path, b"original").unwrap();
        let error = with_backups([path.clone()], || -> Result<()> {
            fs::remove_file(&path)?;
            fs::create_dir(&path)?;
            Err("database write failed".into())
        })
        .unwrap_err();
        let Error::TagRestore {
            source,
            directory: saved,
            failures,
        } = error
        else {
            panic!("missing recovery details");
        };
        assert_eq!(source.to_string(), "database write failed");
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains(&path.display().to_string()))
        );
        assert_eq!(fs::read(saved.join("0")).unwrap(), b"original");
        fs::remove_dir_all(saved).unwrap();
    }

    #[test]
    fn database_only_updates_return_the_update_result() {
        let mut calls = 0;
        let error = with_backups([], || -> Result<()> {
            calls += 1;
            Err("database write failed".into())
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error.to_string(), "database write failed");
    }

    #[test]
    fn a_panicking_update_restores_the_original_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("track");
        fs::write(&path, b"original").unwrap();
        let panic = std::panic::catch_unwind(|| {
            let _ = with_backups([path.clone()], || -> Result<()> {
                fs::write(&path, b"updated")?;
                panic!("tag writer failed");
            });
        });
        assert!(panic.is_err());
        assert_eq!(fs::read(path).unwrap(), b"original");
    }

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
