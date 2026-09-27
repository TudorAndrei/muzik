//! Native incremental import history, seeded once from the beets state file.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;

#[derive(Clone, Debug)]
pub struct IncrementalHistory {
    path: PathBuf,
    seed: BTreeSet<Vec<PathBuf>>,
}

#[derive(Default, Deserialize, Serialize)]
struct HistoryFile {
    entries: BTreeSet<Vec<PathBuf>>,
}

impl IncrementalHistory {
    /// Return the native state path next to the beets pickle state file.
    pub fn path_for_statefile(statefile: &Path) -> PathBuf {
        statefile.with_extension("muzik-history.json")
    }

    /// Seed the native history only when it does not already exist.
    pub fn open_or_seed(statefile: &Path, imported: &[Vec<PathBuf>]) -> Result<Self, Error> {
        let history = Self {
            path: Self::path_for_statefile(statefile),
            seed: imported
                .iter()
                .map(|group| {
                    group
                        .iter()
                        .map(|path| path.canonicalize().unwrap_or_else(|_| path.clone()))
                        .collect()
                })
                .collect(),
        };
        match fs::metadata(&history.path) {
            Ok(_) => {
                history.read()?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(history)
    }

    pub fn contains(&self, paths: &[PathBuf]) -> Result<bool, Error> {
        Ok(self.read()?.entries.contains(paths))
    }

    pub fn record(&self, paths: &[PathBuf]) -> Result<(), Error> {
        let mut file = self.read()?;
        if file.entries.insert(paths.to_vec()) {
            self.write(&file)?;
        }
        Ok(())
    }

    /// Save the migrated beets entries before a real import run.
    pub fn persist_seed(&self) -> Result<(), Error> {
        match fs::metadata(&self.path) {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => self.write(&HistoryFile {
                entries: self.seed.clone(),
            }),
            Err(error) => Err(error.into()),
        }
    }

    fn read(&self) -> Result<HistoryFile, Error> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(HistoryFile {
                entries: self.seed.clone(),
            }),
            Err(error) => Err(error.into()),
        }
    }

    fn write(&self, file: &HistoryFile) -> Result<(), Error> {
        let parent = self.path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut temporary, file)?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::IncrementalHistory;
    use std::path::PathBuf;

    #[test]
    fn seeds_once_and_records_new_source_groups() {
        let temp = tempfile::tempdir().unwrap();
        let statefile = temp.path().join("state.pickle");
        let old = vec![PathBuf::from("/incoming/old")];
        let new = vec![PathBuf::from("/incoming/new")];
        let history =
            IncrementalHistory::open_or_seed(&statefile, std::slice::from_ref(&old)).unwrap();
        assert!(!IncrementalHistory::path_for_statefile(&statefile).exists());
        assert!(history.contains(&old).unwrap());
        assert!(!history.contains(&new).unwrap());
        history.persist_seed().unwrap();
        assert!(IncrementalHistory::path_for_statefile(&statefile).exists());
        history.record(&new).unwrap();
        let reopened = IncrementalHistory::open_or_seed(&statefile, &[]).unwrap();
        assert!(reopened.contains(&old).unwrap());
        assert!(reopened.contains(&new).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn seeded_symlink_directory_matches_canonical_plan_path() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let history =
            IncrementalHistory::open_or_seed(&temp.path().join("state.pickle"), &[vec![alias]])
                .unwrap();
        assert!(history.contains(&[real.canonicalize().unwrap()]).unwrap());
    }
}
