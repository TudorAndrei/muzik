use crate::{
    Error, Plan, Result, Target, Transfer, available_bytes, encodings, plan, record,
    remove_empty_folders, run as transfer_all, stale_files,
};
use muzik_library::{Item, Library, path_from_sql};
use muzik_media::quality::MeasuredQuality;
use muzik_store::Connection;
use parking_lot::Mutex;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Selection {
    pub tracks: Vec<PathBuf>,
    pub covers: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
pub struct Options {
    pub delete: bool,
    pub jobs: usize,
}

pub struct Prepared {
    plan: Plan,
    target: Target,
    options: Options,
    delete_blocked: bool,
    stale: Vec<PathBuf>,
    freed: u64,
    needed: u64,
    available: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortfall {
    pub needed: u64,
    pub space: u64,
}

pub struct Done<'a> {
    pub index: usize,
    pub total: usize,
    pub transfer: &'a Transfer,
    pub result: &'a Result<()>,
    pub record_error: Option<Error>,
}

pub struct Report {
    pub written: usize,
    pub failed: usize,
    pub unrecorded: usize,
}

impl Prepared {
    #[must_use]
    pub const fn plan(&self) -> &Plan {
        &self.plan
    }

    #[must_use]
    pub const fn target(&self) -> &Target {
        &self.target
    }

    #[must_use]
    pub const fn delete(&self) -> bool {
        self.options.delete
    }

    #[must_use]
    pub const fn delete_blocked(&self) -> bool {
        self.delete_blocked
    }

    #[must_use]
    pub fn stale(&self) -> &[PathBuf] {
        &self.stale
    }

    #[must_use]
    pub const fn freed(&self) -> u64 {
        self.freed
    }

    #[must_use]
    pub const fn needed(&self) -> u64 {
        self.needed
    }

    #[must_use]
    pub fn shortfall(&self) -> Option<Shortfall> {
        shortfall(self.needed, self.freed, self.available)
    }
}

fn shortfall(needed: u64, freed: u64, available: Option<u64>) -> Option<Shortfall> {
    let space = available?.saturating_add(freed);
    (needed > space).then_some(Shortfall { needed, space })
}

fn size(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|meta| meta.len())
        .sum()
}

/// # Errors
/// Returns an error if the query is invalid or the library cannot be read.
pub fn select(library: &Library, directory: &Path, query: &str, covers: bool) -> Result<Selection> {
    let items = library.query_items(query)?;
    let tracks: Vec<PathBuf> = items
        .iter()
        .filter_map(|item| item.field("path").and_then(path_from_sql))
        .map(|path| absolute(directory, path))
        .collect();
    let album_ids: BTreeSet<i64> = if covers {
        items.iter().filter_map(Item::album_id).collect()
    } else {
        BTreeSet::new()
    };
    let mut cover_files = Vec::new();
    for id in album_ids {
        let album = library.album(id)?;
        if let Some(path) = album
            .as_ref()
            .and_then(|album| album.field("artpath"))
            .and_then(path_from_sql)
            .map(|path| absolute(directory, path))
            .filter(|path| path.is_file())
        {
            cover_files.push(path);
        }
    }
    Ok(Selection {
        tracks,
        covers: cover_files,
    })
}

/// # Errors
/// Returns an error if the sync records or the target folder cannot be read.
pub fn prepare(
    target: &Target,
    directory: &Path,
    selection: &Selection,
    connection: &Connection,
    options: Options,
    probe: &(dyn Fn(&Path) -> Result<Option<MeasuredQuality>, String> + Sync),
) -> Result<Prepared> {
    let known = encodings(connection, &target.path)?;
    let plan = plan(
        target,
        directory,
        &selection.tracks,
        &selection.covers,
        &known,
        options.jobs,
        probe,
    );
    let delete = options.delete && plan.unreadable.is_empty();
    let stale = if delete {
        stale_files(&target.path, &plan.planned)?
    } else {
        Vec::new()
    };
    let freed = size(&stale);
    let needed = plan.bytes_needed();
    Ok(Prepared {
        plan,
        target: target.clone(),
        options: Options {
            delete,
            jobs: options.jobs,
        },
        delete_blocked: options.delete && !delete,
        stale,
        freed,
        needed,
        available: available_bytes(&target.path),
    })
}

/// # Errors
/// Returns an error if the target is missing, has too little space, or stale files cannot be removed.
pub fn apply(
    prepared: Prepared,
    connection: Connection,
    done: &(dyn Fn(Done<'_>) + Sync),
) -> Result<Report> {
    let Prepared {
        plan,
        target,
        options,
        stale,
        ..
    } = prepared;
    if !target.path.is_dir() {
        return Err(Error::TargetMissing(target.path));
    }
    let stale: Vec<PathBuf> = stale.into_iter().filter(|path| path.exists()).collect();
    if let Some(shortfall) = shortfall(
        plan.bytes_needed(),
        size(&stale),
        available_bytes(&target.path),
    ) {
        return Err(Error::NoSpace(shortfall));
    }
    for path in &stale {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::Message(format!("{}: {error}", path.display()))),
        }
    }
    if options.delete {
        remove_empty_folders(&target.path)?;
    }
    let total = plan.pending.len();
    let count = AtomicUsize::new(0);
    let unrecorded = AtomicUsize::new(0);
    let connection = Mutex::new(connection);
    let finished = |transfer: &Transfer, result: &Result<()>| {
        let index = count.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let record_error = match result {
            Ok(()) => record(&connection.lock(), transfer).err(),
            Err(_) => None,
        };
        if record_error.is_some() {
            unrecorded.fetch_add(1, Ordering::Relaxed);
        }
        done(Done {
            index,
            total,
            transfer,
            result,
            record_error,
        });
    };
    let failed = transfer_all(&plan.pending, options.jobs, &finished);
    Ok(Report {
        written: total,
        failed,
        unrecorded: unrecorded.load(Ordering::Relaxed),
    })
}

fn absolute(directory: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        directory.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::{Shortfall, shortfall};

    #[test]
    fn freed_space_counts_toward_the_available_space() {
        assert_eq!(shortfall(10, 5, Some(5)), None);
        assert_eq!(
            shortfall(10, 4, Some(5)),
            Some(Shortfall {
                needed: 10,
                space: 9
            })
        );
    }

    #[test]
    fn unknown_space_does_not_stop_a_sync() {
        assert_eq!(shortfall(10, 0, None), None);
        assert_eq!(shortfall(u64::MAX, 0, None), None);
    }
}
